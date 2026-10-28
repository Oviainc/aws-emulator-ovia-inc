// SPDX-License-Identifier: CC0-1.0

//! The `currentEphemeris` a satellite read reports, driven through `CreateEphemeris`
//! (`POST /ephemeris`), `UpdateEphemeris` (`PUT /ephemeris/{ephemerisId}`) and
//! `DeleteEphemeris` (`DELETE /ephemeris/{ephemerisId}`), end to end over HTTP through the
//! same front door as the rest of this target's suites.
//!
//! The RULE under test: the ephemeris a satellite's trajectory is computed from — what
//! `GetSatellite` and `ListSatellites` report as `currentEphemeris`
//! (`smithy_query(query="doc", shape="GetSatelliteResponse", member="currentEphemeris")` —
//! "The current ephemeris being used to compute the trajectory of the satellite") — is the
//! one the account holds for that satellite that is in force, which is an ephemeris in
//! `ENABLED` (its data validated and the account asked for it to be used) and no ephemeris
//! in any other status; of several in force the highest `priority` wins ("Higher numbers
//! take precedence" — `smithy_query(query="doc", shape="CreateEphemerisRequest",
//! member="priority")`); and it is reported as that ephemeris's own metadata — `source`
//! `CUSTOMER_PROVIDED`, its `ephemerisId`, the `name` the create named and no `epoch`,
//! which the model says "is not populated for customer-provided ephemerides"
//! (`smithy_query(query="shape", shape="EphemerisMetaData")`).
//!
//! So the suites below assert that rule over SETS of inputs rather than over one call:
//!
//! * a table spanning what the rule generalises over — how many ephemerides the account
//!   holds for a satellite (none, one, several), whether each was asked to be enabled,
//!   their priorities (both published bounds of `CustomerEphemerisPriority` among them,
//!   and a pair tied at one priority, which the chosen tie-break settles),
//!   which `EphemerisData` member carries the data (`tle`, `oem`, `azEl`, inline and from
//!   an Amazon S3 object), whether the data validates, whether the ephemeris has already
//!   expired, and whether it names a satellite at all — each row read back through
//!   `GetSatellite` and compared with what `ListSatellites` reports for the same
//!   satellite, because the two operations report ONE record;
//! * the moves that change which ephemeris is in force, on one satellite: the admission in
//!   `VALIDATING` before the validation falls due, a second ephemeris of higher priority
//!   taking over, an update that disables the one in force, an update that lowers its
//!   priority, and the delete of the one in force — with every satellite read taken twice,
//!   so a read is shown to change nothing a later caller sees.
//!
//! The harness, the signed `restJson1` requests, the onboarding helper and the ephemeris
//! data builders are [`crate::scheduling_loop`]'s and [`crate::ephemeris_catalog`]'s,
//! reused rather than rebuilt.

use serde_json::{json, Value};
use service_com_amazonaws_groundstation_groundstation::state::EPHEMERIS_SOURCE_CUSTOMER_PROVIDED;
use smithy_server_runtime::MultiServiceRouter;

use crate::ephemeris_catalog::{
    az_el_inline, create_ok, onboard, s3_object, tle_inline, wait_out_validation, FUTURE, OEM_BODY,
    PAST,
};
use crate::scheduling_loop::{harness_with_engine, list_pages, send};

/// One ephemeris a row stores: what its `CreateEphemeris` request asks for.
struct Supplied {
    /// The `name` the request names, which is what the satellite must report back for the
    /// ephemeris in force.
    name: &'static str,
    /// The `ephemeris` member the request carries.
    data: Value,
    /// Whether the request asks for the ephemeris to be enabled once it validates.
    enabled: bool,
    /// The `priority` the request names, or `None` where it names none and the documented
    /// default applies.
    priority: Option<i64>,
    /// The `expirationTime` the request names, or `None` where it names none and the
    /// supplied data's own validity is what expires the ephemeris.
    expiration_time: Option<i64>,
    /// Whether the request names this row's satellite at all: an ephemeris stored without a
    /// `satelliteId` is held by the account for no satellite.
    for_satellite: bool,
}

