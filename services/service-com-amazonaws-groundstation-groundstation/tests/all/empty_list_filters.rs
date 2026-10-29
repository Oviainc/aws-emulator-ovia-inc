// SPDX-License-Identifier: CC0-1.0

//! What a list-valued FILTER member of a listing means when it names NO values, end to end
//! over HTTP through the same front door as the rest of this target's suites.
//!
//! The RULE under test: a list-valued filter member narrows a listing to the values it
//! NAMES, so a request that names none of them narrows nothing — every value is reported.
//! Each such member of this service is modelled with a minimum length of zero, so a list
//! that names nothing reaches the handler as valid input, and each is documented as the
//! values to RETURN rather than as a switch, so naming none asks about all of them rather
//! than about none. One reading for every listing that takes such a member
//! (`state::list_filter_admits`).
//!
//! So the suites below assert that rule over SETS of inputs rather than over one call:
//!
//! * `ListContacts$statusList` over a table of filters — none, one status, two statuses in
//!   either order, every status a stored contact can hold, and a status nothing holds —
//!   against a store holding contacts in several statuses, with the expectation for each
//!   row computed from the rule rather than from the handler;
//! * the page walk under a filter that names nothing, so the whole sequence it selects is
//!   reachable rather than only its first page;
//! * `ListEphemerides$statusList` over the same shape of table, including the member left
//!   out altogether, which is the same request written another way.
//!
//! The third listing that takes such a member, `ListGroundStationReservations`, binds its
//! `reservationTypes` in the query string, where naming none is the only way to write it;
//! `tests/all/ground_station_reservations.rs` pins that row.

use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ephemeris_catalog::{create_ok, tle_inline, wait_out_validation, FUTURE, PAST};
use crate::scheduling_loop::{
    create_mission_profile, harness_with_engine, send, stock_ground_stations, AFTER_SETTLE,
};

/// The ground station every contact of these suites is reserved at.
const STATION: &str = "Ohio-1";

/// Every `ContactStatus` a contact this service STORES can hold: the model's own enum
/// (`smithy_query(query="shape", shape="ContactStatus")`) less `AVAILABLE`, which is a
/// window the service offers and nothing has reserved rather than a contact the store
/// holds, and which a request may only name together with the pairing an availability
/// search is computed for.
const EVERY_STORED_CONTACT_STATUS: [&str; 12] = [
    "SCHEDULING",
    "FAILED_TO_SCHEDULE",
    "SCHEDULED",
    "CANCELLED",
    "AWS_CANCELLED",
    "PREPASS",
    "PASS",
    "POSTPASS",
    "COMPLETED",
    "FAILED",
    "CANCELLING",
    "AWS_FAILED",
];

/// Every `EphemerisStatus` the model declares (`smithy_query(query="shape",
/// shape="EphemerisStatus")`).
const EVERY_EPHEMERIS_STATUS: [&str; 6] = [
    "VALIDATING",
    "INVALID",
    "ERROR",
    "ENABLED",
    "DISABLED",
    "EXPIRED",
];

/// The clock, in epoch seconds: the contacts below are placed relative to it so one of them
/// is inside its own pass while the suite runs.
fn now_seconds() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("the clock is after the epoch")
            .as_secs(),
    )
    .expect("an epoch-seconds instant fits in i64")
}

/// Whether a filter that named `named` admits an item carrying `value` — the rule under
/// test, written here independently of the handler so every expectation below comes from the
/// rule rather than from the answer.
fn admits(named: &[&str], value: &str) -> bool {
    named.is_empty() || named.contains(&value)
}

/// One stored record a listing may report: its id and the status it holds.
#[derive(Debug, Clone)]
struct Held {
    id: String,
    status: String,
}

/// The ids of `held` the rule admits under a filter naming `named`, sorted.
fn expected(held: &[Held], named: &[&str]) -> Vec<String> {
    let mut ids: Vec<String> = held
        .iter()
        .filter(|record| admits(named, &record.status))
        .map(|record| record.id.clone())
        .collect();
    ids.sort();
    ids
}

/// A `ListContacts` body over `window` naming the statuses `named`, plus `maxResults` when
/// one is given.
fn contacts_request(window: (i64, i64), named: &[&str], max_results: Option<i64>) -> Value {
    let mut body = json!({
        "statusList": named,
        "startTime": window.0,
        "endTime": window.1,
    });
    if let Some(max) = max_results {
        body["maxResults"] = json!(max);
    }
    body
}

