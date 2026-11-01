// SPDX-License-Identifier: CC0-1.0

//! The two forms a metric alarm's series can be stated in are MUTUALLY EXCLUSIVE.
//!
//! The rule under test: **a `PutMetricAlarm` request that states the series it watches as a
//! `Metrics` query array may carry none of the single-metric identity members — `Namespace`,
//! `MetricName`, `Dimensions`, `Period`, `Unit`, `Statistic`, `ExtendedStatistic` — and a
//! request carrying both forms is refused before anything is written, with the refusal naming
//! the offending member** ("If you use the `Metrics` parameter, you cannot include the
//! `Namespace`, `MetricName`, `Dimensions`, `Period`, `Unit`, `Statistic`, or
//! `ExtendedStatistic` parameters of `PutMetricAlarm` in the same operation. Instead, you
//! retrieve the metrics you are using in your math expression as part of the `Metrics` array."
//! — `smithy_query(query="doc",shape="PutMetricAlarmInput",member="Metrics")`).
//!
//! The rows differ along what the rule generalises over:
//!
//! * WHICH member is carried beside the array — every one of the seven the sentence lists, each
//!   in its own refusal;
//! * HOW MANY of them are carried at once, including the recorded combination;
//! * the SHAPE of the array itself (one retrieving query, a retrieving query with an expression
//!   over it, two sources and an expression over both), because the rule is about the member
//!   being there at all and not about what the array computes;
//! * whether the refused call would have CREATED an alarm or REPLACED one, since a refusal
//!   writes nothing either way.
//!
//! Each form ON ITS OWN is accepted and reads back through `DescribeAlarms`, so what is asserted
//! is the exclusion rather than a blanket refusal of either form.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{
    build_router, call_in_query_mode, described_alarm, error_code, error_message, merged,
    metric_alarm_request, ok, AlarmKind,
};

/// The class the body of this refusal names, reduced to its bare shape name: the
/// request-validation class every cross-member refusal of this service belongs to.
const REFUSAL_CODE: &str = "ValidationException";

/// The header a query-compatible client reads the legacy error code in, and the report it must
/// carry for this class — the code such a client reports the refusal under.
const QUERY_ERROR_HEADER: &str = "x-amzn-query-error";

/// The legacy code and fault side of the request-validation class.
const LEGACY_REFUSAL_REPORT: &str = "ValidationError;Sender";

/// The namespace the rows' metrics live in.
const NAMESPACE: &str = "Custom/Exclusion";

/// The single-metric identity members a request may not carry beside a `Metrics` array, each with
/// a well-formed value, in the order the model's sentence lists them
/// (`smithy_query(query="doc",shape="PutMetricAlarmInput",member="Metrics")`).
fn excluded_members() -> Vec<(&'static str, Value)> {
    vec![
        ("Namespace", json!(NAMESPACE)),
        ("MetricName", json!("Errors")),
        (
            "Dimensions",
            json!([{"Name": "Instance", "Value": "i-0123456789abcdef0"}]),
        ),
        ("Period", json!(60)),
        ("Unit", json!("Count")),
        ("Statistic", json!("Maximum")),
        ("ExtendedStatistic", json!("p90")),
    ]
}

/// How CloudWatch states that `member` may not be set beside a `Metrics` array.
fn exclusion_refusal(member: &str) -> String {
    format!("{member} should not be set if list of Metrics is set.")
}

/// One `MetricStat` query under `id` over the metric `metric`, whose own data is not returned.
fn source_query(id: &str, metric: &str) -> Value {
    json!({
        "Id": id,
        "ReturnData": false,
        "MetricStat": {
            "Metric": {"Namespace": NAMESPACE, "MetricName": metric},
            "Period": 60,
            "Stat": "Maximum",
        },
    })
}

