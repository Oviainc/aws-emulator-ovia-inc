// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for CloudTrail Lake dashboards (`CreateDashboard` /
//! `GetDashboard`).
//!
//! Every case goes over the wire through the shared harness
//! (`tests/all/emulator.rs`): an `awsJson1_1` `POST /` with this service's
//! published `X-Amz-Target` prefix, dispatched through the same
//! `MultiServiceRouter` the production router serves, so what is asserted here is
//! what a client sees — including the HTTP status and the error code.
//!
//! The rules under test, each grounded in this service's Smithy model:
//!
//! * a created dashboard is persisted and the create response and every later read
//!   report the same stored values, addressed by name or by ARN;
//! * the dashboard TYPE follows from the name: the reserved Highlights name names
//!   the dashboard CloudTrail manages (`MANAGED`), every other name a `CUSTOM` one;
//! * the Highlights dashboard needs an ENABLED refresh schedule of 6 HOURS and
//!   carries no caller-supplied widgets;
//! * a refresh schedule's frequency is one the model publishes (1, 6, 12 or 24
//!   HOURS, or 1 DAYS) and its time of day is a UTC `HH:MM` defaulting to `00:00`;
//! * a custom dashboard carries at most ten widgets;
//! * a dashboard name is taken once per Region: a second create is a conflict, never
//!   an overwrite, and a name in another Region is a different dashboard;
//! * a tag set carries no duplicate key and no more than the documented limit;
//! * a new dashboard reads `CREATING` until the clock settles it into `CREATED`.

use serde_json::{json, Value};
use std::thread::sleep;
use std::time::Duration;

use service_com_amazonaws_cloudtrail_cloudtrail_20131101::state;

// The emulator-under-test harness is shared by this crate's suites; see
// `tests/all/emulator.rs`.
use super::emulator::{Emulator, Reply, HOME_REGION};

/// Another Region, for the cases that pin a dashboard to the Region it was made in.
const OTHER_REGION: &str = "us-west-2";

/// A margin on top of [`state::DASHBOARD_SETTLE_AFTER`], so a test that waits for the
/// clock does not race the instant the handler reads.
fn past_settle() -> Duration {
    state::DASHBOARD_SETTLE_AFTER + Duration::from_millis(250)
}

/// One widget, built so the rows below can differ in widget COUNT without repeating
/// the shape. `ViewProperties` is required on `RequestWidget`.
fn widget(index: usize) -> Value {
    json!({
        "QueryStatement": format!("SELECT eventSource, count(*) FROM EDS GROUP BY eventSource -- {index}"),
        "QueryParameters": ["$StartTime$", "$EndTime$"],
        "ViewProperties": {"type": "bar", "colors": format!("{index}")},
    })
}

/// The dashboard calls these cases make, on the shared harness.
impl Emulator {
    /// Create a dashboard called `name` with `extra` merged into the request, and
    /// hand back the reply.
    async fn create(&self, name: &str, extra: Value) -> Reply {
        let mut request = json!({"Name": name});
        if let Some(members) = extra.as_object() {
            for (key, value) in members {
                request[key] = value.clone();
            }
        }
        self.call("CreateDashboard", &request).await
    }

    /// Read a dashboard back by name or ARN.
    async fn get(&self, dashboard_id: &str) -> Reply {
        self.call("GetDashboard", &json!({"DashboardId": dashboard_id}))
            .await
    }
}

/// The ARN a dashboard called `name` in `region` must carry.
fn arn(region: &str, name: &str) -> String {
    format!("arn:aws:cloudtrail:{region}:000000000000:dashboard/{name}")
}

/// The `RefreshSchedule` a request's schedule is STORED as: the request's own, with
/// `TimeOfDay` defaulted to `00:00` (`RefreshSchedule$TimeOfDay`: "default is 00:00").
fn expected_schedule(request: &Value) -> Value {
    match request.get("RefreshSchedule") {
        None => Value::Null,
        Some(schedule) => {
            let mut schedule = schedule.clone();
            if schedule.get("TimeOfDay").is_none() {
                schedule["TimeOfDay"] = json!(state::DEFAULT_REFRESH_TIME_OF_DAY);
            }
            schedule
        }
    }
}

