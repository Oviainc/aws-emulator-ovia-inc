// SPDX-License-Identifier: CC0-1.0

//! The router input start path, end to end over HTTP:
//! CreateRouterInput -> StartRouterInput -> GetRouterInput.
//!
//! The subject is StartRouterInput: it must move the STORED router input its ARN addresses
//! into the non-terminal `STARTING` state, let the clock settle it into `ACTIVE`, and report
//! the maintenance window the input is next maintained in — a window DERIVED from the
//! input's own maintenance configuration and persisted, never a value fixed in the handler.
//!
//! The cases therefore vary what those two rules generalise over: which of the five kinds of
//! router input configuration the input was created with, which state it was in when the
//! start arrived, and which maintenance preference it carries (every day of the week, and
//! times of day across the whole day, plus the `Default` configuration that states no
//! preference).
//!
//! The harness is the one `flow_lifecycle` owns (one router, one shared engine), so every
//! case here drives the same request path a MediaConnect client reaches: routing, restJson1
//! deserialization, constraint validation, handler, serialization.

use serde_json::{json, Value};

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;

use service_com_amazonaws_mediaconnect_mediaconnect::ops::types::{
    MaintenanceSchedule, WindowMaintenanceSchedule,
};
use service_com_amazonaws_mediaconnect_mediaconnect::state as mc;

use crate::flow_lifecycle::{Emulator, REGION};
use crate::router_input_restart::{
    create_body, minted_configuration_rows, past_transition_span, settle_router_input_in,
    stored_state,
    union, union_member,
};

async fn create_router_input(emulator: &Emulator, body: Value) -> (u16, Option<String>, Value) {
    emulator.send("POST", "/v1/routerInput", Some(body)).await
}

async fn get_router_input(emulator: &Emulator, arn: &str) -> (u16, Option<String>, Value) {
    emulator
        .send("GET", &format!("/v1/routerInput/{arn}"), None)
        .await
}

async fn start_router_input(emulator: &Emulator, arn: &str) -> (u16, Option<String>, Value) {
    emulator
        .send("POST", &format!("/v1/routerInput/start/{arn}"), None)
        .await
}

async fn delete_router_input(emulator: &Emulator, arn: &str) -> (u16, Option<String>, Value) {
    emulator
        .send("DELETE", &format!("/v1/routerInput/{arn}"), None)
        .await
}

/// Seconds in an hour and in a day, the units the assertions below state a time of day and a
/// weekday in.
const SECONDS_PER_HOUR: i64 = 60 * 60;
/// See [`SECONDS_PER_HOUR`].
const SECONDS_PER_DAY: i64 = 24 * SECONDS_PER_HOUR;

/// The epoch second of a modelled `Timestamp` as the response carries it.
///
/// `WindowMaintenanceSchedule`'s three members are `@timestampFormat("date-time")`
/// (`smithy_query(query="shape",shape="WindowMaintenanceSchedule")`), so each is an RFC-3339
/// string; an epoch-second number is read too, because that is what restJson1 carries a
/// timestamp as without that trait — the assertions below are about the INSTANT, not about
/// its spelling.
fn instant_secs(case: &str, value: &Value) -> i64 {
    if let Some(text) = value.as_str() {
        return DateTime::from_str(text, Format::DateTime)
            .unwrap_or_else(|error| {
                panic!("case {case:?}: {text:?} is not an RFC-3339 instant: {error}")
            })
            .secs();
    }
    if let Some(seconds) = value.as_i64() {
        return seconds;
    }
    if let Some(seconds) = value.as_f64() {
        return seconds.trunc() as i64;
    }
    panic!("case {case:?}: expected an instant, got {value}")
}

/// A maintenance schedule as `(Start, End, ScheduledTime)` in epoch seconds.
///
/// `MaintenanceSchedule` is a union whose only member is `Window`
/// (`smithy_query(query="shape",shape="MaintenanceSchedule")`), read under either key the
/// crate's union codec can answer with (see [`union_member`]).
fn window_secs(case: &str, schedule: &Value) -> (i64, i64, i64) {
    let window = union_member(schedule, "Window", "window");
    assert!(
        window.is_object(),
        "case {case:?}: the schedule reports its Window member, got {schedule}"
    );
    (
        instant_secs(case, &window["start"]),
        instant_secs(case, &window["end"]),
        instant_secs(case, &window["scheduledTime"]),
    )
}

