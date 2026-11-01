// SPDX-License-Identifier: CC0-1.0

//! `CreateEphemeris` (`POST /ephemeris`) with azimuth-elevation pointing data, end to end
//! over HTTP through the same front door as the rest of this target's suites.
//!
//! The RULE under test: supplied data that breaks a rule the model states for it describes no
//! usable trajectory, so the validation the create starts lands the ephemeris on `INVALID`
//! carrying the `EphemerisInvalidReason` for the kind of fault — the times the data declares
//! (`TIME_RANGE_INVALID`) or the pointing data itself (`TRAJECTORY_INVALID`) — instead of
//! `ENABLED`, and an ephemeris that did not validate is in force for nothing: it is not the
//! satellite's `currentEphemeris` and no availability search reads a pass from it.
//!
//! The rules the model states for this data, each the source of a group of rows below:
//!
//! * the segments "must be in chronological order with no overlaps"
//!   (`smithy_query(query="doc", shape="AzElSegments", member="azElSegmentList")`);
//! * a segment's points "must be in ascending order with no duplicates" by `dt`
//!   (`smithy_query(query="doc", shape="TimeAzEl", member="dt")`);
//! * an `az` holds "-180 to 360 degrees" or "-π to 2π radians" and an `el` "-90 to 90 degrees"
//!   or "-π/2 to π/2 radians", per `angleUnit` (`smithy_query(query="doc", shape="TimeAzEl",
//!   member="az")` / `member="el"`).
//!
//! So the suites below assert that rule over a SET of inputs rather than over one call: a
//! table spanning each of those rules, each tested on both `AngleUnits` values where the rule
//! is stated per unit, on each published bound of every range and one step past it, and on
//! the forms a fault takes (descending, duplicated and one-out-of-place `dt`; segments
//! reversed, overlapping and merely touching), beside the rows that satisfy every rule and
//! must therefore be enabled.

