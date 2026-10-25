// SPDX-License-Identifier: CC0-1.0

//! StartRouterOutput, end to end over HTTP: `POST /v1/routerOutput/start/{Arn}`.
//!
//! The rules the cases below pin, over a table rather than over one request:
//!
//! 1. A start acts on the STORED router output its `@httpLabel Arn` addresses and is
//!    accepted whenever that record is settled (the clock owes it no state change and no
//!    accepted deletion): it answers the record's own ARN and name with the non-terminal
//!    `STARTING` state, which the clock settles into `ACTIVE` one `mc::TRANSITION_SPAN`
//!    later. An output that is already started or already starting keeps the state it is
//!    in; one whose clock still owes a different change is the modelled
//!    `ConflictException` (409); an ARN the store holds no output for is the modelled
//!    `NotFoundException` (404); and an ARN outside the modelled `RouterOutputArn` pattern
//!    never reaches the handler (`BadRequestException`, 400).
//!
//! 2. Starting the output is what SCHEDULES the maintenance window it reports: the window
//!    is the next occurrence, strictly in the future, of the day and time the output's own
//!    maintenance configuration prefers — or of the hour this service documents as its
//!    default (`mc::DEFAULT_MAINTENANCE_TIME_OF_DAY`) for a configuration that states no
//!    preference — it stays open for `mc::MAINTENANCE_WINDOW_SPAN`, its maintenance is
//!    scheduled for the instant it opens, and the type reported beside it is the variant of
//!    the schedule union. The window is then HELD on the record, so GetRouterOutput reports
//!    the window the start answered rather than a fresh one, and a repeated start does not
//!    reschedule it.
//!
//! The rows vary what rule 2 generalises over: every value of the `Day` enum the model
//! declares, checked against the catalog this crate GENERATES from the model
//! (`ops::types::DAY_VALUES`), a different preferred time of day for each (including both
//! ends of the clock and a reading that carries seconds), a configuration that names the
//! DEFAULT maintenance and a request that names no maintenance configuration at all — each
//! crossed with a different `RouterOutputConfiguration` variant, tier, bitrate, routing
//! scope and Region, so no part of the answer can be a value fixed in the handler. A
//! preference the service cannot read is not one a stored output can carry: the operations
//! that take one (the create and the update) refuse it, which is the last case below.
//!
//! The harness is the one `flow_lifecycle` owns (one router, one shared engine), so every
//! case drives the same request path a MediaConnect client reaches: routing, restJson1
//! deserialization, constraint validation, handler, serialization. The fixtures, the create
//! helper and the minted configuration targets (a configuration naming a resource of this
//! service the account does not hold is refused, so each case mints its router network
//! interface and flow first) are the ones `router_outputs` owns, the store-seeding helpers
//! the ones `router_output_stop` owns, and the window assertions the ones
//! `router_input_start` owns, so the router suites cannot drift apart.

use std::time::Duration;

use serde_json::{json, Value};

use service_com_amazonaws_mediaconnect_mediaconnect::ops::types::{
    DAY_VALUES, MAINTENANCE_SCHEDULE_TYPE_VALUES, ROUTER_OUTPUT_STATE_VALUES,
};
use service_com_amazonaws_mediaconnect_mediaconnect::state as mc;

use crate::flow_lifecycle::Emulator;
use crate::router_input_restart::{union, union_member};
use crate::router_input_start::{
    assert_window_shape, day_of_week, now_secs, time_of_day, SECONDS_PER_DAY, SECONDS_PER_HOUR,
};
use crate::router_output_stop::{settle_router_output_in, stored_state};
use crate::router_outputs::{
    create_router_output, minted_configuration_targets, standard_with_protocol,
    ConfigurationTargets, MEDIALIVE_INPUT_ARN, ROLE_ARN, SECRET_ARN,
};

/// The resources of THIS service the configurations below name: a router output's
/// configuration must name a router network interface / flow the account HOLDS, so every
/// case mints them on the emulator it runs on first, through the one helper
/// `router_outputs` owns.
async fn mint_targets(emulator: &Emulator) -> ConfigurationTargets {
    minted_configuration_targets(emulator, "ro-start").await
}

