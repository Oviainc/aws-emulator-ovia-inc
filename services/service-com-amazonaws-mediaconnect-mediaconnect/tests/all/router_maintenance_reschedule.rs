// SPDX-License-Identifier: CC0-1.0

//! What a REPLACED maintenance configuration does to the maintenance window a router
//! resource reports, end to end over HTTP: create -> start -> update -> read back.
//!
//! The rule the cases below pin: a maintenance window is DERIVED from the resource's
//! maintenance configuration and never carried by a request, so it belongs to the preference
//! it was derived from — an update that replaces that preference re-derives the window as the
//! next occurrence of the preference the resource now carries, and a read reports that one;
//! an update that leaves the preference where it was (or does not carry one at all) leaves
//! the window alone, and a resource nothing has scheduled a window for still has none.
//!
//! The rows vary what the rule generalises over: which day and time the resource was created
//! preferring, which day and time it is updated to prefer (another day, another time, both,
//! and the ends of the clock), both directions between a stated preference and the `Default`
//! configuration that states none, a replacement that is the preference the resource already
//! carried, and both sides of the router — a router INPUT and a router OUTPUT, which hold the
//! same derived window.
//!
//! The harness is the one `flow_lifecycle` owns (one router, one shared engine) and the
//! request helpers, fixtures and window assertions are the ones the router suites already
//! own, so every case drives the request path a MediaConnect client reaches: routing,
//! restJson1 deserialization, constraint validation, handler, serialization.

use serde_json::{json, Value};

use service_com_amazonaws_mediaconnect_mediaconnect::state as mc;

use crate::flow_lifecycle::Emulator;
use crate::router_input_restart::{
    create_body, create_router_input, get_router_input, minted_configuration_rows,
    settle_router_input_in, union, union_member,
};
use crate::router_input_start::{
    assert_window_shape, day_of_week, now_secs, time_of_day, SECONDS_PER_DAY, SECONDS_PER_HOUR,
};
use crate::router_output_configuration_targets::{get_router_output, update_router_output};
use crate::router_output_stop::settle_router_output_in;
use crate::router_outputs::{create_router_output, minted_network_interface, standard};
use crate::update_router_input::update_router_input;

/// A `maintenanceConfiguration` naming a preferred `day` and `time`.
fn preferred(day: &str, time: &str) -> Value {
    union(
        "PreferredDayTime",
        "preferredDayTime",
        json!({ "day": day, "time": time }),
    )
}

/// The `Default` maintenance configuration, which states no preference at all.
fn default_maintenance() -> Value {
    union("Default", "default", json!({}))
}

/// One row: the preference a router resource is created with, the preference an update
/// replaces it with, and the window that replacement must leave the resource reporting.
struct ReplacementRow {
    case: &'static str,
    original: Value,
    replacement: Value,
    /// The position in a Monday-first week of the day the window must open on, or `None`
    /// when the replacement preference names no day.
    expected_day: Option<i64>,
    /// The time of day, in seconds past midnight UTC, the window must open at.
    expected_time: i64,
    /// Whether the replacement is a DIFFERENT preference, so the window the start scheduled
    /// no longer belongs to it and must be replaced.
    reschedules: bool,
}