use serde_json::{json, Value};
use service_com_amazonaws_groundstation_groundstation::state::{
    service_state, ANGLE_UNIT_DEGREE_ANGLE, ANGLE_UNIT_RADIAN, BAD_REQUEST_ERROR_CODE,
    EPHEMERIS_ENABLED, EPHEMERIS_INVALID, EPHEMERIS_TIME_RANGE_INVALID,
    EPHEMERIS_TRAJECTORY_INVALID,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::available_contacts::{availability, list_ok, GROUND_STATION};
use crate::ephemeris_catalog::{create_ok, onboard, wait_out_validation, FUTURE};
use crate::ephemeris_description::describe_ok;
use crate::scheduling_loop::{create_mission_profile, harness_with_engine, send, Reply};

/// The NORAD catalogue number this suite's satellite is on-boarded under, inside
/// `state::NORAD_SATELLITE_ID_RANGE`.
const NORAD: i32 = 25_994;

/// The instant the pointing data of the status table is placed at: a fixed instant well ahead
/// of any clock this suite runs on, since what the table is about is the DATA rather than what
/// can still be flown.
const BASE: i64 = 1_800_000_000;

/// How long each segment of the table declares its validity for.
const SEGMENT_SECONDS: i64 = 3_600;

/// One time-tagged pointing point as a request body carries it: `(dt, az, el)`, the three
/// required members of `TimeAzEl` (`smithy_query(query="shape", shape="TimeAzEl")`).
type Point = (f64, f64, f64);

/// Five points ascending in `dt` ten seconds apart, every one carrying `az` and `el`.
///
/// Five is the fewest a segment may carry ("must contain at least five points to support 4th
/// order Lagrange interpolation" — `smithy_query(query="doc", shape="AzElSegment",
/// member="azElList")`), so a row that is about something other than the point count carries
/// exactly that many.
fn five_points(az: f64, el: f64) -> Vec<Point> {
    (0..5)
        .map(|step| (f64::from(step) * 10.0, az, el))
        .collect()
}

/// One segment as the request body carries it: its reference epoch, the validity range it
/// declares, and its points.
fn segment(reference_epoch: i64, valid: (i64, i64), points: &[Point]) -> Value {
    let list: Vec<Value> = points
        .iter()
        .map(|(dt, az, el)| json!({"dt": dt, "az": az, "el": el}))
        .collect();
    json!({
        "referenceEpoch": reference_epoch,
        "validTimeRange": {"startTime": valid.0, "endTime": valid.1},
        "azElList": list,
    })
}

/// An `azEl` ephemeris body for this suite's ground station, measuring its angles in
/// `angle_unit` and carrying `segments`.
fn az_el_data(angle_unit: &str, segments: Vec<Value>) -> Value {
    json!({"azEl": {
        "groundStation": GROUND_STATION,
        "data": {"azElData": {"angleUnit": angle_unit, "azElSegmentList": segments}},
    }})
}

/// An `azEl` ephemeris body of ONE segment at [`BASE`], carrying five points at `az` and `el`
/// measured in `angle_unit`.
fn one_segment(angle_unit: &str, az: f64, el: f64) -> Value {
    az_el_data(
        angle_unit,
        vec![segment(
            BASE,
            (BASE, BASE + SEGMENT_SECONDS),
            &five_points(az, el),
        )],
    )
}

/// An `azEl` ephemeris body of ONE segment at [`BASE`] in degrees, carrying `points` verbatim.
fn one_segment_of(points: &[Point]) -> Value {
    az_el_data(
        ANGLE_UNIT_DEGREE_ANGLE,
        vec![segment(BASE, (BASE, BASE + SEGMENT_SECONDS), points)],
    )
}

/// An `azEl` ephemeris body of two segments in degrees, each declaring its own validity range
/// and carrying five usable points offset from its own reference epoch.
fn two_segments(first: (i64, i64), second: (i64, i64)) -> Value {
    az_el_data(
        ANGLE_UNIT_DEGREE_ANGLE,
        vec![
            segment(first.0, first, &five_points(10.0, 20.0)),
            segment(second.0, second, &five_points(11.0, 21.0)),
        ],
    )
}

/// One row of the table: the `ephemeris` member the create carries, and the
/// `EphemerisInvalidReason` the validation must land on — `None` where the data breaks no rule
/// and the ephemeris must be enabled instead.
struct Row {
    case: &'static str,
    data: Value,
    reason: Option<&'static str>,
}

/// The table: one row per way azimuth-elevation data can stand against the rules the model
/// states for it, and the verdict each earns.
///
/// Every published bound of every documented range is a row, with one step past it beside it,
/// in BOTH units the ranges are stated in — the angles are the same angles in either unit, so
/// a value legal in one is illegal in the other, which the last rows put.
fn rows() -> Vec<Row> {
    let radian_azimuth = (-std::f64::consts::PI, std::f64::consts::TAU);
    let radian_elevation = (
        -std::f64::consts::FRAC_PI_2,
        std::f64::consts::FRAC_PI_2,
    );
    vec![
        // Nothing is wrong with these.
        Row {
            case: "degrees, one segment, every rule satisfied",
            data: one_segment(ANGLE_UNIT_DEGREE_ANGLE, 120.0, 45.5),
            reason: None,
        },
        Row {
            case: "radians, one segment, every rule satisfied",
            data: one_segment(ANGLE_UNIT_RADIAN, 2.1, 0.8),
            reason: None,
        },
        Row {
            case: "degrees, angles at both ends of both ranges",
            data: az_el_data(
                ANGLE_UNIT_DEGREE_ANGLE,
                vec![segment(
                    BASE,
                    (BASE, BASE + SEGMENT_SECONDS),
                    &[
                        (0.0, -180.0, -90.0),
                        (10.0, 360.0, 90.0),
                        (20.0, 0.0, 0.0),
                        (30.0, 359.5, 89.5),
                        (40.0, -179.5, -89.5),
                    ],
                )],
            ),
            reason: None,
        },
        Row {
            case: "radians, angles at both ends of both ranges",
            data: az_el_data(
                ANGLE_UNIT_RADIAN,
                vec![segment(
                    BASE,
                    (BASE, BASE + SEGMENT_SECONDS),
                    &[
                        (0.0, radian_azimuth.0, radian_elevation.0),
                        (10.0, radian_azimuth.1, radian_elevation.1),
                        (20.0, 0.0, 0.0),
                        (30.0, 3.0, 1.5),
                        (40.0, -3.0, -1.5),
                    ],
                )],
            ),
            reason: None,
        },
        Row {
            case: "two segments that touch at an instant",
            data: two_segments(
                (BASE, BASE + SEGMENT_SECONDS),
                (BASE + SEGMENT_SECONDS, BASE + 2 * SEGMENT_SECONDS),
            ),
            reason: None,
        },
        // The points of a segment must ascend in `dt`, with no duplicates.
        Row {
            case: "every dt descending",
            data: one_segment_of(&[
                (40.0, 10.0, 20.0),
                (30.0, 11.0, 21.0),
                (20.0, 12.0, 22.0),
                (10.0, 13.0, 23.0),
                (0.0, 14.0, 24.0),
            ]),
            reason: Some(EPHEMERIS_TRAJECTORY_INVALID),
        },
        Row {
            case: "one dt out of place in the middle",
            data: one_segment_of(&[
                (0.0, 10.0, 20.0),
                (10.0, 11.0, 21.0),
                (5.0, 12.0, 22.0),
                (30.0, 13.0, 23.0),
                (40.0, 14.0, 24.0),
            ]),
            reason: Some(EPHEMERIS_TRAJECTORY_INVALID),
        },
        Row {
            case: "a duplicated dt",
            data: one_segment_of(&[
                (0.0, 10.0, 20.0),
                (10.0, 11.0, 21.0),
                (10.0, 12.0, 22.0),
                (30.0, 13.0, 23.0),
                (40.0, 14.0, 24.0),
            ]),
            reason: Some(EPHEMERIS_TRAJECTORY_INVALID),
        },
        Row {
            case: "the last two dt values swapped",
            data: one_segment_of(&[
                (0.0, 10.0, 20.0),
                (10.0, 11.0, 21.0),
                (20.0, 12.0, 22.0),
                (40.0, 13.0, 23.0),
                (30.0, 14.0, 24.0),
            ]),
            reason: Some(EPHEMERIS_TRAJECTORY_INVALID),
        },
        Row {
            case: "a later segment's points descending",
            data: az_el_data(
                ANGLE_UNIT_DEGREE_ANGLE,
                vec![
                    segment(
                        BASE,
                        (BASE, BASE + SEGMENT_SECONDS),
                        &five_points(10.0, 20.0),
                    ),
                    segment(
                        BASE + SEGMENT_SECONDS,
                        (BASE + SEGMENT_SECONDS, BASE + 2 * SEGMENT_SECONDS),
                        &[
                            (40.0, 10.0, 20.0),
                            (30.0, 11.0, 21.0),
                            (20.0, 12.0, 22.0),
                            (10.0, 13.0, 23.0),
                            (0.0, 14.0, 24.0),
                        ],
                    ),
                ],
            ),
            reason: Some(EPHEMERIS_TRAJECTORY_INVALID),
        },
        // The segments must be in chronological order with no overlaps.
        Row {
            case: "two segments in reverse chronological order",
            data: two_segments(
                (BASE + 2 * SEGMENT_SECONDS, BASE + 3 * SEGMENT_SECONDS),
                (BASE, BASE + SEGMENT_SECONDS),
            ),
            reason: Some(EPHEMERIS_TIME_RANGE_INVALID),
        },
        Row {
            case: "two segments overlapping by a second",
            data: two_segments(
                (BASE, BASE + SEGMENT_SECONDS),
                (BASE + SEGMENT_SECONDS - 1, BASE + 2 * SEGMENT_SECONDS),
            ),
            reason: Some(EPHEMERIS_TIME_RANGE_INVALID),
        },
        Row {
            case: "a later segment wholly inside an earlier one",
            data: two_segments(
                (BASE, BASE + 4 * SEGMENT_SECONDS),
                (BASE + SEGMENT_SECONDS, BASE + 2 * SEGMENT_SECONDS),
            ),
            reason: Some(EPHEMERIS_TIME_RANGE_INVALID),
        },
        Row {
            case: "a segment whose validity runs backwards",
            data: az_el_data(
                ANGLE_UNIT_DEGREE_ANGLE,
                vec![segment(
                    BASE,
                    (BASE + SEGMENT_SECONDS, BASE),
                    &five_points(10.0, 20.0),
                )],
            ),
            reason: Some(EPHEMERIS_TIME_RANGE_INVALID),
        },
        // An angle must lie inside the range its unit documents.
        Row {
            case: "an elevation one step above the degree maximum",
            data: one_segment(ANGLE_UNIT_DEGREE_ANGLE, 120.0, 90.5),
            reason: Some(EPHEMERIS_TRAJECTORY_INVALID),
        },
        Row {
            case: "an elevation far above the degree maximum",
            data: one_segment(ANGLE_UNIT_DEGREE_ANGLE, 120.0, 500.0),
            reason: Some(EPHEMERIS_TRAJECTORY_INVALID),
        },
        Row {
            case: "an elevation one step below the degree minimum",
            data: one_segment(ANGLE_UNIT_DEGREE_ANGLE, 120.0, -90.5),
            reason: Some(EPHEMERIS_TRAJECTORY_INVALID),
        },
        Row {
            case: "an azimuth one step above the degree maximum",
            data: one_segment(ANGLE_UNIT_DEGREE_ANGLE, 360.5, 20.0),
            reason: Some(EPHEMERIS_TRAJECTORY_INVALID),
        },
        Row {
            case: "an azimuth one step below the degree minimum",
            data: one_segment(ANGLE_UNIT_DEGREE_ANGLE, -180.5, 20.0),
            reason: Some(EPHEMERIS_TRAJECTORY_INVALID),
        },
        Row {
            case: "one point of five outside the degree elevation range",
            data: one_segment_of(&[
                (0.0, 10.0, 20.0),
                (10.0, 11.0, 21.0),
                (20.0, 12.0, 95.0),
                (30.0, 13.0, 23.0),
                (40.0, 14.0, 24.0),
            ]),
            reason: Some(EPHEMERIS_TRAJECTORY_INVALID),
        },
        Row {
            case: "an elevation just past the radian maximum",
            data: one_segment(ANGLE_UNIT_RADIAN, 2.1, radian_elevation.1 + 0.001),
            reason: Some(EPHEMERIS_TRAJECTORY_INVALID),
        },
        Row {
            case: "an elevation just past the radian minimum",
            data: one_segment(ANGLE_UNIT_RADIAN, 2.1, radian_elevation.0 - 0.001),
            reason: Some(EPHEMERIS_TRAJECTORY_INVALID),
        },
        Row {
            case: "an azimuth just past the radian maximum",
            data: one_segment(ANGLE_UNIT_RADIAN, radian_azimuth.1 + 0.001, 0.8),
            reason: Some(EPHEMERIS_TRAJECTORY_INVALID),
        },
        Row {
            case: "an azimuth just past the radian minimum",
            data: one_segment(ANGLE_UNIT_RADIAN, radian_azimuth.0 - 0.001, 0.8),
            reason: Some(EPHEMERIS_TRAJECTORY_INVALID),
        },
        Row {
            case: "angles that are legal in degrees, declared in radians",
            data: one_segment(ANGLE_UNIT_RADIAN, 120.0, 45.5),
            reason: Some(EPHEMERIS_TRAJECTORY_INVALID),
        },
    ]
}

#[tokio::test]
async fn az_el_data_that_breaks_a_rule_the_model_states_settles_to_invalid_with_its_reason() {
    let (router, engine) = harness_with_engine();
    let satellite_id = onboard(&engine, &format!("{NORAD}={GROUND_STATION}"))
        .first()
        .cloned()
        .expect("the satellite is on-boarded");

    let rows = rows();
    let mut created: Vec<(String, &Row)> = Vec::new();
    for row in &rows {
        let id = create_ok(
            &router,
            row.case,
            json!({
                "name": "az el data",
                "enabled": true,
                "satelliteId": satellite_id,
                "expirationTime": FUTURE,
                "ephemeris": row.data.clone(),
            }),
        )
        .await;
        created.push((id, row));
    }

    wait_out_validation();
    for (id, row) in &created {
        let body = describe_ok(&router, row.case, id).await;
        let status = body
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let reason = body.get("invalidReason").and_then(Value::as_str);
        match row.reason {
            Some(expected) => {
                assert_eq!(
                    status, EPHEMERIS_INVALID,
                    "case {}: data the model declares unusable must settle to \
                     {EPHEMERIS_INVALID}, got {body}",
                    row.case
                );
                assert_eq!(
                    reason,
                    Some(expected),
                    "case {}: expected the reason {expected}, got {body}",
                    row.case
                );
            }
            None => {
                assert_eq!(
                    status, EPHEMERIS_ENABLED,
                    "case {}: data that breaks no rule must settle to {EPHEMERIS_ENABLED}, got \
                     {body}",
                    row.case
                );
                assert_eq!(
                    reason, None,
                    "case {}: data that breaks no rule names no invalidReason, got {body}",
                    row.case
                );
            }
        }
    }
}

/// The ARN of the satellite `satellite_id`, which is how `ListContacts` names it.
fn satellite_arn(engine: &engine::InMemoryEngine, satellite_id: &str) -> String {
    service_state(engine)
        .expect("this service's state store")
        .lock()
        .satellites
        .get(satellite_id)
        .expect("the on-boarded satellite")
        .satellite_arn
        .clone()
}

/// The `currentEphemeris` the satellite `satellite_id` reports, as the `ephemerisId` it names.
async fn current_ephemeris(
    router: &MultiServiceRouter,
    case: &str,
    satellite_id: &str,
) -> Option<String> {
    let reply = send(router, "GET", &format!("/satellite/{satellite_id}"), None).await;
    assert_eq!(
        reply.status, 200,
        "case {case}: expected GetSatellite 200, got {} {}",
        reply.status, reply.body
    );
    reply
        .body
        .get("currentEphemeris")
        .and_then(|current| current.get("ephemerisId"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// One row of the in-force table: pointing data placed against a future instant, and whether
/// the ephemeris carrying it is in force once its validation has fallen due.
struct InForce {
    case: &'static str,
    /// The `ephemeris` member the create carries, built against the instant the pass starts.
    data: fn(i64) -> Value,
    in_force: bool,
}

#[tokio::test]
async fn az_el_data_that_did_not_validate_is_in_force_for_nothing() {
    // One pass of 120 seconds a minute after the instant the data is placed at, described
    // four ways: once satisfying every rule, and once per rule broken. Only the first can be
    // read as the pointing data in force.
    let rows = vec![
        InForce {
            case: "every rule satisfied",
            data: |base| pass_segments(base, Standing::Sound),
            in_force: true,
        },
        InForce {
            case: "the points of its segment descending",
            data: |base| pass_segments(base, Standing::PointsDescending),
            in_force: false,
        },
        InForce {
            case: "its segments overlapping",
            data: |base| pass_segments(base, Standing::SegmentsOverlapping),
            in_force: false,
        },
        InForce {
            case: "an elevation outside the range of its unit",
            data: |base| pass_segments(base, Standing::AngleOutOfRange),
            in_force: false,
        },
    ];

    for row in &rows {
        let (router, engine) = harness_with_engine();
        let satellite_id = onboard(&engine, &format!("{NORAD}={GROUND_STATION}"))
            .first()
            .cloned()
            .expect("the satellite is on-boarded");
        let arn = satellite_arn(&engine, &satellite_id);
        let base = future_base();
        let ephemeris_id = create_ok(
            &router,
            row.case,
            json!({
                "name": "pointing data",
                "enabled": true,
                "satelliteId": satellite_id,
                "ephemeris": (row.data)(base),
            }),
        )
        .await;
        wait_out_validation();

        let current = current_ephemeris(&router, row.case, &satellite_id).await;
        assert_eq!(
            current.as_deref() == Some(ephemeris_id.as_str()),
            row.in_force,
            "case {}: expected in force {}, got {current:?}",
            row.case,
            row.in_force
        );

        let profile_arn =
            create_mission_profile(&router, row.case, 60, Some(120), Some(60)).await;
        let page = list_ok(
            &router,
            row.case,
            availability((base - 3_600, base + 86_400), &arn, &profile_arn, json!({})),
        )
        .await;
        let offered = usize::from(row.in_force);
        assert_eq!(
            page.len(),
            offered,
            "case {}: expected {offered} offered window(s), got {page:?}",
            row.case
        );
    }
}

/// How the pointing data of an in-force row stands against the rules.
enum Standing {
    /// Every rule satisfied: two chronological segments, points ascending, angles in range.
    Sound,
    /// The same, with the points of the first segment in the reverse order, so their `dt`
    /// values descend.
    PointsDescending,
    /// The same, with the second segment's validity starting inside the first's.
    SegmentsOverlapping,
    /// The same, with one elevation above the maximum its unit documents.
    AngleOutOfRange,
}

/// The instant the in-force rows place their pass at: an hour ahead of the clock, so the pass
/// is one the pairing can still fly.
fn future_base() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs() as i64
        + 3_600
}

/// Two segments whose FIRST describes one pass of 120 seconds a minute after `base`, standing
/// against the rules as `standing` says. The second segment's own points span 40 seconds,
/// which is under the minimum viable duration the row's mission profile carries, so the pass
/// of the first segment is the only window sound data offers.
fn pass_segments(base: i64, standing: Standing) -> Value {
    let mut points: Vec<Point> = [-5.0, 10.0, 45.5, 8.0, -3.0]
        .iter()
        .enumerate()
        .map(|(step, el)| (step as f64 * 60.0, 120.0, *el))
        .collect();
    let first = (base, base + SEGMENT_SECONDS);
    let mut second = (base + SEGMENT_SECONDS, base + 2 * SEGMENT_SECONDS);
    match standing {
        Standing::Sound => {}
        Standing::PointsDescending => points.reverse(),
        Standing::SegmentsOverlapping => second.0 = first.1 - 1,
        Standing::AngleOutOfRange => points[2].2 = 95.0,
    }
    az_el_data(
        ANGLE_UNIT_DEGREE_ANGLE,
        vec![
            segment(base, first, &points),
            segment(second.0, second, &five_points(10.0, 20.0)),
        ],
    )
}

#[tokio::test]
async fn an_angle_unit_outside_the_modelled_enum_is_refused_before_the_handler() {
    // `angleUnit` is a modelled value set (`AngleUnits` — DEGREE_ANGLE and RADIAN), so a token
    // outside it fails the model's own rules and never reaches the handler. That is what lets
    // the range checks above treat the unit as one of the two.
    let (router, engine) = harness_with_engine();
    let satellite_id = onboard(&engine, &format!("{NORAD}={GROUND_STATION}"))
        .first()
        .cloned()
        .expect("the satellite is on-boarded");
    for unit in ["GRADIAN", "degree_angle", "", "DEGREE"] {
        let reply: Reply = send(
            &router,
            "POST",
            "/ephemeris",
            Some(json!({
                "name": "az el data",
                "enabled": true,
                "satelliteId": satellite_id,
                "expirationTime": FUTURE,
                "ephemeris": one_segment(unit, 120.0, 45.5),
            })),
        )
        .await;
        assert_eq!(
            reply.status, 400,
            "case {unit}: expected 400, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            BAD_REQUEST_ERROR_CODE,
            "case {unit}: expected {BAD_REQUEST_ERROR_CODE}, got {}",
            reply.body
        );
    }
    assert!(
        service_state(&*engine)
            .expect("this service's state store")
            .lock()
            .ephemerides
            .is_empty(),
        "case the whole table: a refused create stores nothing"
    );
}
