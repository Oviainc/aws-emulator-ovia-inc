// SPDX-License-Identifier: CC0-1.0

//! The granularity a Contributor Insights report may be asked at, and the aggregate it states
//! for each contributor it carries.
//!
//! Three rules, all of them about what `GetInsightRuleReport` answers:
//!
//! 1. **A report's `Period` is a whole number of minutes within one day** — a multiple of 60
//!    seconds, from 60 to 86400 — and any other value is refused `InvalidParameterValue` (HTTP
//!    400) with the fixed sentence for that condition, BEFORE the rule the request names is
//!    resolved: a value no report could be computed for is a fault in the request itself,
//!    whichever rule it names, so the same value is refused alike for a rule that exists and
//!    for a name nothing was ever created under.
//! 2. **A contributor's `ApproximateAggregateValue` is the aggregate the request's `OrderBy`
//!    statistic computes over that contributor's per-period values** — its total over the
//!    window under `Sum`, and the largest value it reached in any ONE period under `Maximum`
//!    ("Determines what statistic to use to rank the contributors. Valid values are Sum and
//!    Maximum.", `GetInsightRuleReportInput$OrderBy`) — and the contributors are ranked
//!    descending by that same value, so a steady contributor with the larger total ranks below
//!    a spiky one with the larger single-period peak under `Maximum` and above it under `Sum`.
//! 3. **The report's `AggregationStatistic` is the statistic name the rule's aggregation
//!    computes**, not the keyword the rule definition spells that aggregation with: counting
//!    occurrences is reported as the sample count, summing a field as the sum.
//!
//! The rows differ along what each rule generalises over: the period itself (every published
//! bound, one step inside and one step outside each of them, and values no minute divides), the
//! shape of what a contributor contributed over the window (steady, spiky, one period only,
//! negative) and the aggregation the rule is defined with.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_cloudwatch_graniteserviceversion20100801::state as cloudwatch;
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{
    build_router_with_engine, call, call_in_query_mode, error_code, error_message, ok,
};

/// The instant every window below is measured from: a whole DAY past the epoch, so a datapoint's
/// anchored timestamp is predictable for every period a row asks for, up to the widest one.
const ANCHOR_SECS: i64 = 1_767_225_600;

/// The log group the rows ingest into.
const GROUP: &str = "/app/edge-logs";

/// The error class a value this operation cannot serve is refused with (the modelled
/// `InvalidParameterValueException`, HTTP 400).
const INVALID_VALUE: &str = "InvalidParameterValueException";

/// The wire code that class carries for a query-compatible client: its `awsQueryError` code and
/// the fault side, reported in the `x-amzn-query-error` header.
const QUERY_ERROR: &str = "InvalidParameterValue;Sender";

/// The header that report arrives in.
const QUERY_ERROR_HEADER: &str = "x-amzn-query-error";

/// The sentence a period no report can be aggregated at is refused with. Entirely fixed text for
/// the condition: the reason code and the accepted set, and nothing of the submitted value.
const INVALID_PERIOD: &str =
    "INVALID_PERIOD: A Period must be a multiple of 60 seconds, from 60 to 86400.";

/// A rule definition in the Contributor Insights rule syntax: a `JSON` rule over one log group,
/// keyed on one field, with `overrides` merged over it.
fn definition(overrides: Value) -> String {
    let mut rule = json!({
        "Schema": {"Name": "CloudWatchLogRule", "Version": 1},
        "LogGroupNames": [GROUP],
        "LogFormat": "JSON",
        "Contribution": {"Keys": ["$.host"]},
    });
    let members = rule.as_object_mut().expect("the base rule is an object");
    for (member, value) in overrides.as_object().expect("the overrides are an object") {
        members.insert(member.clone(), value.clone());
    }
    serde_json::to_string(&rule).expect("the definition serializes")
}

/// Create the rule `name` with `rule_definition`, enabled.
async fn given_rule(router: &MultiServiceRouter, name: &str, rule_definition: &str) {
    ok(
        router,
        "PutInsightRule",
        &json!({
            "RuleName": name,
            "RuleDefinition": rule_definition,
            "RuleState": "ENABLED",
        }),
    )
    .await;
}

/// Ingest log events into [`GROUP`], through the SAME seam the CloudWatch Logs side of this
/// emulator calls when a `PutLogEvents` batch is stored: each event is `(seconds after
/// [`ANCHOR_SECS`], message)`.
fn ingest(engine: &engine::InMemoryEngine, events: &[(i64, String)]) {
    let stored: Vec<(i64, String)> = events
        .iter()
        .map(|(offset_secs, message)| ((ANCHOR_SECS + offset_secs) * 1_000, message.clone()))
        .collect();
    cloudwatch::insight_rules::record_log_events(engine, GROUP, &stored);
}