/// Long enough for a transition the clock owes to be due: twice [`mc::TRANSITION_SPAN`], so
/// a settled state is never read early.
fn past_transition_span() -> Duration {
    mc::TRANSITION_SPAN * 2
}

async fn start_router_output(emulator: &Emulator, arn: &str) -> (u16, Option<String>, Value) {
    emulator
        .send("POST", &format!("/v1/routerOutput/start/{arn}"), None)
        .await
}

async fn get_router_output(emulator: &Emulator, arn: &str) -> (u16, Option<String>, Value) {
    emulator
        .send("GET", &format!("/v1/routerOutput/{arn}"), None)
        .await
}

async fn delete_router_output(emulator: &Emulator, arn: &str) -> (u16, Option<String>, Value) {
    emulator
        .send("DELETE", &format!("/v1/routerOutput/{arn}"), None)
        .await
}

/// The `state` member of a response body.
fn state_of(body: &Value) -> &str {
    body.get("state").and_then(Value::as_str).unwrap_or_default()
}

/// The router output GetRouterOutput reports.
fn reported_output(body: &Value) -> &Value {
    body.get("routerOutput").unwrap_or(&Value::Null)
}

/// One case: a maintenance configuration and the window it must resolve to.
struct ScheduleCase {
    case: String,
    /// The `maintenanceConfiguration` the create carries, or `None` for a create that names
    /// none at all.
    maintenance_configuration: Option<Value>,
    /// The position in a Monday-first week of the `Day` the configuration prefers, or `None`
    /// when it prefers no day.
    preferred_day: Option<i64>,
    /// The time of day, in seconds past midnight UTC, the scheduled window must open at.
    expected_time: i64,
    /// Which of [`create_bodies`] the create carries: the configuration variant, tier,
    /// bitrate, routing scope and Region the output is created with.
    body_index: usize,
}

/// A create body per `RouterOutputConfiguration` member the model declares
/// (`smithy_query(query="shape",shape="RouterOutputConfiguration")`: Standard,
/// MediaConnectFlow, MediaLiveInput) — two protocol configurations for the standard kind —
/// differing also in tier, maximum bitrate, routing scope and Region. The rows of
/// [`schedule_cases`] cycle through them, so the schedule rule is asserted over the whole
/// class of router outputs rather than over one configuration.
fn create_bodies(targets: &ConfigurationTargets) -> Vec<Value> {
    let bodies = vec![
        json!({
            "configuration": standard_with_protocol(&targets.interface_arn, json!({
                "Rtp": {
                    "destinationAddress": "203.0.113.20",
                    "destinationPort": 5004,
                    "forwardErrorCorrection": "ENABLED",
                }
            }), "RTP"),
            "maximumBitrate": 20_000_000_i64,
            "routingScope": "REGIONAL",
            "tier": "OUTPUT_20",
        }),
        json!({
            "configuration": standard_with_protocol(&targets.interface_arn, json!({
                "SrtListener": { "port": 3200, "minimumLatencyMilliseconds": 120 }
            }), "SRT_LISTENER"),
            "maximumBitrate": 100_000_000_i64,
            "routingScope": "GLOBAL",
            "tier": "OUTPUT_100",
            "availabilityZone": "us-east-1c",
            "tags": { "team": "broadcast" },
        }),
        json!({
            "configuration": {
                "MediaConnectFlow": {
                    "flowArn": targets.flow_arn,
                    "flowSourceArn": targets.flow_source_arn,
                    "destinationTransitEncryption": {
                        "encryptionKeyType": "SECRETS_MANAGER",
                        "encryptionKeyConfiguration": {
                            "SecretsManager": { "secretArn": SECRET_ARN, "roleArn": ROLE_ARN }
                        },
                    },
                }
            },
            "maximumBitrate": 500_000_000_i64,
            "routingScope": "GLOBAL",
            "tier": "OUTPUT_500",
            "regionName": "eu-west-1",
            "availabilityZone": "eu-west-1b",
        }),
        json!({
            "configuration": {
                "MediaLiveInput": {
                    "mediaLiveInputArn": MEDIALIVE_INPUT_ARN,
                    "mediaLivePipelineId": "PIPELINE_1",
                    "destinationTransitEncryption": {
                        "encryptionKeyType": "AUTOMATIC",
                        "encryptionKeyConfiguration": { "Automatic": {} },
                    },
                }
            },
            "maximumBitrate": 250_i64,
            "routingScope": "REGIONAL",
            "tier": "OUTPUT_250",
        }),
    ];
    assert_eq!(
        bodies.len(),
        BODY_COUNT,
        "BODY_COUNT must be the number of create bodies, got {}",
        bodies.len()
    );
    bodies
}