impl Supplied {
    /// An ephemeris the request asks to be enabled, supplied inline as a two-line element
    /// set valid from [`FUTURE`] on, naming this row's satellite and no priority.
    fn enabled(name: &'static str) -> Self {
        Self {
            name,
            data: tle_inline(&[(FUTURE, FUTURE + 86_400)]),
            enabled: true,
            priority: None,
            expiration_time: None,
            for_satellite: true,
        }
    }

    /// The same, with `priority`.
    fn at_priority(name: &'static str, priority: i64) -> Self {
        Self {
            priority: Some(priority),
            ..Self::enabled(name)
        }
    }
}

/// One row of the table: the ephemerides the account holds for this row's satellite, and
/// the `name` of the one whose metadata the satellite must report (`None` where the account
/// holds none in force for it).
struct Row {
    case: &'static str,
    supplied: Vec<Supplied>,
    in_force: Option<&'static str>,
}

/// The table: one row per way the set of ephemerides an account holds for a satellite can
/// stand, spanning count, enabled flag, priority, data form, validity, expiration and
/// whether the ephemeris names a satellite at all.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "no ephemeris at all",
            supplied: Vec::new(),
            in_force: None,
        },
        Row {
            case: "one asked to be enabled",
            supplied: vec![Supplied::enabled("the only one")],
            in_force: Some("the only one"),
        },
        Row {
            case: "one not asked to be enabled",
            supplied: vec![Supplied {
                enabled: false,
                ..Supplied::enabled("kept disabled")
            }],
            in_force: None,
        },
        Row {
            case: "three in force, differing in priority",
            supplied: vec![
                // Both published bounds of `CustomerEphemerisPriority`
                // (`smithy_query(query="operation", operation="CreateEphemeris")` — range
                // 1..99999) and a value between them.
                Supplied::at_priority("the lowest priority", 1),
                Supplied::at_priority("the highest priority", 99_999),
                Supplied::at_priority("a middling priority", 500),
            ],
            in_force: Some("the highest priority"),
        },
        Row {
            case: "two in force at the same priority",
            supplied: vec![
                Supplied::at_priority("stored first at four", 4),
                Supplied::at_priority("stored second at four", 4),
            ],
            // The documented priority leaves this pair tied, and the tie-break is the
            // CHOSEN one: the most recently created of equal priority.
            in_force: Some("stored second at four"),
        },
        Row {
            case: "the highest priority is not enabled, so the highest enabled one is in force",
            supplied: vec![
                Supplied {
                    enabled: false,
                    ..Supplied::at_priority("disabled at the highest priority", 99_999)
                },
                Supplied::at_priority("enabled at seven", 7),
                Supplied::at_priority("enabled at three", 3),
            ],
            in_force: Some("enabled at seven"),
        },
        Row {
            case: "an oem body supplied inline",
            supplied: vec![Supplied {
                data: json!({"oem": {"oemData": OEM_BODY}}),
                ..Supplied::enabled("an oem body")
            }],
            in_force: Some("an oem body"),
        },
        Row {
            case: "azEl segments supplied from an Amazon S3 object",
            supplied: vec![Supplied {
                data: json!({"azEl": {
                    "groundStation": "Ohio 1",
                    "data": {"s3Object": s3_object("azel/ohio.json")},
                }}),
                ..Supplied::enabled("an azEl object")
            }],
            in_force: Some("an azEl object"),
        },
        Row {
            case: "azEl segments supplied inline",
            supplied: vec![Supplied {
                data: az_el_inline(&[(FUTURE, FUTURE + 3_600)]),
                ..Supplied::enabled("azEl segments")
            }],
            in_force: Some("azEl segments"),
        },
        Row {
            case: "a tle supplied from an Amazon S3 object",
            supplied: vec![Supplied {
                data: json!({"tle": {"s3Object": s3_object("tle/aqua.txt")}}),
                ..Supplied::enabled("a tle object")
            }],
            in_force: Some("a tle object"),
        },
        Row {
            case: "an expiration that has already passed",
            supplied: vec![Supplied {
                expiration_time: Some(PAST),
                ..Supplied::enabled("already expired")
            }],
            in_force: None,
        },
        Row {
            case: "data whose validity runs backwards, so it fails validation",
            supplied: vec![Supplied {
                data: tle_inline(&[(FUTURE + 86_400, FUTURE)]),
                ..Supplied::enabled("never validated")
            }],
            in_force: None,
        },
        Row {
            case: "an expired one at the highest priority beside an enabled one at the lowest",
            supplied: vec![
                Supplied {
                    expiration_time: Some(PAST),
                    ..Supplied::at_priority("expired at the highest priority", 99_999)
                },
                Supplied::at_priority("enabled at the lowest priority", 1),
            ],
            in_force: Some("enabled at the lowest priority"),
        },
        Row {
            case: "an ephemeris stored for no satellite",
            supplied: vec![Supplied {
                for_satellite: false,
                ..Supplied::enabled("held for no satellite")
            }],
            in_force: None,
        },
    ]
}