/// RULE: a created dashboard is persisted as the request configured it, and the create
/// response and every later read — by name or by ARN — report those same stored values.
///
/// The rows differ in widget COUNT, in which optional members are present, in the
/// refresh frequency unit and value, in the time of day, in the tag set and in each
/// boolean, including the members whose absence takes a documented default.
#[tokio::test]
async fn a_created_dashboard_reports_the_configuration_it_was_given() {
    let emulator = Emulator::new();
    let rows: Vec<(&str, Value)> = vec![
        ("no optional member at all", json!({"Name": "bare-dashboard"})),
        (
            "one widget",
            json!({"Name": "one-widget", "Widgets": [widget(0)]}),
        ),
        (
            "three widgets and a tag set",
            json!({
                "Name": "three_widgets",
                "Widgets": [widget(0), widget(1), widget(2)],
                "TagsList": [{"Key": "team", "Value": "security"}, {"Key": "cost-centre"}],
            }),
        ),
        (
            "the widget-count maximum",
            json!({
                "Name": "ten-widgets",
                "Widgets": (0..state::CUSTOM_DASHBOARD_WIDGET_LIMIT).map(widget).collect::<Vec<_>>(),
            }),
        ),
        (
            "an hourly schedule with an explicit time of day",
            json!({
                "Name": "hourly-schedule",
                "RefreshSchedule": {
                    "Frequency": {"Unit": "HOURS", "Value": 12},
                    "Status": "ENABLED",
                    "TimeOfDay": "23:59",
                },
                "Widgets": [widget(0)],
            }),
        ),
        (
            "a daily schedule taking the default time of day",
            json!({
                "Name": "daily-schedule",
                "RefreshSchedule": {
                    "Frequency": {"Unit": "DAYS", "Value": 1},
                    "Status": "DISABLED",
                },
            }),
        ),
        (
            "termination protection on",
            json!({"Name": "protected-dashboard", "TerminationProtectionEnabled": true}),
        ),
        (
            "termination protection off, explicitly",
            json!({"Name": "unprotected-dashboard", "TerminationProtectionEnabled": false}),
        ),
    ];

    for (case, request) in rows {
        let name = request["Name"].as_str().expect("the row names a dashboard");
        let created = emulator.call("CreateDashboard", &request).await;
        let created = created.expect_ok(case).clone();

        let expected_widgets = request.get("Widgets").cloned().unwrap_or_else(|| json!([]));
        let expected_tags = request.get("TagsList").cloned().unwrap_or_else(|| json!([]));
        let expected_protection = request
            .get("TerminationProtectionEnabled")
            .and_then(Value::as_bool)
            .unwrap_or(false);

        assert_eq!(
            created["DashboardArn"],
            json!(arn(HOME_REGION, name)),
            "case {case:?}: wrong dashboard ARN, got {created:?}"
        );
        assert_eq!(
            created["Name"],
            json!(name),
            "case {case:?}: the response must name the dashboard created, got {created:?}"
        );
        assert_eq!(
            created["Type"],
            json!(state::DASHBOARD_TYPE_CUSTOM),
            "case {case:?}: a dashboard the caller composed is CUSTOM, got {created:?}"
        );
        assert_eq!(
            created["Widgets"], expected_widgets,
            "case {case:?}: the stored widgets are not the ones requested, got {created:?}"
        );
        assert_eq!(
            created["TagsList"], expected_tags,
            "case {case:?}: the stored tags are not the ones requested, got {created:?}"
        );
        assert_eq!(
            created["RefreshSchedule"],
            expected_schedule(&request),
            "case {case:?}: the stored refresh schedule is wrong, got {created:?}"
        );
        assert_eq!(
            created["TerminationProtectionEnabled"],
            json!(expected_protection),
            "case {case:?}: wrong TerminationProtectionEnabled, got {created:?}"
        );

        // The same dashboard, read back through GetDashboard both ways it can be
        // addressed.
        for (reader, identifier) in [
            ("by name", name.to_string()),
            ("by ARN", arn(HOME_REGION, name)),
        ] {
            let case = format!("{case} read {reader}");
            let fetched = emulator.get(&identifier).await;
            let fetched = fetched.expect_ok(&case).clone();
            assert_eq!(
                (
                    &fetched["DashboardArn"],
                    &fetched["Type"],
                    &fetched["Widgets"],
                    &fetched["RefreshSchedule"],
                    &fetched["TerminationProtectionEnabled"],
                ),
                (
                    &created["DashboardArn"],
                    &created["Type"],
                    &created["Widgets"],
                    &created["RefreshSchedule"],
                    &created["TerminationProtectionEnabled"],
                ),
                "case {case:?}: the dashboard read back is not the one created, got {fetched:?}"
            );
            assert!(
                fetched["CreatedTimestamp"].as_f64().is_some_and(|t| t > 0.0),
                "case {case:?}: expected a CreatedTimestamp, got {fetched:?}"
            );
            assert_eq!(
                fetched["UpdatedTimestamp"], fetched["CreatedTimestamp"],
                "case {case:?}: a dashboard nobody updated was last updated when it was \
                 created, got {fetched:?}"
            );
            assert!(
                fetched.get("LastRefreshId").is_none()
                    && fetched.get("LastRefreshFailureReason").is_none(),
                "case {case:?}: no refresh has run, so no refresh may be reported, got \
                 {fetched:?}"
            );
        }
    }
}