/// The replacements the rule has to hold over.
///
/// The days are values of the `Day` enum the model declares
/// (`smithy_query(query="shape",shape="Day")`: MONDAY | TUESDAY | WEDNESDAY | THURSDAY |
/// FRIDAY | SATURDAY | SUNDAY) and the times span the clock, including midnight and the last
/// minute of the day; the `Default` rows carry the hour this service documents as its default
/// (`mc::DEFAULT_MAINTENANCE_TIME_OF_DAY`), on no particular day.
fn replacement_rows() -> Vec<ReplacementRow> {
    let default_hour =
        i64::try_from(mc::DEFAULT_MAINTENANCE_TIME_OF_DAY.as_secs()).unwrap_or(i64::MAX);
    vec![
        ReplacementRow {
            case: "another day at another time",
            original: preferred("MONDAY", "02:00"),
            replacement: preferred("FRIDAY", "10:00"),
            expected_day: Some(4),
            expected_time: 10 * SECONDS_PER_HOUR,
            reschedules: true,
        },
        ReplacementRow {
            case: "the same day at another time",
            original: preferred("TUESDAY", "02:00"),
            replacement: preferred("TUESDAY", "23:30"),
            expected_day: Some(1),
            expected_time: 23 * SECONDS_PER_HOUR + 30 * 60,
            reschedules: true,
        },
        ReplacementRow {
            case: "another day at the same time",
            original: preferred("WEDNESDAY", "12:15"),
            replacement: preferred("SATURDAY", "12:15"),
            expected_day: Some(5),
            expected_time: 12 * SECONDS_PER_HOUR + 15 * 60,
            reschedules: true,
        },
        ReplacementRow {
            case: "midnight for the last minute of the day",
            original: preferred("MONDAY", "00:00"),
            replacement: preferred("MONDAY", "23:59"),
            expected_day: Some(0),
            expected_time: 23 * SECONDS_PER_HOUR + 59 * 60,
            reschedules: true,
        },
        ReplacementRow {
            case: "a stated preference for the default maintenance",
            original: preferred("THURSDAY", "13:00"),
            replacement: default_maintenance(),
            expected_day: None,
            expected_time: default_hour,
            reschedules: true,
        },
        ReplacementRow {
            case: "the default maintenance for a stated preference",
            original: default_maintenance(),
            replacement: preferred("SUNDAY", "00:00"),
            expected_day: Some(6),
            expected_time: 0,
            reschedules: true,
        },
        ReplacementRow {
            case: "the preference the resource already carried",
            original: preferred("FRIDAY", "06:30"),
            replacement: preferred("FRIDAY", "06:30"),
            expected_day: Some(4),
            expected_time: 6 * SECONDS_PER_HOUR + 30 * 60,
            reschedules: false,
        },
        ReplacementRow {
            case: "the default maintenance again",
            original: default_maintenance(),
            replacement: default_maintenance(),
            expected_day: None,
            expected_time: default_hour,
            reschedules: false,
        },
    ]
}

/// Assert the window `schedule` reports is the next occurrence of the preference the row
/// expects, as of `called_at`.
fn assert_next_occurrence(
    case: &str,
    schedule: &Value,
    called_at: i64,
    expected_day: Option<i64>,
    expected_time: i64,
) {
    let (start, _, scheduled_time) = assert_window_shape(case, schedule, called_at);
    assert_eq!(
        time_of_day(start),
        expected_time,
        "case {case:?}: the window opens at the preferred time of day, got {schedule}"
    );
    match expected_day {
        Some(expected_day) => {
            assert_eq!(
                day_of_week(start),
                expected_day,
                "case {case:?}: the window opens on the preferred day, got {schedule}"
            );
            assert!(
                start - called_at <= 7 * SECONDS_PER_DAY,
                "case {case:?}: the window is the NEXT occurrence of the preference, got \
                 {start} for a call at {called_at}"
            );
        }
        None => assert!(
            start - called_at <= SECONDS_PER_DAY,
            "case {case:?}: a preference that names no day comes round every day, got {start} \
             for a call at {called_at}"
        ),
    }
    assert_eq!(
        scheduled_time, start,
        "case {case:?}: the maintenance is scheduled for the instant the window opens, got \
         {schedule}"
    );
}

/// Assert the maintenance configuration a read reports is the one the update carried.
///
/// The request body names the union member under both keys the crate's codec can read (see
/// [`union`]) while a response carries exactly one, so the members are compared rather than
/// the envelopes.
fn assert_reported_preference(case: &str, reported: &Value, requested: &Value) {
    // The members `MaintenanceConfiguration` declares
    // (`smithy_query(query="shape",shape="MaintenanceConfiguration")`: PreferredDayTime,
    // Default), each with the `jsonName` the model gives it.
    for (member_name, json_name) in [
        ("PreferredDayTime", "preferredDayTime"),
        ("Default", "default"),
    ] {
        let wanted = union_member(requested, member_name, json_name);
        if wanted.is_null() {
            continue;
        }
        assert_eq!(
            union_member(reported, member_name, json_name),
            wanted,
            "case {case:?}: the stored preference is the {member_name} one the update \
             carried, got {reported}"
        );
        return;
    }
    panic!("case {case:?}: every row carries a maintenance configuration, got {requested}");
}

/// A name no other row uses, derived from the row so a failure names the row.
fn case_name(prefix: &str, case: &str) -> String {
    let slug: String = case
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character
            } else {
                '-'
            }
        })
        .collect();
    format!("{prefix}-{slug}")
}

