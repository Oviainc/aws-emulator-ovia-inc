// SPDX-License-Identifier: CC0-1.0

//! The ONE ephemeris a satellite's trajectory is computed from, read through BOTH operations
//! that answer for it — `GetSatellite` (`GET /satellite/{satelliteId}`) and an `AVAILABLE`
//! search of `ListContacts` (`POST /contacts`), whose offered windows carry the `ephemeris`
//! their geometry was read from — end to end over HTTP through the same front door as the
//! rest of this target's suites.
//!
//! The RULE under test: of the ephemerides an account holds `ENABLED` for one satellite,
//! which one is in force over the instants more than one of them describes is settled by ONE
//! total order — the highest `priority` ("A priority score that determines which ephemeris to
//! use when multiple ephemerides overlap. Higher numbers take precedence." —
//! `smithy_query(query="doc", shape="CreateEphemerisRequest", member="priority")`), then the
//! most recently created, then the larger `ephemerisId` — so every read of the service
//! answers with the same ephemeris for the same satellite at the same instant: the one a
//! satellite read reports as its `currentEphemeris` is the one an offered window over those
//! instants names, and the peak elevation that window reports is the one THAT ephemeris
//! carries.
//!
//! So the suites below assert that rule over SETS of inputs rather than over one call:
//!
//! * a table spanning what the order generalises over — how many ephemerides describe the
//!   same pass (two to five), where in creation order the winning `priority` sits, ties of
//!   two, three, four and five ephemerides at one priority (both published bounds of
//!   `CustomerEphemerisPriority` among them), a tie at the top beside a lower one on either
//!   side of it, and the absent `priority` member that takes the documented default — each
//!   row read through `GetSatellite` and through the availability search and the two answers
//!   required to agree;
//! * the instants only ONE ephemeris describes, which the order does not arbitrate: two
//!   ephemerides tied at one priority whose passes do not overlap each offer their own
//!   window, carrying their own peak.
//!
//! The harness, the signed `restJson1` requests, the onboarding helper, the azimuth-elevation
//! data builders and the satellite reader are [`crate::scheduling_loop`]'s,
//! [`crate::ephemeris_catalog`]'s, [`crate::available_contacts`]'s,
//! [`crate::ephemeris_deletion`]'s and [`crate::ephemeris_supersession`]'s, reused rather
//! than rebuilt.

use serde_json::{json, Value};

use crate::available_contacts::{
    availability, az_el, ephemeris_body, list_ok, now_seconds, number, text, Segment,
    GROUND_STATION,
};
use crate::ephemeris_catalog::{create_ok, onboard, wait_out_validation};
use crate::ephemeris_deletion::satellite_arn;
use crate::ephemeris_supersession::current_ephemeris;
use crate::scheduling_loop::{create_mission_profile, harness_with_engine};

/// The `AngleUnits` the pointing data in this suite is measured in (`smithy_query(
/// query="shape", shape="AngleUnits")`).
const ANGLE_UNIT: &str = "DEGREE_ANGLE";

/// The shortest contact the mission profile every search here is for will take, in seconds:
/// below the 120-second pass [`one_pass`] describes, so what a row's answer differs from
/// another's by is which ephemeris is in force and never the profile.
const MINIMUM_VIABLE_SECONDS: i64 = 60;

/// One azimuth-elevation segment describing ONE pass of 120 seconds, starting a minute after
/// `start` and peaking at `peak`: the run of points at or above the horizon is what makes a
/// pass, so the peak is what identifies which ephemeris a window was read from.
fn one_pass(start: i64, peak: f64) -> Vec<Segment> {
    vec![Segment {
        reference_epoch: start,
        valid: (start, start + 3_600),
        points: vec![
            (0.0, -5.0),
            (60.0, 10.0),
            (120.0, peak),
            (180.0, 8.0),
            (240.0, -3.0),
        ],
    }]
}

/// The peak the ephemeris stored `index`-th in a row carries: distinct per position and
/// inside the 0..90 degrees an elevation above the horizon can hold, so the peak an answer
/// reports names exactly one of the row's ephemerides.
fn peak_of(index: usize) -> f64 {
    20.0 + 7.25 * index as f64
}

