// SPDX-License-Identifier: CC0-1.0

//! What a READ of a metric alarm reports, end to end over the wire.
//!
//! Four rules, each asserted over a TABLE whose rows differ along what the rule generalises
//! over — the form the alarm's configuration identifies its series in, how many dimensions that
//! identity carries, which kind of alarm a relationship read reports, and which class of alarm
//! explains its own state — rather than along one recorded request:
//!
//! * every read of an alarm reports the creation identity of the write that CREATED it:
//!   `<uuid>/<epoch-millis>`, stable across a write that only replaces the configuration, and
//!   different for an alarm that was deleted and written again (recorded behaviour: the member
//!   is not in the on-disk `MetricAlarm`);
//! * the metric identity a read reports is the one the configuration named, in full: every
//!   `Dimensions` list on it is PRESENT, naming no dimension where the configuration named
//!   none — the alarm's own list and the one on each metric of its `Metrics` array alike;
//! * an abbreviated relationship read reports an alarm's state in full wherever it reports
//!   state at all: the value, the instant of the last update AND the instant the value last
//!   changed ("The date and time that the alarm's StateValue most recently changed",
//!   `MetricAlarm$StateTransitionedTimestamp`);
//! * the `StateReason` explains the state the alarm is actually in: an alarm created
//!   `INSUFFICIENT_DATA` has not been checked yet, while an alarm created `OK` because no
//!   contributor of its query is in `ALARM` says so.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::harness::{
    alarm_stamp, build_router, creation_identity_millis, described_alarm, merged,
    metric_alarm_request, now_secs, ok, reported_creation_id, wait_for_evaluation,
    AlarmKind as Kind,
};

/// One alarm configuration the rules below are asserted over.
#[derive(Debug)]
struct Case {
    /// What this row varies, named in every assertion it makes.
    label: &'static str,
    /// The members merged over the static-threshold base request
    /// (`harness::metric_alarm_request`), where a member carried as `null` is REMOVED.
    patch: Value,
}

/// The configuration forms an alarm identifies the series it watches in: one metric and a
/// statistic, a percentile of one metric, a `Metrics` query array, and a query of the alarm's
/// own evaluation criteria — "For each PutMetricAlarm operation, you must specify either
/// MetricName, a Metrics array, or an EvaluationCriteria"
/// (`smithy_query(query="doc",shape="PutMetricAlarmInput",member="Metrics")`).
fn configuration_forms() -> Vec<Case> {
    vec![
        Case {
            label: "one metric and a statistic",
            patch: json!({}),
        },
        Case {
            label: "one metric, dimensioned",
            patch: json!({"Dimensions": [
                {"Name": "Service", "Value": "api"},
                {"Name": "Stage", "Value": "prod"},
            ]}),
        },
        Case {
            label: "a percentile of one metric",
            patch: json!({"Statistic": null, "ExtendedStatistic": "p90"}),
        },
        Case {
            label: "a metric math query array",
            patch: metrics_array_patch(&[]),
        },
        Case {
            label: "a query the alarm evaluates",
            patch: promql_patch("avg_over_time(latency[5m])"),
        },
    ]
}

/// The members that replace the single-metric form with a `Metrics` array retrieving the base
/// request's metric with `dimensions`.
///
/// "If you use the Metrics parameter, you cannot include the Namespace, MetricName, Dimensions,
/// Period, Unit, Statistic, or ExtendedStatistic parameters of PutMetricAlarm in the same
/// operation" (`PutMetricAlarmInput$Metrics`), so every one of them is dropped.
fn metrics_array_patch(dimensions: &[(&str, &str)]) -> Value {
    let mut metric = json!({"Namespace": "Custom/Alarms", "MetricName": "Latency"});
    if !dimensions.is_empty() {
        metric["Dimensions"] = Value::Array(
            dimensions
                .iter()
                .map(|(name, value)| json!({"Name": name, "Value": value}))
                .collect(),
        );
    }
    json!({
        "Namespace": null,
        "MetricName": null,
        "Statistic": null,
        "Period": null,
        "Metrics": [{
            "Id": "watched",
            "MetricStat": {"Metric": metric, "Period": 60, "Stat": "Average"},
            "ReturnData": true,
        }],
    })
}