/// Assert the window `schedule` reports is a well-formed maintenance window as of `called_at`:
/// it opens in the future, it lasts exactly one [`mc::MAINTENANCE_WINDOW_SPAN`], and the
/// maintenance it schedules falls inside it.
fn assert_window_shape(case: &str, schedule: &Value, called_at: i64) -> (i64, i64, i64) {
    let (start, end, scheduled_time) = window_secs(case, schedule);
    assert!(
        start > called_at,
        "case {case:?}: the window opens after the start was called ({called_at}), got {start}"
    );
    let span = i64::try_from(mc::MAINTENANCE_WINDOW_SPAN.as_secs()).unwrap_or(i64::MAX);
    assert_eq!(
        end - start,
        span,
        "case {case:?}: the window lasts one maintenance span, got start {start} end {end}"
    );
    assert!(
        (start..=end).contains(&scheduled_time),
        "case {case:?}: the scheduled time lies inside the window, got {scheduled_time} for \
         start {start} end {end}"
    );
    (start, end, scheduled_time)
}

/// The instant the test calls an operation at, in epoch seconds.
fn now_secs() -> i64 {
    i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
    )
    .unwrap_or(i64::MAX)
}

/// The position of the day `instant` falls on in a Monday-first week.
///
/// Computed here from the epoch second itself — 1970-01-01 was a Thursday, the fourth day of
/// such a week — so this check does not run through the same function the handler does.
fn day_of_week(instant: i64) -> i64 {
    (instant.div_euclid(SECONDS_PER_DAY) + 3).rem_euclid(7)
}

/// The distance from midnight UTC of `instant`, in seconds.
fn time_of_day(instant: i64) -> i64 {
    instant.rem_euclid(SECONDS_PER_DAY)
}