/// One page of `ListContacts` for `request`, asserting it was ANSWERED.
///
/// `ListContacts` binds every input member in the BODY — its `nextToken` carries no
/// `@httpQuery` (`smithy_query(query="operation", operation="ListContacts")`) — so a
/// resumed page is the same body carrying the token.
async fn contacts_page(
    router: &MultiServiceRouter,
    case: &str,
    request: &Value,
    token: Option<&str>,
) -> (Vec<Value>, Option<String>) {
    let mut body = request.clone();
    if let Some(token) = token {
        body["nextToken"] = json!(token);
    }
    let reply = send(router, "POST", "/contacts", Some(body)).await;
    assert_eq!(
        reply.status, 200,
        "case {case}: expected ListContacts 200, got {} {}",
        reply.status, reply.body
    );
    let page = reply
        .field("contactList")
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case}: contactList must be a list, got {}",
                reply.body
            )
        })
        .clone();
    let next = reply.field("nextToken").as_str().map(str::to_string);
    (page, next)
}

/// Every contact `ListContacts` reports for `request`, across as many pages as it takes,
/// by `contactId` and sorted — so a row's expectation is about the SET selected rather
/// than about the order it came in.
async fn listed_contacts(router: &MultiServiceRouter, case: &str, request: &Value) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    // One more page than this suite's bounded store could need at a page size of one, so a
    // token that failed to advance fails as a bounded assertion instead of looping forever.
    for page in 0..16 {
        let (items, next) = contacts_page(router, case, request, token.as_deref()).await;
        for item in &items {
            let contact_id = item.get("contactId").and_then(Value::as_str);
            assert!(
                contact_id.is_some(),
                "case {case} page-{page}: a contact the store holds carries a contactId, got \
                 {item}"
            );
            ids.push(contact_id.unwrap_or_default().to_string());
        }
        token = next;
        if token.is_none() {
            ids.sort();
            return ids;
        }
    }
    panic!("case {case}: the listing of /contacts never closed");
}

/// The `contactStatus` `DescribeContact` reports for `contact_id` — read through the front
/// door, so the table's expectations rest on the status the service itself reports rather
/// than on one this suite assumed.
async fn contact_status(router: &MultiServiceRouter, contact_id: &str) -> String {
    let reply = send(router, "GET", &format!("/contact/{contact_id}"), None).await;
    assert_eq!(
        reply.status, 200,
        "case describe {contact_id}: expected DescribeContact 200, got {} {}",
        reply.status, reply.body
    );
    reply.string("contactStatus")
}

/// `ReserveContact` over `window`, asserting it was accepted, and the `contactId` it minted.
async fn reserve(
    router: &MultiServiceRouter,
    profile_arn: &str,
    case: &str,
    window: (i64, i64),
) -> String {
    let reply = send(
        router,
        "POST",
        "/contact",
        Some(json!({
            "missionProfileArn": profile_arn,
            "groundStation": STATION,
            "startTime": window.0,
            "endTime": window.1,
        })),
    )
    .await;
    assert_eq!(
        reply.status, 200,
        "case {case}: expected ReserveContact 200, got {} {}",
        reply.status, reply.body
    );
    reply.string("contactId")
}

/// A store holding contacts in SEVERAL statuses, and the window that spans every one of
/// them: a reservation the scheduler has confirmed, one whose pass the clock is inside, and
/// one that was released. The statuses are read back rather than assumed.
async fn contacts_in_several_statuses() -> (MultiServiceRouter, Vec<Held>, (i64, i64)) {
    let (router, engine) = harness_with_engine();
    // The account is served by the station every contact below is reserved at.
    stock_ground_stations(&engine, STATION);
    let profile_arn = create_mission_profile(&router, "empty status list", 60, None, None).await;
    let now = now_seconds();

    let ahead = reserve(&router, &profile_arn, "ahead", (now + 3_600, now + 4_200)).await;
    let under_way = reserve(&router, &profile_arn, "under way", (now - 600, now + 1_800)).await;
    let released = reserve(&router, &profile_arn, "released", (now + 7_200, now + 7_800)).await;
    let cancelled = send(&router, "DELETE", &format!("/contact/{released}"), None).await;
    assert_eq!(
        cancelled.status, 200,
        "case a pass to release: expected CancelContact 200, got {} {}",
        cancelled.status, cancelled.body
    );

    // Every move the clock owes these contacts has fallen due by now, so each one holds the
    // status it is going to report for the rest of the suite.
    std::thread::sleep(AFTER_SETTLE);

    let mut held: Vec<Held> = Vec::new();
    for id in [ahead, under_way, released] {
        let status = contact_status(&router, &id).await;
        held.push(Held { id, status });
    }

    let mut distinct: Vec<String> = held.iter().map(|record| record.status.clone()).collect();
    distinct.sort();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        held.len(),
        "the table needs one contact per status, so the three must hold three different \
         statuses, got {held:?}"
    );

    (router, held, (now - 7_200, now + 86_400))
}