/// One log record of the form the rules below read.
fn edge_log(host: &str, bytes: i64) -> String {
    json!({"host": host, "requestBytes": bytes}).to_string()
}

/// A `GetInsightRuleReport` request over `[ANCHOR_SECS, ANCHOR_SECS + window_secs)`.
fn report_request(name: &str, period: i64, window_secs: i64) -> Value {
    json!({
        "RuleName": name,
        "StartTime": ANCHOR_SECS,
        "EndTime": ANCHOR_SECS + window_secs,
        "Period": period,
        "Metrics": ["SampleCount"],
    })
}

/// The `(keys joined, ApproximateAggregateValue)` pairs a report lists, in the order it listed
/// them.
fn reported_contributors(report: &Value) -> Vec<(String, f64)> {
    report
        .get("Contributors")
        .and_then(Value::as_array)
        .map(|contributors| {
            contributors
                .iter()
                .map(|contributor| {
                    let keys = contributor
                        .get("Keys")
                        .and_then(Value::as_array)
                        .map(|keys| {
                            keys.iter()
                                .filter_map(Value::as_str)
                                .collect::<Vec<&str>>()
                                .join(",")
                        })
                        .unwrap_or_default();
                    let value = contributor
                        .get("ApproximateAggregateValue")
                        .and_then(Value::as_f64)
                        .unwrap_or(f64::NAN);
                    (keys, value)
                })
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn a_report_period_must_be_a_whole_number_of_minutes_within_a_day() {
    let (router, _engine) = build_router_with_engine();
    given_rule(&router, "period-rule", &definition(json!({}))).await;

    // (case, the period asked for, whether a report can be aggregated at it). The accepted set
    // is a multiple of 60 seconds from 60 to 86400, so the rows walk each published bound, one
    // step inside and one step outside it, and values no minute divides.
    let cases: Vec<(&str, i64, bool)> = vec![
        ("the narrowest period, one minute", 60, true),
        ("two minutes", 120, true),
        ("five minutes", 300, true),
        ("an hour", 3_600, true),
        ("half a day", 43_200, true),
        ("the widest period, one day", 86_400, true),
        ("a second, which no minute divides", 1, false),
        ("the period of the recorded request", 7, false),
        ("a second short of a minute", 59, false),
        ("a second past a minute", 61, false),
        ("a minute and a half", 90, false),
        ("a minute past the widest period", 86_460, false),
        ("a second past the widest period", 86_401, false),
        ("a multiple of 60 past the widest period", 172_800, false),
    ];

    for (case, period, aggregatable) in cases {
        // A window of one whole period whatever the row's period, so no row is refused for the
        // window instead of for the period.
        let request = report_request("period-rule", period, period.max(60));
        let (status, body) = call(&router, "GetInsightRuleReport", &request).await;
        if aggregatable {
            assert_eq!(
                status, 200,
                "case {case:?}: a period of {period} seconds must be reported, got {status} \
                 {body}"
            );
            continue;
        }
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, INVALID_VALUE),
            "case {case:?}: expected a 400 {INVALID_VALUE} for a period of {period} seconds, got \
             {status} {body}"
        );
        let message = error_message(&body);
        assert_eq!(
            message, INVALID_PERIOD,
            "case {case:?}: expected the fixed refusal {INVALID_PERIOD:?}, got {message:?}"
        );
    }

    // The same refusal as a query-compatible SDK reads it: the class's own query error code.
    let (status, headers, body) = call_in_query_mode(
        &router,
        "GetInsightRuleReport",
        &report_request("period-rule", 7, 600),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"a query-mode client\": expected HTTP 400, got {status} {body}"
    );
    assert_eq!(
        headers
            .get(QUERY_ERROR_HEADER)
            .and_then(|value| value.to_str().ok()),
        Some(QUERY_ERROR),
        "case \"a query-mode client\": expected the {QUERY_ERROR_HEADER} report {QUERY_ERROR:?}, \
         got {headers:?}"
    );
}

/// One row of [`the_period_is_judged_before_the_rule_the_request_names_is_resolved`]: the case,
/// the rule name the request carries, the period it asks for, and the refusal it owes — its
/// status, wire code and message — or `None` where the request is served.
type OrderingRow = (
    &'static str,
    &'static str,
    i64,
    Option<(u16, &'static str, String)>,
);

#[tokio::test]
async fn the_period_is_judged_before_the_rule_the_request_names_is_resolved() {
    let (router, _engine) = build_router_with_engine();
    given_rule(&router, "ordering-rule", &definition(json!({}))).await;

    // Rows differ along whether the named rule exists and whether the period is one a report can
    // be aggregated at: the period decides the answer whenever it is bad, and only a request
    // whose values are all serviceable reaches the rule lookup.
    let cases: Vec<OrderingRow> = vec![
        (
            "a stored rule at a serviceable period",
            "ordering-rule",
            60,
            None,
        ),
        (
            "a stored rule at a period no report can be aggregated at",
            "ordering-rule",
            7,
            Some((400, INVALID_VALUE, INVALID_PERIOD.to_string())),
        ),
        (
            "a name nothing was created under, at a bad period",
            "absent-rule",
            7,
            Some((400, INVALID_VALUE, INVALID_PERIOD.to_string())),
        ),
        (
            "a name nothing was created under, at a sub-minute period",
            "absent-rule",
            1,
            Some((400, INVALID_VALUE, INVALID_PERIOD.to_string())),
        ),
        (
            "a name nothing was created under, at a serviceable period",
            "absent-rule",
            60,
            Some((
                400,
                INVALID_VALUE,
                "RULE_NAME: absent-rule".to_string(),
            )),
        ),
    ];

    for (case, rule_name, period, expected) in cases {
        let (status, body) = call(
            &router,
            "GetInsightRuleReport",
            &report_request(rule_name, period, 600),
        )
        .await;
        match expected {
            None => assert_eq!(
                status, 200,
                "case {case:?}: expected the report to be served, got {status} {body}"
            ),
            Some((expected_status, expected_code, expected_message)) => {
                assert_eq!(
                    (status, error_code(&body).as_str()),
                    (expected_status, expected_code),
                    "case {case:?}: expected a {expected_status} {expected_code}, got {status} \
                     {body}"
                );
                let message = error_message(&body);
                assert_eq!(
                    message, expected_message,
                    "case {case:?}: expected the message {expected_message:?}, got {message:?}"
                );
            }
        }
    }
}

/// One contributor of [`the_reported_aggregate_is_the_statistic_the_request_ranks_by`]: the host
/// it is keyed by, and what it contributed in each period of the window, by the period's index.
struct Contributing {
    /// The value its `Contribution.Keys` extract.
    host: &'static str,
    /// `(period index, the values it contributed in that period)`.
    periods: Vec<(i64, Vec<i64>)>,
}

impl Contributing {
    /// Its total over the whole window — the aggregate an `OrderBy=Sum` report states for it.
    fn total(&self) -> f64 {
        let total: i64 = self
            .periods
            .iter()
            .flat_map(|(_, values)| values.iter().copied())
            .sum();
        #[allow(clippy::cast_precision_loss)]
        let total = total as f64;
        total
    }

    /// The largest total it reached in any ONE period — the aggregate an `OrderBy=Maximum`
    /// report states for it.
    fn largest_period(&self) -> f64 {
        let largest: i64 = self
            .periods
            .iter()
            .map(|(_, values)| values.iter().sum::<i64>())
            .max()
            .unwrap_or_default();
        #[allow(clippy::cast_precision_loss)]
        let largest = largest as f64;
        largest
    }
}

#[tokio::test]
async fn the_reported_aggregate_is_the_statistic_the_request_ranks_by() {
    // The rows differ along the SHAPE of what each contributor contributed over the window:
    // steady across every period, spiky in one, present in one period only, and negative — so a
    // row's Sum order and Maximum order disagree in both directions.
    const PERIOD_SECS: i64 = 60;
    const PERIODS: i64 = 4;
    let contributing: Vec<Contributing> = vec![
        Contributing {
            host: "steady",
            periods: (0..PERIODS).map(|index| (index, vec![20, 20, 20])).collect(),
        },
        Contributing {
            host: "spiky",
            periods: vec![(1, vec![50, 50, 50])],
        },
        Contributing {
            host: "once",
            periods: vec![(3, vec![45])],
        },
        Contributing {
            host: "rising",
            periods: vec![(0, vec![10]), (1, vec![25]), (2, vec![70])],
        },
        Contributing {
            host: "refunding",
            periods: vec![(0, vec![-30]), (2, vec![-10])],
        },
    ];

    let (router, engine) = build_router_with_engine();
    given_rule(
        &router,
        "aggregate-rule",
        &definition(json!({
            "Contribution": {"Keys": ["$.host"], "ValueOf": "$.requestBytes"},
            "AggregateOn": "Sum",
        })),
    )
    .await;
    let mut events: Vec<(i64, String)> = Vec::new();
    for contributor in &contributing {
        for (index, values) in &contributor.periods {
            for value in values {
                events.push((index * PERIOD_SECS + 1, edge_log(contributor.host, *value)));
            }
        }
    }
    ingest(&engine, &events);

    // (case, the OrderBy the request names, the aggregate each contributor owes under it)
    let orderings: Vec<(&str, Option<&str>, BTreeMap<String, f64>)> = vec![
        (
            "omitted, which aggregates and ranks by Sum",
            None,
            contributing
                .iter()
                .map(|contributor| (contributor.host.to_string(), contributor.total()))
                .collect(),
        ),
        (
            "Sum",
            Some("Sum"),
            contributing
                .iter()
                .map(|contributor| (contributor.host.to_string(), contributor.total()))
                .collect(),
        ),
        (
            "Maximum",
            Some("Maximum"),
            contributing
                .iter()
                .map(|contributor| {
                    (contributor.host.to_string(), contributor.largest_period())
                })
                .collect(),
        ),
    ];

    for (case, order_by, expected_aggregates) in orderings {
        let mut request = report_request("aggregate-rule", PERIOD_SECS, PERIODS * PERIOD_SECS);
        if let Some(order_by) = order_by {
            request["OrderBy"] = json!(order_by);
        }
        let report = ok(&router, "GetInsightRuleReport", &request).await;
        let reported = reported_contributors(&report);

        // Every contributor carries the aggregate the ranking statistic comes to for it.
        let got: BTreeMap<String, f64> = reported.iter().cloned().collect();
        assert_eq!(
            got, expected_aggregates,
            "case \"OrderBy {case}\": expected the aggregates {expected_aggregates:?}, got \
             {got:?} in {report}"
        );

        // And the report lists them descending by that same aggregate, ties broken by keys.
        let mut owed: Vec<(String, f64)> = expected_aggregates.into_iter().collect();
        owed.sort_by(|left, right| {
            right.1.total_cmp(&left.1).then_with(|| left.0.cmp(&right.0))
        });
        assert_eq!(
            reported, owed,
            "case \"OrderBy {case}\": expected the contributors ranked {owed:?}, got {reported:?} \
             in {report}"
        );

        // Whichever statistic ranks them, the report still totals every contributor:
        // "The sum of the values from all individual contributors that match the rule."
        // (`GetInsightRuleReportOutput$AggregateValue`)
        let window_total: f64 = contributing
            .iter()
            .map(Contributing::total)
            .sum();
        assert_eq!(
            report.get("AggregateValue").and_then(Value::as_f64),
            Some(window_total),
            "case \"OrderBy {case}\": expected the window total {window_total}, got {report}"
        );
    }

    // The two orderings really do disagree: the largest total and the largest single period
    // belong to different contributors, which is what the Maximum aggregate exists to surface.
    let top = |order_by: &str| -> String {
        contributing
            .iter()
            .max_by(|left, right| {
                let (left_value, right_value) = if order_by == "Maximum" {
                    (left.largest_period(), right.largest_period())
                } else {
                    (left.total(), right.total())
                };
                left_value.total_cmp(&right_value)
            })
            .map(|contributor| contributor.host.to_string())
            .unwrap_or_default()
    };
    assert_ne!(
        top("Sum"),
        top("Maximum"),
        "case \"the two rankings\": the table must hold a row where Sum and Maximum disagree"
    );
}

#[tokio::test]
async fn the_reported_aggregation_statistic_names_the_statistic_the_rule_computes() {
    let (router, engine) = build_router_with_engine();

    // (case, what the rule definition aggregates on, the statistic name the report owes)
    // The names come from the statistics a datapoint of this operation carries
    // (`smithy_query(query="doc",shape="GetInsightRuleReportInput",member="Metrics")`):
    // counting occurrences is the sample count, summing a field is the sum.
    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "a rule counting occurrences",
            json!({"AggregateOn": "Count"}),
            "SampleCount",
        ),
        (
            "a rule summing the field its contribution names",
            json!({
                "AggregateOn": "Sum",
                "Contribution": {"Keys": ["$.host"], "ValueOf": "$.requestBytes"},
            }),
            "Sum",
        ),
        (
            "a rule whose definition names no aggregation, which counts",
            json!({}),
            "SampleCount",
        ),
    ];

    for (index, (case, overrides, expected)) in cases.into_iter().enumerate() {
        let name = format!("statistic-rule-{index}");
        given_rule(&router, &name, &definition(overrides)).await;
        ingest(&engine, &[(1, edge_log("203.0.113.42", 12))]);
        let report = ok(
            &router,
            "GetInsightRuleReport",
            &report_request(&name, 60, 60),
        )
        .await;
        assert_eq!(
            report.get("AggregationStatistic").and_then(Value::as_str),
            Some(expected),
            "case {case:?}: expected the statistic name {expected:?}, got {report}"
        );
        // The report did read the rule's contributor data, so the statistic names a series that
        // is really there.
        assert!(
            !reported_contributors(&report).is_empty(),
            "case {case:?}: expected the contributor the rule collected, got {report}"
        );
    }
}