/// A start moves every kind of router input into `STARTING`, the clock settles it into
/// `ACTIVE`, and both the state and the maintenance window it scheduled are PERSISTED.
///
/// The rows vary the configuration kind (all five union members the model declares) and the
/// state the input was in when the start arrived, which is what the rule generalises over.
#[tokio::test]
async fn a_start_activates_every_kind_of_router_input_and_the_clock_settles_it() {
    let emulator = Emulator::new();

    // The settled states a start is accepted from: an input that finished being created and
    // waits in STANDBY, and one that is not carrying content because something is wrong.
    let state_rows = [mc::ROUTER_INPUT_STATE_STANDBY, mc::ROUTER_INPUT_STATE_ERROR];

    let mut started: Vec<(String, String, Value)> = Vec::new();
    for (kind, member_name, json_name, configuration, expected_input_type) in
        minted_configuration_rows(&emulator, "start-kinds").await
    {
        for started_in in state_rows {
            let case = format!("{kind} from {started_in}");
            let name = format!("input-{kind}-{started_in}").replace('/', "-");
            let (status, code, created) =
                create_router_input(&emulator, create_body(&name, configuration.clone())).await;
            assert_eq!(
                (status, code.as_deref()),
                (201, None),
                "case {case:?}: CreateRouterInput answers 201, got {created}"
            );
            let router_input = &created["routerInput"];
            let arn = router_input["arn"].as_str().unwrap_or_default().to_string();
            assert_eq!(
                router_input["inputType"].as_str(),
                Some(expected_input_type),
                "case {case:?}: InputType follows from the configuration union member, got \
                 {router_input}"
            );
            assert!(
                router_input["maintenanceSchedule"].is_null(),
                "case {case:?}: nothing has scheduled a window for an input that was only \
                 created, got {router_input}"
            );

            settle_router_input_in(&emulator, &arn, started_in);

            let called_at = now_secs();
            let (status, code, accepted) = start_router_input(&emulator, &arn).await;
            assert_eq!(
                (status, code.as_deref()),
                (202, None),
                "case {case:?}: StartRouterInput answers 202, got {accepted}"
            );
            assert_eq!(
                (
                    accepted["arn"].as_str(),
                    accepted["name"].as_str(),
                    accepted["state"].as_str(),
                    accepted["maintenanceScheduleType"].as_str(),
                ),
                (
                    Some(arn.as_str()),
                    Some(name.as_str()),
                    Some(mc::ROUTER_INPUT_STATE_STARTING),
                    Some("WINDOW"),
                ),
                "case {case:?}: the start reports the addressed input in STARTING with a \
                 window schedule, got {accepted}"
            );
            assert_window_shape(&case, &accepted["maintenanceSchedule"], called_at);

            // The state and the window are PERSISTED, not just reported: the reader computes
            // both from the same record.
            let (status, _, read_back) = get_router_input(&emulator, &arn).await;
            let stored = &read_back["routerInput"];
            assert_eq!(
                (
                    status,
                    stored["state"].as_str(),
                    stored["maintenanceScheduleType"].as_str(),
                ),
                (
                    200,
                    Some(mc::ROUTER_INPUT_STATE_STARTING),
                    Some("WINDOW"),
                ),
                "case {case:?}: GetRouterInput reports the start in flight, got {read_back}"
            );
            assert_eq!(
                stored["maintenanceSchedule"], accepted["maintenanceSchedule"],
                "case {case:?}: the reader reports the window the start scheduled, got \
                 {read_back}"
            );
            assert!(
                !union_member(&stored["streamDetails"], member_name, json_name).is_null(),
                "case {case:?}: StreamDetails still reports the {member_name} member, got \
                 {stored}"
            );

            started.push((case, arn, accepted["maintenanceSchedule"].clone()));
        }
    }

    // One wait for every row: each start owes `ACTIVE` at its own instant, and all of them
    // are due once a transition span has passed.
    tokio::time::sleep(past_transition_span()).await;

    for (case, arn, scheduled_window) in started {
        let (status, code, read_back) = get_router_input(&emulator, &arn).await;
        assert_eq!(
            (
                status,
                code.as_deref(),
                read_back["routerInput"]["state"].as_str()
            ),
            (200, None, Some(mc::ROUTER_INPUT_STATE_ACTIVE)),
            "case {case:?}: the clock settles the started input into ACTIVE, got {read_back}"
        );
        // A read does not move the state again, and it does not reschedule the window: the
        // second reader sees exactly what the first one did.
        let (_, _, read_again) = get_router_input(&emulator, &arn).await;
        assert_eq!(
            (
                read_again["routerInput"]["state"].as_str(),
                &read_again["routerInput"]["maintenanceSchedule"],
            ),
            (Some(mc::ROUTER_INPUT_STATE_ACTIVE), &scheduled_window),
            "case {case:?}: reading twice reports one state and one window, got {read_again}"
        );
    }
}

/// One maintenance row: the configuration an input was created with, and what the window the
/// start schedules must be — the day of the week it falls on (`None` for a configuration that
/// states no preferred day) and its time of day in seconds past midnight UTC.
struct MaintenanceRow {
    case: &'static str,
    configuration: Value,
    day_of_week: Option<i64>,
    time_of_day: i64,
}

