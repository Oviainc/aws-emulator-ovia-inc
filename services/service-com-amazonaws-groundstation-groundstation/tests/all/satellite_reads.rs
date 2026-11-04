// SPDX-License-Identifier: CC0-1.0

//! `GetSatellite` (`GET /satellite/{satelliteId}`), end to end over HTTP through the same
//! front door as the rest of this target's suites.
//!
//! The RULE under test: a reader REPORTS the stored resource its path addresses, so
//! `GetSatellite` answers the one satellite this account is on-boarded to under the
//! `{satelliteId}` it carries — that satellite's own `satelliteId`, `satelliteArn`,
//! `noradSatelliteID`, `groundStations` and `currentEphemeris` — refuses an id that names
//! no onboarding of this account, and changes nothing a later caller sees.
//!
//! So the suites below assert that rule over SETS of inputs rather than over one call:
//!
//! * a catalogue of satellites differing in NORAD catalogue number (each published bound
//!   of `noradSatelliteID` among them), in how many ground stations they are on-boarded to
//!   (none, one, three), and in whether an ephemeris is known for them — each read back by
//!   its own id and compared member by member with what the store holds, and with what
//!   `ListSatellites` reports for the same satellite, because the two operations report ONE
//!   record;
//! * the same reads repeated, and the listing taken after them, so a read is shown to
//!   change nothing;
//! * ids that name no onboarding — several well-formed `Uuid`s, on an account on-boarded to
//!   satellites and on one on-boarded to none — refused as a bad parameter;
//! * ids outside the modelled `Uuid`, refused as a bad request before the handler runs.
//!
//! Satellites are not created through this API (the model declares no operation that
//! onboards one), so the catalogue is seeded into this service's own state — exactly where
//! `state::SATELLITES_ENV` lands it — and then read back through the front door.

