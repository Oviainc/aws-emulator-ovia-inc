// SPDX-License-Identifier: CC0-1.0

//! `ListAntennas` (`GET /groundstation/{groundStationId}/antenna`), end to end over HTTP
//! through the same front door as the rest of this target's suites.
//!
//! The RULE under test: a listing REPORTS the store it reads, so `ListAntennas` answers
//! exactly the antennas recorded at the ground station its `groundStationId` names — one
//! `AntennaListItem` per antenna, carrying that ground station's name, the antenna's own
//! name and the Region the request addressed — in a stable order, a page at a time, and a
//! `groundStationId` naming no ground station this account knows is refused as the bad
//! parameter it is rather than answered with an empty page.
//!
//! So the suites below assert that rule over SETS of inputs rather than over one call:
//!
//! * an inventory of ground stations differing in how many antennas each has (several,
//!   one, none) and in the length of those names, including each published bound of
//!   `AntennaName`, plus a ground station this account knows only because a satellite is
//!   on-boarded to it;
//! * paging over a table of page sizes — dividing the sequence evenly, unevenly, exactly,
//!   and larger than the inventory — each of which must yield every antenna exactly once
//!   in the listing's order, with `maxResults` asserted on each published bound of
//!   `PaginationMaxResults` and one value past each;
//! * a `nextToken` this listing did not issue — including one it issued for ANOTHER ground
//!   station — refused rather than resumed;
//! * ground-station ids this account does not know, refused, beside ids the model's own
//!   length bounds refuse before the handler runs;
//! * the operator setting that describes the inventory, over a table of forms it accepts
//!   and forms it refuses, including each published bound of `AntennaName`'s length and the
//!   modelled bound on how many antennas one ground station can report.
//!
//! Neither a ground station nor an antenna is created through this API (the model declares
//! no operation that builds one), so the inventory is seeded into this service's own state
//! — exactly where `state::GROUND_STATIONS_ENV` lands it — and then read back through the
//! front door.

use std::collections::HashMap;

