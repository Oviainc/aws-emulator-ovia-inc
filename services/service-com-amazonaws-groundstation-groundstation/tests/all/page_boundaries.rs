// SPDX-License-Identifier: CC0-1.0

//! What a listing's `nextToken` addresses, over sequences that CHANGE while a caller is
//! paging — end to end over HTTP through the same front door as the rest of this target's
//! suites.
//!
//! The RULE under test: a page token names the record the page ENDED ON, so the next page
//! is the records ordered after it — never a count of the records before it. Every listing
//! of this service recomputes its sequence from the live store on each call (and an
//! `AVAILABLE` search computes it from the clock and the antenna time held now), so a
//! record can leave the sequence between two pages; a count would then address a different
//! record and the ones it passed over would be returned by no page of the walk.
//!
//! So the suites below assert the rule over SETS of inputs rather than over one call:
//!
//! * a COMPUTED sequence — an `AVAILABLE` search — over a table of windows reserved
//!   between the first page and the rest: one the first page opened with, the one it ended
//!   on, the one the next page would open with, one further down the walk, and none at all;
//! * a STORED sequence — `ListMissionProfiles` — over the same table of positions, deleting
//!   the record instead of reserving it, at two page sizes.
//!
//! Each row asserts the whole walk: every record still in the sequence and ordered at or
//! after the boundary comes back, exactly once, in the listing's own order.

use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use service_com_amazonaws_groundstation_groundstation::state::{service_state, CONTACT_AVAILABLE};
use smithy_server_runtime::MultiServiceRouter;

use crate::ephemeris_catalog::{az_el_inline, create_ok, onboard, wait_out_validation};
use crate::scheduling_loop::{
    create_mission_profile, harness, harness_with_engine, list_pages, send, Reply,
};

/// The ground station every search in this suite is for: the station
/// [`az_el_inline`] supplies its pointing data for, and one whose antenna inventory this
/// account has not been told about — so it has the single antenna the placement numbers
/// for such a station, and a reservation over a window takes the only antenna time there
/// is for it.
const GROUND_STATION: &str = "Ohio 1";

/// The NORAD catalogue number this suite's satellite is on-boarded under, inside
/// `state::NORAD_SATELLITE_ID_RANGE`.
const NORAD: i32 = 25_994;

/// How many windows (and how many stored records) each row of the tables below walks: more
/// than one page at the page sizes used, with records both before and behind the boundary.
const RECORDS: usize = 6;

/// How long the pass one segment of [`az_el_inline`] describes lasts: its five points are
/// ten seconds apart and every one of them is above the horizon, so the pass runs from the
/// segment's reference epoch to forty seconds after it.
const PASS_SECONDS: i64 = 40;

/// The `minimumViableContactDurationSeconds` of the mission profile every search here
/// names: under [`PASS_SECONDS`], so the passes are long enough to be offered.
const MINIMUM_SECONDS: i64 = 30;

/// The pre- and post-pass durations of that profile.
const PRE_PASS: i64 = 120;
const POST_PASS: i64 = 60;

/// The page size every walk below pages at.
const PAGE: i64 = 2;

/// `now`, in epoch seconds: an availability search is about what can STILL be flown, so
/// the pointing data is placed relative to the clock rather than at a fixed instant.
fn now_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs() as i64
}

/// An account whose satellite has [`RECORDS`] future passes over [`GROUND_STATION`], with
/// the router in front of it and a mission profile to search under.
struct Search {
    router: MultiServiceRouter,
    /// The ARN `ListContacts` names the satellite by.
    satellite_arn: String,
    /// The ARN of the mission profile the search is under.
    profile_arn: String,
    /// The `(startTime, endTime)` of every pass the pointing data describes, in the
    /// start-time order a search offers them in.
    windows: Vec<(i64, i64)>,
}