/// The query array shapes the rule is asserted over: the member being present is what the rule
/// speaks about, so the array beside it varies from a single retrieved series to an expression
/// over two of them (the recorded shape).
fn arrays() -> Vec<(&'static str, Value)> {
    vec![
        (
            "one retrieving query",
            json!([{
                "Id": "m1",
                "ReturnData": true,
                "MetricStat": {
                    "Metric": {"Namespace": NAMESPACE, "MetricName": "Errors"},
                    "Period": 60,
                    "Stat": "Maximum",
                },
            }]),
        ),
        (
            "a retrieving query and an expression over it",
            json!([
                source_query("m1", "Errors"),
                {"Id": "doubled", "Expression": "m1 * 2", "ReturnData": true},
            ]),
        ),
        (
            "two sources and an expression over both",
            json!([
                source_query("errors", "Errors"),
                source_query("requests", "Requests"),
                {
                    "Id": "rate",
                    "Expression": "(errors / requests) * 100",
                    "Label": "ErrorRatePercent",
                    "ReturnData": true,
                },
            ]),
        ),
    ]
}

/// A metric-math alarm request for `name` watching `queries`: the members an alarm needs beside
/// the array it watches, and none of the single-metric members.
fn math_alarm_request(name: &str, queries: &Value) -> Value {
    json!({
        "AlarmName": name,
        "Metrics": queries.clone(),
        "EvaluationPeriods": 1,
        "Threshold": 50.0,
        "ComparisonOperator": "GreaterThanThreshold",
    })
}

/// Assert that `request` is refused with HTTP 400 in the request-validation class, carrying one
/// of the sentences in `expected`.
async fn assert_refused(
    router: &MultiServiceRouter,
    case: &str,
    request: &Value,
    expected: &[String],
) {
    let (status, headers, body) = call_in_query_mode(router, "PutMetricAlarm", request).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected the combination to be refused with HTTP 400, got {status} with \
         body {body}"
    );
    let code = error_code(&body);
    assert_eq!(
        code, REFUSAL_CODE,
        "case {case:?}: expected the error code {REFUSAL_CODE}, got {code:?} in {body}"
    );
    let reported = headers
        .get(QUERY_ERROR_HEADER)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    assert_eq!(
        reported, LEGACY_REFUSAL_REPORT,
        "case {case:?}: expected {QUERY_ERROR_HEADER} to read {LEGACY_REFUSAL_REPORT:?}, got \
         {reported:?}"
    );
    let message = error_message(&body);
    assert!(
        expected.contains(&message),
        "case {case:?}: expected one of {expected:?}, got {message:?}"
    );
}