/// How many create bodies [`create_bodies`] has. [`schedule_cases`] picks one per row before
/// there is an emulator to mint the configuration targets on, so it needs the count first.
const BODY_COUNT: usize = 4;

/// The create body `case` carries, with every reference to a resource of this service
/// pointed at the targets minted on the emulator the case runs on.
fn create_body(targets: &ConfigurationTargets, case: &ScheduleCase) -> Value {
    create_bodies(targets)[case.body_index].clone()
}

/// A preferred time of day as the test states it: the 24-hour `HH:MM` reading, with the
/// seconds past midnight it names stated beside it, so the expectation is not computed by
/// the same code the service reads the preference with.
struct PreferredTime {
    reading: &'static str,
    seconds: i64,
}

/// One preferred time per day of the week, spanning the clock: midnight, both ends of the
/// day, a half-hour and a quarter-hour reading, and a reading that carries seconds.
fn preferred_times() -> [PreferredTime; 7] {
    [
        PreferredTime { reading: "00:00", seconds: 0 },
        PreferredTime { reading: "03:30", seconds: 3 * SECONDS_PER_HOUR + 30 * 60 },
        PreferredTime { reading: "06:15", seconds: 6 * SECONDS_PER_HOUR + 15 * 60 },
        PreferredTime { reading: "12:00", seconds: 12 * SECONDS_PER_HOUR },
        PreferredTime { reading: "17:45", seconds: 17 * SECONDS_PER_HOUR + 45 * 60 },
        PreferredTime { reading: "22:00", seconds: 22 * SECONDS_PER_HOUR },
        PreferredTime { reading: "23:59:30", seconds: 23 * SECONDS_PER_HOUR + 59 * 60 + 30 },
    ]
}

/// The table: every `Day` the model declares, each with a preferred time of its own, then
/// the two ways a configuration can state no preference (the DEFAULT member, and a create
/// that carries no maintenance configuration at all).
fn schedule_cases() -> Vec<ScheduleCase> {
    let times = preferred_times();
    // The `Day` enum's values, in the order a Monday-first week runs: the catalog the crate
    // generates from the model is alphabetical, so the position of each day is stated here
    // and the catalog is what the names are checked against.
    let week = [
        "MONDAY",
        "TUESDAY",
        "WEDNESDAY",
        "THURSDAY",
        "FRIDAY",
        "SATURDAY",
        "SUNDAY",
    ];
    for day in week {
        assert!(
            DAY_VALUES.contains(&day),
            "the table names {day:?}, which must be a Day the model declares, got \
             {DAY_VALUES:?}"
        );
    }
    assert_eq!(
        week.len(),
        DAY_VALUES.len(),
        "the table covers every Day the model declares, got {DAY_VALUES:?}"
    );

    let mut cases: Vec<ScheduleCase> = week
        .iter()
        .enumerate()
        .map(|(index, day)| {
            let time = &times[index % times.len()];
            ScheduleCase {
                case: format!("preferred {day} at {}", time.reading),
                maintenance_configuration: Some(union(
                    "PreferredDayTime",
                    "preferredDayTime",
                    json!({ "day": day, "time": time.reading }),
                )),
                preferred_day: Some(i64::try_from(index).unwrap_or_default()),
                expected_time: time.seconds,
                body_index: index % BODY_COUNT,
            }
        })
        .collect();

    // A configuration that states no preference is scheduled at the hour this service
    // documents as its default, whatever day that falls on.
    let default_time = i64::try_from(mc::DEFAULT_MAINTENANCE_TIME_OF_DAY.as_secs())
        .expect("the default maintenance hour is a time of day");
    cases.push(ScheduleCase {
        case: "the DEFAULT maintenance configuration".to_string(),
        maintenance_configuration: Some(union("Default", "default", json!({}))),
        preferred_day: None,
        expected_time: default_time,
        body_index: 0,
    });
    cases.push(ScheduleCase {
        case: "a create that names no maintenance configuration".to_string(),
        maintenance_configuration: None,
        preferred_day: None,
        expected_time: default_time,
        body_index: 2,
    });
    cases
}