/// A fresh account on-boarded to one satellite, holding one enabled `azEl` ephemeris whose
/// segments describe [`RECORDS`] passes a day apart, the first of them `base`.
async fn search(base: i64) -> Search {
    let (router, engine) = harness_with_engine();
    let onboarded = onboard(&engine, &format!("{NORAD}={GROUND_STATION}"));
    let satellite_id = onboarded
        .first()
        .cloned()
        .expect("the satellite is on-boarded");
    let satellite_arn = service_state(&*engine)
        .expect("this service's state store")
        .lock()
        .satellites
        .get(&satellite_id)
        .expect("the on-boarded satellite")
        .satellite_arn
        .clone();

    // One segment per pass, a day apart so no two of them touch: each describes one pass
    // from its own reference epoch.
    let segments: Vec<(i64, i64)> = (0..RECORDS as i64)
        .map(|day| {
            let start = base + day * 86_400;
            (start, start + 3_600)
        })
        .collect();
    create_ok(
        &router,
        "paged pointing data",
        json!({
            "name": "paged pointing data",
            "enabled": true,
            "satelliteId": satellite_id,
            "ephemeris": az_el_inline(&segments),
        }),
    )
    .await;
    wait_out_validation();

    let profile_arn = create_mission_profile(
        &router,
        "page boundaries",
        MINIMUM_SECONDS,
        Some(PRE_PASS),
        Some(POST_PASS),
    )
    .await;
    Search {
        router,
        satellite_arn,
        profile_arn,
        windows: segments
            .iter()
            .map(|(start, _)| (*start, start + PASS_SECONDS))
            .collect(),
    }
}

/// The `AVAILABLE` search request body for `search`, over a window covering every pass it
/// holds.
fn availability(search: &Search) -> Value {
    let first = search.windows.first().expect("a pass").0;
    let last = search.windows.last().expect("a pass").1;
    json!({
        "statusList": [CONTACT_AVAILABLE],
        "startTime": first - 3_600,
        "endTime": last + 3_600,
        "groundStation": GROUND_STATION,
        "satelliteArn": search.satellite_arn,
        "missionProfileArn": search.profile_arn,
    })
}

/// One page of `ListContacts` for `body`, resuming after `token` when there is one: the
/// `(startTime, endTime)` of each record on it, and the token that resumes after it.
async fn contacts_page(
    router: &MultiServiceRouter,
    body: &Value,
    token: Option<&str>,
    case: &str,
) -> (Vec<(i64, i64)>, Option<String>) {
    let mut request = body.clone();
    request["maxResults"] = json!(PAGE);
    if let Some(token) = token {
        request["nextToken"] = json!(token);
    }
    let reply = send(router, "POST", "/contacts", Some(request)).await;
    assert_eq!(
        reply.status, 200,
        "case {case}: expected ListContacts 200, got {} {}",
        reply.status, reply.body
    );
    let page = reply
        .field("contactList")
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: contactList must be a list, got {}", reply.body))
        .iter()
        .map(|item| (seconds(item, "startTime", case), seconds(item, "endTime", case)))
        .collect();
    (page, reply.field("nextToken").as_str().map(str::to_string))
}

/// The epoch-seconds member `member` of `item`, which every window this suite stores is
/// placed at a whole second.
fn seconds(item: &Value, member: &str, case: &str) -> i64 {
    item.get(member)
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("case {case}: a record must carry {member}, got {item}")) as i64
}

/// `ReserveContact` over `window` for the pairing `search` is for.
async fn reserve(search: &Search, window: (i64, i64), case: &str) -> Reply {
    let reply = send(
        &search.router,
        "POST",
        "/contact",
        Some(json!({
            "missionProfileArn": search.profile_arn,
            "groundStation": GROUND_STATION,
            "satelliteArn": search.satellite_arn,
            "startTime": window.0,
            "endTime": window.1,
        })),
    )
    .await;
    assert_eq!(
        reply.status, 200,
        "case {case}: a window the service offered must be reservable, got {} {}",
        reply.status, reply.body
    );
    reply
}

/// The positions a record can leave the sequence from, relative to the boundary a first
/// page of [`PAGE`] records ends on: the two rows inside that page are records the walk
/// has already returned, and the rest are records still ahead of it.
const POSITIONS: [(&str, Option<usize>); 5] = [
    ("nothing leaves the sequence", None),
    ("the record the first page opened with", Some(0)),
    ("the record the first page ended on", Some(1)),
    ("the record the next page would open with", Some(2)),
    ("a record further down the walk", Some(4)),
];

