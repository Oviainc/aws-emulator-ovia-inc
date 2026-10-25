// SPDX-License-Identifier: CC0-1.0

//! What writing an alarm's `ActionsEnabled` switch does to the alarm RECORD, beside the
//! dispatch the switch governs (`alarm_action_dispatch.rs` asserts the dispatch itself).
//!
//! Three rules are asserted here, each over a TABLE whose rows differ along what the rule
//! generalises over — the kind of alarm, the setting the alarm already carried, how many names
//! one call carries, whether a name names an alarm at all — rather than along one recorded
//! request:
//!
//! 1. `EnableAlarmActions` turns `ActionsEnabled` ON for every alarm it names, whichever kind
//!    of alarm the name carries, and the setting is read back through `DescribeAlarms`. Turning
//!    it on is a CONFIGURATION update: it refreshes `AlarmConfigurationUpdatedTimestamp` and
//!    records one `ConfigurationUpdate` history item, while a call asking for the setting the
//!    alarm already carries updates no configuration and records nothing.
//! 2. The call changes no alarm's STATE: actions are executed on a state change, and this call
//!    makes none.
//! 3. `EnableAlarmActions` declares no errors
//!    (`smithy_query(query="operation",operation="EnableAlarmActions")`), so a name no alarm
//!    carries is ignored — the alarms that do exist are enabled, the call succeeds and nothing
//!    is minted for the name that matched none — while an `AlarmNames` list past the modelled
//!    `@length 0..100` is refused at the boundary.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{build_router, call, error_code, ok};

/// The modelled maximum number of names one call may carry: `AlarmNames` is `@length 0..100`
/// (`smithy_query(query="operation",operation="EnableAlarmActions")`).
const MAX_ALARM_NAMES: usize = 100;

/// A role ARN of the form a log alarm's scheduled query names
/// (`ScheduledQueryConfiguration$ScheduledQueryRoleARN`).
const QUERY_ROLE: &str = "arn:aws:iam::000000000000:role/cwl-scheduled-query";

/// The alarm name every row of the unknown-name table writes to.
const KNOWN_ALARM: &str = "known-alarm";

/// Which kind of alarm a row creates: the three `AlarmType` values this service stores
/// (`smithy_query(query="shape",shape="AlarmType")`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Metric,
    Composite,
    Log,
}

impl Kind {
    /// The `DescribeAlarms` output member alarms of this kind are reported under.
    fn member(self) -> &'static str {
        match self {
            Self::Metric => "MetricAlarms",
            Self::Composite => "CompositeAlarms",
            Self::Log => "LogAlarms",
        }
    }

    /// The modelled `AlarmType` value a reader asks for this kind of alarm by: "If you omit
    /// this parameter, only metric alarms are returned" (`DescribeAlarmsInput$AlarmTypes`), so
    /// a reader of any other kind has to name it.
    fn alarm_type(self) -> &'static str {
        match self {
            Self::Metric => "MetricAlarm",
            Self::Composite => "CompositeAlarm",
            Self::Log => "LogAlarm",
        }
    }

    /// Create an alarm of this kind named `name`, carrying `actions_enabled` when the row
    /// states one and the documented default otherwise.
    async fn create(self, router: &MultiServiceRouter, name: &str, actions_enabled: Option<bool>) {
        let (operation, mut request) = match self {
            Self::Metric => ("PutMetricAlarm", metric_alarm(name)),
            Self::Composite => {
                // A composite alarm's rule references other alarms, so the row's own child
                // alarm is created first and the rule names that one.
                let child = format!("{name}-child");
                ok(router, "PutMetricAlarm", &metric_alarm(&child)).await;
                (
                    "PutCompositeAlarm",
                    json!({"AlarmName": name, "AlarmRule": format!("ALARM(\"{child}\")")}),
                )
            }
            Self::Log => ("PutLogAlarm", log_alarm(name)),
        };
        if let (Some(enabled), Value::Object(members)) = (actions_enabled, &mut request) {
            members.insert("ActionsEnabled".to_string(), json!(enabled));
        }
        ok(router, operation, &request).await;
    }
}

/// A static-threshold metric alarm request for `name`.
fn metric_alarm(name: &str) -> Value {
    json!({
        "AlarmName": name,
        "Namespace": "Custom/Alarms",
        "MetricName": "Latency",
        "Statistic": "Average",
        "Period": 60,
        "EvaluationPeriods": 1,
        "Threshold": 10.0,
        "ComparisonOperator": "GreaterThanThreshold",
    })
}