/// Create the router output `case` describes and answer its `(Arn, Name)`.
async fn create(
    emulator: &Emulator,
    targets: &ConfigurationTargets,
    case: &ScheduleCase,
    name: &str,
) -> (String, String) {
    let mut body = create_body(targets, case);
    body["name"] = json!(name);
    if let Some(configuration) = case.maintenance_configuration.clone() {
        body["maintenanceConfiguration"] = configuration;
    }
    let (status, code, created) = create_router_output(emulator, body).await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "case {:?}: CreateRouterOutput answers 201, got {status} {code:?} with {created:?}",
        case.case
    );
    let output = reported_output(&created).clone();
    (
        output
            .get("arn")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        output
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
    )
}

/// Assert the window `schedule` carries is the one `row` asks for, as of `called_at` (epoch
/// seconds).
///
/// The shape of the window — it opens in the future, lasts one
/// [`mc::MAINTENANCE_WINDOW_SPAN`] and contains the maintenance it schedules — is asserted
/// through the ONE reading the router input start suite owns ([`assert_window_shape`]), so
/// both resources' windows are read the same way; what is added here is the preference the
/// window must follow.
fn assert_scheduled_window(case: &str, schedule: &Value, called_at: i64, row: &ScheduleCase) {
    assert!(
        union_member(schedule, "Window", "window").is_object(),
        "case {case:?}: the schedule reports its Window member, got {schedule}"
    );
    let (start, _, scheduled_time) = assert_window_shape(case, schedule, called_at);
    assert_eq!(
        time_of_day(start),
        row.expected_time,
        "case {case:?}: the window opens at the preferred time of day ({} seconds past \
         midnight), got {start}",
        row.expected_time
    );
    match row.preferred_day {
        Some(day) => {
            assert_eq!(
                day_of_week(start),
                day,
                "case {case:?}: the window falls on the preferred day, got {start}"
            );
            // The next occurrence of a weekly preference is at most a week away.
            assert!(
                start - called_at <= 7 * SECONDS_PER_DAY,
                "case {case:?}: the window is the NEXT occurrence of the preference, got \
                 {start} for a call at {called_at}"
            );
        }
        // A preference that names no day comes round every day.
        None => assert!(
            start - called_at <= SECONDS_PER_DAY,
            "case {case:?}: the window is the next occurrence of the default hour, got \
             {start} for a call at {called_at}"
        ),
    }
    assert_eq!(
        scheduled_time, start,
        "case {case:?}: the maintenance is scheduled for the instant the window opens, got \
         {scheduled_time} in a window opening at {start}"
    );
}