/// Create a started router input carrying `maintenance_configuration` and answer its ARN and
/// the window the start scheduled for it.
async fn started_router_input(
    emulator: &Emulator,
    case: &str,
    name: &str,
    configuration: Value,
    maintenance_configuration: Value,
) -> (String, Value) {
    let mut body = create_body(name, configuration);
    body["maintenanceConfiguration"] = maintenance_configuration;
    let (status, code, created) = create_router_input(emulator, body).await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "case {case:?}: CreateRouterInput answers 201, got {created}"
    );
    let arn = created["routerInput"]["arn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    settle_router_input_in(emulator, &arn, mc::ROUTER_INPUT_STATE_STANDBY);

    let (status, code, accepted) = emulator
        .send("POST", &format!("/v1/routerInput/start/{arn}"), None)
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (202, None),
        "case {case:?}: StartRouterInput answers 202, got {accepted}"
    );
    (arn, accepted["maintenanceSchedule"].clone())
}

/// Replacing a started router input's maintenance preference re-derives the window it is
/// next maintained in, and every reader reports the new one.
#[tokio::test]
async fn a_replaced_preference_reschedules_a_router_inputs_maintenance_window() {
    let emulator = Emulator::new();
    let (_, _, _, configuration, _) = minted_configuration_rows(&emulator, "ri-reschedule")
        .await
        .into_iter()
        .next()
        .expect("the configuration table has rows");

    for row in replacement_rows() {
        let ReplacementRow {
            case,
            original,
            replacement,
            expected_day,
            expected_time,
            reschedules,
        } = row;
        let name = case_name("ri-resched", case);
        let (arn, scheduled) =
            started_router_input(&emulator, case, &name, configuration.clone(), original).await;
        assert!(
            !scheduled.is_null(),
            "case {case:?}: the start scheduled a window, got {scheduled}"
        );

        let called_at = now_secs();
        let (status, code, updated) = update_router_input(
            &emulator,
            &arn,
            json!({ "maintenanceConfiguration": replacement.clone() }),
        )
        .await;
        assert_eq!(
            (status, code.as_deref()),
            (202, None),
            "case {case:?}: UpdateRouterInput answers 202, got {updated}"
        );
        let reported = &updated["routerInput"];
        assert_eq!(
            reported["maintenanceScheduleType"].as_str(),
            Some("WINDOW"),
            "case {case:?}: the schedule type follows the window the record holds, got \
             {reported}"
        );
        let window = reported["maintenanceSchedule"].clone();
        assert_next_occurrence(case, &window, called_at, expected_day, expected_time);
        if reschedules {
            assert_ne!(
                window, scheduled,
                "case {case:?}: the window derived from the replaced preference is gone, got \
                 {window}"
            );
        } else {
            assert_eq!(
                window, scheduled,
                "case {case:?}: a preference that did not move leaves the window alone, got \
                 {window}"
            );
        }

        // The window is PERSISTED, not just reported: the reader computes it from the same
        // record, and reading twice reports one window.
        let (status, _, read_back) = get_router_input(&emulator, &arn).await;
        let stored = &read_back["routerInput"];
        assert_eq!(
            (status, &stored["maintenanceSchedule"]),
            (200, &window),
            "case {case:?}: GetRouterInput reports the window the update left on the record, \
             got {read_back}"
        );
        assert_reported_preference(case, &stored["maintenanceConfiguration"], &replacement);
        let (_, _, read_again) = get_router_input(&emulator, &arn).await;
        assert_eq!(
            read_again["routerInput"]["maintenanceSchedule"], window,
            "case {case:?}: reading twice reports one window, got {read_again}"
        );
    }
}