#[tokio::test]
async fn a_contact_status_filter_naming_no_status_narrows_nothing() {
    let (router, held, window) = contacts_in_several_statuses().await;
    let statuses: Vec<&str> = held.iter().map(|record| record.status.as_str()).collect();
    // A status the three contacts were read back NOT holding, chosen from the model's enum
    // rather than written out, so the row stays a "nothing holds it" row whatever the clock
    // made of them.
    let unheld = *EVERY_STORED_CONTACT_STATUS
        .iter()
        .find(|status| !statuses.contains(status))
        .expect("the model declares more statuses than this suite stores contacts in");

    // Rows differ along HOW MANY values the filter names and in which order: none at all,
    // one, two in either order, every status a stored contact can hold, and one status
    // nothing holds. The expectation for each is the rule applied to the statuses the three
    // contacts were read back holding.
    let rows: Vec<(String, Vec<&str>)> = vec![
        ("no status at all".to_string(), vec![]),
        (format!("one status: {}", statuses[0]), vec![statuses[0]]),
        (
            format!("two statuses: {} and {}", statuses[0], statuses[1]),
            vec![statuses[0], statuses[1]],
        ),
        (
            format!("the same two reversed: {} and {}", statuses[1], statuses[0]),
            vec![statuses[1], statuses[0]],
        ),
        (
            "every status a stored contact can hold".to_string(),
            EVERY_STORED_CONTACT_STATUS.to_vec(),
        ),
        (
            format!("a status nothing holds: {unheld}"),
            vec![unheld],
        ),
    ];

    for (case, named) in &rows {
        let request = contacts_request(window, named, Some(100));
        let reported = listed_contacts(&router, case, &request).await;
        assert_eq!(
            reported,
            expected(&held, named),
            "case {case}: the listing must report exactly the contacts whose status the \
             filter admits, got {reported:?} for {held:?}"
        );
    }

    // The two readings of "narrows nothing" agree: naming no status answers exactly what
    // naming every status a contact can hold answers.
    let none_named = listed_contacts(
        &router,
        "no status at all",
        &contacts_request(window, &[], Some(100)),
    )
    .await;
    let all_named = listed_contacts(
        &router,
        "every status a stored contact can hold",
        &contacts_request(window, &EVERY_STORED_CONTACT_STATUS, Some(100)),
    )
    .await;
    assert_eq!(
        none_named, all_named,
        "case narrows-nothing: naming no status must answer what naming every status \
         answers, got {none_named:?} against {all_named:?}"
    );

    // And a filter naming nothing still narrows on the OTHER members: the window is a
    // filter of its own, so a window holding no contact answers an empty page.
    let elsewhere = listed_contacts(
        &router,
        "no status at all, in an empty window",
        &contacts_request((window.0 - 400 * 86_400, window.0 - 399 * 86_400), &[], None),
    )
    .await;
    assert!(
        elsewhere.is_empty(),
        "case no status at all, in an empty window: the window still narrows, got \
         {elsewhere:?}"
    );
}

#[tokio::test]
async fn a_contact_status_filter_naming_no_status_pages_through_every_contact() {
    let (router, held, window) = contacts_in_several_statuses().await;
    let whole: Vec<String> = expected(&held, &[]);

    // Rows differ along the page size the walk is driven at: one contact per page, two —
    // which splits the sequence unevenly — all of them, and no page size at all.
    for page_size in [Some(1), Some(2), Some(3), None] {
        let case = format!("no status at all, maxResults={page_size:?}");
        let request = contacts_request(window, &[], page_size);
        let reported = listed_contacts(&router, &case, &request).await;
        assert_eq!(
            reported, whole,
            "case {case}: the walk must reach every contact the empty filter selects, got \
             {reported:?}"
        );
    }
}