/// RULE: the dashboard type follows from the name — the reserved Highlights name names
/// the dashboard CloudTrail manages, every other name a custom one — and the Highlights
/// dashboard needs an ENABLED refresh schedule of 6 HOURS.
#[tokio::test]
async fn the_dashboard_type_follows_from_the_name() {
    let emulator = Emulator::new();
    // The Highlights schedule the model documents: Status ENABLED, 6 HOURS.
    let highlights_schedule = json!({
        "RefreshSchedule": {
            "Frequency": {
                "Unit": state::REFRESH_FREQUENCY_UNIT_HOURS,
                "Value": state::HIGHLIGHTS_REFRESH_FREQUENCY_VALUE,
            },
            "Status": state::REFRESH_SCHEDULE_ENABLED,
        },
    });

    // (case, name, extra request members, expected Type)
    let accepted: Vec<(&str, &str, Value)> = vec![
        ("a plain name", "ordinary-dashboard", json!({})),
        ("a name of digits", "12345", json!({})),
        (
            "a name that only looks reserved",
            "AWSCloudTrail-Highlights-2",
            json!({}),
        ),
    ];
    for (case, name, extra) in accepted {
        let created = emulator.create(name, extra).await;
        let created = created.expect_ok(case).clone();
        assert_eq!(
            created["Type"],
            json!(state::DASHBOARD_TYPE_CUSTOM),
            "case {case:?}: {name} is not the reserved name, so the dashboard is CUSTOM, \
             got {created:?}"
        );
    }

    let created = emulator
        .create(state::HIGHLIGHTS_DASHBOARD_NAME, highlights_schedule.clone())
        .await;
    let created = created.expect_ok("the Highlights dashboard").clone();
    assert_eq!(
        created["Type"],
        json!(state::DASHBOARD_TYPE_MANAGED),
        "case {:?}: the reserved name names the CloudTrail-managed dashboard, got {created:?}",
        state::HIGHLIGHTS_DASHBOARD_NAME
    );
    let fetched = emulator.get(state::HIGHLIGHTS_DASHBOARD_NAME).await;
    assert_eq!(
        fetched.expect_ok("the Highlights dashboard read back")["Type"],
        json!(state::DASHBOARD_TYPE_MANAGED),
        "case {:?}: the stored dashboard must keep its type, got {fetched:?}",
        state::HIGHLIGHTS_DASHBOARD_NAME
    );

    // The Highlights dashboard refuses every request that is not the documented one.
    // Each row uses a fresh emulator so the refusal is not the name conflict.
    let refused: Vec<(&str, Value)> = vec![
        ("no refresh schedule at all", json!({})),
        (
            "a refresh schedule that is not enabled",
            json!({"RefreshSchedule": {"Frequency": {"Unit": "HOURS", "Value": 6}, "Status": "DISABLED"}}),
        ),
        (
            "an enabled schedule at another frequency",
            json!({"RefreshSchedule": {"Frequency": {"Unit": "HOURS", "Value": 12}, "Status": "ENABLED"}}),
        ),
        (
            "an enabled daily schedule",
            json!({"RefreshSchedule": {"Frequency": {"Unit": "DAYS", "Value": 1}, "Status": "ENABLED"}}),
        ),
        (
            "caller-supplied widgets",
            {
                let mut request = highlights_schedule.clone();
                request["Widgets"] = json!([widget(0)]);
                request
            },
        ),
    ];
    for (case, extra) in refused {
        let fresh = Emulator::new();
        let reply = fresh.create(state::HIGHLIGHTS_DASHBOARD_NAME, extra).await;
        reply.expect_error(case, 400, "ValidationException");
        fresh
            .get(state::HIGHLIGHTS_DASHBOARD_NAME)
            .await
            .expect_error(
                &format!("{case}: nothing may be persisted by a refusal"),
                400,
                "ResourceNotFoundException",
            );
    }
}