use serde_json::Value;
use service_com_amazonaws_groundstation_groundstation::ops::types::EphemerisMetaData;
use service_com_amazonaws_groundstation_groundstation::state::{
    service_state, Satellite, BAD_REQUEST_ERROR_CODE, INVALID_REQUEST_BODY,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::refusal_message_templates::bad_parameter_message;
use crate::satellite_catalog::{assert_satellite_arn, in_listing_order, member, onboard};
use crate::scheduling_loop::{harness_with_engine, is_uuid, list_pages, send, Reply, ABSENT_UUID};

/// The catalogue these suites read: satellites differing in NORAD catalogue number —
/// including both published bounds of `noradSatelliteID` (`smithy_query(query="shape",
/// shape="noradSatelliteID")` — 0..339999) — and in how many ground stations each is
/// on-boarded to (none, one, three).
const CATALOG: &str = "0,25994=Ohio 1+Oregon 1+Hawaii 1,27424=Punta Arenas 1,339999";

/// Well-formed `Uuid`s (model pattern
/// `^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$`) that name no
/// onboarding: a run of zeros, a run of `f`s and a mixed one, so an assertion about the
/// refusal cannot be passing on one id's shape.
const ABSENT_UUIDS: &[&str] = &[
    ABSENT_UUID,
    "ffffffff-ffff-4fff-bfff-ffffffffffff",
    "0a1b2c3d-4e5f-4a6b-8c7d-9e0f1a2b3c4d",
];

/// `GetSatellite` for `satellite_id`.
async fn get(router: &MultiServiceRouter, satellite_id: &str) -> Reply {
    send(router, "GET", &format!("/satellite/{satellite_id}"), None).await
}

/// `GetSatellite` for `satellite_id`, asserting it succeeded, and answering its body.
async fn get_ok(router: &MultiServiceRouter, case: &str, satellite_id: &str) -> Value {
    let reply = get(router, satellite_id).await;
    assert_eq!(
        reply.status, 200,
        "case {case}: expected GetSatellite 200, got {} {}",
        reply.status, reply.body
    );
    reply.body
}

/// Assert `reported` carries exactly what the store holds for `satellite`.
fn assert_reports(case: &str, reported: &Value, satellite: &Satellite) {
    assert_eq!(
        member(reported, "satelliteId"),
        satellite.satellite_id,
        "case {case}: the reader must report the stored satelliteId, got {reported}"
    );
    assert!(
        is_uuid(&satellite.satellite_id),
        "case {case}: a satelliteId must be a Uuid, got {:?}",
        satellite.satellite_id
    );
    assert_satellite_arn(
        &member(reported, "satelliteArn"),
        &satellite.satellite_id,
        case,
    );
    assert_eq!(
        reported.get("noradSatelliteID").and_then(Value::as_i64),
        Some(i64::from(satellite.norad_satellite_id)),
        "case {case}: expected noradSatelliteID {}, got {reported}",
        satellite.norad_satellite_id
    );
    assert_eq!(
        reported
            .get("groundStations")
            .cloned()
            .unwrap_or(Value::Null),
        serde_json::json!(satellite.ground_stations),
        "case {case}: expected the stored groundStations {:?}, got {reported}",
        satellite.ground_stations
    );
    match &satellite.current_ephemeris {
        None => assert!(
            reported
                .get("currentEphemeris")
                .is_none_or(Value::is_null),
            "case {case}: no ephemeris is known for this satellite, got {reported}"
        ),
        Some(known) => {
            let ephemeris = reported
                .get("currentEphemeris")
                .unwrap_or_else(|| panic!("case {case}: currentEphemeris missing in {reported}"));
            assert_eq!(
                member(ephemeris, "source"),
                known.source,
                "case {case}: expected the stored ephemeris source, got {ephemeris}"
            );
            assert_eq!(
                member(ephemeris, "ephemerisId"),
                known.ephemeris_id.clone().unwrap_or_default(),
                "case {case}: expected the stored ephemerisId, got {ephemeris}"
            );
            assert_eq!(
                member(ephemeris, "name"),
                known.name.clone().unwrap_or_default(),
                "case {case}: expected the stored ephemeris name, got {ephemeris}"
            );
            let expected_epoch = known.epoch.as_ref().map(aws_smithy_types::DateTime::as_secs_f64);
            let reported_epoch = ephemeris.get("epoch").and_then(Value::as_f64);
            match (expected_epoch, reported_epoch) {
                (Some(expected), Some(actual)) => assert!(
                    (expected - actual).abs() < 1.0,
                    "case {case}: expected the epoch {expected}, got {actual}"
                ),
                (expected, actual) => panic!(
                    "case {case}: expected an epoch {expected:?}, got {actual:?} in {ephemeris}"
                ),
            }
        }
    }
}

#[tokio::test]
async fn get_satellite_reports_the_satellite_its_path_addresses() {
    let (router, engine) = harness_with_engine();
    let mut catalog = onboard(&engine, CATALOG);

    // One satellite has an ephemeris known for it, so `currentEphemeris` is asserted both
    // where the store holds one and where it holds none.
    let ephemeris = EphemerisMetaData {
        source: "SPACE_TRACK".to_string(),
        ephemeris_id: Some("abcdef01-2345-4678-89ab-cdef01234567".to_string()),
        epoch: Some(aws_smithy_types::DateTime::from_secs_f64(1_700_000_000_f64)),
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
        stored.current_ephemeris = Some(ephemeris);
        catalog.insert(with_ephemeris.clone(), stored.clone());
    }

    let expected = in_listing_order(&catalog);
    assert_eq!(
        expected.len(),
        4,
        "case setup: the catalogue onboards four satellites, got {expected:?}"
    );

    // Every item `ListSatellites` reports, keyed by the satellite it describes, so each
    // read can be compared with the listing's own view of the SAME record.
    let listed: Vec<Value> = list_pages(&router, "/satellite", "satellites", None).await;

    for satellite in &expected {
        let case = format!("norad-{}", satellite.norad_satellite_id);
        let reported = get_ok(&router, &case, &satellite.satellite_id).await;
        assert_reports(&case, &reported, satellite);

        // The reader and the listing report one record, member for member.
        let item = listed
            .iter()
            .find(|item| member(item, "satelliteId") == satellite.satellite_id)
            .unwrap_or_else(|| {
                panic!("case {case}: the listing must carry this satellite, got {listed:?}")
            });
        assert_eq!(
            &reported, item,
            "case {case}: GetSatellite and ListSatellites must report the same record, got \
             {reported} and {item}"
        );

        // And it reports THIS satellite only: no other id's record comes back under it.
        for other in &expected {
            if other.satellite_id == satellite.satellite_id {
                continue;
            }
            assert_ne!(
                member(&reported, "satelliteId"),
                other.satellite_id,
                "case {case}: the reader must not answer another satellite's record"
            );
        }
    }

    // A read is a REPORT: reading each satellite again answers the same body, and the
    // listing taken after every read is the one taken before them.
    for satellite in &expected {
        let case = format!("repeated-norad-{}", satellite.norad_satellite_id);
        let again = get_ok(&router, &case, &satellite.satellite_id).await;
        assert_reports(&case, &again, satellite);
    }
    assert_eq!(
        listed,
        list_pages(&router, "/satellite", "satellites", None).await,
        "case repeated-read: reading satellites must not change what a later caller sees"
    );
}

#[tokio::test]
async fn get_satellite_refuses_an_id_that_names_no_onboarding() {
    // On an account on-boarded to satellites, an id none of them carries names nothing…
    let (router, engine) = harness_with_engine();
    let catalog = onboard(&engine, CATALOG);
    for satellite_id in ABSENT_UUIDS {
        let case = format!("onboarded-account {satellite_id}");
        let reply = get(&router, satellite_id).await;
        let message = bad_parameter_message(&case, &reply);
        assert_eq!(
            message, "Satellite does not exist.",
            "case {case}: GetSatellite answers its satellite literal, got {message:?}"
        );
        assert!(
            !message.contains(*satellite_id),
            "case {case}: the literal discloses no id, got {message:?}"
        );
        for satellite in catalog.values() {
            assert!(
                !message.contains(&satellite.satellite_id),
                "case {case}: the literal discloses no satellite this account holds, got \
                 {message:?}"
            );
        }
    }

    // …and on an account on-boarded to none, every well-formed id does.
    let (bare_router, _bare_engine) = harness_with_engine();
    for satellite_id in ABSENT_UUIDS {
        let case = format!("no-onboarding {satellite_id}");
        let reply = get(&bare_router, satellite_id).await;
        let message = bad_parameter_message(&case, &reply);
        assert_eq!(
            message, "Satellite does not exist.",
            "case {case}: an account on-boarded to nothing reads no satellite, got {message:?}"
        );
    }

    // The refusals disturbed nothing: every on-boarded satellite still reads back.
    for satellite in in_listing_order(&catalog) {
        let case = format!("after-refusals norad-{}", satellite.norad_satellite_id);
        let reported = get_ok(&router, &case, &satellite.satellite_id).await;
        assert_reports(&case, &reported, &satellite);
    }
}

#[tokio::test]
async fn get_satellite_refuses_an_id_outside_the_modelled_uuid() {
    let (router, engine) = harness_with_engine();
    let catalog = onboard(&engine, CATALOG);
    let present = in_listing_order(&catalog)
        .first()
        .map(|satellite| satellite.satellite_id.clone())
        .expect("the catalogue onboards a satellite");

    // An id outside the modelled `Uuid` (len 36..36, lower-case hex in five groups) never
    // reaches the handler: this service's request validator answers it, and it is a bad
    // request rather than a missing resource. Rows span each published bound of that
    // length and one value past each, plus a case the pattern excludes.
    let malformed: &[(&str, String)] = &[
        ("not hexadecimal at all", "nope".to_string()),
        (
            "one character short",
            present[..present.len() - 1].to_string(),
        ),
        ("one character long", format!("{present}0")),
        ("upper-case hex", present.to_uppercase()),
        ("the groups joined without separators", present.replace('-', "")),
    ];
    for (case, satellite_id) in malformed {
        let reply = get(&router, satellite_id).await;
        assert_eq!(
            reply.status, 400,
            "case {case} ({satellite_id}): expected 400, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            BAD_REQUEST_ERROR_CODE,
            "case {case} ({satellite_id}): expected {BAD_REQUEST_ERROR_CODE}, got {}",
            reply.body
        );
        assert_eq!(
            reply.string("message"),
            INVALID_REQUEST_BODY,
            "case {case} ({satellite_id}): wrong refusal text, got {}",
            reply.body
        );
    }

    // And the id that IS spellable still reads back, so the refusals above are the
    // validator's answer to the id rather than to the route.
    let reported = get_ok(&router, "spellable", &present).await;
    assert_eq!(
        member(&reported, "satelliteId"),
        present,
        "case spellable: the reader answers the satellite its path addresses, got {reported}"
    );
}