/// The `CreateEphemeris` body `supplied` asks for, naming `satellite_id` when the row
/// attaches it to one and spelling the name under `prefix` so every row's names are its
/// own.
fn create_body(supplied: &Supplied, satellite_id: &str, prefix: &str) -> Value {
    let mut body = json!({
        "name": ephemeris_name(prefix, supplied.name),
        "enabled": supplied.enabled,
        "ephemeris": supplied.data,
    });
    if supplied.for_satellite {
        body["satelliteId"] = json!(satellite_id);
    }
    if let Some(priority) = supplied.priority {
        body["priority"] = json!(priority);
    }
    if let Some(expiration) = supplied.expiration_time {
        body["expirationTime"] = json!(expiration);
    }
    body
}

/// The `name` a row stores an ephemeris under: the row's own prefix and the row's name for
/// it, which keeps the names of one account's ephemerides distinct across rows while
/// staying inside the modelled `SafeName` pattern `^[ a-zA-Z0-9_:-]{1,256}$`.
fn ephemeris_name(prefix: &str, name: &str) -> String {
    format!("{prefix} {name}")
}

/// `GetSatellite` for `satellite_id`, asserting it was answered, and the body it answered
/// with.
async fn get_ok(router: &MultiServiceRouter, case: &str, satellite_id: &str) -> Value {
    let reply = send(router, "GET", &format!("/satellite/{satellite_id}"), None).await;
    assert_eq!(
        reply.status, 200,
        "case {case}: expected GetSatellite 200, got {} {}",
        reply.status, reply.body
    );
    reply.body
}

/// The satellite `satellite_id` as both readers report it, asserting they agree — the two
/// operations report ONE record — and that reading twice answers the same body.
async fn read_both(router: &MultiServiceRouter, case: &str, satellite_id: &str) -> Value {
    let reported = get_ok(router, case, satellite_id).await;
    let again = get_ok(router, case, satellite_id).await;
    assert_eq!(
        reported, again,
        "case {case}: reading a satellite must not change what a later caller sees, got \
         {reported} then {again}"
    );

    let listed = list_pages(router, "/satellite", "satellites", None).await;
    let item = listed
        .iter()
        .find(|item| item.get("satelliteId") == reported.get("satelliteId"))
        .unwrap_or_else(|| panic!("case {case}: the listing must carry this satellite, got {listed:?}"));
    assert_eq!(
        &reported, item,
        "case {case}: GetSatellite and ListSatellites must report the same record, got \
         {reported} and {item}"
    );
    reported
}