/// The members that replace the single-metric form with the PromQL criteria carrying `query`:
/// such an alarm names no metric, no threshold and no comparison, because the query itself
/// decides its contributors (`AlarmPromQLCriteria$Query`).
fn promql_patch(query: &str) -> Value {
    json!({
        "Namespace": null,
        "MetricName": null,
        "Statistic": null,
        "Period": null,
        "EvaluationPeriods": null,
        "Threshold": null,
        "ComparisonOperator": null,
        "EvaluationCriteria": {"PromQLCriteria": {"Query": query}},
    })
}

/// The `CreationId` `DescribeAlarms` reports for the metric alarm `name`.
async fn creation_id(
    router: &smithy_server_runtime::MultiServiceRouter,
    name: &str,
) -> Option<String> {
    let alarm = described_alarm(router, Kind::Metric, name).await?;
    reported_creation_id(&alarm).map(str::to_string)
}

#[tokio::test]
async fn every_alarm_read_reports_the_creation_identity_of_the_write_that_created_it() {
    let router = build_router();
    let mut identities: BTreeSet<String> = BTreeSet::new();

    for case in configuration_forms() {
        let name = format!("creation-{}", case.label.replace(' ', "-"));
        let before = now_secs();
        ok(
            &router,
            "PutMetricAlarm",
            &merged(metric_alarm_request(&name), &case.patch),
        )
        .await;
        let after = now_secs();

        let reported = creation_id(&router, &name).await;
        let minted = reported.clone().unwrap_or_default();
        let millis = creation_identity_millis(&minted);
        assert!(
            millis.is_some(),
            "case {:?}: expected a CreationId of the form <uuid>/<epoch-millis>, got {reported:?}",
            case.label
        );
        let millis = millis.unwrap_or_default();
        #[allow(clippy::cast_possible_truncation)]
        let (lower, upper) = (
            (before * 1_000.0) as i64 - 1_000,
            (after * 1_000.0) as i64 + 1_000,
        );
        assert!(
            (lower..=upper).contains(&millis),
            "case {:?}: the creation instant must be the instant of the write ({lower}..={upper} \
             ms), got {millis}",
            case.label
        );
        assert!(
            identities.insert(minted.clone()),
            "case {:?}: every alarm carries its own creation identity, got the repeated \
             {minted:?}",
            case.label
        );

        // A write that only replaces the configuration of an alarm that already exists keeps
        // the identity the alarm was created with — it is the identity of the CREATE.
        ok(
            &router,
            "PutMetricAlarm",
            &merged(
                metric_alarm_request(&name),
                &merged(case.patch.clone(), &json!({"AlarmDescription": "replaced"})),
            ),
        )
        .await;
        let after_update = creation_id(&router, &name).await;
        assert_eq!(
            after_update.as_deref(),
            Some(minted.as_str()),
            "case {:?}: an update keeps the creation identity, got {after_update:?}",
            case.label
        );

        // An alarm that was deleted and written again is a new alarm, so it carries a new one.
        ok(&router, "DeleteAlarms", &json!({"AlarmNames": [&name]})).await;
        ok(
            &router,
            "PutMetricAlarm",
            &merged(metric_alarm_request(&name), &case.patch),
        )
        .await;
        let recreated = creation_id(&router, &name).await;
        assert!(
            recreated
                .as_deref()
                .is_some_and(|recreated| recreated != minted),
            "case {:?}: a re-created alarm carries a new creation identity, got {recreated:?} \
             for the alarm created as {minted:?}",
            case.label
        );
        assert!(
            recreated
                .as_deref()
                .and_then(creation_identity_millis)
                .is_some(),
            "case {:?}: the re-created alarm's identity keeps the form, got {recreated:?}",
            case.label
        );
    }
}