/// A `PreferredDayTime` maintenance preference whose `Time` names no time of day is refused
/// by the operations that take one for a router output — the create and the update — so no
/// stored output carries a preference no window can be scheduled inside, the way the sibling
/// CreateRouterInput refuses the same preference.
///
/// The rows are the ways a string can fail to be the documented 24-hour `HH:MM` time of day:
/// not a time at all, empty, an hour or a minute past its bound, and digits with no
/// separator.
#[tokio::test]
async fn a_preferred_maintenance_time_that_is_not_a_time_of_day_is_refused() {
    let emulator = Emulator::new();
    let targets = mint_targets(&emulator).await;
    let row = &schedule_cases()[0];

    let unreadable = [
        ("not a time", "whenever"),
        ("empty", ""),
        ("hour past the day", "24:00"),
        ("minute past the hour", "02:60"),
        ("no separator", "0200"),
    ];

    for (case, time) in unreadable {
        let mut body = create_body(&targets, row);
        body["name"] = json!(format!("ro-time-{}", case.replace(' ', "-")));
        body["maintenanceConfiguration"] = union(
            "PreferredDayTime",
            "preferredDayTime",
            json!({ "day": "TUESDAY", "time": time }),
        );
        let (status, code, refused) = create_router_output(&emulator, body).await;
        assert_eq!(
            (status, code.as_deref()),
            (400, Some("BadRequestException")),
            "case {case:?}: a create carrying a preferred time of {time:?} is refused as a \
             bad request, got {status} {code:?} with {refused:?}"
        );
        assert!(
            refused
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .contains(time),
            "case {case:?}: the refusal names the time the request carried, got {refused:?}"
        );
    }

    // The same preference with a readable time is accepted, so the refusals above are about
    // the time and nothing else — and the output it creates is what the update rows act on.
    let (arn, _) = create(&emulator, &targets, row, "ro-time-readable").await;

    for (case, time) in unreadable {
        let (status, code, refused) = emulator
            .send(
                "PUT",
                &format!("/v1/routerOutput/{arn}"),
                Some(json!({
                    "maintenanceConfiguration": union(
                        "PreferredDayTime",
                        "preferredDayTime",
                        json!({ "day": "TUESDAY", "time": time }),
                    )
                })),
            )
            .await;
        assert_eq!(
            (status, code.as_deref()),
            (400, Some("BadRequestException")),
            "case {case:?}: an update carrying a preferred time of {time:?} is refused as a \
             bad request, got {status} {code:?} with {refused:?}"
        );
    }

    // The refused updates stored nothing: the output still carries the preference its create
    // set, so a start schedules the window that preference asks for.
    tokio::time::sleep(past_transition_span()).await;
    let called_at = now_secs();
    let (status, code, started) = start_router_output(&emulator, &arn).await;
    assert_eq!(
        (status, code.as_deref()),
        (202, None),
        "the start of the output whose preference is readable is accepted, got {status} \
         {code:?} with {started:?}"
    );
    assert_scheduled_window(
        "readable preference after refused updates",
        &started
            .get("maintenanceSchedule")
            .cloned()
            .unwrap_or(Value::Null),
        called_at,
        row,
    );
}