/// Assert the satellite `reported` carries the metadata of the ephemeris `expected` —
/// `(ephemerisId, name)` — as its `currentEphemeris`, or none where the account holds none
/// in force for it.
fn assert_current_ephemeris(case: &str, reported: &Value, expected: Option<(&str, &str)>) {
    let current = reported.get("currentEphemeris").filter(|value| !value.is_null());
    match (expected, current) {
        (None, None) => {}
        (None, Some(current)) => panic!(
            "case {case}: the account holds no ephemeris in force for this satellite, got \
             {current}"
        ),
        (Some((ephemeris_id, name)), None) => panic!(
            "case {case}: expected the ephemeris {ephemeris_id} ({name}) in force, got \
             {reported}"
        ),
        (Some((ephemeris_id, name)), Some(current)) => {
            assert_eq!(
                current.get("ephemerisId").and_then(Value::as_str),
                Some(ephemeris_id),
                "case {case}: expected the ephemerisId of the ephemeris in force, got {current}"
            );
            assert_eq!(
                current.get("name").and_then(Value::as_str),
                Some(name),
                "case {case}: expected the name the create named, got {current}"
            );
            assert_eq!(
                current.get("source").and_then(Value::as_str),
                Some(EPHEMERIS_SOURCE_CUSTOMER_PROVIDED),
                "case {case}: an ephemeris this account supplied is \
                 {EPHEMERIS_SOURCE_CUSTOMER_PROVIDED}, got {current}"
            );
            assert!(
                current.get("epoch").is_none_or(Value::is_null),
                "case {case}: an epoch is not populated for a customer-provided ephemeris, got \
                 {current}"
            );
        }
    }
}

#[tokio::test]
async fn a_satellite_reports_the_ephemeris_the_account_holds_in_force_for_it() {
    let (router, engine) = harness_with_engine();

    // One satellite per row, so no row's ephemerides can answer another row's read, and
    // every row lives in the one account. The NORAD catalogue numbers are the row indices
    // (each a distinct spacecraft), which also fixes the order the listing reports them in.
    let rows = rows();
    let setting: Vec<String> = (0..rows.len())
        .map(|index| format!("{}=Ohio 1", index + 1))
        .collect();
    let satellites = onboard(&engine, &setting.join(","));
    assert_eq!(
        satellites.len(),
        rows.len(),
        "case setup: the setting must onboard one satellite per row, got {satellites:?}"
    );

    // Every row's ephemerides, stored through the front door, and the id each name was
    // minted under.
    let mut minted: Vec<Vec<(String, String)>> = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        let prefix = format!("row {index}");
        let mut ids: Vec<(String, String)> = Vec::new();
        for supplied in &row.supplied {
            let body = create_body(supplied, &satellites[index], &prefix);
            let id = create_ok(&router, row.case, body).await;
            ids.push((supplied.name.to_string(), id));
        }
        minted.push(ids);
    }

    wait_out_validation();
    for (index, row) in rows.iter().enumerate() {
        let expected = row.in_force.map(|name| {
            let id = minted[index]
                .iter()
                .find(|(stored, _)| stored == name)
                .map(|(_, id)| id.clone())
                .unwrap_or_else(|| panic!("case {}: the row stores {name}", row.case));
            (id, ephemeris_name(&format!("row {index}"), name))
        });
        let reported = read_both(&router, row.case, &satellites[index]).await;
        assert_current_ephemeris(
            row.case,
            &reported,
            expected
                .as_ref()
                .map(|(id, name)| (id.as_str(), name.as_str())),
        );
    }
}

/// `UpdateEphemeris` for `ephemeris_id` with `body`, asserting it was accepted.
async fn update_ok(router: &MultiServiceRouter, case: &str, ephemeris_id: &str, body: Value) {
    let reply = send(
        router,
        "PUT",
        &format!("/ephemeris/{ephemeris_id}"),
        Some(body),
    )
    .await;
    assert_eq!(
        reply.status, 200,
        "case {case}: expected UpdateEphemeris 200, got {} {}",
        reply.status, reply.body
    );
}

/// `DeleteEphemeris` for `ephemeris_id`, asserting it was accepted.
async fn delete_ok(router: &MultiServiceRouter, case: &str, ephemeris_id: &str) {
    let reply = send(router, "DELETE", &format!("/ephemeris/{ephemeris_id}"), None).await;
    assert_eq!(
        reply.status, 200,
        "case {case}: expected DeleteEphemeris 200, got {} {}",
        reply.status, reply.body
    );
}