#[tokio::test]
async fn the_per_metric_reader_reports_the_same_creation_identity_as_the_general_reader() {
    let router = build_router();

    // Rows differ in the dimension set that identifies the metric the alarm watches, which is
    // what the per-metric reader selects on.
    for (label, dimensions) in [
        ("no dimension", json!([])),
        (
            "one dimension",
            json!([{"Name": "Service", "Value": "api"}]),
        ),
        (
            "three dimensions",
            json!([
                {"Name": "Service", "Value": "api"},
                {"Name": "Stage", "Value": "prod"},
                {"Name": "Shard", "Value": "7"},
            ]),
        ),
    ] {
        let name = format!("per-metric-{}", label.replace(' ', "-"));
        let request = merged(
            metric_alarm_request(&name),
            &json!({"Dimensions": dimensions}),
        );
        ok(&router, "PutMetricAlarm", &request).await;

        let general = creation_id(&router, &name).await;
        let for_metric = ok(
            &router,
            "DescribeAlarmsForMetric",
            &json!({
                "Namespace": request["Namespace"],
                "MetricName": request["MetricName"],
                "Dimensions": dimensions,
            }),
        )
        .await;
        let reported = reported_creation_id(&for_metric["MetricAlarms"][0]).map(str::to_string);
        assert!(
            general.is_some() && reported == general,
            "case {label:?}: the per-metric reader reports the alarm the general reader does, \
             with the same creation identity ({general:?}), got {reported:?}"
        );
    }
}

/// The `Dimensions` list a read reports for `alarm`, or `None` when the member is absent or is
/// not a list at all.
fn dimension_count(alarm: &Value) -> Option<usize> {
    alarm.get("Dimensions")?.as_array().map(Vec::len)
}

#[tokio::test]
async fn an_alarm_read_reports_every_dimension_list_its_configuration_named() {
    let router = build_router();

    // Rows differ in how many dimensions the metric identity carries and in WHICH identity
    // carries them: the alarm's own, or the one nested in a query of its `Metrics` array.
    let cases = [
        ("no dimension", json!({"Dimensions": null}), Some(0), None),
        (
            "one dimension",
            json!({"Dimensions": [{"Name": "Service", "Value": "api"}]}),
            Some(1),
            None,
        ),
        (
            "three dimensions",
            json!({"Dimensions": [
                {"Name": "Service", "Value": "api"},
                {"Name": "Stage", "Value": "prod"},
                {"Name": "Shard", "Value": "7"},
            ]}),
            Some(3),
            None,
        ),
        (
            "a query array naming no dimension",
            metrics_array_patch(&[]),
            Some(0),
            Some(0),
        ),
        (
            "a query array naming two dimensions",
            metrics_array_patch(&[("Service", "api"), ("Stage", "prod")]),
            Some(0),
            Some(2),
        ),
    ];

    for (label, patch, own, nested) in cases {
        let name = format!("dimensions-{}", label.replace(' ', "-"));
        ok(
            &router,
            "PutMetricAlarm",
            &merged(metric_alarm_request(&name), &patch),
        )
        .await;
        let alarm = described_alarm(&router, Kind::Metric, &name)
            .await
            .unwrap_or(Value::Null);
        assert_eq!(
            dimension_count(&alarm),
            own,
            "case {label:?}: the alarm's own Dimensions must be the list it names, empty where \
             the configuration named none, got {alarm}"
        );
        let reported_nested = alarm["Metrics"][0]["MetricStat"]["Metric"]
            .get("Dimensions")
            .map(|dimensions| dimensions.as_array().map(Vec::len));
        assert_eq!(
            reported_nested,
            nested.map(Some),
            "case {label:?}: the metric nested in each query reports its own Dimensions list \
             the same way, got {alarm}"
        );
    }
}