/// RULE: a refresh schedule is accepted only at a frequency the model publishes — 1, 6,
/// 12 or 24 HOURS, or 1 DAYS — and only at a real UTC time of day.
///
/// Both published bounds of the hourly set are rows, and so is one step past each end.
#[tokio::test]
async fn a_refresh_schedule_outside_the_published_set_is_refused() {
    let emulator = Emulator::new();

    // Every published frequency, from `RefreshScheduleFrequency$Value`.
    let mut accepted: Vec<(String, Value)> = state::REFRESH_FREQUENCY_HOURS_VALUES
        .iter()
        .map(|value| {
            (
                format!("every {value} {}", state::REFRESH_FREQUENCY_UNIT_HOURS),
                json!({"Frequency": {"Unit": state::REFRESH_FREQUENCY_UNIT_HOURS, "Value": value}}),
            )
        })
        .collect();
    accepted.push((
        format!(
            "every {} {}",
            state::REFRESH_FREQUENCY_DAYS_VALUE,
            state::REFRESH_FREQUENCY_UNIT_DAYS
        ),
        json!({"Frequency": {"Unit": state::REFRESH_FREQUENCY_UNIT_DAYS, "Value": state::REFRESH_FREQUENCY_DAYS_VALUE}}),
    ));
    // The published bounds of the time of day, and the default it takes when absent.
    for time_of_day in ["00:00", "23:59", "12:30"] {
        accepted.push((
            format!("at {time_of_day}"),
            json!({"Frequency": {"Unit": "HOURS", "Value": 1}, "TimeOfDay": time_of_day}),
        ));
    }

    for (index, (case, schedule)) in accepted.iter().enumerate() {
        let name = format!("accepted-{index}");
        let created = emulator
            .create(&name, json!({"RefreshSchedule": schedule}))
            .await;
        let created = created.expect_ok(case).clone();
        let expected = expected_schedule(&json!({"RefreshSchedule": schedule}));
        assert_eq!(
            created["RefreshSchedule"], expected,
            "case {case:?}: the stored schedule must be the one requested, got {created:?}"
        );
    }

    // One step past each end of the hourly set, a value in between it does not
    // publish, a daily value it does not publish, a frequency missing half of
    // itself, and a time of day outside each of its two fields.
    let refused: Vec<(&str, Value)> = vec![
        ("zero hours, one step below the set", json!({"Frequency": {"Unit": "HOURS", "Value": 0}})),
        ("25 hours, one step above the set", json!({"Frequency": {"Unit": "HOURS", "Value": 25}})),
        ("5 hours, inside the range but unpublished", json!({"Frequency": {"Unit": "HOURS", "Value": 5}})),
        ("two days", json!({"Frequency": {"Unit": "DAYS", "Value": 2}})),
        ("a unit with no value", json!({"Frequency": {"Unit": "HOURS"}})),
        ("a value with no unit", json!({"Frequency": {"Value": 6}})),
        ("hour 24", json!({"Frequency": {"Unit": "HOURS", "Value": 6}, "TimeOfDay": "24:00"})),
        ("minute 60", json!({"Frequency": {"Unit": "HOURS", "Value": 6}, "TimeOfDay": "00:60"})),
    ];
    for (index, (case, schedule)) in refused.iter().enumerate() {
        let name = format!("refused-{index}");
        let reply = emulator
            .create(&name, json!({"RefreshSchedule": schedule}))
            .await;
        reply.expect_error(case, 400, "ValidationException");
        emulator.get(&name).await.expect_error(
            &format!("{case}: nothing may be persisted by a refusal"),
            400,
            "ResourceNotFoundException",
        );
    }
}