#[tokio::test]
async fn the_ephemeris_in_force_follows_every_move_of_the_account_s_ephemerides() {
    let (router, engine) = harness_with_engine();
    // Two satellites: the moves below are made on the first, and the second holds an
    // ephemeris of its own throughout, so each satellite is shown to report the ephemeris
    // held for IT rather than whichever the account stored last.
    let satellites = onboard(&engine, "25994=Ohio 1,27424=Punta Arenas 1");
    let (flown, other) = (satellites[0].clone(), satellites[1].clone());

    let others = create_ok(
        &router,
        "the other satellite's ephemeris",
        json!({
            "name": "the other satellite",
            "enabled": true,
            "satelliteId": other,
            "ephemeris": tle_inline(&[(FUTURE, FUTURE + 86_400)]),
        }),
    )
    .await;
    let first = create_ok(
        &router,
        "the first ephemeris",
        json!({
            "name": "first at five",
            "enabled": true,
            "priority": 5,
            "satelliteId": flown,
            "ephemeris": tle_inline(&[(FUTURE, FUTURE + 86_400)]),
        }),
    )
    .await;

    // Admitted in `VALIDATING`: nothing is in force for a satellite whose only ephemeris
    // is still being validated. Read with one call, inside the span the create recorded.
    let admitted = get_ok(&router, "while the first is validating", &flown).await;
    assert_current_ephemeris("while the first is validating", &admitted, None);

    wait_out_validation();
    let settled = read_both(&router, "once the first validated", &flown).await;
    assert_current_ephemeris(
        "once the first validated",
        &settled,
        Some((first.as_str(), "first at five")),
    );
    let neighbour = read_both(&router, "the other satellite", &other).await;
    assert_current_ephemeris(
        "the other satellite",
        &neighbour,
        Some((others.as_str(), "the other satellite")),
    );

    // A second ephemeris of higher priority takes over once IT has validated, and not
    // before.
    let second = create_ok(
        &router,
        "the second ephemeris",
        json!({
            "name": "second at nine",
            "enabled": true,
            "priority": 9,
            "satelliteId": flown,
            "ephemeris": tle_inline(&[(FUTURE, FUTURE + 86_400)]),
        }),
    )
    .await;
    let mid_validation = get_ok(&router, "while the second is validating", &flown).await;
    assert_current_ephemeris(
        "while the second is validating",
        &mid_validation,
        Some((first.as_str(), "first at five")),
    );
    wait_out_validation();
    let taken_over = read_both(&router, "once the second validated", &flown).await;
    assert_current_ephemeris(
        "once the second validated",
        &taken_over,
        Some((second.as_str(), "second at nine")),
    );

    // Disabling the one in force hands the satellite back to the next one in force.
    update_ok(
        &router,
        "disable the second",
        &second,
        json!({"enabled": false}),
    )
    .await;
    let disabled = read_both(&router, "the second disabled", &flown).await;
    assert_current_ephemeris(
        "the second disabled",
        &disabled,
        Some((first.as_str(), "first at five")),
    );

    // Re-enabled below the first's priority, it stays out of force; raised above it, it
    // takes over again.
    update_ok(
        &router,
        "re-enable the second below the first",
        &second,
        json!({"enabled": true, "priority": 2}),
    )
    .await;
    let outranked = read_both(&router, "the second outranked", &flown).await;
    assert_current_ephemeris(
        "the second outranked",
        &outranked,
        Some((first.as_str(), "first at five")),
    );
    update_ok(
        &router,
        "raise the second above the first",
        &second,
        json!({"enabled": true, "priority": 6}),
    )
    .await;
    let outranking = read_both(&router, "the second outranking", &flown).await;
    assert_current_ephemeris(
        "the second outranking",
        &outranking,
        Some((second.as_str(), "second at nine")),
    );

    // Deleting the one in force hands the satellite to what is left, and deleting that one
    // leaves it with nothing in force — while the other satellite's own ephemeris is
    // untouched throughout.
    delete_ok(&router, "delete the second", &second).await;
    let after_delete = read_both(&router, "the second deleted", &flown).await;
    assert_current_ephemeris(
        "the second deleted",
        &after_delete,
        Some((first.as_str(), "first at five")),
    );
    delete_ok(&router, "delete the first", &first).await;
    let emptied = read_both(&router, "both deleted", &flown).await;
    assert_current_ephemeris("both deleted", &emptied, None);
    let untouched = read_both(&router, "the other satellite, at the end", &other).await;
    assert_current_ephemeris(
        "the other satellite, at the end",
        &untouched,
        Some((others.as_str(), "the other satellite")),
    );
}
