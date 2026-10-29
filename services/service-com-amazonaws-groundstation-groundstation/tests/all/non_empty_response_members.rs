// SPDX-License-Identifier: CC0-1.0

//! How a response of this service carries a collection member when it has NOTHING to
//! report, end to end over HTTP.
//!
//! The RULE under test: a response member whose modelled shape carries a non-zero minimum
//! length admits no empty value, so "nothing to report" is reported by LEAVING THE MEMBER
//! OUT; a member whose shape admits zero entries stays present and empty. Which way a
//! member goes is decided by its own shape, not by the operation it belongs to.
//!
//! So the suites below assert that rule over SETS of inputs rather than over one call:
//!
//! * every way a `ListEphemerides` request can select nothing — a status, a type or a
//!   satellite nothing holds, a window on either side of every expiration, filters that
//!   cannot both hold — beside the rows that do select something, at two page sizes;
//! * the same member over the lifecycle of the store it reports: absent before the first
//!   ephemeris exists, present while it does, absent again once it is deleted;
//! * a table of this service's listings spanning both readings — the page members with no
//!   modelled minimum (`ConfigList`, `SatelliteList`, `MissionProfileList`,
//!   `DataflowEndpointGroupList`, `ContactList`) against the one with a minimum
//!   (`EphemeridesList`);
//! * the same two readings inside ONE response of a single-resource read:
//!   `DescribeEphemeris` reports `errorReasons` (`len 1..50`) absent while it reports
//!   `tags` (a map with no minimum) present and empty.