/// Rule 2, and the half of rule 1 about an accepted start: a start moves the stored output
/// through `STARTING` into `ACTIVE` and schedules the maintenance window the output's own
/// configuration resolves to, which every later read then reports.
#[tokio::test]
async fn start_moves_the_output_into_active_and_schedules_its_maintenance_window() {
    // The schedule type the responses report is the model's own single value: the generated
    // catalog is the source of this assertion, so a value the model gains shows up here.
    assert_eq!(
        MAINTENANCE_SCHEDULE_TYPE_VALUES,
        [mc::MAINTENANCE_SCHEDULE_TYPE_WINDOW],
        "the rule names the one MaintenanceScheduleType the model declares, got \
         {MAINTENANCE_SCHEDULE_TYPE_VALUES:?}"
    );
    for named in [
        mc::ROUTER_OUTPUT_STATE_STARTING,
        mc::ROUTER_OUTPUT_STATE_ACTIVE,
        mc::ROUTER_OUTPUT_STATE_STANDBY,
    ] {
        assert!(
            ROUTER_OUTPUT_STATE_VALUES.contains(&named),
            "the rule names {named:?}, which must be a RouterOutputState the model declares, \
             got {ROUTER_OUTPUT_STATE_VALUES:?}"
        );
    }

    for (index, row) in schedule_cases().iter().enumerate() {
        let case = row.case.clone();
        let case = case.as_str();
        let emulator = Emulator::new();
        let targets = mint_targets(&emulator).await;
        let (arn, name) = create(&emulator, &targets, row, &format!("ro-start-{index}")).await;

        // The create does not schedule maintenance: an output that is not carrying content
        // has no window, which is what the optional members report.
        tokio::time::sleep(past_transition_span()).await;
        let (status, code, before) = get_router_output(&emulator, &arn).await;
        let before = reported_output(&before).clone();
        assert_eq!(
            (
                status,
                code.as_deref(),
                before.get("state").and_then(Value::as_str),
                before.get("maintenanceSchedule"),
                before.get("maintenanceScheduleType"),
            ),
            (
                200,
                None,
                Some(mc::ROUTER_OUTPUT_STATE_STANDBY),
                None,
                None
            ),
            "case {case:?}: a created output waits in STANDBY with no window scheduled, got \
             {before:?}"
        );

        let called_at = now_secs();
        let (status, code, started) = start_router_output(&emulator, &arn).await;
        assert_eq!(
            (status, code.as_deref()),
            (202, None),
            "case {case:?}: StartRouterOutput answers 202, got {status} {code:?} with \
             {started:?}"
        );
        assert_eq!(
            (
                started.get("arn").and_then(Value::as_str),
                started.get("name").and_then(Value::as_str),
                state_of(&started),
                started
                    .get("maintenanceScheduleType")
                    .and_then(Value::as_str),
            ),
            (
                Some(arn.as_str()),
                Some(name.as_str()),
                mc::ROUTER_OUTPUT_STATE_STARTING,
                Some(mc::MAINTENANCE_SCHEDULE_TYPE_WINDOW),
            ),
            "case {case:?}: the start answers the stored output's ARN and name with the \
             transient STARTING and the schedule's own type, got {started:?}"
        );
        let schedule = started
            .get("maintenanceSchedule")
            .cloned()
            .unwrap_or(Value::Null);
        assert_scheduled_window(case, &schedule, called_at, row);

        // The window is HELD on the record: the read side reports the same window, and a
        // second read reports it again rather than a window computed afresh.
        let (status, code, read) = get_router_output(&emulator, &arn).await;
        let read = reported_output(&read).clone();
        assert_eq!(
            (
                status,
                code.as_deref(),
                read.get("state").and_then(Value::as_str),
                read.get("maintenanceSchedule"),
                read.get("maintenanceScheduleType").and_then(Value::as_str),
            ),
            (
                200,
                None,
                Some(mc::ROUTER_OUTPUT_STATE_STARTING),
                Some(&schedule),
                Some(mc::MAINTENANCE_SCHEDULE_TYPE_WINDOW),
            ),
            "case {case:?}: the output reads back STARTING with the window the start \
             scheduled, got {read:?}"
        );

        // A repeat while the start is still in flight answers the same thing and does not
        // reschedule the window.
        let (status, code, again) = start_router_output(&emulator, &arn).await;
        assert_eq!(
            (
                status,
                code.as_deref(),
                state_of(&again),
                again.get("maintenanceSchedule"),
            ),
            (
                202,
                None,
                mc::ROUTER_OUTPUT_STATE_STARTING,
                Some(&schedule)
            ),
            "case {case:?}: a start repeated while the output is starting answers STARTING \
             and the window already scheduled, got {status} {code:?} with {again:?}"
        );

        // The clock settles the start: the output carries content, with the same window.
        tokio::time::sleep(past_transition_span()).await;
        let (status, code, settled) = get_router_output(&emulator, &arn).await;
        let settled = reported_output(&settled).clone();
        assert_eq!(
            (
                status,
                code.as_deref(),
                settled.get("state").and_then(Value::as_str),
                settled.get("maintenanceSchedule"),
            ),
            (
                200,
                None,
                Some(mc::ROUTER_OUTPUT_STATE_ACTIVE),
                Some(&schedule)
            ),
            "case {case:?}: the clock settles a started output into ACTIVE, keeping its \
             window, got {settled:?}"
        );

        // And a start of an output that is already carrying content answers that state with
        // the window it already has.
        let (status, code, started_again) = start_router_output(&emulator, &arn).await;
        assert_eq!(
            (
                status,
                code.as_deref(),
                state_of(&started_again),
                started_again.get("maintenanceSchedule"),
            ),
            (
                202,
                None,
                mc::ROUTER_OUTPUT_STATE_ACTIVE,
                Some(&schedule)
            ),
            "case {case:?}: starting an output that is already started answers ACTIVE and \
             does not reschedule its window, got {status} {code:?} with {started_again:?}"
        );

        // Everything else the read reported is the record the store already held: the start
        // moved the state, the window and the timestamp of the last update, nothing else.
        for (member, value) in before.as_object().into_iter().flatten() {
            if ["state", "updatedAt", "maintenanceSchedule", "maintenanceScheduleType"]
                .contains(&member.as_str())
            {
                continue;
            }
            assert_eq!(
                settled[member], *value,
                "case {case:?}: a start leaves {member} as the store held it, got {:?}",
                settled[member]
            );
        }
    }
}