use serde_json::{json, Value};
use service_com_amazonaws_groundstation_groundstation::state::{
    parse_ground_stations, parse_satellites, service_state, GroundStation, BAD_REQUEST_ERROR_CODE,
    MAX_ANTENNAS_PER_GROUND_STATION,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::scheduling_loop::{harness_with_engine, list_pages, send, REGION};

/// The shortest name an antenna can carry: `AntennaName` is 4..256 characters
/// (`smithy_query(query="shape", shape="AntennaName")`).
const SHORTEST_ANTENNA: &str = "Ant1";

/// The longest name an antenna can carry, per the same bound.
fn longest_antenna() -> String {
    "A".repeat(256)
}

/// The ground stations `setting` describes, written into this service's store, as a
/// handler sees them.
fn describe(engine: &engine::InMemoryEngine, setting: &str) -> HashMap<String, GroundStation> {
    let inventory = parse_ground_stations(setting);
    let state = service_state(engine).expect("this service's state store");
    state.lock().ground_stations = inventory.clone();
    inventory
}

/// Onboard the satellites `setting` describes, so the ground stations they are on-boarded
/// to are ones this account knows even where the inventory describes none.
fn onboard(engine: &engine::InMemoryEngine, setting: &str) {
    let catalog = parse_satellites(setting);
    let state = service_state(engine).expect("this service's state store");
    state.lock().satellites = catalog;
}

/// This operation's route for `ground_station`. A `GroundStationName` may carry spaces, so
/// the one character of that set a path segment cannot hold raw is percent-encoded.
fn route(ground_station: &str) -> String {
    format!(
        "/groundstation/{}/antenna",
        ground_station.replace(' ', "%20")
    )
}

/// Every antenna `ListAntennas` reports at `ground_station`, across as many pages as it
/// takes, in the order the listing gave them.
async fn list_all(
    router: &MultiServiceRouter,
    ground_station: &str,
    page_size: Option<i64>,
) -> Vec<Value> {
    list_pages(router, &route(ground_station), "antennaList", page_size).await
}

/// A JSON member of `item`, as a string (empty when absent, so a missing member fails the
/// comparison it is part of rather than panicking here).
fn member(item: &Value, name: &str) -> String {
    item.get(name)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The antennas at `ground_station` in the order `ListAntennas` must report them: by
/// antenna name.
fn in_listing_order(
    inventory: &HashMap<String, GroundStation>,
    ground_station: &str,
) -> Vec<String> {
    let mut antennas = inventory
        .get(ground_station)
        .map(|station| station.antennas.clone())
        .unwrap_or_default();
    antennas.sort();
    antennas
}

#[tokio::test]
async fn list_antennas_reports_exactly_the_antennas_at_the_named_ground_station() {
    let (router, engine) = harness_with_engine();
    let longest = longest_antenna();

    // Rows differ in how many antennas the ground station has and in the length of their
    // names: several (named out of order, so the listing's order is the handler's and not
    // the setting's), one, and none at all.
    let setting = format!(
        "Ohio 1=Antenna 2+Antenna 1+{SHORTEST_ANTENNA}+{longest},Oregon 1=Antenna 1,Dubbo 1"
    );
    let inventory = describe(&engine, &setting);
    // A ground station this account knows only because a satellite is on-boarded to it:
    // known, with nothing to list.
    onboard(&engine, "25994=Punta Arenas 1");

    let rows = [
        ("several antennas", "Ohio 1", 4_usize),
        ("one antenna", "Oregon 1", 1),
        ("no antenna in the inventory", "Dubbo 1", 0),
        ("known only through an onboarding", "Punta Arenas 1", 0),
    ];

    for (case, ground_station, expected_count) in rows {
        let expected = in_listing_order(&inventory, ground_station);
        assert_eq!(
            expected.len(),
            expected_count,
            "case {case}: the setting should record {expected_count} antennas at \
             {ground_station}, got {expected:?}"
        );

        let reply = send(&router, "GET", &route(ground_station), None).await;
        assert_eq!(
            reply.status, 200,
            "case {case}: expected ListAntennas 200 for {ground_station}, got {} {}",
            reply.status, reply.body
        );
        assert!(
            reply.field("nextToken").is_null(),
            "case {case}: a listing with nothing more to come carries no nextToken, got {}",
            reply.body
        );
        if expected.is_empty() {
            assert_eq!(
                reply.field("antennaList"),
                &json!([]),
                "case {case}: a ground station with no antenna recorded must list none, got {}",
                reply.body
            );
        }

        let items = list_all(&router, ground_station, None).await;
        let reported: Vec<String> = items
            .iter()
            .map(|item| member(item, "antennaName"))
            .collect();
        assert_eq!(
            reported, expected,
            "case {case}: expected the recorded antennas in name order, got {reported:?}"
        );

        for (item, antenna_name) in items.iter().zip(&expected) {
            assert_eq!(
                member(item, "groundStationName"),
                ground_station,
                "case {case}: every item must name the ground station it was listed at, got {item}"
            );
            // The Region of an antenna is the Region the request addressed — the one the
            // caller's credential scope names — and is spellable as an `AWSRegion`
            // (`^[\w-]+$`, len 1..128 — `smithy_query(query="shape", shape="AWSRegion")`).
            let region = member(item, "region");
            assert_eq!(
                region, REGION,
                "case {case}: expected the Region the request addressed, got {item}"
            );
            assert!(
                (1..=128).contains(&region.len())
                    && region
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'),
                "case {case}: an AWSRegion is 1..128 characters of [\\w-], got {region:?}"
            );
            assert!(
                (4..=256).contains(&antenna_name.len()),
                "case {case}: an AntennaName is 4..256 characters, got {} in {antenna_name:?}",
                antenna_name.len()
            );
        }

        // A listing is a REPORT: reading it twice reports the same store.
        assert_eq!(
            items,
            list_all(&router, ground_station, None).await,
            "case {case}: a listing must not change what the next caller sees"
        );
    }

    // Both published bounds of `AntennaName`'s length are in the answer, not only the
    // middling names.
    let ohio = in_listing_order(&inventory, "Ohio 1");
    assert!(
        ohio.contains(&SHORTEST_ANTENNA.to_string()) && ohio.contains(&longest),
        "case name-bounds: the inventory must keep the shortest and longest allowed antenna \
         names, got {ohio:?}"
    );
}

#[tokio::test]
async fn list_antennas_pages_through_every_antenna_once_whatever_the_page_size() {
    let (router, engine) = harness_with_engine();
    // Six antennas, so a page size can divide the sequence evenly, unevenly, exactly, or
    // not bound it at all.
    let inventory = describe(
        &engine,
        "Hawaii 1=Antenna 1+Antenna 2+Antenna 3+Antenna 4+Antenna 5+Antenna 6",
    );
    assert_eq!(
        in_listing_order(&inventory, "Hawaii 1").len(),
        6,
        "case setup: the setting should record six antennas, got {inventory:?}"
    );

    let whole: Vec<String> = list_all(&router, "Hawaii 1", None)
        .await
        .iter()
        .map(|item| member(item, "antennaName"))
        .collect();
    assert_eq!(
        whole.len(),
        6,
        "case whole-listing: expected every recorded antenna, got {whole:?}"
    );

    for size in [1_i64, 2, 3, 4, 5, 6, 7, 100] {
        let paged: Vec<String> = list_all(&router, "Hawaii 1", Some(size))
            .await
            .iter()
            .map(|item| member(item, "antennaName"))
            .collect();
        assert_eq!(
            paged, whole,
            "case maxResults={size}: paging must yield every antenna exactly once, in the \
             listing's order, got {paged:?}"
        );
    }

    // Each published bound of `PaginationMaxResults` and one value past it (source:
    // `smithy_query(query="operation", operation="ListAntennas")` — range 1..100). A value
    // the model refuses never reaches the handler: this service's validator answers it.
    for (case, size, accepted) in [
        ("the lower bound", 1_i64, true),
        ("the upper bound", 100, true),
        ("one below the lower bound", 0, false),
        ("one above the upper bound", 101, false),
    ] {
        let reply = send(
            &router,
            "GET",
            &format!("{}?maxResults={size}", route("Hawaii 1")),
            None,
        )
        .await;
        if accepted {
            assert_eq!(
                reply.status, 200,
                "case maxResults={size} ({case}): expected 200, got {} {}",
                reply.status, reply.body
            );
        } else {
            assert_eq!(
                reply.status, 400,
                "case maxResults={size} ({case}): expected a refusal, got {} {}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.error_code(),
                BAD_REQUEST_ERROR_CODE,
                "case maxResults={size} ({case}): expected {BAD_REQUEST_ERROR_CODE}, got {}",
                reply.body
            );
        }
    }
}

#[tokio::test]
async fn list_antennas_refuses_a_next_token_it_did_not_issue() {
    let (router, engine) = harness_with_engine();
    describe(
        &engine,
        "Ohio 1=Antenna 1+Antenna 2+Antenna 3,Oregon 1=Antenna 1+Antenna 2+Antenna 3",
    );
    onboard(&engine, "11111,22222,33333");

    let own_token = send(
        &router,
        "GET",
        &format!("{}?maxResults=1", route("Ohio 1")),
        None,
    )
    .await
    .field("nextToken")
    .as_str()
    .map(str::to_string)
    .expect("a capped page hands back a token");

    let resumed = send(
        &router,
        "GET",
        &format!("{}?maxResults=1&nextToken={own_token}", route("Ohio 1")),
        None,
    )
    .await;
    assert_eq!(
        resumed.status, 200,
        "case own-token: a token this listing issued must resume it, got {} {}",
        resumed.status, resumed.body
    );

    let satellites_token = send(&router, "GET", "/satellite?maxResults=1", None)
        .await
        .field("nextToken")
        .as_str()
        .map(str::to_string)
        .expect("a capped page of satellites hands back a token");

    // A token addresses an offset into the sequence that issued it, so this listing accepts
    // only its own — for THIS ground station — and refuses anything else rather than
    // resuming at an offset into an unrelated sequence.
    let refused: Vec<(&str, String, &str)> = vec![
        (
            "a token this service never issued",
            "not-a-token".to_string(),
            "Ohio 1",
        ),
        (
            "a token naming no offset",
            "antennas.not-a-number".to_string(),
            "Ohio 1",
        ),
        (
            "a token of this listing without its ground station scope",
            "antennas.1".to_string(),
            "Ohio 1",
        ),
        ("a token from another listing", satellites_token, "Ohio 1"),
        (
            "this listing's token for another ground station",
            own_token.clone(),
            "Oregon 1",
        ),
    ];

    for (case, token, ground_station) in refused {
        let reply = send(
            &router,
            "GET",
            &format!("{}?maxResults=1&nextToken={token}", route(ground_station)),
            None,
        )
        .await;
        assert_eq!(
            reply.status, 431,
            "case {case} ({token}): expected 431, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "InvalidParameterException",
            "case {case} ({token}): wrong error shape, got {}",
            reply.body
        );
        assert_eq!(
            member(&reply.body, "parameterName"),
            "nextToken",
            "case {case} ({token}): the refusal must name the parameter at fault, got {}",
            reply.body
        );
    }
}

#[tokio::test]
async fn list_antennas_refuses_a_ground_station_this_account_does_not_know() {
    let (router, engine) = harness_with_engine();
    describe(&engine, "Ohio 1=Antenna 1");
    onboard(&engine, "25994=Punta Arenas 1");

    // Rows differ along WHY the account does not know the station: a name nothing records,
    // one differing only in case from a station it does know (a ground station is addressed
    // by its exact name), one that is a prefix of a known name, and one that only a
    // satellite's onboarding list from another account's shape would carry.
    for (case, ground_station) in [
        ("a name nothing records", "Bahrain 1"),
        ("a known name in another case", "ohio 1"),
        ("a prefix of a known name", "Ohio"),
        ("a name shaped like a known one", "Ohio 2"),
        ("a name with the longest allowed length", &"Z".repeat(97)),
    ] {
        let reply = send(&router, "GET", &route(ground_station), None).await;
        assert_eq!(
            reply.status, 431,
            "case {case} ({ground_station}): expected 431, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "InvalidParameterException",
            "case {case} ({ground_station}): wrong error shape, got {}",
            reply.body
        );
        assert_eq!(
            member(&reply.body, "parameterName"),
            "groundStationId",
            "case {case} ({ground_station}): the refusal must name the parameter at fault, got {}",
            reply.body
        );
        assert!(
            member(&reply.body, "message").contains(ground_station),
            "case {case}: the refusal must name the id it was given, got {}",
            reply.body
        );
    }

    // And a `groundStationId` outside the modelled length never reaches the handler: the
    // bounds are `GroundStationName`'s own 4..97 (`smithy_query(query="shape",
    // shape="GroundStationName")`), so one character past each is refused by the validator.
    for (case, ground_station) in [
        ("one character too short", "Ohi".to_string()),
        ("one character too long", "Z".repeat(98)),
    ] {
        let reply = send(&router, "GET", &route(&ground_station), None).await;
        assert_eq!(
            reply.status, 400,
            "case {case}: expected the modelled refusal, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            BAD_REQUEST_ERROR_CODE,
            "case {case}: expected {BAD_REQUEST_ERROR_CODE}, got {}",
            reply.body
        );
    }

    // A station the account DOES know still answers, so the refusals above are about the
    // id and not about the route.
    let known = send(&router, "GET", &route("Ohio 1"), None).await;
    assert_eq!(
        known.status, 200,
        "case known-station: expected 200, got {} {}",
        known.status, known.body
    );
}

/// One row of the operator-setting table: the setting, and the inventory it describes as
/// `(ground station, antennas)` pairs.
struct Inventory {
    case: &'static str,
    setting: String,
    describes: Vec<(String, Vec<String>)>,
}

impl Inventory {
    fn new(case: &'static str, setting: &str, describes: &[(&str, &[&str])]) -> Self {
        Self {
            case,
            setting: setting.to_string(),
            describes: describes
                .iter()
                .map(|(station, antennas)| {
                    (
                        (*station).to_string(),
                        antennas.iter().map(|name| (*name).to_string()).collect(),
                    )
                })
                .collect(),
        }
    }
}

#[tokio::test]
async fn the_operator_setting_describes_ground_stations_entry_by_entry() {
    // A `GroundStationName` is 4..97 characters and an `AntennaName` 4..256, both over
    // `[ a-zA-Z0-9-._:=]` (`smithy_query(query="shape", shape="GroundStationName")` /
    // `shape="AntennaName"`), so the table carries each published bound of both lengths and
    // one value past each.
    let shortest_station = "Ohio".to_string();
    let longest_station = "S".repeat(97);
    let station_too_short = "Ohi".to_string();
    let station_too_long = "S".repeat(98);
    let longest = longest_antenna();
    let antenna_too_short = "Ant".to_string();
    let antenna_too_long = "A".repeat(257);

    let rows = vec![
        Inventory::new("a bare ground station", "Ohio 1", &[("Ohio 1", &[])]),
        Inventory::new(
            "one antenna",
            "Ohio 1=Antenna 1",
            &[("Ohio 1", &["Antenna 1"])],
        ),
        Inventory::new(
            "several antennas",
            "Ohio 1=Antenna 1+Antenna 2+Antenna 3",
            &[("Ohio 1", &["Antenna 1", "Antenna 2", "Antenna 3"])],
        ),
        Inventory::new(
            "several entries, with whitespace",
            "  Ohio 1 =  Antenna 1 + Antenna 2 , Oregon 1 ,, Dubbo 1=Antenna 1 ",
            &[
                ("Ohio 1", &["Antenna 1", "Antenna 2"]),
                ("Oregon 1", &[]),
                ("Dubbo 1", &["Antenna 1"]),
            ],
        ),
        Inventory::new("unset", "", &[]),
        Inventory::new("nothing but separators", " , , ", &[]),
        Inventory::new(
            "a repeated ground station",
            "Ohio 1=Antenna 1,Ohio 1=Antenna 2",
            &[("Ohio 1", &["Antenna 1"])],
        ),
        Inventory::new(
            "a repeated antenna at one station",
            "Ohio 1=Antenna 1+Antenna 1+Antenna 2",
            &[("Ohio 1", &["Antenna 1", "Antenna 2"])],
        ),
        Inventory::new(
            "an antenna name carrying a further = after the one the entry splits on",
            "gs:region=ohio=ant:id=1",
            &[("gs:region", &["ohio=ant:id=1"])],
        ),
        Inventory::new(
            "a shortest-allowed station name",
            &format!("{shortest_station}=Antenna 1"),
            &[("Ohio", &["Antenna 1"])],
        ),
        Inventory::new(
            "a longest-allowed station name",
            &format!("{longest_station}=Antenna 1"),
            &[(&longest_station, &["Antenna 1"])],
        ),
        Inventory::new(
            "a station name one character too short",
            &format!("{station_too_short}=Antenna 1"),
            &[],
        ),
        Inventory::new(
            "a station name one character too long",
            &format!("{station_too_long}=Antenna 1"),
            &[],
        ),
        Inventory::new(
            "a station name with a character the pattern excludes",
            "Ohio/1=Antenna 1,Oregon 1=Antenna 1",
            &[("Oregon 1", &["Antenna 1"])],
        ),
        Inventory::new(
            "a shortest-allowed antenna name",
            &format!("Ohio 1={SHORTEST_ANTENNA}"),
            &[("Ohio 1", &[SHORTEST_ANTENNA])],
        ),
        Inventory::new(
            "a longest-allowed antenna name",
            &format!("Ohio 1={longest}"),
            &[("Ohio 1", &[&longest])],
        ),
        Inventory::new(
            "an antenna name one character too short",
            &format!("Ohio 1={antenna_too_short}+Antenna 1"),
            &[("Ohio 1", &["Antenna 1"])],
        ),
        Inventory::new(
            "an antenna name one character too long",
            &format!("Ohio 1={antenna_too_long}+Antenna 1"),
            &[("Ohio 1", &["Antenna 1"])],
        ),
        Inventory::new(
            "an antenna name with a character the pattern excludes",
            "Ohio 1=Antenna/1+Antenna 2",
            &[("Ohio 1", &["Antenna 2"])],
        ),
    ];

    for row in &rows {
        let inventory = parse_ground_stations(&row.setting);
        let mut described: Vec<(String, Vec<String>)> = inventory
            .values()
            .map(|station| {
                let mut antennas = station.antennas.clone();
                antennas.sort();
                (station.ground_station_name.clone(), antennas)
            })
            .collect();
        described.sort();
        let mut expected = row.describes.clone();
        for (_, antennas) in &mut expected {
            antennas.sort();
        }
        expected.sort();
        assert_eq!(
            described, expected,
            "case {}: {:?} should describe {expected:?}, got {described:?}",
            row.case, row.setting
        );

        // The inventory is keyed by the name a request addresses a ground station by.
        for (name, station) in &inventory {
            assert_eq!(
                name, &station.ground_station_name,
                "case {}: the inventory must be keyed by ground-station name, got {station:?}",
                row.case
            );
        }
    }

    // And one ground station's antennas are bounded by the modelled list length, so a
    // setting naming more than the model can report is cut to the bound rather than
    // answered past it.
    let names: Vec<String> = (0..MAX_ANTENNAS_PER_GROUND_STATION + 2)
        .map(|index| format!("Antenna {index:04}"))
        .collect();
    let inventory = parse_ground_stations(&format!("Ohio 1={}", names.join("+")));
    let described = inventory
        .get("Ohio 1")
        .expect("the entry describes Ohio 1")
        .antennas
        .clone();
    assert_eq!(
        described.len(),
        MAX_ANTENNAS_PER_GROUND_STATION,
        "case antenna-bound: a ground station reports at most \
         {MAX_ANTENNAS_PER_GROUND_STATION} antennas, got {}",
        described.len()
    );
    assert_eq!(
        described[..2],
        names[..2],
        "case antenna-bound: the bound keeps the first names the setting named, got {:?}",
        &described[..2]
    );
}