use serde_json::{json, Value};
use service_com_amazonaws_groundstation_groundstation::state::{
    EPHEMERIS_ENABLED, EPHEMERIS_EXPIRED, EPHEMERIS_INVALID,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::ephemeris_catalog::{
    az_el_inline, create_ok, onboard, tle_inline, wait_out_validation, FUTURE, OEM_BODY,
};
use crate::scheduling_loop::{harness, harness_with_engine, send};

/// A window that excludes no expiration these suites store: the first second of the epoch
/// to 2100-01-01. `startTime` and `endTime` are required members, so every listing names a
/// window.
const WHOLE_WINDOW: (i64, i64) = (1, 4_102_444_800);

/// The expiration the first stored ephemeris carries; the others are offset from it.
const EDGE: i64 = 2_000_000_000;

/// A `ListEphemerides` request body: the window every listing names, plus the members of
/// `filter`.
fn request(window: (i64, i64), filter: &Value) -> Value {
    let mut body = json!({"startTime": window.0, "endTime": window.1});
    for (member, value) in filter.as_object().into_iter().flatten() {
        body[member] = value.clone();
    }
    body
}

/// The `ephemerisId`s one `ListEphemerides` page reports, asserting the rule on the way:
/// the member is absent when the page selected nothing, and a non-empty list when it
/// selected something.
async fn page_ids(
    router: &MultiServiceRouter,
    case: &str,
    route: &str,
    body: &Value,
) -> Vec<String> {
    let reply = send(router, "POST", route, Some(body.clone())).await;
    assert_eq!(
        reply.status, 200,
        "case {case}: expected ListEphemerides 200, got {} {}",
        reply.status, reply.body
    );
    match reply.body.get("ephemerides") {
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
                "case {case}: ephemerides is len 1.., so a page that selected nothing must \
                 omit the member rather than answer [], got {}",
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
    }
}

/// One row of the selection table: the window and filters a `ListEphemerides` request
/// names, and the `ephemerisId`s that request must select from the store the suite stored.
struct Selection {
    case: &'static str,
    window: (i64, i64),
    filter: Value,
    expected: Vec<String>,
}

#[tokio::test]
async fn list_ephemerides_omits_its_page_member_for_every_request_that_selects_nothing() {
    let (router, engine) = harness_with_engine();
    // Three satellites, one of which is on-boarded and holds no ephemeris at all.
    let satellites = onboard(&engine, "25994=Ohio 1,27424,33591");

    let enabled_tle = create_ok(
        &router,
        "enabled tle",
        json!({
            "name": "enabled tle",
            "satelliteId": satellites[0],
            "enabled": true,
            "expirationTime": EDGE,
            "ephemeris": tle_inline(&[(1_800_000_000, 1_800_086_400)]),
        }),
    )
    .await;
    let disabled_oem = create_ok(
        &router,
        "disabled oem",
        json!({
            "name": "disabled oem",
            "satelliteId": satellites[0],
            "enabled": false,
            "expirationTime": EDGE + 3600,
            "ephemeris": json!({"oem": {"oemData": OEM_BODY}}),
        }),
    )
    .await;
    let enabled_az_el = create_ok(
        &router,
        "enabled azEl",
        json!({
            "name": "enabled azEl",
            "satelliteId": satellites[1],
            "enabled": true,
            "expirationTime": EDGE + 7200,
            "ephemeris": az_el_inline(&[(1_800_000_000, 1_800_086_400)]),
        }),
    )
    .await;
    wait_out_validation();

    let everything = vec![
        enabled_tle.clone(),
        disabled_oem.clone(),
        enabled_az_el.clone(),
    ];

    // Each row is one way a request lands on a page, and the ids that page must report:
    // the rows expecting nothing differ along WHICH filter empties the page — status,
    // type, satellite, window on either side of every expiration, or two filters that
    // cannot both hold — and the rows expecting something are the same listing answering
    // normally.
    let rows = vec![
        Selection {
            case: "no filter but the window",
            window: WHOLE_WINDOW,
            filter: json!({}),
            expected: everything.clone(),
        },
        Selection {
            case: "a status nothing holds",
            window: WHOLE_WINDOW,
            filter: json!({"statusList": [EPHEMERIS_EXPIRED]}),
            expected: Vec::new(),
        },
        Selection {
            case: "several statuses, none of them held",
            window: WHOLE_WINDOW,
            filter: json!({"statusList": [EPHEMERIS_EXPIRED, EPHEMERIS_INVALID]}),
            expected: Vec::new(),
        },
        Selection {
            case: "a status that is held",
            window: WHOLE_WINDOW,
            filter: json!({"statusList": [EPHEMERIS_ENABLED]}),
            expected: vec![enabled_tle.clone(), enabled_az_el.clone()],
        },
        Selection {
            // `SERVICE_MANAGED` is the `EphemerisType` value that belongs to an ephemeris
            // AWS supplies itself, which no customer request creates.
            case: "a type nothing holds",
            window: WHOLE_WINDOW,
            filter: json!({"ephemerisType": "SERVICE_MANAGED"}),
            expected: Vec::new(),
        },
        Selection {
            case: "a type that is held",
            window: WHOLE_WINDOW,
            filter: json!({"ephemerisType": "OEM"}),
            expected: vec![disabled_oem.clone()],
        },
        Selection {
            case: "an on-boarded satellite holding no ephemeris",
            window: WHOLE_WINDOW,
            filter: json!({"satelliteId": satellites[2]}),
            expected: Vec::new(),
        },
        Selection {
            case: "a satellite holding one",
            window: WHOLE_WINDOW,
            filter: json!({"satelliteId": satellites[1]}),
            expected: vec![enabled_az_el.clone()],
        },
        Selection {
            case: "two filters that cannot both hold",
            window: WHOLE_WINDOW,
            filter: json!({"satelliteId": satellites[0], "ephemerisType": "AZ_EL"}),
            expected: Vec::new(),
        },
        Selection {
            case: "a window ending one second before the first expiration",
            window: (1, EDGE - 1),
            filter: json!({}),
            expected: Vec::new(),
        },
        Selection {
            case: "a window starting one second past the last expiration",
            window: (EDGE + 7201, WHOLE_WINDOW.1),
            filter: json!({}),
            expected: Vec::new(),
        },
        Selection {
            case: "a window of one instant, on no expiration",
            window: (EDGE - 1, EDGE - 1),
            filter: json!({}),
            expected: Vec::new(),
        },
        Selection {
            case: "a window of one instant, on an expiration",
            window: (EDGE, EDGE),
            filter: json!({}),
            expected: vec![enabled_tle.clone()],
        },
    ];

    for row in &rows {
        let Selection {
            case,
            window,
            filter,
            expected,
        } = row;
        let body = request(*window, filter);

        let listed = page_ids(&router, case, "/ephemerides", &body).await;
        assert_eq!(
            listed, *expected,
            "case {case} ({body}): expected {expected:?}, got {listed:?}"
        );

        // The same rule at the smallest page size the model allows: a page size cannot
        // turn an empty selection into an empty list, nor a selection into an absent
        // member.
        let first_page = page_ids(&router, case, "/ephemerides?maxResults=1", &body).await;
        let expected_first: Vec<String> = expected.iter().take(1).cloned().collect();
        assert_eq!(
            first_page, expected_first,
            "case {case} ({body}) maxResults=1: expected {expected_first:?}, got {first_page:?}"
        );
    }

    // A request that selects nothing also has nothing more to come, so it carries no
    // `nextToken` either: both members of this response are absent together.
    let nothing = send(
        &router,
        "POST",
        "/ephemerides",
        Some(request(
            WHOLE_WINDOW,
            &json!({"statusList": [EPHEMERIS_EXPIRED]}),
        )),
    )
    .await;
    assert!(
        nothing.field("nextToken").is_null(),
        "case nothing-selected: a page with nothing more to come carries no nextToken, got {}",
        nothing.body
    );
}

#[tokio::test]
async fn the_page_member_follows_the_store_it_reports_over_the_whole_lifecycle() {
    let (router, _engine) = harness_with_engine();
    let body = request(WHOLE_WINDOW, &json!({}));

    // Before the first ephemeris exists.
    let before = page_ids(&router, "before-any-create", "/ephemerides", &body).await;
    assert!(
        before.is_empty(),
        "case before-any-create: a fresh account holds no ephemeris, got {before:?}"
    );

    let id = create_ok(
        &router,
        "the only ephemeris",
        json!({
            "name": "the only ephemeris",
            "enabled": true,
            "expirationTime": FUTURE,
            "ephemeris": tle_inline(&[(1_800_000_000, 1_800_086_400)]),
        }),
    )
    .await;
    wait_out_validation();

    // While it exists: the member is present, because there is now something it may
    // report.
    let during = page_ids(&router, "one-stored", "/ephemerides", &body).await;
    assert_eq!(
        during,
        vec![id.clone()],
        "case one-stored: the listing must report the stored ephemeris, got {during:?}"
    );

    let deleted = send(&router, "DELETE", &format!("/ephemeris/{id}"), None).await;
    assert_eq!(
        deleted.status, 200,
        "case deleted: expected DeleteEphemeris 200, got {} {}",
        deleted.status, deleted.body
    );

    // And once it is gone the member is absent again, so the answer tracks the store
    // rather than whatever the previous answer carried.
    let after = page_ids(&router, "after-the-delete", "/ephemerides", &body).await;
    assert!(
        after.is_empty(),
        "case after-the-delete: a deleted ephemeris must not be listed, got {after:?}"
    );
}

/// One row of the listing table: how to ask, the member that carries its page, and whether
/// that member's modelled list shape admits the EMPTY page a fresh account selects.
struct Listing {
    case: &'static str,
    method: &'static str,
    route: &'static str,
    body: Option<Value>,
    member: &'static str,
    empty_page_is_serialized: bool,
}

#[tokio::test]
async fn whether_an_empty_page_is_serialized_follows_the_member_s_own_modelled_minimum() {
    let router = harness();

    // Source for each `empty_page_is_serialized` below: `smithy_query(query="shape",
    // shape="ConfigList,SatelliteList,MissionProfileList,DataflowEndpointGroupList,\
    // ContactList,EphemeridesList")` — the first five carry no `length` constraint at all,
    // so the empty list is a value they may take; `EphemeridesList` is
    // `@length(min: 1, max: 500)`, so it is not.
    let listings = [
        Listing {
            case: "ListConfigs",
            method: "GET",
            route: "/config",
            body: None,
            member: "configList",
            empty_page_is_serialized: true,
        },
        Listing {
            case: "ListSatellites",
            method: "GET",
            route: "/satellite",
            body: None,
            member: "satellites",
            empty_page_is_serialized: true,
        },
        Listing {
            case: "ListMissionProfiles",
            method: "GET",
            route: "/missionprofile",
            body: None,
            member: "missionProfileList",
            empty_page_is_serialized: true,
        },
        Listing {
            case: "ListDataflowEndpointGroups",
            method: "GET",
            route: "/dataflowEndpointGroup",
            body: None,
            member: "dataflowEndpointGroupList",
            empty_page_is_serialized: true,
        },
        Listing {
            case: "ListContacts",
            method: "POST",
            route: "/contacts",
            body: Some(json!({
                "statusList": ["SCHEDULED"],
                "startTime": WHOLE_WINDOW.0,
                "endTime": WHOLE_WINDOW.1,
            })),
            member: "contactList",
            empty_page_is_serialized: true,
        },
        Listing {
            case: "ListEphemerides",
            method: "POST",
            route: "/ephemerides",
            body: Some(request(WHOLE_WINDOW, &json!({}))),
            member: "ephemerides",
            empty_page_is_serialized: false,
        },
    ];

    for listing in &listings {
        let reply = send(&router, listing.method, listing.route, listing.body.clone()).await;
        assert_eq!(
            reply.status, 200,
            "case {}: expected 200 over an empty account, got {} {}",
            listing.case, reply.status, reply.body
        );
        if listing.empty_page_is_serialized {
            assert_eq!(
                reply.field(listing.member),
                &json!([]),
                "case {}: {} admits the empty list, so an empty page is serialized, got {}",
                listing.case,
                listing.member,
                reply.body
            );
        } else {
            assert!(
                reply.body.get(listing.member).is_none(),
                "case {}: {} admits no empty list, so an empty page must leave it out, got {}",
                listing.case,
                listing.member,
                reply.body
            );
        }
    }
}

#[tokio::test]
async fn one_read_answers_both_readings_for_the_members_of_its_own_response() {
    let (router, _engine) = harness_with_engine();
    let id = create_ok(
        &router,
        "a valid ephemeris",
        json!({
            "name": "a valid ephemeris",
            "enabled": true,
            "expirationTime": FUTURE,
            "ephemeris": tle_inline(&[(1_800_000_000, 1_800_086_400)]),
        }),
    )
    .await;
    wait_out_validation();

    let read = send(&router, "GET", &format!("/ephemeris/{id}"), None).await;
    assert_eq!(
        read.status, 200,
        "case valid-ephemeris: expected DescribeEphemeris 200, got {} {}",
        read.status, read.body
    );

    // `errorReasons` is `EphemerisErrorReasonList`, `@length(min: 1, max: 50)`
    // (`smithy_query(query="shape", shape="DescribeEphemerisResponse")`): an ephemeris
    // with no validation error to report leaves it out.
    assert!(
        read.body.get("errorReasons").is_none(),
        "case valid-ephemeris: errorReasons admits no empty list, so it must be left out, \
         got {}",
        read.body
    );
    // `tags` is a `TagsMap`, which carries no minimum, so the same response reports "no
    // tags" as the empty map.
    assert_eq!(
        read.field("tags"),
        &json!({}),
        "case valid-ephemeris: tags admits the empty map, so an untagged resource reports \
         it, got {}",
        read.body
    );
}