/// Rule 1: a start cannot be accepted while the clock already owes the addressed output a
/// DIFFERENT state change, and an output on its way out is not one a start can act on.
/// The refusal persists nothing, and the same call is accepted once the transition it
/// conflicted with has settled.
#[tokio::test]
async fn a_start_is_refused_while_another_state_change_is_in_flight() {
    for (index, row) in schedule_cases().into_iter().take(4).enumerate() {
        let case = row.case.clone();
        let case = case.as_str();
        let emulator = Emulator::new();
        let targets = mint_targets(&emulator).await;
        let (arn, _) =
            create(&emulator, &targets, &row, &format!("ro-start-conflict-{index}")).await;

        // The create's own transition is still in flight, so the start conflicts with it.
        let (status, code, refused) = start_router_output(&emulator, &arn).await;
        assert_eq!(
            (status, code.as_deref()),
            (409, Some("ConflictException")),
            "case {case:?} (start while creating): expected the modelled ConflictException, \
             got {status} {code:?} with {refused:?}"
        );
        assert!(
            refused
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .contains(mc::ROUTER_OUTPUT_STATE_CREATING),
            "case {case:?}: the refusal names the state the output is in, got {refused:?}"
        );
        assert_eq!(
            stored_state(&emulator, &arn),
            mc::ROUTER_OUTPUT_STATE_CREATING,
            "case {case:?} (start while creating): the refusal persisted nothing"
        );
        assert!(
            reported_output(&get_router_output(&emulator, &arn).await.2)
                .get("maintenanceSchedule")
                .is_none(),
            "case {case:?}: a refused start schedules no maintenance window"
        );

        // Once the create has settled, the SAME call is accepted.
        tokio::time::sleep(past_transition_span()).await;
        let (status, code, started) = start_router_output(&emulator, &arn).await;
        assert_eq!(
            (status, code.as_deref(), state_of(&started)),
            (202, None, mc::ROUTER_OUTPUT_STATE_STARTING),
            "case {case:?}: the start is accepted once the create has settled, got {status} \
             {code:?} with {started:?}"
        );

        // A router output on its way OUT is not one a start can act on, whatever state it
        // is reported in.
        tokio::time::sleep(past_transition_span()).await;
        let (status, _, deleted) = delete_router_output(&emulator, &arn).await;
        assert_eq!(
            (status, state_of(&deleted)),
            (200, mc::ROUTER_OUTPUT_STATE_DELETING),
            "case {case:?}: the delete reports DELETING, got {status} with {deleted:?}"
        );
        let (status, code, refused) = start_router_output(&emulator, &arn).await;
        assert_eq!(
            (status, code.as_deref()),
            (409, Some("ConflictException")),
            "case {case:?} (start while deleting): expected the modelled ConflictException, \
             got {status} {code:?} with {refused:?}"
        );
        assert_eq!(
            stored_state(&emulator, &arn),
            mc::ROUTER_OUTPUT_STATE_DELETING,
            "case {case:?} (start while deleting): the refusal did not cancel the deletion"
        );

        // The clock completes the deletion, after which the ARN addresses nothing.
        tokio::time::sleep(past_transition_span()).await;
        let (status, code, body) = start_router_output(&emulator, &arn).await;
        assert_eq!(
            (status, code.as_deref()),
            (404, Some("NotFoundException")),
            "case {case:?}: a start of a deleted router output is NotFoundException, got \
             {status} {code:?} with {body:?}"
        );
    }
}

/// Rule 1, over the states a start can arrive in: every value of the state enum the model
/// declares for a router output. A settled state a start can move out of is accepted (and
/// the two states that already carry content keep theirs); a state the clock owes a change
/// for, and an output on its way out, are refused as conflicts.
#[tokio::test]
async fn a_start_acts_on_a_settled_output_in_any_state_the_model_declares() {
    let row = &schedule_cases()[0];

    for (index, state) in ROUTER_OUTPUT_STATE_VALUES.iter().enumerate() {
        let case = *state;
        let emulator = Emulator::new();
        let targets = mint_targets(&emulator).await;
        let (arn, _) =
            create(&emulator, &targets, row, &format!("ro-start-state-{index}")).await;
        settle_router_output_in(&emulator, &arn, state);

        let called_at = now_secs();
        let (status, code, body) = start_router_output(&emulator, &arn).await;

        // A SETTLED record is one a start acts on, whatever state it is settled in: the
        // state the start reports is the transient STARTING, except for the two states that
        // already carry content or are already on their way there, which it keeps.
        let already_started = [
            mc::ROUTER_OUTPUT_STATE_ACTIVE,
            mc::ROUTER_OUTPUT_STATE_STARTING,
        ]
        .contains(state);
        let expected = if already_started {
            *state
        } else {
            mc::ROUTER_OUTPUT_STATE_STARTING
        };
        assert_eq!(
            (status, code.as_deref(), state_of(&body)),
            (202, None, expected),
            "case {case:?}: a start of a settled output answers {expected}, got {status} \
             {code:?} with {body:?}"
        );
        let schedule = body
            .get("maintenanceSchedule")
            .cloned()
            .unwrap_or(Value::Null);
        assert_scheduled_window(case, &schedule, called_at, row);
    }
}