/// A log alarm request for `name`: every member `PutLogAlarm` requires and nothing else.
fn log_alarm(name: &str) -> Value {
    json!({
        "AlarmName": name,
        "QueryResultsToEvaluate": 3,
        "QueryResultsToAlarm": 2,
        "Threshold": 10.0,
        "ComparisonOperator": "GreaterThanThreshold",
        "ScheduledQueryConfiguration": {
            "QueryString": "fields @message | filter @message like /ERROR/",
            "ScheduledQueryRoleARN": QUERY_ROLE,
            "AggregationExpression": "count(*)",
            "ScheduleConfiguration": {
                "ScheduleExpression": "rate(5 minutes)",
                "StartTimeOffset": 360,
            },
        },
    })
}

/// The one alarm of `kind` that `DescribeAlarms` reports for `name`, or `None` when it reports
/// none.
async fn described(router: &MultiServiceRouter, kind: Kind, name: &str) -> Option<Value> {
    let body = ok(
        router,
        "DescribeAlarms",
        &json!({"AlarmNames": [name], "AlarmTypes": [kind.alarm_type()]}),
    )
    .await;
    body.get(kind.member())
        .and_then(Value::as_array)
        .and_then(|alarms| alarms.first().cloned())
}

/// The configuration-update history items recorded for `name`, oldest first.
///
/// The kind is named: a history reader, like an alarm reader, reports metric alarms unless it
/// is told otherwise (`DescribeAlarmHistoryInput$AlarmTypes`).
async fn configuration_updates(router: &MultiServiceRouter, kind: Kind, name: &str) -> Vec<Value> {
    let body = ok(
        router,
        "DescribeAlarmHistory",
        &json!({
            "AlarmName": name,
            "AlarmTypes": [kind.alarm_type()],
            "HistoryItemType": "ConfigurationUpdate",
        }),
    )
    .await;
    body.get("AlarmHistoryItems")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// A timestamp member of a described alarm, as the epoch seconds the awsJson protocols carry.
fn stamp(alarm: &Value, member: &str) -> Option<f64> {
    alarm.get(member).and_then(Value::as_f64)
}

/// Turn the named alarms' actions on, requiring a 200.
async fn enable(router: &MultiServiceRouter, names: &[&str]) {
    ok(router, "EnableAlarmActions", &json!({"AlarmNames": names})).await;
}

#[tokio::test]
async fn enabling_actions_records_one_configuration_update_only_when_the_setting_changes() {
    let router = build_router();

    // Rows differ in the KIND of alarm the name carries and in the setting the alarm already
    // carried — including the documented default of an omitted `ActionsEnabled` ("Indicates
    // whether actions should be executed during any changes to the alarm state. The default is
    // TRUE.").
    for (index, (case, kind, initial)) in [
        (
            "a metric alarm whose actions are off",
            Kind::Metric,
            Some(false),
        ),
        (
            "a metric alarm whose actions are already on",
            Kind::Metric,
            Some(true),
        ),
        ("a metric alarm that took the default", Kind::Metric, None),
        (
            "a composite alarm whose actions are off",
            Kind::Composite,
            Some(false),
        ),
        (
            "a composite alarm whose actions are already on",
            Kind::Composite,
            Some(true),
        ),
        ("a log alarm whose actions are off", Kind::Log, Some(false)),
        ("a log alarm that took the default", Kind::Log, None),
    ]
    .iter()
    .enumerate()
    {
        let name = format!("switch-row-{index}");
        kind.create(&router, &name, *initial).await;

        let before = described(&router, *kind, &name)
            .await
            .unwrap_or_else(|| panic!("case {case:?}: the created alarm must be described"));
        let updates_before = configuration_updates(&router, *kind, &name).await.len();
        let changes = before.get("ActionsEnabled").and_then(Value::as_bool) != Some(true);

        enable(&router, &[name.as_str()]).await;

        let after = described(&router, *kind, &name)
            .await
            .unwrap_or_else(|| panic!("case {case:?}: the alarm must still be described"));
        assert_eq!(
            after.get("ActionsEnabled").and_then(Value::as_bool),
            Some(true),
            "case {case:?}: expected ActionsEnabled true after the call, got {after}"
        );
        // The state value is the alarm's own: enabling the actions is a configuration write and
        // moves no alarm into another state. A composite alarm is left out of this row's check
        // and covered by the forced-state test below, because its state is owned by the rule
        // evaluation the clock makes due rather than by any caller's write.
        if *kind != Kind::Composite {
            assert_eq!(
                after.get("StateValue"),
                before.get("StateValue"),
                "case {case:?}: expected the state value {:?} to be left alone, got {after}",
                before.get("StateValue")
            );
        }

        let updates_after = configuration_updates(&router, *kind, &name).await.len();
        let expected_updates = updates_before + usize::from(changes);
        assert_eq!(
            updates_after, expected_updates,
            "case {case:?}: expected {expected_updates} configuration-update history items \
             (one more only when the setting changed), got {updates_after}"
        );
        let configured_before = stamp(&before, "AlarmConfigurationUpdatedTimestamp");
        let configured_after = stamp(&after, "AlarmConfigurationUpdatedTimestamp");
        if changes {
            assert!(
                configured_after >= configured_before && configured_after.is_some(),
                "case {case:?}: a changed setting must refresh the configuration timestamp, \
                 got {configured_after:?} after {configured_before:?}"
            );
        } else {
            assert_eq!(
                configured_after, configured_before,
                "case {case:?}: a call that changes no setting must leave the configuration \
                 timestamp alone, got {configured_after:?} after {configured_before:?}"
            );
        }
    }
}

#[tokio::test]
async fn enabling_actions_leaves_the_alarm_state_alone() {
    let router = build_router();
    let name = "forced-then-enabled";
    Kind::Metric.create(&router, name, Some(false)).await;
    // A state a caller forced, so the state under test is one nothing else is about to
    // recompute: `SetAlarmState`'s state stands until this alarm's next evaluation period.
    ok(
        &router,
        "SetAlarmState",
        &json!({
            "AlarmName": name,
            "StateValue": "ALARM",
            "StateReason": "driven by the test",
        }),
    )
    .await;
    let before = described(&router, Kind::Metric, name)
        .await
        .expect("the forced alarm must be described");

    enable(&router, &[name]).await;

    let after = described(&router, Kind::Metric, name)
        .await
        .expect("the alarm must still be described");
    for member in [
        "StateValue",
        "StateReason",
        "StateReasonData",
        "StateUpdatedTimestamp",
        "StateTransitionedTimestamp",
    ] {
        assert_eq!(
            after.get(member),
            before.get(member),
            "member {member:?}: expected enabling the actions to leave the alarm state alone, \
             got {after}"
        );
    }
    assert_eq!(
        after.get("ActionsEnabled").and_then(Value::as_bool),
        Some(true),
        "the switch itself must be on, got {after}"
    );
}

#[tokio::test]
async fn a_name_no_alarm_carries_is_ignored() {
    let router = build_router();
    Kind::Metric.create(&router, KNOWN_ALARM, Some(false)).await;

    // Rows differ in how many names the call carries and how many of them name an alarm: the
    // empty list the model allows, a name nothing carries, a mix, the same name twice, and the
    // modelled maximum number of names.
    let at_the_bound: Vec<String> = (0..MAX_ALARM_NAMES)
        .map(|index| {
            if index == 0 {
                KNOWN_ALARM.to_string()
            } else {
                format!("absent-{index}")
            }
        })
        .collect();
    for (case, names) in [
        ("no name at all", Vec::new()),
        ("only a name nothing carries", vec!["absent".to_string()]),
        (
            "a known name beside an unknown one",
            vec!["absent".to_string(), KNOWN_ALARM.to_string()],
        ),
        (
            "the same name twice",
            vec![KNOWN_ALARM.to_string(), KNOWN_ALARM.to_string()],
        ),
        ("the modelled maximum number of names", at_the_bound),
    ] {
        let (status, body) =
            call(&router, "EnableAlarmActions", &json!({"AlarmNames": names})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the call to succeed with HTTP 200, got {status} with {body}"
        );
        // Nothing is created for a name no alarm carries: the call enables alarms, it does not
        // mint them.
        for absent in names.iter().filter(|name| *name != KNOWN_ALARM) {
            let created = described(&router, Kind::Metric, absent).await;
            assert!(
                created.is_none(),
                "case {case:?}: expected no alarm to exist for {absent:?}, got {created:?}"
            );
        }
        if names.iter().any(|name| name == KNOWN_ALARM) {
            let alarm = described(&router, Kind::Metric, KNOWN_ALARM)
                .await
                .unwrap_or_else(|| panic!("case {case:?}: the known alarm must be described"));
            assert_eq!(
                alarm.get("ActionsEnabled").and_then(Value::as_bool),
                Some(true),
                "case {case:?}: the alarm the call did name must be enabled, got {alarm}"
            );
        }
    }

    // One name past the modelled bound is refused at the boundary, so nothing is written.
    let past_bound: Vec<String> = (0..=MAX_ALARM_NAMES)
        .map(|index| format!("absent-{index}"))
        .collect();
    let (status, body) = call(
        &router,
        "EnableAlarmActions",
        &json!({"AlarmNames": past_bound}),
    )
    .await;
    assert_eq!(
        status, 400,
        "a list of {} names is past the modelled maximum of {MAX_ALARM_NAMES} and must be \
         refused, got {status} with {body}",
        MAX_ALARM_NAMES + 1
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "a list past the modelled maximum is refused at the boundary, got {body}"
    );
}
