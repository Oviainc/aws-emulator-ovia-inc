// SPDX-License-Identifier: CC0-1.0

//! `ListSatellites` (`GET /satellite`), end to end over HTTP through the same front door
//! as the rest of this target's suites.
//!
//! The RULE under test: a listing REPORTS the store it reads, so `ListSatellites` answers
//! exactly the satellites this account is on-boarded to at the moment of the call — one
//! `SatelliteListItem` per on-boarded satellite, carrying that satellite's own
//! `satelliteId`, `satelliteArn`, `noradSatelliteID`, `groundStations` and
//! `currentEphemeris` — in a stable order, a page at a time, and changes nothing a later
//! caller sees.
//!
//! So the suites below assert that rule over SETS of inputs rather than over one call:
//!
//! * a catalogue of satellites differing in NORAD catalogue number (each published bound
//!   of `noradSatelliteID` among them), in how many ground stations they are on-boarded
//!   to, and in whether an ephemeris is known for them — plus the account on-boarded to
//!   none;
//! * paging over a table of page sizes — dividing the sequence evenly, unevenly, exactly,
//!   and larger than the store — each of which must yield every satellite exactly once in
//!   the listing's order, with `maxResults` asserted on each published bound of
//!   `PaginationMaxResults` and one value past each;
//! * a `nextToken` this listing did not issue, refused rather than resumed;
//! * the operator setting that onboards a satellite, over a table of forms it accepts and
//!   forms it refuses, including each published bound of `GroundStationName`'s length.
//!
//! Satellites are not created through this API (the model declares no operation that
//! onboards one), so the catalogue is seeded into this service's own state — exactly where
//! `state::SATELLITES_ENV` lands it — and then read back through the front door.

use std::collections::HashMap;