/// RULE: a custom dashboard carries at most the documented number of widgets; a request
/// beyond it is refused as a quota failure and persists nothing.
#[tokio::test]
async fn a_custom_dashboard_is_bounded_by_the_published_widget_limit() {
    let emulator = Emulator::new();
    let limit = state::CUSTOM_DASHBOARD_WIDGET_LIMIT;
    // Each count, up to and including the limit, is accepted; one past it is not.
    for count in [0, 1, limit - 1, limit] {
        let name = format!("widgets-{count}");
        let widgets: Vec<Value> = (0..count).map(widget).collect();
        let created = emulator
            .create(&name, json!({"Widgets": widgets.clone()}))
            .await;
        let created = created.expect_ok(&format!("{count} widgets")).clone();
        assert_eq!(
            created["Widgets"],
            json!(widgets),
            "case {count} widgets: the stored widgets are not the ones requested, got {created:?}"
        );
    }

    for count in [limit + 1, limit * 2] {
        let name = format!("too-many-{count}");
        let widgets: Vec<Value> = (0..count).map(widget).collect();
        let reply = emulator.create(&name, json!({"Widgets": widgets})).await;
        reply.expect_error(
            &format!("{count} widgets"),
            400,
            "ServiceQuotaExceededException",
        );
        emulator.get(&name).await.expect_error(
            &format!("{count} widgets: nothing may be persisted by a refusal"),
            400,
            "ResourceNotFoundException",
        );
    }
}

/// RULE: a dashboard name is taken once per Region — a second create is a conflict and
/// leaves the stored dashboard alone — and the same name in another Region is a
/// different dashboard.
#[tokio::test]
async fn a_name_already_taken_is_a_conflict_not_an_overwrite() {
    let emulator = Emulator::new();
    let name = "contested-dashboard";
    emulator
        .create(name, json!({"Widgets": [widget(0)]}))
        .await
        .expect_ok("the first create");

    // A second create, asking for a different configuration, must change nothing.
    let again = emulator
        .create(
            name,
            json!({"Widgets": [widget(7), widget(8)], "TerminationProtectionEnabled": true}),
        )
        .await;
    again.expect_error("the second create", 409, "ConflictException");
    let stored = emulator.get(name).await;
    let stored = stored.expect_ok("the dashboard after the conflict").clone();
    assert_eq!(
        stored["Widgets"],
        json!([widget(0)]),
        "the stored dashboard was overwritten by the refused create, got {stored:?}"
    );
    assert_eq!(
        stored["TerminationProtectionEnabled"],
        json!(false),
        "the stored dashboard was overwritten by the refused create, got {stored:?}"
    );

    // The same name in another Region is a different dashboard, reachable only from
    // that Region or by its own ARN.
    let elsewhere = emulator
        .call_in(
            "CreateDashboard",
            OTHER_REGION,
            &json!({"Name": name, "Widgets": [widget(9)]}),
        )
        .await;
    let elsewhere = elsewhere.expect_ok("the same name in another Region").clone();
    assert_eq!(
        elsewhere["DashboardArn"],
        json!(arn(OTHER_REGION, name)),
        "a dashboard carries the Region it was created in, got {elsewhere:?}"
    );
    let by_arn = emulator.get(&arn(OTHER_REGION, name)).await;
    assert_eq!(
        by_arn.expect_ok("the other Region's dashboard by ARN")["Widgets"],
        json!([widget(9)]),
        "the ARN must address the dashboard in the Region it names, got {by_arn:?}"
    );
    let by_name = emulator.get(name).await;
    assert_eq!(
        by_name.expect_ok("the home Region's dashboard by name")["Widgets"],
        json!([widget(0)]),
        "a bare name must address the caller's own Region, got {by_name:?}"
    );
}