/// An ARN of the modelled form that the store holds no router output for is a
/// `NotFoundException` (404) from the start — never a success and never a 501 — with the
/// request's own label in this service's not-found envelope. An ARN outside the modelled
/// `RouterOutputArn` pattern never reaches the handler: the `Arn` member declares that
/// pattern, so the boundary refuses it with `BadRequestException` (400).
#[tokio::test]
async fn a_start_refuses_an_arn_that_addresses_no_router_output() {
    let emulator = Emulator::new();
    let targets = mint_targets(&emulator).await;
    let cases = schedule_cases();

    // One stored, settled router output, so a refusal is about the ARN rather than an empty
    // store — and the positive control at the end of the test.
    let (held, _) = create(&emulator, &targets, &cases[0], "ro-start-present").await;
    tokio::time::sleep(past_transition_span()).await;

    let not_found = [
        (
            "an id this store never minted",
            "arn:aws:mediaconnect:us-east-1:000000000000:routerOutput:ffffffffffff",
        ),
        (
            "an id of digits only",
            "arn:aws:mediaconnect:us-east-1:000000000000:routerOutput:000000000042",
        ),
        (
            "another Region",
            "arn:aws:mediaconnect:eu-west-1:000000000000:routerOutput:000000000001",
        ),
        (
            "another account",
            "arn:aws:mediaconnect:us-east-1:000000000001:routerOutput:000000000001",
        ),
        (
            "another partition",
            "arn:aws-cn:mediaconnect:cn-north-1:000000000000:routerOutput:abcdefabcdef",
        ),
    ];
    for (case, arn) in not_found {
        let (status, code, body) = start_router_output(&emulator, arn).await;
        assert_eq!(
            (status, code.as_deref(), body.get("message").and_then(Value::as_str)),
            (
                404,
                Some("NotFoundException"),
                Some(mc::not_found_message(arn).as_str())
            ),
            "case {case:?}: StartRouterOutput answers NotFoundException naming the \
             identifier the request carried, got {status} {code:?} with {body:?}"
        );
    }

    let outside_the_pattern = [
        ("not an ARN at all", "router-output-1"),
        (
            "a router INPUT ARN",
            "arn:aws:mediaconnect:us-east-1:000000000000:routerInput:000000000001",
        ),
        (
            "an id that is one character too short",
            "arn:aws:mediaconnect:us-east-1:000000000000:routerOutput:00000000001",
        ),
        (
            "an id with an uppercase character",
            "arn:aws:mediaconnect:us-east-1:000000000000:routerOutput:00000000000A",
        ),
    ];
    for (case, arn) in outside_the_pattern {
        let (status, code, body) = start_router_output(&emulator, arn).await;
        assert_eq!(
            (status, code.as_deref()),
            (400, Some("BadRequestException")),
            "case {case:?}: the boundary refuses an ARN outside the modelled pattern, got \
             {status} {code:?} with {body:?}"
        );
    }

    // The positive control: the ARN the store DOES hold is still startable, so none of the
    // refusals above removed or hid it.
    let (status, code, started) = start_router_output(&emulator, &held).await;
    assert_eq!(
        (status, code.as_deref(), state_of(&started)),
        (202, None, mc::ROUTER_OUTPUT_STATE_STARTING),
        "the router output the store holds is started, got {status} {code:?} with {started:?}"
    );
}