/// An update that does not carry a maintenance configuration leaves the window alone, and an
/// input nothing has started has no window for an update to re-derive.
#[tokio::test]
async fn an_update_schedules_no_window_of_its_own() {
    let emulator = Emulator::new();
    let (_, _, _, configuration, _) = minted_configuration_rows(&emulator, "ri-no-window")
        .await
        .into_iter()
        .next()
        .expect("the configuration table has rows");

    // 1. A started input, updated in a way that says nothing about maintenance.
    let (arn, scheduled) = started_router_input(
        &emulator,
        "another member alone",
        "ri-other-member",
        configuration.clone(),
        preferred("MONDAY", "02:00"),
    )
    .await;
    let (status, _, updated) =
        update_router_input(&emulator, &arn, json!({ "name": "ri-other-member-renamed" })).await;
    assert_eq!(
        (status, &updated["routerInput"]["maintenanceSchedule"]),
        (202, &scheduled),
        "case \"another member alone\": an update that carries no maintenance configuration \
         leaves the window alone, got {updated}"
    );
    let (_, _, read_back) = get_router_input(&emulator, &arn).await;
    assert_eq!(
        read_back["routerInput"]["maintenanceSchedule"], scheduled,
        "case \"another member alone\": the reader reports the window the start scheduled, \
         got {read_back}"
    );

    // 2. An input nothing has started: there is no window, and replacing the preference does
    //    not schedule one — the start is the call that schedules the first window.
    let mut body = create_body("ri-never-started", configuration);
    body["maintenanceConfiguration"] = preferred("MONDAY", "02:00");
    let (status, _, created) = create_router_input(&emulator, body).await;
    assert_eq!(
        status, 201,
        "case \"never started\": CreateRouterInput answers 201, got {created}"
    );
    let arn = created["routerInput"]["arn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let replacement = preferred("FRIDAY", "10:00");
    let (status, _, updated) = update_router_input(
        &emulator,
        &arn,
        json!({ "maintenanceConfiguration": replacement.clone() }),
    )
    .await;
    assert_eq!(
        status, 202,
        "case \"never started\": UpdateRouterInput answers 202, got {updated}"
    );
    assert!(
        updated["routerInput"]["maintenanceSchedule"].is_null()
            && updated["routerInput"]["maintenanceScheduleType"].is_null(),
        "case \"never started\": an update schedules no window of its own, got {updated}"
    );
    let (_, _, read_back) = get_router_input(&emulator, &arn).await;
    assert!(
        read_back["routerInput"]["maintenanceSchedule"].is_null(),
        "case \"never started\": the reader reports no window either, got {read_back}"
    );
    assert_eq!(
        union_member(
            &read_back["routerInput"]["maintenanceConfiguration"],
            "PreferredDayTime",
            "preferredDayTime",
        )["day"]
            .as_str(),
        Some("FRIDAY"),
        "case \"never started\": the replaced preference IS stored, got {read_back}"
    );
}

/// The same rule on the other side of the router: replacing a started router OUTPUT's
/// maintenance preference re-derives the window it is next maintained in.
#[tokio::test]
async fn a_replaced_preference_reschedules_a_router_outputs_maintenance_window() {
    let emulator = Emulator::new();
    let interface_arn = minted_network_interface(&emulator, "rni-ro-reschedule").await;

    for row in replacement_rows() {
        let ReplacementRow {
            case,
            original,
            replacement,
            expected_day,
            expected_time,
            reschedules,
        } = row;
        let name = case_name("ro-resched", case);
        let body = json!({
            "name": name,
            "configuration": standard(&interface_arn, json!({
                "Rtp": { "destinationAddress": "203.0.113.30", "destinationPort": 5004 },
            })),
            "maximumBitrate": 20_000_000_i64,
            "routingScope": "REGIONAL",
            "tier": "OUTPUT_20",
            "maintenanceConfiguration": original,
        });
        let (status, code, created) = create_router_output(&emulator, body).await;
        assert_eq!(
            (status, code.as_deref()),
            (201, None),
            "case {case:?}: CreateRouterOutput answers 201, got {created}"
        );
        let arn = created["routerOutput"]["arn"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        settle_router_output_in(&emulator, &arn, mc::ROUTER_OUTPUT_STATE_STANDBY);

        let (status, code, started) = emulator
            .send("POST", &format!("/v1/routerOutput/start/{arn}"), None)
            .await;
        assert_eq!(
            (status, code.as_deref()),
            (202, None),
            "case {case:?}: StartRouterOutput answers 202, got {started}"
        );
        let scheduled = started["maintenanceSchedule"].clone();

        let called_at = now_secs();
        let (status, code, updated) = update_router_output(
            &emulator,
            &arn,
            json!({ "maintenanceConfiguration": replacement.clone() }),
        )
        .await;
        assert_eq!(
            (status, code.as_deref()),
            (202, None),
            "case {case:?}: UpdateRouterOutput answers 202, got {updated}"
        );
        let window = updated["routerOutput"]["maintenanceSchedule"].clone();
        assert_next_occurrence(case, &window, called_at, expected_day, expected_time);
        if reschedules {
            assert_ne!(
                window, scheduled,
                "case {case:?}: the window derived from the replaced preference is gone, got \
                 {window}"
            );
        } else {
            assert_eq!(
                window, scheduled,
                "case {case:?}: a preference that did not move leaves the window alone, got \
                 {window}"
            );
        }

        let (status, _, read_back) = get_router_output(&emulator, &arn).await;
        let stored = &read_back["routerOutput"];
        assert_eq!(
            (
                status,
                stored["maintenanceScheduleType"].as_str(),
                &stored["maintenanceSchedule"],
            ),
            (200, Some("WINDOW"), &window),
            "case {case:?}: GetRouterOutput reports the window the update left on the record, \
             got {read_back}"
        );
        assert_reported_preference(case, &stored["maintenanceConfiguration"], &replacement);
    }
}