/// One row of the table: the enabled ephemerides one satellite holds, all describing the
/// SAME pass, as the `priority` each `CreateEphemeris` names in creation order, and which of
/// them the precedence order puts in force.
struct Row {
    case: &'static str,
    /// The `priority` each ephemeris of the row is created with, in creation order; `None`
    /// where the request names none and the documented default of 1 applies.
    priorities: Vec<Option<i64>>,
    /// The index, in creation order, of the ephemeris in force for the row's satellite.
    in_force: usize,
}

/// The table: one row per way several ephemerides describing one pass can stand, spanning
/// their count, where in creation order the winner sits, ties of two to five at one priority,
/// and the absent `priority` member.
fn rows() -> Vec<Row> {
    vec![
        Row {
            // The documented priority leaves this pair tied, so the tie-break decides, and
            // it is the CHOSEN one the satellite coupling takes: the most recently created.
            case: "two tied at one priority",
            priorities: vec![Some(4), Some(4)],
            in_force: 1,
        },
        Row {
            case: "three tied at one priority",
            priorities: vec![Some(4), Some(4), Some(4)],
            in_force: 2,
        },
        Row {
            // The published LOWEST bound of `CustomerEphemerisPriority`
            // (`smithy_query(query="operation", operation="CreateEphemeris")` — range
            // 1..99999).
            case: "four tied at the lowest priority the member admits",
            priorities: vec![Some(1), Some(1), Some(1), Some(1)],
            in_force: 3,
        },
        Row {
            // And its published HIGHEST bound.
            case: "five tied at the highest priority the member admits",
            priorities: vec![
                Some(99_999),
                Some(99_999),
                Some(99_999),
                Some(99_999),
                Some(99_999),
            ],
            in_force: 4,
        },
        Row {
            // "The default is 1. Must be 1 or greater." (`smithy_query(query="doc",
            // shape="CreateEphemerisRequest", member="priority")`), so two requests naming
            // no priority are tied at that default.
            case: "two tied at the documented default, neither naming a priority",
            priorities: vec![None, None],
            in_force: 1,
        },
        Row {
            case: "the absent priority member against an explicit two",
            priorities: vec![None, Some(2)],
            in_force: 1,
        },
        Row {
            case: "the absent priority member outranking nothing but itself",
            priorities: vec![Some(2), None],
            in_force: 0,
        },
        Row {
            case: "two, the higher priority stored second",
            priorities: vec![Some(1), Some(9)],
            in_force: 1,
        },
        Row {
            case: "two, the higher priority stored first",
            priorities: vec![Some(9), Some(1)],
            in_force: 0,
        },
        Row {
            case: "three, the highest priority stored in the middle",
            priorities: vec![Some(1), Some(99_999), Some(500)],
            in_force: 1,
        },
        Row {
            case: "a tie at the top, with the lower one stored last",
            priorities: vec![Some(4), Some(4), Some(2)],
            in_force: 1,
        },
        Row {
            case: "a tie at the top, with the lower one stored between them",
            priorities: vec![Some(4), Some(2), Some(4)],
            in_force: 2,
        },
    ]
}