/// The maintenance preferences a router input can carry.
///
/// The days are the whole `Day` enum the model declares
/// (`smithy_query(query="shape",shape="Day")`: MONDAY | TUESDAY | WEDNESDAY | THURSDAY |
/// FRIDAY | SATURDAY | SUNDAY), paired with times of day that span the clock — midnight, the
/// documented default hour, the middle of the day, a half hour and the last minute of the day
/// — plus the `Default` configuration, which states no preference at all.
fn maintenance_rows() -> Vec<MaintenanceRow> {
    fn preferred(day: &str, time: &str) -> Value {
        union(
            "PreferredDayTime",
            "preferredDayTime",
            json!({ "day": day, "time": time }),
        )
    }

    vec![
        MaintenanceRow {
            case: "monday at midnight",
            configuration: preferred("MONDAY", "00:00"),
            day_of_week: Some(0),
            time_of_day: 0,
        },
        MaintenanceRow {
            case: "tuesday at the default hour",
            configuration: preferred("TUESDAY", "02:00"),
            day_of_week: Some(1),
            time_of_day: 2 * SECONDS_PER_HOUR,
        },
        MaintenanceRow {
            case: "wednesday at midday",
            configuration: preferred("WEDNESDAY", "12:15"),
            day_of_week: Some(2),
            time_of_day: 12 * SECONDS_PER_HOUR + 15 * 60,
        },
        MaintenanceRow {
            case: "thursday in the afternoon",
            configuration: preferred("THURSDAY", "13:00"),
            day_of_week: Some(3),
            time_of_day: 13 * SECONDS_PER_HOUR,
        },
        MaintenanceRow {
            case: "friday half past six",
            configuration: preferred("FRIDAY", "06:30"),
            day_of_week: Some(4),
            time_of_day: 6 * SECONDS_PER_HOUR + 30 * 60,
        },
        MaintenanceRow {
            case: "saturday at the last minute of the day",
            configuration: preferred("SATURDAY", "23:59"),
            day_of_week: Some(5),
            time_of_day: 23 * SECONDS_PER_HOUR + 59 * 60,
        },
        MaintenanceRow {
            case: "sunday at the top of the last hour",
            configuration: preferred("SUNDAY", "23:00"),
            day_of_week: Some(6),
            time_of_day: 23 * SECONDS_PER_HOUR,
        },
        MaintenanceRow {
            case: "no preference at all",
            configuration: union("Default", "default", json!({})),
            day_of_week: None,
            time_of_day: 2 * SECONDS_PER_HOUR,
        },
    ]
}

/// A maintenance preference whose `Time` names no time of day is refused by the create, so
/// no stored input carries a preference no window can be scheduled inside.
///
/// The rows are the ways a string can fail to be the documented 24-hour `HH:MM` time of day:
/// not a time at all, an hour or a minute past its bound, and digits with no separator.
#[tokio::test]
async fn a_preferred_maintenance_time_that_is_not_a_time_of_day_is_refused() {
    let emulator = Emulator::new();
    let (_, _, _, configuration, _) = minted_configuration_rows(&emulator, "bad-time")
        .await
        .into_iter()
        .next()
        .expect("the configuration table has rows");

    for (case, time) in [
        ("not a time", "whenever"),
        ("empty", ""),
        ("hour past the day", "24:00"),
        ("minute past the hour", "02:60"),
        ("no separator", "0200"),
    ] {
        let mut body = create_body(&format!("input-{}", case.replace(' ', "-")), configuration.clone());
        body["maintenanceConfiguration"] = union(
            "PreferredDayTime",
            "preferredDayTime",
            json!({ "day": "TUESDAY", "time": time }),
        );
        let (status, code, refused) = create_router_input(&emulator, body).await;
        assert_eq!(
            (status, code.as_deref()),
            (400, Some("BadRequestException")),
            "case {case:?}: a preferred time of {time:?} is refused as a bad request, got \
             {refused}"
        );
        assert!(
            refused["message"]
                .as_str()
                .unwrap_or_default()
                .contains(time),
            "case {case:?}: the refusal names the time the request carried, got {refused}"
        );
    }

    // The same preference with a readable time is accepted, so the refusals above are about
    // the time and nothing else.
    let mut body = create_body("input-readable-time", configuration);
    body["maintenanceConfiguration"] = union(
        "PreferredDayTime",
        "preferredDayTime",
        json!({ "day": "TUESDAY", "time": "02:00" }),
    );
    let (status, _, created) = create_router_input(&emulator, body).await;
    assert_eq!(
        status, 201,
        "case \"readable time\": the create is accepted, got {created}"
    );
}