use serde_json::{json, Value};
use service_com_amazonaws_groundstation_groundstation::ops::types::EphemerisMetaData;
use service_com_amazonaws_groundstation_groundstation::state::{
    parse_satellites, service_state, Satellite, BAD_REQUEST_ERROR_CODE,
    MAX_GROUND_STATIONS_PER_SATELLITE, NORAD_SATELLITE_ID_RANGE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::scheduling_loop::{harness_with_engine, is_uuid, list_pages, send};

/// The catalogue the front-door suites read: satellites differing in NORAD catalogue
/// number — including both published bounds of `noradSatelliteID`
/// (`smithy_query(query="shape", shape="noradSatelliteID")` — 0..339999) — and in how many
/// ground stations each is on-boarded to (none, one, three).
const CATALOG: &str = "25994=Ohio 1+Oregon 1+Hawaii 1,27424=Punta Arenas 1,0,339999,43013";

/// The satellites `setting` onboards, written into this service's store, as a handler
/// sees them.
fn onboard(engine: &engine::InMemoryEngine, setting: &str) -> HashMap<String, Satellite> {
    let catalog = parse_satellites(setting);
    let state = service_state(engine).expect("this service's state store");
    state.lock().satellites = catalog.clone();
    catalog
}

/// The on-boarded satellites in the order `ListSatellites` must report them: by NORAD
/// catalogue number, then by `satelliteId`.
fn in_listing_order(catalog: &HashMap<String, Satellite>) -> Vec<Satellite> {
    let mut ordered: Vec<Satellite> = catalog.values().cloned().collect();
    ordered.sort_by(|left, right| {
        (left.norad_satellite_id, &left.satellite_id)
            .cmp(&(right.norad_satellite_id, &right.satellite_id))
    });
    ordered
}

/// Every item `ListSatellites` returns, across as many pages as it takes, in the order the
/// listing gave them. `page_size` is the caller's `maxResults`, or `None` to name none.
async fn list_all(router: &MultiServiceRouter, page_size: Option<i64>) -> Vec<Value> {
    list_pages(router, "/satellite", "satellites", page_size).await
}

/// Assert `arn` is a `satelliteArn` for `satellite_id`: `arn:aws:groundstation` with an
/// EMPTY region segment (the modelled pattern makes the region optional — an on-boarded
/// satellite belongs to the account, not to one region), the emulator's account, the
/// `satellite/<uuid>` resource, and a length inside the modelled 82..132.
fn assert_satellite_arn(arn: &str, satellite_id: &str, case: &str) {
    let expected = format!(
        "arn:aws:groundstation::{}:satellite/{satellite_id}",
        engine::emulator_account_id()
    );
    assert_eq!(
        arn, expected,
        "case {case}: expected the region-less satellite ARN {expected}, got {arn:?}"
    );
    assert!(
        (82..=132).contains(&arn.len()),
        "case {case}: a satelliteArn is 82..132 characters, got {} in {arn:?}",
        arn.len()
    );
}

/// A JSON member of `item`, as a string (empty when absent, so a missing member fails the
/// comparison it is part of rather than panicking here).
fn member(item: &Value, name: &str) -> String {
    item.get(name)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

#[tokio::test]
async fn list_satellites_reports_exactly_the_onboarded_satellites() {
    let (router, engine) = harness_with_engine();

    // An account on-boarded to no satellite lists none: present, empty, and closed.
    let empty = send(&router, "GET", "/satellite", None).await;
    assert_eq!(
        empty.status, 200,
        "case no-onboarding: expected ListSatellites 200, got {} {}",
        empty.status, empty.body
    );
    assert_eq!(
        empty.field("satellites"),
        &json!([]),
        "case no-onboarding: an account on-boarded to no satellite must list none, got {}",
        empty.body
    );
    assert!(
        empty.field("nextToken").is_null(),
        "case no-onboarding: a listing with nothing more to come carries no nextToken, got {}",
        empty.body
    );

    let mut catalog = onboard(&engine, CATALOG);

    // One satellite has an ephemeris known for it, so the member is asserted both where
    // the store holds one and where it holds none.
    let epoch_seconds = 1_700_000_000_f64;
    let ephemeris = EphemerisMetaData {
        source: "SPACE_TRACK".to_string(),
        ephemeris_id: Some("abcdef01-2345-4678-89ab-cdef01234567".to_string()),
        epoch: Some(aws_smithy_types::DateTime::from_secs_f64(epoch_seconds)),
        name: Some("space track tle".to_string()),
    };
    let with_ephemeris = catalog
        .values()
        .find(|satellite| satellite.norad_satellite_id == 27424)
        .map(|satellite| satellite.satellite_id.clone())
        .expect("the catalogue onboards NORAD 27424");
    {
        let state = service_state(&*engine).expect("this service's state store");
        let mut guard = state.lock();
        let stored = guard
            .satellites
            .get_mut(&with_ephemeris)
            .expect("the on-boarded satellite");
        stored.current_ephemeris = Some(ephemeris.clone());
        catalog.insert(with_ephemeris.clone(), stored.clone());
    }

    let expected = in_listing_order(&catalog);
    let items = list_all(&router, None).await;
    assert_eq!(
        items.len(),
        expected.len(),
        "case whole-listing: expected one item per on-boarded satellite, got {items:?}"
    );

    for (item, satellite) in items.iter().zip(&expected) {
        let case = format!("norad-{}", satellite.norad_satellite_id);
        assert_eq!(
            member(item, "satelliteId"),
            satellite.satellite_id,
            "case {case}: the listing must report the stored satelliteId, got {item}"
        );
        assert!(
            is_uuid(&satellite.satellite_id),
            "case {case}: a satelliteId must be a Uuid, got {:?}",
            satellite.satellite_id
        );
        assert_satellite_arn(&member(item, "satelliteArn"), &satellite.satellite_id, &case);
        assert_eq!(
            item.get("noradSatelliteID").and_then(Value::as_i64),
            Some(i64::from(satellite.norad_satellite_id)),
            "case {case}: expected noradSatelliteID {}, got {item}",
            satellite.norad_satellite_id
        );
        assert_eq!(
            item.get("groundStations").cloned().unwrap_or(Value::Null),
            json!(satellite.ground_stations),
            "case {case}: expected the stored groundStations {:?}, got {item}",
            satellite.ground_stations
        );

        match &satellite.current_ephemeris {
            None => assert!(
                item.get("currentEphemeris")
                    .is_none_or(serde_json::Value::is_null),
                "case {case}: no ephemeris is known for this satellite, got {item}"
            ),
            Some(known) => {
                let reported = item
                    .get("currentEphemeris")
                    .unwrap_or_else(|| panic!("case {case}: currentEphemeris missing in {item}"));
                assert_eq!(
                    member(reported, "source"),
                    known.source,
                    "case {case}: expected the stored ephemeris source, got {reported}"
                );
                assert_eq!(
                    member(reported, "ephemerisId"),
                    known.ephemeris_id.clone().unwrap_or_default(),
                    "case {case}: expected the stored ephemerisId, got {reported}"
                );
                assert_eq!(
                    member(reported, "name"),
                    known.name.clone().unwrap_or_default(),
                    "case {case}: expected the stored ephemeris name, got {reported}"
                );
                let reported_epoch = reported
                    .get("epoch")
                    .and_then(Value::as_f64)
                    .unwrap_or_else(|| panic!("case {case}: epoch is not a number in {reported}"));
                assert!(
                    (reported_epoch - epoch_seconds).abs() < 1.0,
                    "case {case}: expected the epoch {epoch_seconds}, got {reported_epoch}"
                );
            }
        }
    }

    // The order is the NORAD catalogue number's: ascending, whatever order the store
    // happens to iterate in.
    let reported_norads: Vec<i64> = items
        .iter()
        .filter_map(|item| item.get("noradSatelliteID").and_then(Value::as_i64))
        .collect();
    let mut ascending = reported_norads.clone();
    ascending.sort_unstable();
    assert_eq!(
        reported_norads, ascending,
        "case ordering: the listing must be ordered by noradSatelliteID, got {reported_norads:?}"
    );
    assert_eq!(
        reported_norads.first().copied(),
        Some(i64::from(*NORAD_SATELLITE_ID_RANGE.start())),
        "case ordering: the catalogue's lowest NORAD number comes first, got {reported_norads:?}"
    );
    assert_eq!(
        reported_norads.last().copied(),
        Some(i64::from(*NORAD_SATELLITE_ID_RANGE.end())),
        "case ordering: the catalogue's highest NORAD number comes last, got {reported_norads:?}"
    );

    // A listing is a REPORT: reading it twice reports the same store.
    assert_eq!(
        items,
        list_all(&router, None).await,
        "case repeated-read: a listing must not change what the next caller sees"
    );
}

#[tokio::test]
async fn list_satellites_pages_through_every_satellite_once_whatever_the_page_size() {
    let (router, engine) = harness_with_engine();
    // Six satellites, so a page size can divide the sequence evenly, unevenly, exactly, or
    // not bound it at all.
    let catalog = onboard(
        &engine,
        "11111=Ohio 1,22222,33333=Oregon 1+Hawaii 1,44444,55555,66666",
    );
    assert_eq!(
        catalog.len(),
        6,
        "case setup: the setting onboards six satellites, got {catalog:?}"
    );

    let whole: Vec<String> = list_all(&router, None)
        .await
        .iter()
        .map(|item| member(item, "satelliteId"))
        .collect();
    assert_eq!(
        whole.len(),
        6,
        "case whole-listing: expected every on-boarded satellite, got {whole:?}"
    );

    for size in [1_i64, 2, 3, 4, 5, 6, 7, 100] {
        let paged: Vec<String> = list_all(&router, Some(size))
            .await
            .iter()
            .map(|item| member(item, "satelliteId"))
            .collect();
        assert_eq!(
            paged, whole,
            "case maxResults={size}: paging must yield every satellite exactly once, in the \
             listing's order, got {paged:?}"
        );
    }

    // Each published bound of `PaginationMaxResults` and one value past it (source:
    // `smithy_query(query="operation", operation="ListSatellites")` — range 1..100). A
    // value the model refuses never reaches the handler: this service's validator answers
    // it.
    for (case, size, accepted) in [
        ("the lower bound", 1_i64, true),
        ("the upper bound", 100, true),
        ("one below the lower bound", 0, false),
        ("one above the upper bound", 101, false),
    ] {
        let reply = send(&router, "GET", &format!("/satellite?maxResults={size}"), None).await;
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
async fn list_satellites_refuses_a_next_token_it_did_not_issue() {
    let (router, engine) = harness_with_engine();
    onboard(&engine, "11111,22222,33333");

    let own_token = send(&router, "GET", "/satellite?maxResults=1", None)
        .await
        .field("nextToken")
        .as_str()
        .map(str::to_string)
        .expect("a capped page hands back a token");
    let configs_token = send(&router, "GET", "/config?maxResults=1", None)
        .await
        .field("nextToken")
        .as_str()
        .map(str::to_string);

    let resumed = send(
        &router,
        "GET",
        &format!("/satellite?maxResults=1&nextToken={own_token}"),
        None,
    )
    .await;
    assert_eq!(
        resumed.status, 200,
        "case own-token: a token this listing issued must resume it, got {} {}",
        resumed.status, resumed.body
    );

    // A token addresses an offset into the sequence that issued it, so this listing
    // accepts only its own, and refuses anything else rather than resuming at an offset
    // into an unrelated sequence.
    let mut refused: Vec<(&str, String)> = vec![
        (
            "a token this service never issued",
            "not-a-token".to_string(),
        ),
        (
            "a token naming no offset",
            "satellites.not-a-number".to_string(),
        ),
        (
            "a token of another listing's name",
            "missionprofiles.1".to_string(),
        ),
    ];
    // An account that holds no config lists none, so `ListConfigs` hands out no token
    // here; when it does, it is another listing's and must be refused too.
    if let Some(token) = configs_token {
        refused.push(("a token from another listing", token));
    }

    for (case, token) in refused {
        let reply = send(
            &router,
            "GET",
            &format!("/satellite?maxResults=1&nextToken={token}"),
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
    }
}

/// One row of the operator-setting table: the setting, and the satellites it onboards as
/// `(noradSatelliteID, groundStations)` pairs in NORAD order.
struct Onboarding {
    case: &'static str,
    setting: String,
    onboards: Vec<(i32, Vec<String>)>,
}

impl Onboarding {
    fn new(case: &'static str, setting: &str, onboards: &[(i32, &[&str])]) -> Self {
        Self {
            case,
            setting: setting.to_string(),
            onboards: onboards
                .iter()
                .map(|(norad, stations)| {
                    (
                        *norad,
                        stations.iter().map(|name| (*name).to_string()).collect(),
                    )
                })
                .collect(),
        }
    }
}

#[tokio::test]
async fn the_operator_setting_onboards_satellites_entry_by_entry() {
    // A `GroundStationName` is 4..97 characters over `[ a-zA-Z0-9-._:=]`
    // (`smithy_query(query="shape", shape="GroundStationName")`), so the table carries each
    // published bound of that length and one value past each.
    let shortest = "Ohio".to_string();
    let longest = "A".repeat(97);
    let too_short = "Ohi".to_string();
    let too_long = "A".repeat(98);

    // Rows differ along the form of the setting: a bare NORAD number, one with ground
    // stations, several entries, surrounding whitespace, each published bound of
    // `noradSatelliteID` and of a ground-station name, and entries that are not a NORAD
    // number inside the modelled range or not a spellable ground-station name — each of
    // which onboards nothing rather than something unintended.
    let rows = vec![
        Onboarding::new("a bare norad number", "25994", &[(25994, &[])]),
        Onboarding::new(
            "one ground station",
            "25994=Ohio 1",
            &[(25994, &["Ohio 1"])],
        ),
        Onboarding::new(
            "several ground stations",
            "25994=Ohio 1+Oregon 1+Hawaii 1",
            &[(25994, &["Ohio 1", "Oregon 1", "Hawaii 1"])],
        ),
        Onboarding::new(
            "several entries, with whitespace",
            "  25994 =  Ohio 1 + Oregon 1 , 27424 ,, 43013=Punta Arenas 1 ",
            &[
                (25994, &["Ohio 1", "Oregon 1"]),
                (27424, &[]),
                (43013, &["Punta Arenas 1"]),
            ],
        ),
        Onboarding::new("the lower norad bound", "0", &[(0, &[])]),
        Onboarding::new("the upper norad bound", "339999", &[(339999, &[])]),
        Onboarding::new("one past the upper norad bound", "340000", &[]),
        Onboarding::new("one below the lower norad bound", "-1", &[]),
        Onboarding::new("not a norad number at all", "aqua", &[]),
        Onboarding::new("unset", "", &[]),
        Onboarding::new("nothing but separators", " , , ", &[]),
        Onboarding::new(
            "a repeated norad number",
            "25994=Ohio 1,25994=Oregon 1",
            &[(25994, &["Ohio 1"])],
        ),
        Onboarding::new(
            "a name carrying the = the entry splits on",
            "25994=gs:region=ohio",
            &[(25994, &["gs:region=ohio"])],
        ),
        Onboarding::new(
            "a shortest-allowed name",
            &format!("25994={shortest}"),
            &[(25994, &["Ohio"])],
        ),
        Onboarding::new(
            "a longest-allowed name",
            &format!("25994={longest}"),
            &[(25994, &[&longest])],
        ),
        Onboarding::new(
            "a name one character too short",
            &format!("25994={too_short}"),
            &[(25994, &[])],
        ),
        Onboarding::new(
            "a name one character too long",
            &format!("25994={too_long}"),
            &[(25994, &[])],
        ),
        Onboarding::new(
            "a name with a character the pattern excludes",
            "25994=Ohio/1+Oregon 1",
            &[(25994, &["Oregon 1"])],
        ),
    ];

    for row in &rows {
        let catalog = parse_satellites(&row.setting);
        let onboarded: Vec<(i32, Vec<String>)> = in_listing_order(&catalog)
            .into_iter()
            .map(|satellite| (satellite.norad_satellite_id, satellite.ground_stations))
            .collect();
        assert_eq!(
            onboarded, row.onboards,
            "case {}: {:?} should onboard {:?}, got {onboarded:?}",
            row.case, row.setting, row.onboards
        );

        // Every onboarding is minted a `Uuid` of its own and the region-less ARN that
        // addresses it, whatever the setting said.
        for (satellite_id, satellite) in &catalog {
            assert_eq!(
                satellite_id, &satellite.satellite_id,
                "case {}: the catalogue must be keyed by satelliteId, got {satellite:?}",
                row.case
            );
            assert!(
                is_uuid(&satellite.satellite_id),
                "case {}: a satelliteId must be a Uuid, got {:?}",
                row.case,
                satellite.satellite_id
            );
            assert_satellite_arn(&satellite.satellite_arn, satellite_id, row.case);
            assert!(
                satellite.current_ephemeris.is_none(),
                "case {}: an onboarding knows no ephemeris yet, got {:?}",
                row.case,
                satellite.current_ephemeris
            );
        }
    }

    // And one satellite's ground stations are bounded by the modelled list length, so a
    // setting naming more than the model can report is cut to the bound rather than
    // answered past it.
    let names: Vec<String> = (0..MAX_GROUND_STATIONS_PER_SATELLITE + 2)
        .map(|index| format!("Station {index:04}"))
        .collect();
    let catalog = parse_satellites(&format!("25994={}", names.join("+")));
    let onboarded = in_listing_order(&catalog);
    assert_eq!(
        onboarded.len(),
        1,
        "case ground-station-bound: one entry onboards one satellite, got {onboarded:?}"
    );
    assert_eq!(
        onboarded[0].ground_stations.len(),
        MAX_GROUND_STATIONS_PER_SATELLITE,
        "case ground-station-bound: a satellite reports at most \
         {MAX_GROUND_STATIONS_PER_SATELLITE} ground stations, got {}",
        onboarded[0].ground_stations.len()
    );
    assert_eq!(
        onboarded[0].ground_stations[..2],
        names[..2],
        "case ground-station-bound: the bound keeps the first names the setting named, got {:?}",
        &onboarded[0].ground_stations[..2]
    );
}