#[tokio::test]
async fn an_availability_walk_resumes_after_the_window_it_ended_on_when_the_offers_change() {
    for (case, reserved) in POSITIONS {
        // A fresh account per row: a reservation is a write, so one row's antenna time must
        // not be held when the next row searches.
        let search = search(now_seconds() + 3_600).await;
        let request = availability(&search);

        let (first, token) = contacts_page(&search.router, &request, None, case).await;
        assert_eq!(
            first,
            search.windows[..PAGE as usize],
            "case {case}: the first page must offer the earliest windows, got {first:?}"
        );
        let mut token = token.unwrap_or_else(|| {
            panic!("case {case}: a page with more to come must carry a nextToken")
        });

        // The sequence changes underneath the walk: the caller reserves one of the offered
        // windows, which is the discovery-then-reserve loop the search exists to serve, so
        // the window stops being offered and everything behind it moves one place earlier.
        if let Some(index) = reserved {
            reserve(&search, search.windows[index], case).await;
        }

        let mut walked = first;
        // More pages than the sequence could need at this page size, so a token that fails
        // to advance fails the row instead of looping forever.
        for _ in 0..RECORDS {
            let (page, next) = contacts_page(&search.router, &request, Some(&token), case).await;
            walked.extend(page);
            match next {
                None => break,
                Some(next) => token = next,
            }
        }

        // What the walk owes: the records it already returned, then every record still
        // offered and ordered after the boundary.
        let mut expected: Vec<(i64, i64)> = search.windows[..PAGE as usize].to_vec();
        expected.extend(
            search
                .windows
                .iter()
                .enumerate()
                .filter(|(index, _)| *index >= PAGE as usize && Some(*index) != reserved)
                .map(|(_, window)| *window),
        );
        assert_eq!(
            walked, expected,
            "case {case}: the walk must return every window still offered after the boundary, \
             once each, got {walked:?}"
        );
    }
}

#[tokio::test]
async fn a_stored_listing_resumes_after_the_record_it_ended_on_when_the_store_changes() {
    for (case, deleted) in POSITIONS {
        let router = harness();
        for index in 0..RECORDS {
            create_mission_profile(
                &router,
                &format!("page boundary {index}"),
                MINIMUM_SECONDS,
                Some(PRE_PASS),
                Some(POST_PASS),
            )
            .await;
        }
        // The order the listing holds them in, read off the listing itself rather than
        // assumed from the order they were created in.
        let ids: Vec<String> = list_pages(&router, "/missionprofile", "missionProfileList", None)
            .await
            .iter()
            .map(|item| {
                item.get("missionProfileId")
                    .and_then(Value::as_str)
                    .unwrap_or_else(|| panic!("case {case}: a record must carry a missionProfileId"))
                    .to_string()
            })
            .collect();
        assert_eq!(
            ids.len(),
            RECORDS,
            "case {case}: the store must hold {RECORDS} profiles, got {}",
            ids.len()
        );

        let first = send(
            &router,
            "GET",
            &format!("/missionprofile?maxResults={PAGE}"),
            None,
        )
        .await;
        assert_eq!(
            first.status, 200,
            "case {case}: expected ListMissionProfiles 200, got {} {}",
            first.status, first.body
        );
        let mut walked = profile_ids(&first, case);
        assert_eq!(
            walked,
            ids[..PAGE as usize],
            "case {case}: the first page must hold the first records of the order, got {walked:?}"
        );
        let mut token = first
            .field("nextToken")
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| {
                panic!("case {case}: a page with more to come must carry a nextToken")
            });

        // The sequence changes underneath the walk: one of the records is deleted.
        if let Some(index) = deleted {
            let removed = send(
                &router,
                "DELETE",
                &format!("/missionprofile/{}", ids[index]),
                None,
            )
            .await;
            assert_eq!(
                removed.status, 200,
                "case {case}: expected DeleteMissionProfile 200, got {} {}",
                removed.status, removed.body
            );
        }

        for _ in 0..RECORDS {
            let reply = send(
                &router,
                "GET",
                &format!("/missionprofile?maxResults={PAGE}&nextToken={token}"),
                None,
            )
            .await;
            assert_eq!(
                reply.status, 200,
                "case {case}: expected ListMissionProfiles 200, got {} {}",
                reply.status, reply.body
            );
            walked.extend(profile_ids(&reply, case));
            match reply.field("nextToken").as_str() {
                None => break,
                Some(next) => token = next.to_string(),
            }
        }

        let mut expected: Vec<String> = ids[..PAGE as usize].to_vec();
        expected.extend(
            ids.iter()
                .enumerate()
                .filter(|(index, _)| *index >= PAGE as usize && Some(*index) != deleted)
                .map(|(_, id)| id.clone()),
        );
        assert_eq!(
            walked, expected,
            "case {case}: the walk must return every record still held after the boundary, once \
             each, got {walked:?}"
        );
    }
}

/// The `missionProfileId` of each record on the page `reply` carries.
fn profile_ids(reply: &Reply, case: &str) -> Vec<String> {
    reply
        .field("missionProfileList")
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case}: missionProfileList must be a list, got {}",
                reply.body
            )
        })
        .iter()
        .map(|item| {
            item.get("missionProfileId")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case}: a record must carry a missionProfileId"))
                .to_string()
        })
        .collect()
}