/// RULE: a tag set carries each key once and no more tags than the documented limit.
#[tokio::test]
async fn a_tag_set_the_service_cannot_hold_is_refused() {
    let emulator = Emulator::new();
    let limit = state::TAGS_PER_RESOURCE_LIMIT;
    let tags = |count: usize| -> Value {
        (0..count)
            .map(|index| json!({"Key": format!("key-{index}"), "Value": format!("value-{index}")}))
            .collect()
    };

    // The limit itself is accepted; one past it is not.
    let created = emulator
        .create("tagged-at-the-limit", json!({"TagsList": tags(limit)}))
        .await;
    assert_eq!(
        created.expect_ok("the tag limit")["TagsList"],
        tags(limit),
        "case the tag limit: the stored tags are not the ones requested, got {created:?}"
    );

    let refused: Vec<(&str, Value)> = vec![
        ("one tag past the limit", tags(limit + 1)),
        (
            "the same key twice",
            json!([{"Key": "team", "Value": "one"}, {"Key": "team", "Value": "two"}]),
        ),
    ];
    for (index, (case, tags_list)) in refused.iter().enumerate() {
        let name = format!("bad-tags-{index}");
        let reply = emulator.create(&name, json!({"TagsList": tags_list})).await;
        reply.expect_error(case, 400, "InvalidTagParameterException");
        emulator.get(&name).await.expect_error(
            &format!("{case}: nothing may be persisted by a refusal"),
            400,
            "ResourceNotFoundException",
        );
    }
}

/// RULE: a new dashboard reads `CREATING` until the clock settles it into `CREATED`;
/// reading it is not what moves it, and it settles whether or not anybody reads it.
#[tokio::test]
async fn a_new_dashboard_is_creating_until_the_clock_settles_it() {
    let emulator = Emulator::new();
    for (case, name) in [
        ("read before the clock settles it", "watched-dashboard"),
        ("never read before it settles", "unwatched-dashboard"),
    ] {
        emulator.create(name, json!({})).await.expect_ok(case);
        if case.starts_with("read") {
            // Two reads inside the settle span, to pin that a read does not settle
            // the move early.
            for attempt in 0..2 {
                let fresh = emulator.get(name).await;
                assert_eq!(
                    fresh.expect_ok(case)["Status"],
                    json!(state::DASHBOARD_STATUS_CREATING),
                    "case {case:?} (read {attempt}): a new dashboard reads CREATING until \
                     the clock settles it, got {fresh:?}"
                );
            }
        }

        sleep(past_settle());
        let settled = emulator.get(name).await;
        let settled = settled.expect_ok(case).clone();
        assert_eq!(
            settled["Status"],
            json!(state::DASHBOARD_STATUS_CREATED),
            "case {case:?}: the dashboard must settle into CREATED, got {settled:?}"
        );
        // Settling is not an update to the dashboard, and reading twice reports the
        // same resource.
        assert_eq!(
            settled["UpdatedTimestamp"], settled["CreatedTimestamp"],
            "case {case:?}: settling the status is not an update to the dashboard, got \
             {settled:?}"
        );
        let again = emulator.get(name).await;
        assert_eq!(
            again.expect_ok(case),
            &settled,
            "case {case:?}: a second read must report the same dashboard, got {again:?}"
        );
    }
}

/// RULE: an identifier that names no stored dashboard is not found — whichever of the
/// two forms it is written in, and whether it could name one at all — and CloudTrail
/// Lake states that refusal in its own fixed words, without naming the identifier
/// back.
#[tokio::test]
async fn an_identifier_naming_no_dashboard_is_not_found() {
    let emulator = Emulator::new();
    emulator
        .create("present-dashboard", json!({}))
        .await
        .expect_ok("the dashboard that does exist");

    let rows: Vec<(&str, String)> = vec![
        ("an unused name", "absent-dashboard".to_string()),
        ("an unused ARN", arn(HOME_REGION, "absent-dashboard")),
        (
            "an existing name in another Region's ARN",
            arn(OTHER_REGION, "present-dashboard"),
        ),
        ("a name too short to be one", "ab".to_string()),
        (
            "an ARN of another service",
            "arn:aws:s3:us-east-1:000000000000:dashboard/present-dashboard".to_string(),
        ),
        (
            "an ARN of another resource type",
            format!("arn:aws:cloudtrail:{HOME_REGION}:000000000000:trail/present-dashboard"),
        ),
        (
            "an ARN missing its account",
            format!("arn:aws:cloudtrail:{HOME_REGION}::dashboard/present-dashboard"),
        ),
    ];
    for (case, identifier) in rows {
        let reply = emulator.get(&identifier).await;
        reply.expect_error(case, 400, "ResourceNotFoundException");
        // The refusal is CloudTrail's own fixed not-found text, the same for every
        // identifier that names nothing: it reports no part of the request.
        assert_eq!(
            reply.message(),
            state::DASHBOARD_NOT_FOUND_MESSAGE,
            "case {case:?}: expected the not-found message, got {reply:?}"
        );
    }
}