#[tokio::test]
async fn the_children_of_a_composite_alarm_report_when_their_state_last_changed() {
    let router = build_router();

    // One child of each kind a composite alarm's rule may reference: "the alarms specified in a
    // composite alarm's rule expression can include metric alarms and other composite alarms"
    // (`PutCompositeAlarm`).
    ok(
        &router,
        "PutMetricAlarm",
        &metric_alarm_request("transition-leaf"),
    )
    .await;
    ok(
        &router,
        "PutMetricAlarm",
        &metric_alarm_request("transition-metric-child"),
    )
    .await;
    ok(
        &router,
        "PutCompositeAlarm",
        &json!({
            "AlarmName": "transition-composite-child",
            "AlarmRule": "ALARM(\"transition-leaf\")",
        }),
    )
    .await;
    ok(
        &router,
        "PutCompositeAlarm",
        &json!({
            "AlarmName": "transition-parent",
            "AlarmRule": "ALARM(\"transition-metric-child\") OR \
                          ALARM(\"transition-composite-child\")",
        }),
    )
    .await;
    wait_for_evaluation();

    let children = ok(
        &router,
        "DescribeAlarms",
        &json!({"ChildrenOfAlarmName": "transition-parent"}),
    )
    .await;
    for (label, member, kind, child) in [
        (
            "a metric alarm child",
            "MetricAlarms",
            Kind::Metric,
            "transition-metric-child",
        ),
        (
            "a composite alarm child",
            "CompositeAlarms",
            Kind::Composite,
            "transition-composite-child",
        ),
    ] {
        let reported = children[member]
            .as_array()
            .and_then(|alarms| {
                alarms
                    .iter()
                    .find(|alarm| alarm["AlarmName"].as_str() == Some(child))
            })
            .cloned()
            .unwrap_or(Value::Null);
        for state_member in [
            "StateValue",
            "StateUpdatedTimestamp",
            "StateTransitionedTimestamp",
        ] {
            assert!(
                reported.get(state_member).is_some(),
                "case {label:?}: a child read reports {state_member}, got {reported}"
            );
        }
        // The same instant the FULL read of that alarm reports: one alarm, one state.
        let full = described_alarm(&router, kind, child)
            .await
            .unwrap_or(Value::Null);
        assert_eq!(
            alarm_stamp(&reported, "StateTransitionedTimestamp"),
            alarm_stamp(&full, "StateTransitionedTimestamp"),
            "case {label:?}: the child read reports the instant the full read does, got \
             {reported} against {full}"
        );
    }

    // "Only the Alarm Name and ARN are returned by this operation when you use
    // [ParentsOfAlarmName]" — a read that reports no state reports no instant of one either.
    let parents = ok(
        &router,
        "DescribeAlarms",
        &json!({"ParentsOfAlarmName": "transition-metric-child"}),
    )
    .await;
    let parent = &parents["CompositeAlarms"][0];
    for state_member in [
        "StateValue",
        "StateUpdatedTimestamp",
        "StateTransitionedTimestamp",
    ] {
        assert_eq!(
            parent.get(state_member),
            None,
            "a parent read reports no state at all, so no {state_member} either, got {parents}"
        );
    }
}

#[tokio::test]
async fn an_alarm_reports_the_reason_for_the_state_it_was_created_in() {
    let router = build_router();

    // Rows differ in the class of alarm, which is what decides the state a write creates it in:
    // "When this operation creates an alarm, the alarm state is immediately set to
    // INSUFFICIENT_DATA. For PromQL alarms, the alarm state is instead immediately set to OK."
    // (`PutMetricAlarm`) — and the reason explains the state the alarm is in.
    let cases = [
        (
            "a static-threshold alarm",
            json!({}),
            "INSUFFICIENT_DATA",
            "Unchecked: Initial alarm creation",
        ),
        (
            "a query alarm over a bare selector",
            promql_patch("latency"),
            "OK",
            "No time series evaluated to ALARM",
        ),
        (
            "a query alarm over an aggregation",
            promql_patch("max_over_time(latency[1m]) > 10"),
            "OK",
            "No time series evaluated to ALARM",
        ),
        (
            "a query alarm with pending and recovery periods",
            merged(
                promql_patch("sum by (shard) (errors) > 0"),
                &json!({"EvaluationCriteria": {"PromQLCriteria": {
                    "Query": "sum by (shard) (errors) > 0",
                    "PendingPeriod": 120,
                    "RecoveryPeriod": 60,
                }}}),
            ),
            "OK",
            "No time series evaluated to ALARM",
        ),
    ];

    for (label, patch, state, reason) in cases {
        let name = format!("reason-{}", label.replace(' ', "-"));
        ok(
            &router,
            "PutMetricAlarm",
            &merged(metric_alarm_request(&name), &patch),
        )
        .await;

        // On the very next read, and again once the evaluation the clock owes has been
        // materialised: the explanation is of the state the alarm is in, not of its age.
        for when in ["on the next read", "once the evaluation is due"] {
            let alarm = described_alarm(&router, Kind::Metric, &name)
                .await
                .unwrap_or(Value::Null);
            assert_eq!(
                alarm["StateValue"].as_str(),
                Some(state),
                "case {label:?} {when}: expected StateValue {state}, got {alarm}"
            );
            assert_eq!(
                alarm["StateReason"].as_str(),
                Some(reason),
                "case {label:?} {when}: expected StateReason {reason:?}, got {alarm}"
            );
            wait_for_evaluation();
        }
    }
}