#[tokio::test]
async fn an_ephemeris_status_filter_naming_no_status_narrows_nothing() {
    let (router, _engine) = harness_with_engine();

    // Three ephemerides differing along the status their request and their data earn them:
    // enabled and in force, created disabled, and one whose expiration the clock has
    // already passed.
    let rows: [(&str, bool, i64); 3] = [
        ("in force", true, FUTURE),
        ("created disabled", false, FUTURE),
        ("already expired", true, PAST),
    ];
    let mut ids: Vec<(&str, String)> = Vec::new();
    for (case, enabled, expiration) in rows {
        let id = create_ok(
            &router,
            case,
            json!({
                "name": case.replace(' ', "-"),
                "enabled": enabled,
                "expirationTime": expiration,
                "ephemeris": tle_inline(&[(1_800_000_000, 1_800_086_400)]),
            }),
        )
        .await;
        ids.push((case, id));
    }
    // Every status move the clock owes them — the end of validation, and the expiration
    // that has passed — has fallen due by now.
    wait_out_validation();

    // The window every listing names is tested against an ephemeris's EXPIRATION, so it
    // spans both the expiration already passed and the one far ahead.
    let window = (1_i64, 4_102_444_800_i64);
    let mut held: Vec<Held> = Vec::new();
    for (case, id) in &ids {
        let described = send(&router, "GET", &format!("/ephemeris/{id}"), None).await;
        assert_eq!(
            described.status, 200,
            "case {case}: expected DescribeEphemeris 200, got {} {}",
            described.status, described.body
        );
        held.push(Held {
            id: id.clone(),
            status: described.string("status"),
        });
    }

    let statuses: Vec<&str> = held.iter().map(|record| record.status.as_str()).collect();
    // A status none of the three was read back holding, chosen from the model's enum rather
    // than written out.
    let unheld = *EVERY_EPHEMERIS_STATUS
        .iter()
        .find(|status| !statuses.contains(status))
        .expect("the model declares more statuses than this suite stores ephemerides in");
    // The same shape of table as the contact listing's, plus the row only an OPTIONAL
    // member has: the member left out altogether, which is the same request as one naming
    // an empty list.
    let filters: Vec<(String, Option<Vec<&str>>)> = vec![
        ("the member left out".to_string(), None),
        ("no status at all".to_string(), Some(vec![])),
        (
            format!("one status: {}", statuses[0]),
            Some(vec![statuses[0]]),
        ),
        (
            format!("two statuses: {} and {}", statuses[1], statuses[0]),
            Some(vec![statuses[1], statuses[0]]),
        ),
        (
            "every status the model declares".to_string(),
            Some(EVERY_EPHEMERIS_STATUS.to_vec()),
        ),
        (
            format!("a status nothing holds: {unheld}"),
            Some(vec![unheld]),
        ),
    ];

    for (case, named) in &filters {
        let mut request = json!({"startTime": window.0, "endTime": window.1});
        if let Some(named) = named {
            request["statusList"] = json!(named);
        }
        let reply = send(&router, "POST", "/ephemerides?maxResults=100", Some(request)).await;
        assert_eq!(
            reply.status, 200,
            "case {case}: expected ListEphemerides 200, got {} {}",
            reply.status, reply.body
        );
        // `EphemeridesList` is `len 1..500` (`smithy_query(query="shape",
        // shape="EphemeridesList")`), so a filter that admits nothing is answered with the
        // member left out rather than with an empty list; a page that selected something
        // carries it as a non-empty list.
        let mut reported: Vec<String> = match reply.body.get("ephemerides") {
            None => Vec::new(),
            Some(page) => {
                let items = page.as_array().unwrap_or_else(|| {
                    panic!(
                        "case {case}: a present ephemerides must be a list, got {}",
                        reply.body
                    )
                });
                assert!(
                    !items.is_empty(),
                    "case {case}: ephemerides admits no empty list, so a page that selected \
                     nothing must omit it, got {}",
                    reply.body
                );
                items
                    .iter()
                    .map(|item| {
                        item.get("ephemerisId")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string()
                    })
                    .collect()
            }
        };
        reported.sort();
        assert_eq!(
            reported,
            expected(&held, named.as_deref().unwrap_or_default()),
            "case {case}: the listing must report exactly the ephemerides whose status the \
             filter admits, got {reported:?} for {held:?}"
        );
    }
}