#[tokio::test]
async fn a_satellite_read_and_an_availability_search_name_one_ephemeris_in_force() {
    let (router, engine) = harness_with_engine();
    let rows = rows();
    // One satellite per row, each holding only its own ephemerides, so no row's pointing data
    // can answer another row's search. The NORAD catalogue numbers fall inside
    // `state::NORAD_SATELLITE_ID_RANGE`.
    let setting: Vec<String> = (0..rows.len())
        .map(|row| format!("{}={GROUND_STATION}", 25_200 + row))
        .collect();
    let satellites = onboard(&engine, &setting.join(","));
    assert_eq!(
        satellites.len(),
        rows.len(),
        "case setup: the onboarding must store one satellite per row, got {satellites:?}"
    );
    let profile_arn = create_mission_profile(
        &router,
        "ephemeris in force",
        MINIMUM_VIABLE_SECONDS,
        None,
        None,
    )
    .await;

    // Every pass this suite stores is placed relative to the clock, because an availability
    // search only offers what can STILL be flown.
    let base = now_seconds() + 3_600;
    let window = (base - 3_600, base + 86_400);

    // Every row's ephemerides, stored through the front door in the row's own order: the
    // creation order is what the tie-break reads, so the ids come back in it.
    let mut minted: Vec<Vec<String>> = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        let mut ids: Vec<String> = Vec::new();
        for (held, priority) in row.priorities.iter().enumerate() {
            ids.push(
                create_ok(
                    &router,
                    row.case,
                    ephemeris_body(
                        &format!("row {index} held {held}"),
                        Some(&satellites[index]),
                        true,
                        *priority,
                        az_el(GROUND_STATION, ANGLE_UNIT, &one_pass(base, peak_of(held))),
                    ),
                )
                .await,
            );
        }
        minted.push(ids);
    }

    // ONE wait for the whole table: every ephemeris leaves `VALIDATING` on the clock alone.
    wait_out_validation();

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let expected_id = minted[index][row.in_force].clone();
        let expected_peak = peak_of(row.in_force);

        let reported = current_ephemeris(&router, case, &satellites[index]).await;
        assert_eq!(
            reported.as_deref(),
            Some(expected_id.as_str()),
            "case {case}: the satellite must report the ephemeris of highest precedence as its \
             currentEphemeris, got {reported:?}"
        );

        let flown = satellite_arn(&engine, &satellites[index]);
        let page = list_ok(
            &router,
            case,
            availability(window, &flown, &profile_arn, json!({})),
        )
        .await;
        assert_eq!(
            page.len(),
            1,
            "case {case}: ephemerides describing ONE pass offer one window, got {page:?}"
        );
        let offered = page[0].get("ephemeris").unwrap_or(&Value::Null).clone();
        assert_eq!(
            text(&offered, "ephemerisId"),
            expected_id,
            "case {case}: an offered window must name the same ephemeris the satellite reports \
             as the one in force ({expected_id}), got {offered}"
        );
        let elevation = page[0]
            .get("maximumElevation")
            .unwrap_or(&Value::Null)
            .clone();
        assert_eq!(
            number(&elevation, "value"),
            expected_peak,
            "case {case}: the peak must be the one the ephemeris in force carries, got \
             {elevation}"
        );
    }
}

#[tokio::test]
async fn ephemerides_tied_at_one_priority_whose_passes_do_not_overlap_each_offer_their_own() {
    let (router, engine) = harness_with_engine();
    let satellites = onboard(&engine, &format!("25994={GROUND_STATION}"));
    let [satellite_id] = <[String; 1]>::try_from(satellites).expect("one satellite is on-boarded");
    let flown = satellite_arn(&engine, &satellite_id);
    let profile_arn = create_mission_profile(
        &router,
        "disjoint passes",
        MINIMUM_VIABLE_SECONDS,
        None,
        None,
    )
    .await;

    // Two ephemerides at ONE priority describing passes two hours apart: no instant is
    // described by both, so neither supersedes the other over the instants it covers.
    let base = now_seconds() + 3_600;
    let mut stored: Vec<(String, i64, f64)> = Vec::new();
    for (held, start) in [base, base + 7_200].into_iter().enumerate() {
        let peak = peak_of(held);
        let id = create_ok(
            &router,
            "disjoint passes",
            ephemeris_body(
                &format!("disjoint held {held}"),
                Some(&satellite_id),
                true,
                Some(4),
                az_el(GROUND_STATION, ANGLE_UNIT, &one_pass(start, peak)),
            ),
        )
        .await;
        stored.push((id, start, peak));
    }
    wait_out_validation();

    let page = list_ok(
        &router,
        "disjoint passes",
        availability(
            (base - 3_600, base + 86_400),
            &flown,
            &profile_arn,
            json!({}),
        ),
    )
    .await;
    assert_eq!(
        page.len(),
        stored.len(),
        "case disjoint passes: a pass no other ephemeris describes is offered whatever the \
         priorities are tied at, got {page:?}"
    );
    for (offered, (ephemeris_id, start, peak)) in page.iter().zip(&stored) {
        assert_eq!(
            number(offered, "startTime"),
            (start + 60) as f64,
            "case disjoint passes: expected the pass the data describes, got {offered}"
        );
        let reported = offered.get("ephemeris").unwrap_or(&Value::Null).clone();
        assert_eq!(
            &text(&reported, "ephemerisId"),
            ephemeris_id,
            "case disjoint passes: each window must name the ephemeris its geometry was read \
             from, got {reported}"
        );
        let elevation = offered
            .get("maximumElevation")
            .unwrap_or(&Value::Null)
            .clone();
        assert_eq!(
            number(&elevation, "value"),
            *peak,
            "case disjoint passes: expected the peak that ephemeris carries, got {elevation}"
        );
    }
}
