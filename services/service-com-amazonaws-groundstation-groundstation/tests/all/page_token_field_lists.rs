// SPDX-License-Identifier: CC0-1.0

//! What a listing of this service accepts as a `nextToken`, end to end over HTTP through
//! the same front door as the rest of this target's suites.
//!
//! The RULE under test: a `nextToken` names a page boundary in ONE listing's order, so a
//! listing resumes only a token it issued itself — the token's scope AND the field list
//! that listing writes, meaning the same number of fields and, at each position, a form
//! the member the order is taken on there can carry. A token that merely starts with the
//! listing's scope but carries any other field list names no place in that order and is
//! refused as the bad parameter it is (the modelled `InvalidParameterException`, HTTP 431,
//! naming `nextToken`), rather than being read as a boundary — which would answer the
//! listing's FIRST page to a caller resuming nothing, or, for a field ordered above every
//! item, an empty page telling the caller the account holds no records at all.
//!
//! So the rule is asserted over a table rather than over one token:
//!
//! * SEVEN listings, spanning the cursors this service writes — one field, two and five,
//!   text and number, members that can carry nothing and members that cannot — and both
//!   the fixed scope an unfiltered listing issues under and the filter-fingerprinted scope
//!   a filtered one does;
//! * per listing, the field lists it never writes: too few fields, too many, and each
//!   position carrying a form that position cannot hold;
//! * beside them, that listing's OWN token, which must still resume it, and a full page
//!   walk, which must still yield every record exactly once.