/// The ids the queries of a described alarm's array are defined under, in order.
fn query_ids(alarm: &Value) -> Vec<String> {
    alarm
        .get("Metrics")
        .and_then(Value::as_array)
        .map(|queries| {
            queries
                .iter()
                .map(|query| {
                    query
                        .get("Id")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The same, for the array a request carried.
fn requested_ids(queries: &Value) -> Vec<String> {
    query_ids(&json!({"Metrics": queries.clone()}))
}

#[tokio::test]
async fn a_metrics_array_beside_one_single_metric_member_is_refused_naming_that_member() {
    let router = build_router();
    for (shape, (array_case, queries)) in arrays().into_iter().enumerate() {
        for (member, value) in excluded_members() {
            let case = format!("{array_case} beside {member}");
            let name = format!("exclusive-{shape}-{member}");
            let request = merged(
                math_alarm_request(&name, &queries),
                &json!({member: value.clone()}),
            );
            assert_refused(&router, &case, &request, &[exclusion_refusal(member)]).await;
            let stored = described_alarm(&router, AlarmKind::Metric, &name).await;
            assert_eq!(
                stored, None,
                "case {case:?}: a refused request must create no alarm, got {stored:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_metrics_array_beside_several_single_metric_members_is_refused_naming_one_of_them() {
    // (case, the single-metric members carried beside the array). The refusal names ONE member,
    // and the recorded call — `Namespace`, `MetricName`, `Statistic` and `Period` beside a
    // three-query array — is told about `MetricName`, the member that states the single-metric
    // form at all ("For each PutMetricAlarm operation, you must specify either MetricName, a
    // Metrics array, or an EvaluationCriteria",
    // `smithy_query(query="doc",shape="PutMetricAlarmInput",member="Metrics")`). Rows that carry
    // no `MetricName` are only required to name a member they did carry: which of them is named
    // is not something the rule fixes.
    let rows: Vec<(&str, Vec<&str>)> = vec![
        (
            "the recorded combination",
            vec!["Namespace", "MetricName", "Statistic", "Period"],
        ),
        (
            "every excluded member at once",
            excluded_members()
                .into_iter()
                .map(|(member, _)| member)
                .collect(),
        ),
        (
            "the metric's identity without its name",
            vec!["Namespace", "Dimensions"],
        ),
        ("the aggregation alone", vec!["Statistic", "Period", "Unit"]),
        (
            "a percentile aggregation",
            vec!["ExtendedStatistic", "Period"],
        ),
    ];

    let router = build_router();
    // The recorded array shape is one row of the shapes the rule holds over, so the
    // combinations are asserted on each of them.
    for (shape, (array_case, queries)) in arrays().into_iter().enumerate() {
        for (index, (combination, members)) in rows.iter().enumerate() {
            let case = format!("{array_case} beside {combination}");
            let name = format!("exclusive-combination-{shape}-{index}");
            let mut request = math_alarm_request(&name, &queries);
            for member in members {
                let value = excluded_members()
                    .into_iter()
                    .find(|(name, _)| name == member)
                    .map(|(_, value)| value)
                    .unwrap_or_else(|| panic!("case {case:?}: {member} is an excluded member"));
                request = merged(request, &json!({(*member): value}));
            }
            let expected: Vec<String> = if members.contains(&"MetricName") {
                vec![exclusion_refusal("MetricName")]
            } else {
                members
                    .iter()
                    .map(|member| exclusion_refusal(member))
                    .collect()
            };
            assert_refused(&router, &case, &request, &expected).await;
            let stored = described_alarm(&router, AlarmKind::Metric, &name).await;
            assert_eq!(
                stored, None,
                "case {case:?}: a refused request must create no alarm, got {stored:?}"
            );
        }
    }
}

#[tokio::test]
async fn either_form_on_its_own_is_accepted_and_reads_back() {
    let router = build_router();

    // A request stating its series as a query array, with none of the single-metric members:
    // accepted, and the array is what the alarm reads back with.
    for (shape, (array_case, queries)) in arrays().into_iter().enumerate() {
        let name = format!("array-only-{shape}");
        ok(
            &router,
            "PutMetricAlarm",
            &math_alarm_request(&name, &queries),
        )
        .await;
        let stored = described_alarm(&router, AlarmKind::Metric, &name)
            .await
            .unwrap_or_else(|| panic!("case {array_case:?}: DescribeAlarms reports no {name}"));
        assert_eq!(
            query_ids(&stored),
            requested_ids(&queries),
            "case {array_case:?}: the stored alarm must watch the array it was created with, got \
             {stored}"
        );
        for (member, _) in excluded_members() {
            if member == "Dimensions" {
                // A list member of a request reads back as the list it named, so "no
                // dimensions" is the empty list rather than an absent member.
                let dimensions = stored.get(member).and_then(Value::as_array);
                assert!(
                    dimensions.is_none_or(|entries| entries.is_empty()),
                    "case {array_case:?}: an array alarm names no dimension, got {stored}"
                );
                continue;
            }
            assert_eq!(
                stored.get(member),
                None,
                "case {array_case:?}: an array alarm reports no {member}, got {stored}"
            );
        }
    }

    // And a request stating the same kind of alarm the single-metric way, with no array: also
    // accepted, and it is the single-metric members that read back.
    let name = "scalar-only";
    ok(&router, "PutMetricAlarm", &metric_alarm_request(name)).await;
    let stored = described_alarm(&router, AlarmKind::Metric, name)
        .await
        .unwrap_or_else(|| panic!("DescribeAlarms reports no {name}"));
    assert_eq!(
        stored.get("Metrics"),
        None,
        "case \"the single-metric form\": an alarm created without an array reports none, got \
         {stored}"
    );
    for member in ["Namespace", "MetricName", "Statistic", "Period"] {
        assert!(
            stored.get(member).is_some(),
            "case \"the single-metric form\": the stored alarm must carry {member}, got {stored}"
        );
    }
}

#[tokio::test]
async fn a_refused_write_leaves_the_alarm_it_would_have_replaced_alone() {
    let router = build_router();
    let (_, queries) = arrays()
        .into_iter()
        .next_back()
        .expect("the table names at least one array shape");

    // (case, the alarm the refused write would have replaced, the members the refused write
    // adds to it, the refusal it must answer with)
    let rows: Vec<(&str, Value, Value, String)> = vec![
        (
            "an array alarm updated with a metric name",
            math_alarm_request("standing-array", &queries),
            json!({"MetricName": "Errors"}),
            exclusion_refusal("MetricName"),
        ),
        (
            "an array alarm updated with an aggregation",
            math_alarm_request("standing-array", &queries),
            json!({"Statistic": "Maximum", "Period": 60}),
            exclusion_refusal("Period"),
        ),
    ];

    for (case, created, update, expected) in rows {
        // The alarm the refused update would have replaced.
        ok(&router, "PutMetricAlarm", &created).await;
        let name = created
            .get("AlarmName")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let before = described_alarm(&router, AlarmKind::Metric, &name)
            .await
            .unwrap_or_else(|| panic!("case {case:?}: DescribeAlarms reports no {name}"));

        assert_refused(&router, case, &merged(created.clone(), &update), &[expected]).await;

        let after = described_alarm(&router, AlarmKind::Metric, &name)
            .await
            .unwrap_or_else(|| panic!("case {case:?}: DescribeAlarms reports no {name}"));
        assert_eq!(
            query_ids(&after),
            query_ids(&before),
            "case {case:?}: a refused update must leave the watched array alone, got {after}"
        );
        for member in ["MetricName", "Namespace", "Statistic", "Period", "Unit"] {
            assert_eq!(
                after.get(member),
                None,
                "case {case:?}: a refused update must not give the alarm {member}, got {after}"
            );
        }
    }
}

/// The same alarm, written the single-metric way and then updated into the array form in one
/// call: the update that ADDS an array to an alarm that already carries the single-metric
/// members must carry the array ALONE, because the request is what the rule speaks about and not
/// the alarm it replaces.
#[tokio::test]
async fn an_update_may_move_an_alarm_between_the_two_forms() {
    let router = build_router();
    let (_, queries) = arrays()
        .into_iter()
        .next()
        .expect("the table names at least one array shape");
    let name = "moved-between-forms";

    ok(&router, "PutMetricAlarm", &metric_alarm_request(name)).await;

    // Adding the array while keeping the members the first write carried is the refused
    // combination, whatever the alarm looked like before.
    let both = merged(
        metric_alarm_request(name),
        &json!({"Metrics": queries.clone()}),
    );
    assert_refused(
        &router,
        "the single-metric members kept beside a new array",
        &both,
        &[exclusion_refusal("MetricName")],
    )
    .await;
    let kept = described_alarm(&router, AlarmKind::Metric, name)
        .await
        .unwrap_or_else(|| panic!("DescribeAlarms reports no {name}"));
    assert_eq!(
        kept.get("Metrics"),
        None,
        "case \"a refused move\": the alarm must keep the form it was written in, got {kept}"
    );

    // The array ALONE replaces the configuration completely ("the update completely overwrites
    // the previous configuration of the alarm",
    // `smithy_query(query="doc",shape="PutMetricAlarmInput")`), so the single-metric members the
    // first write stored are gone.
    ok(
        &router,
        "PutMetricAlarm",
        &math_alarm_request(name, &queries),
    )
    .await;
    let moved = described_alarm(&router, AlarmKind::Metric, name)
        .await
        .unwrap_or_else(|| panic!("DescribeAlarms reports no {name}"));
    assert_eq!(
        query_ids(&moved),
        requested_ids(&queries),
        "case \"a move to the array form\": the alarm must watch the array, got {moved}"
    );
    for member in ["MetricName", "Namespace", "Statistic", "Period"] {
        assert_eq!(
            moved.get(member),
            None,
            "case \"a move to the array form\": the alarm must report no {member}, got {moved}"
        );
    }
}