/// The window a start schedules is the next occurrence of the input's own maintenance
/// preference, and the reader reports that same window.
#[tokio::test]
async fn the_window_a_start_schedules_is_the_next_occurrence_of_the_inputs_preference() {
    let emulator = Emulator::new();
    let (_, _, _, configuration, _) = minted_configuration_rows(&emulator, "window")
        .await
        .into_iter()
        .next()
        .expect("the configuration table has rows");

    for row in maintenance_rows() {
        let MaintenanceRow {
            case,
            configuration: maintenance_configuration,
            day_of_week: expected_day,
            time_of_day: expected_time,
        } = row;
        let name = format!("input-{}", case.replace(' ', "-"));
        let mut body = create_body(&name, configuration.clone());
        body["maintenanceConfiguration"] = maintenance_configuration;
        let (status, _, created) = create_router_input(&emulator, body).await;
        assert_eq!(
            status, 201,
            "case {case:?}: CreateRouterInput answers 201, got {created}"
        );
        let arn = created["routerInput"]["arn"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        settle_router_input_in(&emulator, &arn, mc::ROUTER_INPUT_STATE_STANDBY);

        let called_at = now_secs();
        let (status, code, accepted) = start_router_input(&emulator, &arn).await;
        assert_eq!(
            (status, code.as_deref()),
            (202, None),
            "case {case:?}: StartRouterInput answers 202, got {accepted}"
        );
        let (start, _, scheduled_time) =
            assert_window_shape(case, &accepted["maintenanceSchedule"], called_at);
        assert_eq!(
            time_of_day(start),
            expected_time,
            "case {case:?}: the window opens at the preferred time of day, got {start}"
        );
        if let Some(expected_day) = expected_day {
            assert_eq!(
                day_of_week(start),
                expected_day,
                "case {case:?}: the window opens on the preferred day, got {start}"
            );
            // The next occurrence of a weekly preference is at most a week away.
            assert!(
                start - called_at <= 7 * SECONDS_PER_DAY,
                "case {case:?}: the window is the NEXT occurrence of the preference, got \
                 {start} for a call at {called_at}"
            );
        } else {
            // A preference that names no day comes round every day.
            assert!(
                start - called_at <= SECONDS_PER_DAY,
                "case {case:?}: the window is the next occurrence of the default hour, got \
                 {start} for a call at {called_at}"
            );
        }
        assert_eq!(
            scheduled_time, start,
            "case {case:?}: the maintenance is scheduled for the instant the window opens, \
             got {scheduled_time}"
        );

        let (status, _, read_back) = get_router_input(&emulator, &arn).await;
        assert_eq!(
            (
                status,
                read_back["routerInput"]["maintenanceScheduleType"].as_str(),
            ),
            (200, Some("WINDOW")),
            "case {case:?}: GetRouterInput reports the schedule type, got {read_back}"
        );
        assert_eq!(
            read_back["routerInput"]["maintenanceSchedule"], accepted["maintenanceSchedule"],
            "case {case:?}: the reader reports the window the start scheduled, got {read_back}"
        );
    }
}

/// The window a start scheduled is re-scheduled once the clock has carried the input past
/// it: a spent window is never reported, and the one that replaces it is the next occurrence
/// of the same preference.
///
/// The stored window is moved into the past through the store, because the windows this
/// service schedules are hours or days out and nothing else moves the clock.
#[tokio::test]
async fn a_maintenance_window_the_clock_has_passed_is_rescheduled() {
    let emulator = Emulator::new();
    let (_, _, _, configuration, _) = minted_configuration_rows(&emulator, "spent-window")
        .await
        .into_iter()
        .next()
        .expect("the configuration table has rows");

    let mut body = create_body("spent-window", configuration);
    body["maintenanceConfiguration"] = union(
        "PreferredDayTime",
        "preferredDayTime",
        json!({ "day": "THURSDAY", "time": "13:00" }),
    );
    let (status, _, created) = create_router_input(&emulator, body).await;
    assert_eq!(status, 201, "the create is accepted, got {created}");
    let arn = created["routerInput"]["arn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    settle_router_input_in(&emulator, &arn, mc::ROUTER_INPUT_STATE_STANDBY);

    let (status, _, accepted) = start_router_input(&emulator, &arn).await;
    assert_eq!(status, 202, "the start is accepted, got {accepted}");

    // A window that closed an hour ago: the maintenance it scheduled is over.
    let closed_at = now_secs() - SECONDS_PER_HOUR;
    let spent = MaintenanceSchedule::Window(WindowMaintenanceSchedule {
        start: aws_smithy_types::DateTime::from_secs(closed_at - SECONDS_PER_HOUR),
        end: aws_smithy_types::DateTime::from_secs(closed_at),
        scheduled_time: aws_smithy_types::DateTime::from_secs(closed_at - SECONDS_PER_HOUR),
    });
    {
        let store = emulator
            .engine
            .state::<mc::ComAmazonawsMediaconnectMediaconnectState>();
        let mut store = store.lock();
        store
            .router_inputs
            .get_mut(&arn)
            .expect("the router input the create minted is in the store")
            .maintenance_schedule = Some(spent);
    }

    let called_at = now_secs();
    let (status, _, read_back) = get_router_input(&emulator, &arn).await;
    assert_eq!(
        status, 200,
        "GetRouterInput answers 200 for the started input, got {read_back}"
    );
    let rescheduled = read_back["routerInput"]["maintenanceSchedule"].clone();
    let (start, _, _) = assert_window_shape("spent window", &rescheduled, called_at);
    assert_eq!(
        day_of_week(start),
        3,
        "case \"spent window\": the next window is on the preferred day, got {rescheduled}"
    );
    assert_eq!(
        time_of_day(start),
        13 * SECONDS_PER_HOUR,
        "case \"spent window\": the next window is at the preferred time, got {rescheduled}"
    );
    assert!(
        start > closed_at,
        "case \"spent window\": the spent window (closed at {closed_at}) was replaced, got \
         {rescheduled}"
    );
    assert_eq!(
        read_back["routerInput"]["maintenanceScheduleType"].as_str(),
        Some("WINDOW"),
        "case \"spent window\": the replacement is a window, got {read_back}"
    );

    // The window that replaced it is in the future, so a second read reports the same one.
    let (_, _, read_again) = get_router_input(&emulator, &arn).await;
    assert_eq!(
        read_again["routerInput"]["maintenanceSchedule"], rescheduled,
        "case \"spent window\": reading twice reports one window, got {read_again}"
    );
}

/// A start is refused with `ConflictException` (409) for as long as a state change is in
/// flight — the create's, or a deletion's — and a repeat of an accepted start answers the
/// same state and the same window rather than starting over.
#[tokio::test]
async fn a_start_is_refused_while_a_state_change_is_in_flight() {
    let emulator = Emulator::new();
    let (_, _, _, configuration, _) = minted_configuration_rows(&emulator, "start-in-flight")
        .await
        .into_iter()
        .next()
        .expect("the configuration table has rows");

    let (status, _, created) =
        create_router_input(&emulator, create_body("in-flight", configuration.clone())).await;
    let arn = created["routerInput"]["arn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert_eq!(
        (status, created["routerInput"]["state"].as_str()),
        (201, Some(mc::ROUTER_INPUT_STATE_CREATING)),
        "a created router input is answered in CREATING, got {created}"
    );

    // The create's transition is still in flight, so the start conflicts with it.
    let (status, code, refused) = start_router_input(&emulator, &arn).await;
    assert_eq!(
        (status, code.as_deref()),
        (409, Some("ConflictException")),
        "case \"start while creating\": expected the modelled ConflictException, got {refused}"
    );
    assert_eq!(
        stored_state(&emulator, &arn),
        mc::ROUTER_INPUT_STATE_CREATING,
        "case \"start while creating\": the refusal persisted no state, got {refused}"
    );
    let (_, _, read_back) = get_router_input(&emulator, &arn).await;
    assert!(
        read_back["routerInput"]["maintenanceSchedule"].is_null(),
        "case \"start while creating\": the refusal scheduled no window, got {read_back}"
    );

    tokio::time::sleep(past_transition_span()).await;
    let (status, _, accepted) = start_router_input(&emulator, &arn).await;
    assert_eq!(
        (status, accepted["state"].as_str()),
        (202, Some(mc::ROUTER_INPUT_STATE_STARTING)),
        "case \"start a settled input\": the start is accepted, got {accepted}"
    );
    let window = accepted["maintenanceSchedule"].clone();

    // A repeat while the start is still in flight answers the same state and the same
    // window: it does not begin the transition again, and it does not reschedule.
    let (status, code, repeated) = start_router_input(&emulator, &arn).await;
    assert_eq!(
        (status, code.as_deref(), repeated["state"].as_str()),
        (202, None, Some(mc::ROUTER_INPUT_STATE_STARTING)),
        "case \"start while starting\": the repeat answers the state the input is in, got \
         {repeated}"
    );
    assert_eq!(
        repeated["maintenanceSchedule"], window,
        "case \"start while starting\": the repeat reports the window already scheduled, got \
         {repeated}"
    );

    tokio::time::sleep(past_transition_span()).await;
    let (status, code, running) = start_router_input(&emulator, &arn).await;
    assert_eq!(
        (status, code.as_deref(), running["state"].as_str()),
        (202, None, Some(mc::ROUTER_INPUT_STATE_ACTIVE)),
        "case \"start a running input\": a running input keeps the state it is in, got \
         {running}"
    );
    assert_eq!(
        running["maintenanceSchedule"], window,
        "case \"start a running input\": the window is the one already scheduled, got {running}"
    );

    // An input on its way out is refused whatever state it is reported in.
    let (status, _, deleted) = delete_router_input(&emulator, &arn).await;
    assert_eq!(
        (status, deleted["state"].as_str()),
        (200, Some(mc::ROUTER_INPUT_STATE_DELETING)),
        "DeleteRouterInput accepts the delete, got {deleted}"
    );
    let (status, code, refused) = start_router_input(&emulator, &arn).await;
    assert_eq!(
        (status, code.as_deref()),
        (409, Some("ConflictException")),
        "case \"start while deleting\": expected the modelled ConflictException, got {refused}"
    );
    assert_eq!(
        stored_state(&emulator, &arn),
        mc::ROUTER_INPUT_STATE_DELETING,
        "case \"start while deleting\": the refusal persisted nothing, got {refused}"
    );
}

/// An ARN that addresses no router input is answered with the modelled `NotFoundException`
/// (404), and one that does not satisfy the modelled `RouterInputArn` pattern is refused at
/// the boundary with `BadRequestException` (400) — never a 501 and never a success.
#[tokio::test]
async fn an_arn_that_addresses_no_router_input_cannot_be_started() {
    let emulator = Emulator::new();

    // Rows differ in WHY the ARN resolves nothing: an id this store never minted, another
    // account's, another Region's, another partition's — each a well-formed `RouterInputArn`.
    let unresolved = [
        (
            "never minted",
            format!("arn:aws:mediaconnect:{REGION}:000000000000:routerInput:00000000beef"),
        ),
        (
            "another account",
            format!("arn:aws:mediaconnect:{REGION}:999999999999:routerInput:0000000000a1"),
        ),
        (
            "another region",
            "arn:aws:mediaconnect:eu-west-1:000000000000:routerInput:0000000000a1".to_string(),
        ),
        (
            "another partition",
            format!("arn:aws-us-gov:mediaconnect:{REGION}:000000000000:routerInput:0000000000a1"),
        ),
    ];
    for (case, arn) in unresolved {
        let (status, code, body) = start_router_input(&emulator, &arn).await;
        assert_eq!(
            (status, code.as_deref()),
            (404, Some("NotFoundException")),
            "case {case:?}: expected the modelled NotFoundException, got {body}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some(mc::not_found_message(&arn).as_str()),
            "case {case:?}: the message names the identifier the request carried, got {body}"
        );
    }

    // Rows that are not `RouterInputArn`s at all: the id is the one part the pattern fixes
    // exactly (twelve lowercase alphanumerics), so these are refused before any handler runs.
    let malformed = [
        (
            "id too short",
            "arn:aws:mediaconnect:us-east-1:000000000000:routerInput:0000000000a",
        ),
        (
            "id too long",
            "arn:aws:mediaconnect:us-east-1:000000000000:routerInput:0000000000a1b",
        ),
        (
            "id not lowercase",
            "arn:aws:mediaconnect:us-east-1:000000000000:routerInput:0000000000A1",
        ),
        ("not an arn", "routerInput:0000000000a1"),
    ];
    for (case, arn) in malformed {
        let (status, code, body) = start_router_input(&emulator, arn).await;
        assert_eq!(
            (status, code.as_deref()),
            (400, Some("BadRequestException")),
            "case {case:?}: an ARN failing the modelled pattern is refused at the boundary, \
             got {body}"
        );
        assert!(
            body["message"]
                .as_str()
                .unwrap_or_default()
                .starts_with(mc::VALIDATION_FAILED_PREFIX),
            "case {case:?}: the refusal carries this service's validation envelope, got {body}"
        );
    }
}