use serde_json::{json, Value};
use service_com_amazonaws_groundstation_groundstation::state::{
    parse_ground_stations, parse_maintenance, parse_satellites, service_state,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::dataflow_endpoint_group_in_use_guard::group_holding;
use crate::minute_usage::reserve_pass_at_station;
use crate::scheduling_loop::{
    create_mission_profile, harness_with_engine, list_pages, send, Reply,
};

/// The ground stations this suite's account is served by, and the antennas recorded at
/// each — seeded where the operator setting lands them, because this API models no
/// operation that builds an inventory. The `ListAntennas` rows below address the first.
const INVENTORY: &str = "Ohio-1=Ant1+Ant2+Ant3,Oregon-1=Ant1";

/// Two antennas of that station taken out of service, so the reservation listing holds
/// more than one block to page — seeded where the operator setting lands them, because
/// this API models no operation that schedules maintenance.
const MAINTENANCE: &str = "Ohio-1/Ant1/PLANNED/1800000600/1800001200,\
                           Ohio-1/Ant2/UNPLANNED/1800000600/1800001200";

/// The window the reservation and contact listings below ask about, in epoch seconds.
const WINDOW: (i64, i64) = (1_800_000_000, 1_800_003_600);

/// The satellites this suite's account is on-boarded to, by NORAD catalogue number.
const ONBOARDING: &str = "11111,22222,33333";

/// One field of a cursor carrying the text `value`, as a token spells it: the form's
/// one-letter tag, then the value in hex (`state::CursorField`'s rendering, which
/// `encode_page_token` writes and the listings decode).
fn text_field(value: &str) -> String {
    format!("t{}", hex::encode(value))
}

/// One listing under test.
struct Listing {
    /// Names the listing in every case this row produces.
    name: &'static str,
    /// Its route, without a query string.
    route: String,
    /// The response member a page comes back on.
    items: &'static str,
    /// How many fields the cursor this listing writes carries — how many trailing fields
    /// of a token it issued are the cursor rather than the scope.
    cursor_fields: usize,
    /// Field lists this listing never writes, each with what is wrong with it.
    never_written: Vec<(&'static str, String)>,
    /// Field lists of exactly the shape this listing DOES write, at values its records do
    /// not carry — a boundary in its order all the same, so each must be resumed rather
    /// than refused. Beside the rows above these pin where the rule's edge sits, and pin
    /// that a refusal above came from the FIELD LIST rather than from a scope this suite
    /// cut wrongly.
    also_written: Vec<(&'static str, String)>,
}

/// The scope part of a token this listing ISSUED: everything before its last
/// `cursor_fields` dot-separated fields.
///
/// Taken off a real token rather than written out, so a row covers a listing whose scope
/// is a fixed name and one whose scope carries a fingerprint of its filters without
/// knowing which it is.
fn scope_of(token: &str, cursor_fields: usize) -> String {
    let fields: Vec<&str> = token.split('.').collect();
    assert!(
        fields.len() > cursor_fields,
        "a token carrying {cursor_fields} cursor fields has a scope before them, got {token:?}"
    );
    fields[..fields.len() - cursor_fields].join(".")
}

/// `route` with `query` appended: starting a query string, or joining the one the route
/// already carries (a window-filtered listing binds its filters there).
fn with_query(route: &str, query: &str) -> String {
    let separator = if route.contains('?') { '&' } else { '?' };
    format!("{route}{separator}{query}")
}

/// A `parameterName` (or `message`) of a refusal, as a string; empty when the member is
/// absent, so a missing member fails the comparison it is part of.
fn member(body: &Value, name: &str) -> String {
    body.get(name)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// An account holding enough of each resource for every listing below to page.
async fn stocked() -> MultiServiceRouter {
    let (router, engine) = harness_with_engine();
    {
        let state = service_state(&*engine).expect("this service's state store");
        let mut guard = state.lock();
        guard.ground_stations = parse_ground_stations(INVENTORY);
        guard.satellites = parse_satellites(ONBOARDING);
        guard.maintenance_reservations = parse_maintenance(MAINTENANCE);
    }

    // Each mission profile brings its own tracking config, so this stocks both the mission
    // profile store and the config store.
    create_mission_profile(&router, "token profile a", 60, None, None).await;
    create_mission_profile(&router, "token profile b", 60, None, None).await;
    group_holding(&router, "token group a", "endpoint", &["Endpoint-A"]).await;
    group_holding(&router, "token group b", "endpoint", &["Endpoint-B"]).await;

    router
}

/// The listings under test, with the field lists each never writes.
fn listings() -> Vec<Listing> {
    // The two fields of a configs cursor and the second of a satellites cursor can each
    // carry nothing (`a`), so a field list differing only in that is one the listing CAN
    // write and is not a row here: the rows are the lists it cannot.
    vec![
        Listing {
            name: "ListConfigs",
            route: "/config".to_string(),
            items: "configList",
            cursor_fields: 2,
            never_written: vec![
                (
                    "one number field where it writes two text fields",
                    "n1".to_string(),
                ),
                ("one text field where it writes two", text_field("zzz")),
                (
                    "three text fields where it writes two",
                    format!(
                        "{}.{}.{}",
                        text_field("tracking"),
                        text_field("uplink-echo"),
                        text_field("s3-recording")
                    ),
                ),
                (
                    "a number in the first of its two text fields",
                    format!("n1.{}", text_field("tracking")),
                ),
                (
                    "a number in the second of its two text fields",
                    format!("{}.n1", text_field("tracking")),
                ),
            ],
            also_written: vec![
                (
                    "two text fields, as it writes them",
                    format!("{}.{}", text_field("antenna-downlink"), text_field("zzz")),
                ),
                (
                    "two fields carrying nothing, as both members may",
                    "a.a".to_string(),
                ),
            ],
        },
        Listing {
            name: "ListMissionProfiles",
            route: "/missionprofile".to_string(),
            items: "missionProfileList",
            cursor_fields: 1,
            never_written: vec![
                ("a number where it writes text", "n1".to_string()),
                (
                    "two text fields where it writes one",
                    format!("{}.{}", text_field("alpha"), text_field("beta")),
                ),
                (
                    "a number beside a text where it writes one text",
                    format!("n1.{}", text_field("alpha")),
                ),
            ],
            also_written: vec![("one text field, as it writes it", text_field("zzzzzzzz"))],
        },
        Listing {
            name: "ListDataflowEndpointGroups",
            route: "/dataflowEndpointGroup".to_string(),
            items: "dataflowEndpointGroupList",
            cursor_fields: 1,
            never_written: vec![
                ("a number where it writes text", "n1".to_string()),
                (
                    "nothing beside a text where it writes one field",
                    format!("a.{}", text_field("alpha")),
                ),
                (
                    "two text fields where it writes one",
                    format!("{}.{}", text_field("alpha"), text_field("beta")),
                ),
            ],
            also_written: vec![(
                "one text field, as it writes it",
                text_field("00000000-0000-0000-0000-000000000000"),
            )],
        },
        Listing {
            name: "ListSatellites",
            route: "/satellite".to_string(),
            items: "satellites",
            cursor_fields: 2,
            never_written: vec![
                (
                    "one text field where it writes a number and a text",
                    text_field("zzz"),
                ),
                (
                    "one number field where it writes two",
                    "n22222".to_string(),
                ),
                (
                    "a text in the first of its fields, which carries a catalogue number",
                    format!("{}.{}", text_field("11111"), text_field("alpha")),
                ),
                (
                    "a number in the second of its fields, which carries an id",
                    "n11111.n1".to_string(),
                ),
                (
                    "nothing in the first of its fields, which is never absent",
                    format!("a.{}", text_field("alpha")),
                ),
            ],
            also_written: vec![
                (
                    "a number then a text, as it writes them",
                    format!("n11111.{}", text_field("zzz")),
                ),
                (
                    "a number then nothing, as the second member may be",
                    "n11111.a".to_string(),
                ),
            ],
        },
        Listing {
            // A filtered listing: its scope carries a fingerprint of the ground station it
            // pages the antennas of, and the rule holds there the same way.
            name: "ListAntennas",
            route: "/groundstation/Ohio-1/antenna".to_string(),
            items: "antennaList",
            cursor_fields: 1,
            never_written: vec![
                ("a number where it writes an antenna name", "n1".to_string()),
                (
                    "nothing where it writes a name that is never absent",
                    "a".to_string(),
                ),
                (
                    "two text fields where it writes one",
                    format!("{}.{}", text_field("Ant1"), text_field("Ant2")),
                ),
            ],
            also_written: vec![("one text field, as it writes it", text_field("Ant0"))],
        },
        Listing {
            // A listing whose scope carries a fingerprint of a filter the request left
            // out, which is a scope of its own all the same.
            name: "ListGroundStations",
            route: "/groundstation".to_string(),
            items: "groundStationList",
            cursor_fields: 1,
            never_written: vec![
                ("a number where it writes text", "n1".to_string()),
                (
                    "two text fields where it writes one",
                    format!("{}.{}", text_field("Ohio-1"), text_field("Oregon-1")),
                ),
            ],
            also_written: vec![("one text field, as it writes it", text_field("zzz"))],
        },
        Listing {
            // The longest cursor of this service, and the one whose every position is
            // required: two instants (held as the numbers that order as they do), then the
            // antenna, the reservation type and what the block is held for.
            name: "ListGroundStationReservations",
            route: format!(
                "/groundstation/Ohio-1/reservation?startTime={}&endTime={}",
                WINDOW.0, WINDOW.1
            ),
            items: "reservationList",
            cursor_fields: 5,
            never_written: vec![
                (
                    "four fields where it writes five",
                    format!(
                        "n{}.n{}.{}.{}",
                        WINDOW.0,
                        WINDOW.1,
                        text_field("Ant1"),
                        text_field("MAINTENANCE")
                    ),
                ),
                (
                    "six fields where it writes five",
                    format!(
                        "n{}.n{}.{}.{}.{}.{}",
                        WINDOW.0,
                        WINDOW.1,
                        text_field("Ant1"),
                        text_field("MAINTENANCE"),
                        text_field("PLANNED"),
                        text_field("PLANNED")
                    ),
                ),
                (
                    "a text in the first of its fields, which carries an instant",
                    format!(
                        "{}.n{}.{}.{}.{}",
                        text_field("zzz"),
                        WINDOW.1,
                        text_field("Ant1"),
                        text_field("MAINTENANCE"),
                        text_field("PLANNED")
                    ),
                ),
                (
                    "nothing in the second of its fields, which is never absent",
                    format!(
                        "n{}.a.{}.{}.{}",
                        WINDOW.0,
                        text_field("Ant1"),
                        text_field("MAINTENANCE"),
                        text_field("PLANNED")
                    ),
                ),
                (
                    "a number in the third of its fields, which carries an antenna name",
                    format!(
                        "n{}.n{}.n1.{}.{}",
                        WINDOW.0,
                        WINDOW.1,
                        text_field("MAINTENANCE"),
                        text_field("PLANNED")
                    ),
                ),
            ],
            also_written: vec![(
                "two numbers then three texts, as it writes them",
                format!(
                    "n{}.n{}.{}.{}.{}",
                    WINDOW.0,
                    WINDOW.0,
                    text_field("Ant0"),
                    text_field("MAINTENANCE"),
                    text_field("PLANNED")
                ),
            )],
        },
    ]
}

#[tokio::test]
async fn a_listing_refuses_a_next_token_whose_field_list_it_never_writes() {
    let router = stocked().await;

    for listing in listings() {
        let name = listing.name;

        // The token this listing hands out, carrying the field list it writes.
        let own_token = send(
            &router,
            "GET",
            &with_query(&listing.route, "maxResults=1"),
            None,
        )
        .await
        .field("nextToken")
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| {
            panic!("case {name}: a capped page of a stocked store hands back a token")
        });

        let resumed = send(
            &router,
            "GET",
            &with_query(&listing.route, &format!("maxResults=1&nextToken={own_token}")),
            None,
        )
        .await;
        assert_eq!(
            resumed.status, 200,
            "case {name} own-token: a token this listing issued must resume it, got {} {}",
            resumed.status, resumed.body
        );

        // Every record the listing holds, so the forged rows below are measured against a
        // sequence that is not empty: honouring one of them answers either this whole
        // first page or none of these records at all.
        let whole = list_pages(&router, &listing.route, listing.items, None).await;
        assert!(
            whole.len() > 1,
            "case {name}: this suite's account must hold more than one record to page, got {}",
            whole.len()
        );

        let scope = scope_of(&own_token, listing.cursor_fields);
        for (case, field_list) in &listing.never_written {
            let token = format!("{scope}.{field_list}");
            let reply = send(
                &router,
                "GET",
                &with_query(&listing.route, &format!("maxResults=1&nextToken={token}")),
                None,
            )
            .await;
            assert_eq!(
                reply.status, 431,
                "case {name} {case} ({token}): expected 431, got {} {}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.error_code(),
                "InvalidParameterException",
                "case {name} {case} ({token}): wrong error shape, got {}",
                reply.body
            );
            assert_eq!(
                member(&reply.body, "parameterName"),
                "nextToken",
                "case {name} {case} ({token}): the refusal must name the parameter at fault, \
                 got {}",
                reply.body
            );
            assert!(
                member(&reply.body, "message").contains("nextToken was not issued"),
                "case {name} {case} ({token}): the refusal must say the token was not issued \
                 by a previous call, got {}",
                reply.body
            );
        }

        for (case, field_list) in &listing.also_written {
            let token = format!("{scope}.{field_list}");
            let reply = send(
                &router,
                "GET",
                &with_query(&listing.route, &format!("maxResults=1&nextToken={token}")),
                None,
            )
            .await;
            assert_eq!(
                reply.status, 200,
                "case {name} {case} ({token}): a field list this listing writes names a \
                 boundary in its order and must be resumed, got {} {}",
                reply.status, reply.body
            );
        }

        // A refused token changes nothing: the listing still reports every record, and a
        // walk at a page size of one still yields each exactly once.
        let walked = list_pages(&router, &listing.route, listing.items, Some(1)).await;
        assert_eq!(
            walked, whole,
            "case {name}: the walk must yield the same records in the same order as the \
             unpaged listing, got {walked:?}"
        );
    }
}

/// One page of `ListContacts` over [`WINDOW`], carrying `token` when there is one.
///
/// `ListContacts` binds every input member in the BODY — its `nextToken` carries no
/// `@httpQuery` — so a resumed page is the same body carrying the token.
async fn contacts_page(router: &MultiServiceRouter, token: Option<&str>) -> Reply {
    let mut body = json!({
        "startTime": WINDOW.0,
        "endTime": WINDOW.1,
        "statusList": ["SCHEDULING", "SCHEDULED"],
        "maxResults": 1,
    });
    if let Some(token) = token {
        body["nextToken"] = json!(token);
    }
    send(router, "POST", "/contacts", Some(body)).await
}

/// Every `contactId` `ListContacts` reports over [`WINDOW`], across as many pages as it
/// takes, sorted — so an assertion is about the SET the walk yielded.
async fn contact_ids(router: &MultiServiceRouter, case: &str) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    // One more page than this suite's store could need at a page size of one, so a token
    // that failed to advance fails as a bounded assertion instead of looping forever.
    for page in 0..16 {
        let reply = contacts_page(router, token.as_deref()).await;
        assert_eq!(
            reply.status, 200,
            "case {case} page-{page}: expected ListContacts 200, got {} {}",
            reply.status, reply.body
        );
        for item in reply
            .field("contactList")
            .as_array()
            .cloned()
            .unwrap_or_default()
        {
            ids.push(member(&item, "contactId"));
        }
        token = reply.field("nextToken").as_str().map(str::to_string);
        if token.is_none() {
            ids.sort();
            return ids;
        }
    }
    panic!("case {case}: the listing of contacts never closed");
}

/// `ListContacts` binds every input member in the BODY, and its order is COMPOSED — the
/// contacts the store holds by id, then the windows an availability search computed by
/// start time — so it writes one field list per part and sits in a suite of its own.
#[tokio::test]
async fn list_contacts_refuses_a_next_token_whose_field_list_it_never_writes() {
    let (router, engine) = harness_with_engine();
    {
        let state = service_state(&*engine).expect("this service's state store");
        state.lock().ground_stations = parse_ground_stations(INVENTORY);
    }
    let profile_arn = create_mission_profile(&router, "token contacts", 60, None, None).await;
    // Two reserved contacts inside the window. An antenna serves one pass at a time, so
    // the two windows do not overlap.
    reserve_pass_at_station(&router, &profile_arn, "first", "Ohio-1", WINDOW.0, 600).await;
    reserve_pass_at_station(
        &router,
        &profile_arn,
        "second",
        "Ohio-1",
        WINDOW.0 + 1_200,
        600,
    )
    .await;

    let own_token = contacts_page(&router, None)
        .await
        .field("nextToken")
        .as_str()
        .map(str::to_string)
        .expect("a capped page of two reserved contacts hands back a token");
    let resumed = contacts_page(&router, Some(&own_token)).await;
    assert_eq!(
        resumed.status, 200,
        "case own-token: a token this listing issued must resume it, got {} {}",
        resumed.status, resumed.body
    );

    let scope = scope_of(&own_token, 2);

    // Every contact the listing reports when nothing is resumed, so the rows below are
    // measured against a sequence that is not empty.
    let whole = contact_ids(&router, "unresumed").await;
    assert_eq!(
        whole.len(),
        2,
        "case unresumed: this suite reserved two contacts in the window, got {whole:?}"
    );

    // Field lists this listing never writes: another arity, a form neither part takes its
    // key off, a part marker it never writes, or one part's key behind the OTHER part's
    // marker.
    for (case, field_list) in [
        ("one number field where it writes two", "n0".to_string()),
        (
            "three fields where it writes two",
            format!("n0.{}.{}", text_field("alpha"), text_field("beta")),
        ),
        (
            "a text in the first field, which marks which part of the order",
            format!("{}.{}", text_field("0"), text_field("alpha")),
        ),
        (
            "nothing in the first field, which is never absent",
            format!("a.{}", text_field("alpha")),
        ),
        (
            "a text then a number, which neither part writes",
            format!("{}.n1", text_field("alpha")),
        ),
        (
            "a part marker neither part of the order carries",
            format!("n7.{}", text_field("alpha")),
        ),
        (
            "the stored part's text key behind the window part's marker, which sorts above \
             every record",
            format!("n1.{}", text_field("alpha")),
        ),
        (
            "the window part's number key behind the stored part's marker, which sorts \
             below every record",
            "n0.n5".to_string(),
        ),
    ] {
        let reply = contacts_page(&router, Some(&format!("{scope}.{field_list}"))).await;
        assert_eq!(
            reply.status, 431,
            "case {case} ({field_list}): expected 431, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "InvalidParameterException",
            "case {case} ({field_list}): wrong error shape, got {}",
            reply.body
        );
        assert_eq!(
            member(&reply.body, "parameterName"),
            "nextToken",
            "case {case} ({field_list}): the refusal must name the parameter at fault, got {}",
            reply.body
        );
        assert!(
            member(&reply.body, "message").contains("nextToken was not issued"),
            "case {case} ({field_list}): the refusal must say the token was not issued by a \
             previous call, got {}",
            reply.body
        );
    }

    // Both field lists it DOES write, at values its records do not carry: each names a
    // boundary in its order and is resumed.
    for (case, field_list) in [
        (
            "the stored-contact part: a number then a text",
            format!("n0.{}", text_field("zzz")),
        ),
        (
            "the computed-window part: a number then a number",
            "n1.n0".to_string(),
        ),
        (
            "the computed-window part with no start instant",
            "n1.a".to_string(),
        ),
    ] {
        let reply = contacts_page(&router, Some(&format!("{scope}.{field_list}"))).await;
        assert_eq!(
            reply.status, 200,
            "case {case} ({field_list}): a field list this listing writes names a boundary in \
             its order and must be resumed, got {} {}",
            reply.status, reply.body
        );
    }

    // A refused token changes nothing: the walk still reports both contacts, rather than
    // the empty page a forged cross-part token used to answer.
    let walked = contact_ids(&router, "after-the-refusals").await;
    assert_eq!(
        walked, whole,
        "case after-the-refusals: the walk must still report every contact, got {walked:?}"
    );
}
