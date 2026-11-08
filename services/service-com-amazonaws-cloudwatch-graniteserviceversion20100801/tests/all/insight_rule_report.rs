// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the CloudWatch Contributor Insights READ path: what
//! `GetInsightRuleReport` answers for a rule over the log events it has evaluated.
//!
//! The rule under test: **a report over a window of at least one `Period` is computed from the
//! log events the rule's log groups received while it was ENABLED — one contributor per
//! distinct combination of the values its `Contribution.Keys` extract, counting occurrences or
//! summing the field its `ValueOf` names, ranked by the statistic `OrderBy` asks for and cut to
//! `MaxContributorCount` — and the series it reports is one datapoint per `Period` of the
//! window, anchored at multiples of the period and so strictly ascending in `Timestamp` with no
//! instant repeated, a period holding no observation reporting each statistic asked for at
//! zero; a window shorter than one `Period` is refused.**
//!
//! The rows differ along what the rule generalises over: the log format (`JSON` and `CLF`), how
//! a key addresses its field (a member, a nested member, a `CLF` position and a `CLF` name),
//! how many keys a contributor is made of, what the rule aggregates on, which filters admit an
//! event, how the log groups are named, how many periods the observations span, the `Period`
//! itself, how the contributors are ranked and how many are asked for. The recorded case — one
//! rule keyed on one field over one log group — is one row among them.
//!
//! The log events are evaluated through the SAME entry point the CloudWatch Logs side of this
//! emulator calls on ingest (`state::insight_rules::record_log_events`); the whole
//! ingest-to-report path across the two services is asserted in
//! `core/router/tests/all/contributor_insights_log_events.rs`.

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::harness::{build_router_with_engine, call, error_code, error_message, ok};
use serde_json::{json, Value};
use service_com_amazonaws_cloudwatch_graniteserviceversion20100801::state as cloudwatch;
use smithy_server_runtime::MultiServiceRouter;

/// The instant the windows below are measured from: a whole minute AND a whole five minutes
/// past the epoch, so a datapoint's anchored timestamp is predictable for every period a row
/// asks for.
const ANCHOR_SECS: i64 = 1_767_225_600;

/// The log group the rows ingest into unless they say otherwise.
const GROUP: &str = "/app/access-logs";

/// A rule definition in the Contributor Insights rule syntax, with `overrides` merged over the
/// documented example: a `JSON` rule over one log group, keyed on one field.
fn definition(overrides: Value) -> String {
    let mut rule = json!({
        "Schema": {"Name": "CloudWatchLogRule", "Version": 1},
        "LogGroupNames": [GROUP],
        "LogFormat": "JSON",
        "Contribution": {"Keys": ["$.ip"]},
    });
    let members = rule.as_object_mut().expect("the base rule is an object");
    for (member, value) in overrides.as_object().expect("the overrides are an object") {
        if value.is_null() {
            members.remove(member);
        } else {
            members.insert(member.clone(), value.clone());
        }
    }
    serde_json::to_string(&rule).expect("the definition serializes")
}

/// Create the rule `name` with `rule_definition`, in `state` when the row names one.
async fn given_rule(
    router: &MultiServiceRouter,
    name: &str,
    rule_definition: &str,
    state: Option<&str>,
) {
    let mut request = json!({"RuleName": name, "RuleDefinition": rule_definition});
    if let Some(state) = state {
        request["RuleState"] = json!(state);
    }
    ok(router, "PutInsightRule", &request).await;
}

/// Ingest log events into `log_group`, through the SAME seam the CloudWatch Logs side of this
/// emulator calls when a `PutLogEvents` batch is stored: each event is `(seconds after
/// [`ANCHOR_SECS`], message)`.
fn ingest(engine: &engine::InMemoryEngine, log_group: &str, events: &[(i64, String)]) {
    let stored: Vec<(i64, String)> = events
        .iter()
        .map(|(offset_secs, message)| {
            ((ANCHOR_SECS + offset_secs) * 1_000, message.clone())
        })
        .collect();
    cloudwatch::insight_rules::record_log_events(engine, log_group, &stored);
}

/// A `GetInsightRuleReport` request over `[ANCHOR_SECS, ANCHOR_SECS + window_secs)`.
fn report_request(name: &str, period: i64, window_secs: i64, metrics: &[&str]) -> Value {
    let mut request = json!({
        "RuleName": name,
        "StartTime": ANCHOR_SECS,
        "EndTime": ANCHOR_SECS + window_secs,
        "Period": period,
    });
    if !metrics.is_empty() {
        request["Metrics"] = json!(metrics);
    }
    request
}

/// The `(keys, ApproximateAggregateValue)` pairs a report lists, in the order it listed them.
fn reported_contributors(report: &Value) -> ReportedContributors {
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
                                .filter_map(|key| key.as_str().map(ToString::to_string))
                                .collect()
                        })
                        .unwrap_or_default();
                    let value = contributor
                        .get("ApproximateAggregateValue")
                        .and_then(Value::as_f64)
                        .unwrap_or_default();
                    (keys, value)
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The datapoints a report's series carries.
fn reported_datapoints(report: &Value) -> Vec<Value> {
    report
        .get("MetricDatapoints")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The `Timestamp` of each datapoint, as the epoch seconds this protocol carries.
fn datapoint_timestamps(report: &Value) -> Vec<f64> {
    reported_datapoints(report)
        .iter()
        .filter_map(|point| point.get("Timestamp").and_then(Value::as_f64))
        .collect()
}

/// One statistic of a datapoint.
fn statistic(point: &Value, name: &str) -> Option<f64> {
    point.get(name).and_then(Value::as_f64)
}

/// One row of the filter table: a message and whether the row's filter must admit it.
type FilterCases = Vec<(Value, bool)>;

/// What [`reported_contributors`] answers: each contributor's keys and its aggregate value, in
/// the order the report listed them.
type ReportedContributors = Vec<(Vec<String>, f64)>;

/// What [`reported_datapoints`] answers for a report carrying no series, for the comparison.
fn no_datapoints() -> Vec<Value> {
    Vec::new()
}

/// A JSON access-log message, as the rows below write one.
fn access_log(members: Value) -> String {
    members.to_string()
}

/// One row of the table: a rule, the events its log group received, and the report they make.
struct Row {
    /// What this row varies, named in every assertion.
    case: &'static str,
    /// The rule's definition.
    definition: String,
    /// The log group the events are ingested into.
    group: &'static str,
    /// The events, each `(seconds after the anchor, message)`.
    events: Vec<(i64, String)>,
    /// The keys the report must list as its `KeyLabels`.
    key_labels: Vec<&'static str>,
    /// The `AggregationStatistic` the report must carry.
    aggregation_statistic: &'static str,
    /// The contributors it must list, in order, with their aggregate values.
    contributors: Vec<(Vec<&'static str>, f64)>,
    /// The statistics its one datapoint must carry.
    datapoint: Vec<(&'static str, f64)>,
}

#[tokio::test]
async fn a_report_is_computed_from_the_log_events_the_rule_evaluated() {
    // Every statistic a datapoint can carry, so each row pins the whole fold of its period.
    const EVERY_METRIC: [&str; 7] = [
        "UniqueContributors",
        "MaxContributorValue",
        "SampleCount",
        "Sum",
        "Minimum",
        "Maximum",
        "Average",
    ];

    let rows: Vec<Row> = vec![
        Row {
            case: "a JSON rule keyed on one field, counting occurrences",
            definition: definition(json!({})),
            group: GROUP,
            events: vec![
                (1, access_log(json!({"ip": "203.0.113.42", "requestBytes": 128}))),
                (2, access_log(json!({"ip": "203.0.113.42", "requestBytes": 256}))),
                (3, access_log(json!({"ip": "203.0.113.42", "requestBytes": 512}))),
                (4, access_log(json!({"ip": "198.51.100.7", "requestBytes": 64}))),
            ],
            key_labels: vec!["$.ip"],
            aggregation_statistic: "SampleCount",
            contributors: vec![
                (vec!["203.0.113.42"], 3.0),
                (vec!["198.51.100.7"], 1.0),
            ],
            datapoint: vec![
                ("UniqueContributors", 2.0),
                ("MaxContributorValue", 3.0),
                ("SampleCount", 4.0),
                ("Sum", 4.0),
                ("Minimum", 1.0),
                ("Maximum", 1.0),
                ("Average", 1.0),
            ],
        },
        Row {
            case: "a JSON rule keyed on two fields, so a combination is a contributor",
            definition: definition(json!({
                "Contribution": {"Keys": ["$.ip", "$.httpMethod"]},
            })),
            group: GROUP,
            events: vec![
                (1, access_log(json!({"ip": "203.0.113.42", "httpMethod": "GET"}))),
                (2, access_log(json!({"ip": "203.0.113.42", "httpMethod": "PUT"}))),
                (3, access_log(json!({"ip": "203.0.113.42", "httpMethod": "GET"}))),
            ],
            key_labels: vec!["$.ip", "$.httpMethod"],
            aggregation_statistic: "SampleCount",
            contributors: vec![
                (vec!["203.0.113.42", "GET"], 2.0),
                (vec!["203.0.113.42", "PUT"], 1.0),
            ],
            datapoint: vec![
                ("UniqueContributors", 2.0),
                ("MaxContributorValue", 2.0),
                ("SampleCount", 3.0),
                ("Sum", 3.0),
                ("Average", 1.0),
            ],
        },
        Row {
            case: "a JSON rule summing the field its contribution names",
            definition: definition(json!({
                "Contribution": {"Keys": ["$.ip"], "ValueOf": "$.requestBytes"},
                "AggregateOn": "Sum",
            })),
            group: GROUP,
            events: vec![
                (1, access_log(json!({"ip": "203.0.113.42", "requestBytes": 100}))),
                (2, access_log(json!({"ip": "203.0.113.42", "requestBytes": 300}))),
                (3, access_log(json!({"ip": "198.51.100.7", "requestBytes": 50}))),
            ],
            key_labels: vec!["$.ip"],
            aggregation_statistic: "Sum",
            contributors: vec![
                (vec!["203.0.113.42"], 400.0),
                (vec!["198.51.100.7"], 50.0),
            ],
            datapoint: vec![
                ("UniqueContributors", 2.0),
                ("MaxContributorValue", 400.0),
                ("SampleCount", 3.0),
                ("Sum", 450.0),
                ("Minimum", 50.0),
                ("Maximum", 300.0),
                ("Average", 150.0),
            ],
        },
        Row {
            case: "a JSON rule keyed on a nested field",
            definition: definition(json!({
                "Contribution": {"Keys": ["$.request.ip"]},
            })),
            group: GROUP,
            events: vec![
                (1, access_log(json!({"request": {"ip": "203.0.113.42"}}))),
                (2, access_log(json!({"request": {"ip": "203.0.113.42"}}))),
                (3, access_log(json!({"request": {"ip": "192.0.2.9"}}))),
            ],
            key_labels: vec!["$.request.ip"],
            aggregation_statistic: "SampleCount",
            contributors: vec![(vec!["203.0.113.42"], 2.0), (vec!["192.0.2.9"], 1.0)],
            datapoint: vec![("UniqueContributors", 2.0), ("SampleCount", 3.0)],
        },
        Row {
            case: "a JSON rule keyed on a numeric field, which keys by its value",
            definition: definition(json!({
                "Contribution": {"Keys": ["$.status"]},
            })),
            group: GROUP,
            events: vec![
                (1, access_log(json!({"status": 200}))),
                (2, access_log(json!({"status": 404}))),
                (3, access_log(json!({"status": 404}))),
            ],
            key_labels: vec!["$.status"],
            aggregation_statistic: "SampleCount",
            contributors: vec![(vec!["404"], 2.0), (vec!["200"], 1.0)],
            datapoint: vec![("UniqueContributors", 2.0), ("SampleCount", 3.0)],
        },
        Row {
            case: "a CLF rule keyed on a field its Fields member names",
            definition: definition(json!({
                "LogFormat": "CLF",
                "Fields": {"1": "IpAddress", "4": "Bytes"},
                "Contribution": {"Keys": ["IpAddress"], "ValueOf": "Bytes"},
                "AggregateOn": "Sum",
            })),
            group: GROUP,
            events: vec![
                (1, "203.0.113.42 - - 128".to_string()),
                (2, "203.0.113.42 - - 64".to_string()),
                (3, "198.51.100.7 - - 16".to_string()),
            ],
            key_labels: vec!["IpAddress"],
            aggregation_statistic: "Sum",
            contributors: vec![
                (vec!["203.0.113.42"], 192.0),
                (vec!["198.51.100.7"], 16.0),
            ],
            datapoint: vec![
                ("UniqueContributors", 2.0),
                ("MaxContributorValue", 192.0),
                ("Sum", 208.0),
                ("Minimum", 16.0),
                ("Maximum", 128.0),
            ],
        },
        Row {
            case: "a CLF rule keyed by field position",
            definition: definition(json!({
                "LogFormat": "CLF",
                "Contribution": {"Keys": ["$2"]},
            })),
            group: GROUP,
            events: vec![
                (1, "GET /index.html 200".to_string()),
                (2, "GET /index.html 200".to_string()),
                (3, "GET /health 200".to_string()),
            ],
            key_labels: vec!["$2"],
            aggregation_statistic: "SampleCount",
            contributors: vec![(vec!["/index.html"], 2.0), (vec!["/health"], 1.0)],
            datapoint: vec![("UniqueContributors", 2.0), ("SampleCount", 3.0)],
        },
        Row {
            case: "a rule whose log group is named by ARN",
            definition: definition(json!({
                "LogGroupNames": null,
                "LogGroupARNArray": [
                    "arn:aws:logs:us-east-1:000000000000:log-group:/app/by-arn:*",
                ],
            })),
            group: "/app/by-arn",
            events: vec![
                (1, access_log(json!({"ip": "203.0.113.42"}))),
                (2, access_log(json!({"ip": "203.0.113.42"}))),
            ],
            key_labels: vec!["$.ip"],
            aggregation_statistic: "SampleCount",
            contributors: vec![(vec!["203.0.113.42"], 2.0)],
            datapoint: vec![("UniqueContributors", 1.0), ("SampleCount", 2.0)],
        },
        Row {
            case: "a rule whose filters admit only the events they name",
            definition: definition(json!({
                "Contribution": {
                    "Keys": ["$.ip"],
                    "Filters": [
                        {"Match": "$.httpMethod", "In": ["PUT"]},
                        {"Match": "$.requestBytes", "GreaterThan": 100},
                        {"Match": "$.path", "StartsWith": ["/api"]},
                        {"Match": "$.userAgent", "IsPresent": true},
                    ],
                },
            })),
            group: GROUP,
            events: vec![
                // Admitted: every filter holds.
                (
                    1,
                    access_log(json!({
                        "ip": "203.0.113.42",
                        "httpMethod": "PUT",
                        "requestBytes": 200,
                        "path": "/api/items",
                        "userAgent": "curl",
                    })),
                ),
                // Refused by `In`.
                (
                    2,
                    access_log(json!({
                        "ip": "203.0.113.42",
                        "httpMethod": "GET",
                        "requestBytes": 200,
                        "path": "/api/items",
                        "userAgent": "curl",
                    })),
                ),
                // Refused by `GreaterThan`.
                (
                    3,
                    access_log(json!({
                        "ip": "203.0.113.42",
                        "httpMethod": "PUT",
                        "requestBytes": 10,
                        "path": "/api/items",
                        "userAgent": "curl",
                    })),
                ),
                // Refused by `StartsWith`.
                (
                    4,
                    access_log(json!({
                        "ip": "203.0.113.42",
                        "httpMethod": "PUT",
                        "requestBytes": 200,
                        "path": "/health",
                        "userAgent": "curl",
                    })),
                ),
                // Refused by `IsPresent`: the event carries no user agent.
                (
                    5,
                    access_log(json!({
                        "ip": "203.0.113.42",
                        "httpMethod": "PUT",
                        "requestBytes": 200,
                        "path": "/api/items",
                    })),
                ),
            ],
            key_labels: vec!["$.ip"],
            aggregation_statistic: "SampleCount",
            contributors: vec![(vec!["203.0.113.42"], 1.0)],
            datapoint: vec![("UniqueContributors", 1.0), ("SampleCount", 1.0)],
        },
    ];

    for (index, row) in rows.into_iter().enumerate() {
        let case = row.case;
        let (router, engine) = build_router_with_engine();
        let name = format!("report-rule-{index}");
        given_rule(&router, &name, &row.definition, Some("ENABLED")).await;
        ingest(&engine, row.group, &row.events);

        let report = ok(
            &router,
            "GetInsightRuleReport",
            // A window of exactly one period: every event of a row falls inside it.
            &report_request(&name, 60, 60, &EVERY_METRIC),
        )
        .await;

        assert_eq!(
            report.get("KeyLabels").and_then(Value::as_array).cloned(),
            Some(row.key_labels.iter().map(|key| json!(key)).collect()),
            "case {case:?}: expected the rule's keys as KeyLabels, got {report}"
        );
        assert_eq!(
            report.get("AggregationStatistic").and_then(Value::as_str),
            Some(row.aggregation_statistic),
            "case {case:?}: expected AggregationStatistic {:?}, got {report}",
            row.aggregation_statistic
        );

        let expected: Vec<(Vec<String>, f64)> = row
            .contributors
            .iter()
            .map(|(keys, value)| {
                (keys.iter().map(ToString::to_string).collect(), *value)
            })
            .collect();
        let got = reported_contributors(&report);
        assert_eq!(
            got, expected,
            "case {case:?}: expected the contributors {expected:?}, got {got:?} in {report}"
        );
        assert_eq!(
            report.get("ApproximateUniqueCount").and_then(Value::as_i64),
            Some(i64::try_from(expected.len()).expect("the row lists few contributors")),
            "case {case:?}: expected one unique count per contributor, got {report}"
        );
        let aggregate: f64 = expected.iter().map(|(_, value)| *value).sum();
        assert_eq!(
            report.get("AggregateValue").and_then(Value::as_f64),
            Some(aggregate),
            "case {case:?}: expected AggregateValue {aggregate}, got {report}"
        );

        let datapoints = reported_datapoints(&report);
        assert_eq!(
            datapoints.len(),
            1,
            "case {case:?}: the events fall in one period, so the series holds one datapoint, \
             got {datapoints:?}"
        );
        for (name, value) in row.datapoint {
            assert_eq!(
                statistic(&datapoints[0], name),
                Some(value),
                "case {case:?}: expected {name} {value}, got {:?}",
                datapoints[0]
            );
        }
        #[allow(clippy::cast_precision_loss)]
        let anchor_secs = ANCHOR_SECS as f64;
        assert_eq!(
            statistic(&datapoints[0], "Timestamp"),
            Some(anchor_secs),
            "case {case:?}: a datapoint is anchored at the start of its period, got {:?}",
            datapoints[0]
        );

        // Reading twice changes nothing a later caller sees.
        let again = ok(
            &router,
            "GetInsightRuleReport",
            &report_request(&name, 60, 60, &EVERY_METRIC),
        )
        .await;
        assert_eq!(
            again, report,
            "case {case:?}: a second report of one window must be the same report, got {again}"
        );
    }
}

#[tokio::test]
async fn an_event_the_rule_does_not_admit_contributes_nothing() {
    // Rows differ along WHY the event contributes nothing: the message is not in the rule's
    // format, it carries no value for a key, that value identifies no contributor, a summing
    // rule finds no number to sum, or the event was ingested into another log group.
    let cases: Vec<(&str, String, &'static str, String)> = vec![
        (
            "a message that is not JSON at all",
            definition(json!({})),
            GROUP,
            "ERROR something went wrong".to_string(),
        ),
        (
            "a JSON message carrying no value for the key",
            definition(json!({})),
            GROUP,
            access_log(json!({"userAgent": "curl"})),
        ),
        (
            "a key whose value is an object, which identifies no contributor",
            definition(json!({})),
            GROUP,
            access_log(json!({"ip": {"v4": "203.0.113.42"}})),
        ),
        (
            "a key whose value is an array",
            definition(json!({})),
            GROUP,
            access_log(json!({"ip": ["203.0.113.42"]})),
        ),
        (
            "a summing rule whose value field is not a number",
            definition(json!({
                "Contribution": {"Keys": ["$.ip"], "ValueOf": "$.requestBytes"},
                "AggregateOn": "Sum",
            })),
            GROUP,
            access_log(json!({"ip": "203.0.113.42", "requestBytes": "many"})),
        ),
        (
            "a summing rule whose value field is absent",
            definition(json!({
                "Contribution": {"Keys": ["$.ip"], "ValueOf": "$.requestBytes"},
                "AggregateOn": "Sum",
            })),
            GROUP,
            access_log(json!({"ip": "203.0.113.42"})),
        ),
        (
            "an event ingested into a log group the rule does not name",
            definition(json!({})),
            "/app/other-logs",
            access_log(json!({"ip": "203.0.113.42"})),
        ),
        (
            "a CLF rule and a record with too few fields",
            definition(json!({
                "LogFormat": "CLF",
                "Contribution": {"Keys": ["$7"]},
            })),
            GROUP,
            "203.0.113.42 - -".to_string(),
        ),
    ];

    for (index, (case, rule_definition, group, message)) in cases.into_iter().enumerate() {
        let (router, engine) = build_router_with_engine();
        let name = format!("admits-rule-{index}");
        given_rule(&router, &name, &rule_definition, None).await;
        ingest(&engine, group, &[(1, message)]);

        let report = ok(
            &router,
            "GetInsightRuleReport",
            &report_request(&name, 60, 60, &["UniqueContributors"]),
        )
        .await;
        assert_eq!(
            reported_contributors(&report),
            Vec::new(),
            "case {case:?}: an event the rule does not admit contributes no contributor, got \
             {report}"
        );
        assert_eq!(
            report.get("AggregateValue").and_then(Value::as_f64),
            Some(0.0),
            "case {case:?}: nothing contributed means nothing aggregated, got {report}"
        );
        let datapoints = reported_datapoints(&report);
        assert_eq!(
            datapoints.len(),
            1,
            "case {case:?}: the window spans one period, which the series reports whether or not \
             it holds an observation, got {report}"
        );
        assert_eq!(
            statistic(&datapoints[0], "UniqueContributors"),
            Some(0.0),
            "case {case:?}: a period holding no observation reports the statistic asked for at \
             zero, got {report}"
        );
    }
}

#[tokio::test]
async fn each_filter_operation_admits_exactly_the_events_it_names() {
    // One row per operation the Contributor Insights filter syntax defines, and one for an
    // entry that states no operation at all. Each row's events span the three cases the rule
    // generalises over: the field has the property, it has another value, and the event does
    // not carry the field at all — which only `IsPresent: false` admits, since every other test
    // needs a value to read.
    let cases: Vec<(&str, Value, FilterCases)> = vec![
        (
            "In",
            json!({"Match": "$.httpMethod", "In": ["PUT", "POST"]}),
            vec![
                (json!({"ip": "a", "httpMethod": "PUT"}), true),
                (json!({"ip": "a", "httpMethod": "POST"}), true),
                (json!({"ip": "a", "httpMethod": "GET"}), false),
                (json!({"ip": "a"}), false),
            ],
        ),
        (
            "NotIn",
            json!({"Match": "$.httpMethod", "NotIn": ["GET"]}),
            vec![
                (json!({"ip": "a", "httpMethod": "PUT"}), true),
                (json!({"ip": "a", "httpMethod": "GET"}), false),
                (json!({"ip": "a"}), false),
            ],
        ),
        (
            "StartsWith",
            json!({"Match": "$.path", "StartsWith": ["/api", "/admin"]}),
            vec![
                (json!({"ip": "a", "path": "/api/items"}), true),
                (json!({"ip": "a", "path": "/admin"}), true),
                (json!({"ip": "a", "path": "/health"}), false),
                (json!({"ip": "a"}), false),
            ],
        ),
        (
            "GreaterThan",
            json!({"Match": "$.requestBytes", "GreaterThan": 100}),
            vec![
                (json!({"ip": "a", "requestBytes": 101}), true),
                (json!({"ip": "a", "requestBytes": 100}), false),
                (json!({"ip": "a", "requestBytes": 1}), false),
                (json!({"ip": "a"}), false),
            ],
        ),
        (
            "LessThan",
            json!({"Match": "$.requestBytes", "LessThan": 100}),
            vec![
                (json!({"ip": "a", "requestBytes": 99}), true),
                (json!({"ip": "a", "requestBytes": 100}), false),
                (json!({"ip": "a", "requestBytes": 1000}), false),
                (json!({"ip": "a"}), false),
            ],
        ),
        (
            "EqualTo",
            json!({"Match": "$.status", "EqualTo": 200}),
            vec![
                (json!({"ip": "a", "status": 200}), true),
                (json!({"ip": "a", "status": 404}), false),
                (json!({"ip": "a", "status": "ok"}), false),
                (json!({"ip": "a"}), false),
            ],
        ),
        (
            "NotEqualTo",
            json!({"Match": "$.status", "NotEqualTo": 200}),
            vec![
                (json!({"ip": "a", "status": 404}), true),
                (json!({"ip": "a", "status": 200}), false),
                (json!({"ip": "a"}), false),
            ],
        ),
        (
            "IsPresent true",
            json!({"Match": "$.userAgent", "IsPresent": true}),
            vec![
                (json!({"ip": "a", "userAgent": "curl"}), true),
                (json!({"ip": "a", "userAgent": ""}), true),
                (json!({"ip": "a"}), false),
            ],
        ),
        (
            "IsPresent false",
            json!({"Match": "$.userAgent", "IsPresent": false}),
            vec![
                (json!({"ip": "a"}), true),
                (json!({"ip": "a", "userAgent": "curl"}), false),
            ],
        ),
        (
            "an entry stating no operation, which constrains nothing",
            json!({"Match": "$.userAgent"}),
            vec![
                (json!({"ip": "a", "userAgent": "curl"}), true),
                (json!({"ip": "a"}), true),
            ],
        ),
    ];

    for (index, (case, filter, events)) in cases.into_iter().enumerate() {
        let (router, engine) = build_router_with_engine();
        let name = format!("filter-rule-{index}");
        given_rule(
            &router,
            &name,
            &definition(json!({
                "Contribution": {"Keys": ["$.ip"], "Filters": [filter]},
            })),
            None,
        )
        .await;
        let ingested: Vec<(i64, String)> = events
            .iter()
            .enumerate()
            .map(|(position, (message, _))| {
                (
                    i64::try_from(position).expect("a row ingests few events"),
                    access_log(message.clone()),
                )
            })
            .collect();
        ingest(&engine, GROUP, &ingested);

        let admitted = events.iter().filter(|(_, admitted)| *admitted).count();
        #[allow(clippy::cast_precision_loss)]
        let expected = admitted as f64;
        let report = ok(
            &router,
            "GetInsightRuleReport",
            &report_request(&name, 60, 60, &["SampleCount"]),
        )
        .await;
        assert_eq!(
            report.get("AggregateValue").and_then(Value::as_f64),
            Some(expected),
            "case \"the {case} filter\": expected {expected} of the row's events admitted, got \
             {report}"
        );
    }
}

#[tokio::test]
async fn the_series_holds_one_ascending_datapoint_per_period_of_the_window() {
    // Rows differ along the period the datapoints are aggregated over and how many periods the
    // observations span, including the narrowest period a report may be asked at and the widest
    // the window can carry. The last column names the periods that hold an observation — every
    // OTHER period of the window is reported too, at zero.
    const WINDOW_SECS: i64 = 3_600;
    let cases: Vec<(&str, i64, Vec<i64>, Vec<i64>)> = vec![
        (
            "a one-minute period over three minutes",
            60,
            vec![0, 10, 70, 130, 190],
            vec![0, 60, 120, 180],
        ),
        (
            "a five-minute period folding the same observations into one datapoint",
            300,
            vec![0, 10, 70, 130, 190],
            vec![0],
        ),
        (
            "an hour-long period, the widest this window spans",
            3_600,
            vec![0, 1, 1, 4],
            vec![0],
        ),
        (
            "periods with no observation between the ones that hold them",
            60,
            vec![5, 125],
            vec![0, 120],
        ),
        (
            "a ten-minute period over an hour-long window",
            600,
            vec![0, 599, 600, 2_400],
            vec![0, 600, 2_400],
        ),
    ];

    for (index, (case, period, offsets, observed_buckets)) in cases.into_iter().enumerate() {
        let (router, engine) = build_router_with_engine();
        let name = format!("series-rule-{index}");
        given_rule(&router, &name, &definition(json!({})), None).await;
        let events: Vec<(i64, String)> = offsets
            .iter()
            .map(|offset| (*offset, access_log(json!({"ip": "203.0.113.42"}))))
            .collect();
        ingest(&engine, GROUP, &events);

        let report = ok(
            &router,
            "GetInsightRuleReport",
            &report_request(&name, period, WINDOW_SECS, &["SampleCount"]),
        )
        .await;
        let timestamps = datapoint_timestamps(&report);
        // Every period of the window, and not only the ones holding an observation: "A time
        // series of metric data points that matches the time period in the rule request."
        // (`GetInsightRuleReportOutput$MetricDatapoints`)
        #[allow(clippy::cast_precision_loss)]
        let expected: Vec<f64> = (0..WINDOW_SECS / period)
            .map(|step| (ANCHOR_SECS + step * period) as f64)
            .collect();
        assert_eq!(
            timestamps, expected,
            "case {case:?}: expected one datapoint per period of the window, {} of them, got {} \
             in {report}",
            expected.len(),
            timestamps.len()
        );
        // The series is strictly ascending, so no instant is reported twice.
        assert!(
            timestamps.windows(2).all(|pair| pair[0] < pair[1]),
            "case {case:?}: the series must be strictly ascending by Timestamp, got \
             {timestamps:?}"
        );
        // Each datapoint is anchored at a multiple of the period.
        #[allow(clippy::cast_precision_loss)]
        let period_width = period as f64;
        assert!(
            timestamps
                .iter()
                .all(|at| (at / period_width).fract().abs() <= f64::EPSILON),
            "case {case:?}: each datapoint is anchored at a multiple of the period, got \
             {timestamps:?}"
        );
        // A period reports the observations that fell in it, and a period holding none reports
        // the statistic at zero.
        #[allow(clippy::cast_precision_loss)]
        let expected_counts: Vec<f64> = (0..WINDOW_SECS / period)
            .map(|step| {
                let bucket = step * period;
                offsets
                    .iter()
                    .filter(|offset| *offset - offset.rem_euclid(period) == bucket)
                    .count() as f64
            })
            .collect();
        let counts: Vec<f64> = reported_datapoints(&report)
            .iter()
            .map(|point| statistic(point, "SampleCount").unwrap_or(f64::NAN))
            .collect();
        assert_eq!(
            counts, expected_counts,
            "case {case:?}: expected the per-period counts {expected_counts:?}, got {counts:?} \
             in {report}"
        );
        // The row's own account of which periods hold an observation, read off the series.
        #[allow(clippy::cast_precision_loss)]
        let expected_observed: Vec<f64> = observed_buckets
            .iter()
            .map(|bucket| (ANCHOR_SECS + bucket) as f64)
            .collect();
        let observed: Vec<f64> = reported_datapoints(&report)
            .iter()
            .filter(|point| statistic(point, "SampleCount").unwrap_or_default() > 0.0)
            .filter_map(|point| point.get("Timestamp").and_then(Value::as_f64))
            .collect();
        assert_eq!(
            observed, expected_observed,
            "case {case:?}: expected the periods {expected_observed:?} to hold the observations, \
             got {observed:?} in {report}"
        );
        // Every observation is counted exactly once across the series.
        let counted: f64 = counts.iter().sum();
        #[allow(clippy::cast_precision_loss)]
        let ingested = offsets.len() as f64;
        assert_eq!(
            counted, ingested,
            "case {case:?}: expected the {ingested} observations counted once each, got {counted}"
        );
    }

    // A report asked for no statistic carries no series: there would be nothing in a datapoint
    // to read.
    let (router, engine) = build_router_with_engine();
    given_rule(&router, "no-metrics-rule", &definition(json!({})), None).await;
    ingest(
        &engine,
        GROUP,
        &[(1, access_log(json!({"ip": "203.0.113.42"})))],
    );
    let report = ok(
        &router,
        "GetInsightRuleReport",
        &report_request("no-metrics-rule", 60, 60, &[]),
    )
    .await;
    assert_eq!(
        reported_datapoints(&report),
        no_datapoints(),
        "case \"no statistic asked for\": expected no series, got {report}"
    );
    assert_eq!(
        reported_contributors(&report),
        vec![(vec!["203.0.113.42".to_string()], 1.0)],
        "case \"no statistic asked for\": the contributors are still reported, got {report}"
    );
    // Only the statistics the request names are carried.
    let one = ok(
        &router,
        "GetInsightRuleReport",
        &report_request("no-metrics-rule", 60, 60, &["Sum"]),
    )
    .await;
    let point = reported_datapoints(&one)
        .first()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        statistic(&point, "Sum"),
        Some(1.0),
        "case \"one statistic asked for\": expected the Sum, got {point}"
    );
    for absent in ["UniqueContributors", "SampleCount", "Average", "Minimum"] {
        assert_eq!(
            statistic(&point, absent),
            None,
            "case \"one statistic asked for\": {absent} was not asked for, got {point}"
        );
    }
}

#[tokio::test]
async fn contributors_are_ranked_as_asked_and_cut_to_the_count_asked_for() {
    // Three contributors whose SUM order and MAXIMUM order disagree: `steady` contributes the
    // most over the window, while `spike` holds the largest single period.
    let (router, engine) = build_router_with_engine();
    given_rule(
        &router,
        "ranked-rule",
        &definition(json!({
            "Contribution": {"Keys": ["$.ip"], "ValueOf": "$.requestBytes"},
            "AggregateOn": "Sum",
        })),
        None,
    )
    .await;
    let mut events: Vec<(i64, String)> = Vec::new();
    // steady: 100 in each of four periods — 400 in total, 100 at most.
    for period in 0..4 {
        events.push((
            period * 60 + 1,
            access_log(json!({"ip": "steady", "requestBytes": 100})),
        ));
    }
    // spike: 250 in one period — 250 in total, 250 at most.
    events.push((61, access_log(json!({"ip": "spike", "requestBytes": 250}))));
    // quiet: 10 in one period.
    events.push((61, access_log(json!({"ip": "quiet", "requestBytes": 10}))));
    ingest(&engine, GROUP, &events);

    // (case, the OrderBy the request names, the contributors in the order it must report them)
    let orderings: Vec<(&str, Option<&str>, Vec<&str>)> = vec![
        ("omitted, which ranks by Sum", None, vec!["steady", "spike", "quiet"]),
        ("Sum", Some("Sum"), vec!["steady", "spike", "quiet"]),
        ("Maximum", Some("Maximum"), vec!["spike", "steady", "quiet"]),
    ];
    for (case, order_by, expected) in orderings {
        let mut request = report_request("ranked-rule", 60, 300, &["MaxContributorValue"]);
        if let Some(order_by) = order_by {
            request["OrderBy"] = json!(order_by);
        }
        let report = ok(&router, "GetInsightRuleReport", &request).await;
        let got: Vec<String> = reported_contributors(&report)
            .into_iter()
            .map(|(keys, _)| keys.join(","))
            .collect();
        assert_eq!(
            got, expected,
            "case \"OrderBy {case}\": expected the contributors ranked {expected:?}, got {got:?} \
             in {report}"
        );
        // Whatever the ranking, the report still stands on every contributor.
        assert_eq!(
            report.get("ApproximateUniqueCount").and_then(Value::as_i64),
            Some(3),
            "case \"OrderBy {case}\": expected every contributor counted, got {report}"
        );
        assert_eq!(
            report.get("AggregateValue").and_then(Value::as_f64),
            Some(660.0),
            "case \"OrderBy {case}\": expected the total of every contributor, got {report}"
        );
    }

    // Rows differ along how many contributors the request asks for: fewer than there are,
    // exactly as many, more, the bound's minimum and its maximum.
    for (asked, expected) in [
        (Some(1), vec!["steady"]),
        (Some(2), vec!["steady", "spike"]),
        (Some(3), vec!["steady", "spike", "quiet"]),
        (Some(100), vec!["steady", "spike", "quiet"]),
        (None, vec!["steady", "spike", "quiet"]),
    ] {
        let mut request = report_request("ranked-rule", 60, 300, &[]);
        if let Some(asked) = asked {
            request["MaxContributorCount"] = json!(asked);
        }
        let report = ok(&router, "GetInsightRuleReport", &request).await;
        let got: Vec<String> = reported_contributors(&report)
            .into_iter()
            .map(|(keys, _)| keys.join(","))
            .collect();
        assert_eq!(
            got, expected,
            "case \"MaxContributorCount {asked:?}\": expected {expected:?}, got {got:?} in \
             {report}"
        );
    }

    // Contributors that rank equally are reported in one order, whichever call asks: the tie is
    // broken by their keys rather than left to the walk.
    let (tied_router, tied_engine) = build_router_with_engine();
    given_rule(&tied_router, "tied-rule", &definition(json!({})), None).await;
    let tied: Vec<(i64, String)> = ["delta", "alpha", "charlie", "bravo"]
        .iter()
        .enumerate()
        .map(|(index, ip)| {
            (
                i64::try_from(index).expect("four events"),
                access_log(json!({"ip": ip})),
            )
        })
        .collect();
    ingest(&tied_engine, GROUP, &tied);
    let mut seen: Vec<Vec<String>> = Vec::new();
    for _ in 0..3 {
        let report = ok(
            &tied_router,
            "GetInsightRuleReport",
            &report_request("tied-rule", 60, 60, &["UniqueContributors"]),
        )
        .await;
        seen.push(
            reported_contributors(&report)
                .into_iter()
                .map(|(keys, _)| keys.join(","))
                .collect(),
        );
    }
    assert_eq!(
        seen[0],
        vec!["alpha", "bravo", "charlie", "delta"],
        "case \"equal contributors\": a tie is broken by the contributor's keys, got {:?}",
        seen[0]
    );
    assert!(
        seen.windows(2).all(|pair| pair[0] == pair[1]),
        "case \"equal contributors\": every report must list them in the same order, got {seen:?}"
    );
}

#[tokio::test]
async fn only_an_enabled_rule_collects_the_events_its_log_group_receives() {
    let (router, engine) = build_router_with_engine();
    // (case, the state the rule is in while the events arrive, how many it must collect)
    let cases: Vec<(&str, Option<&str>, f64)> = vec![
        ("the default state, which is enabled", None, 2.0),
        ("explicitly enabled", Some("ENABLED"), 2.0),
        ("disabled, so it is not evaluating log events", Some("DISABLED"), 0.0),
    ];
    for (index, (case, state, expected)) in cases.into_iter().enumerate() {
        let name = format!("state-rule-{index}");
        given_rule(&router, &name, &definition(json!({})), state).await;
        ingest(
            &engine,
            GROUP,
            &[
                (1, access_log(json!({"ip": "203.0.113.42"}))),
                (2, access_log(json!({"ip": "203.0.113.42"}))),
            ],
        );
        let report = ok(
            &router,
            "GetInsightRuleReport",
            &report_request(&name, 60, 60, &["SampleCount"]),
        )
        .await;
        assert_eq!(
            report.get("AggregateValue").and_then(Value::as_f64),
            Some(expected),
            "case {case:?}: expected {expected} contributed, got {report}"
        );
    }

    // A rule enabled after the fact collects what arrives from then on, and keeps what it
    // collected before it was disabled.
    ok(
        &router,
        "EnableInsightRules",
        &json!({"RuleNames": ["state-rule-2"]}),
    )
    .await;
    ingest(
        &engine,
        GROUP,
        &[(3, access_log(json!({"ip": "203.0.113.42"})))],
    );
    let report = ok(
        &router,
        "GetInsightRuleReport",
        &report_request("state-rule-2", 60, 60, &["SampleCount"]),
    )
    .await;
    assert_eq!(
        report.get("AggregateValue").and_then(Value::as_f64),
        Some(1.0),
        "case \"enabled after the events it missed\": only what arrived while enabled is \
         collected, got {report}"
    );

    // A window the observations fall outside of reports none of them.
    let outside = ok(
        &router,
        "GetInsightRuleReport",
        &json!({
            "RuleName": "state-rule-0",
            "StartTime": ANCHOR_SECS + 3_600,
            "EndTime": ANCHOR_SECS + 7_200,
            "Period": 60,
            "Metrics": ["SampleCount"],
        }),
    )
    .await;
    assert_eq!(
        reported_contributors(&outside),
        Vec::new(),
        "case \"a window the observations fall outside\": expected no contributor, got {outside}"
    );
}

#[tokio::test]
async fn a_window_shorter_than_one_period_is_refused() {
    let (router, _engine) = build_router_with_engine();
    given_rule(&router, "window-rule", &definition(json!({})), None).await;

    // Rows differ along how the window relates to the period: shorter than one period, exactly
    // one period, longer, zero-length and reversed — on the narrowest period a report may be
    // asked at as well as on wider ones.
    let cases: Vec<(&str, i64, i64, bool)> = vec![
        ("exactly one period", 60, 60, true),
        ("two periods", 60, 120, true),
        ("a second short of one period", 60, 59, false),
        ("one second of a one-minute period", 60, 1, false),
        ("a zero-length window", 60, 0, false),
        ("a reversed window", 60, -60, false),
        ("exactly the widest period a report may be asked at", 86_400, 86_400, true),
        ("a second short of that widest period", 86_400, 86_399, false),
        ("exactly one ten-minute period", 600, 600, true),
        ("nine minutes of a ten-minute period", 600, 540, false),
        // The other end of the same ratio: a window spanning more periods than one report
        // carries is refused, and the same window at a period that fits is served.
        ("a window spanning more periods than one report carries", 60, 6_100_000, false),
        ("the same window at a period that fits it", 3_600, 6_100_000, true),
    ];
    for (case, period, window_secs, expected_ok) in cases {
        let (status, body) = call(
            &router,
            "GetInsightRuleReport",
            &report_request("window-rule", period, window_secs, &["SampleCount"]),
        )
        .await;
        if expected_ok {
            assert_eq!(
                status, 200,
                "case {case:?}: a window of at least one period must be reported, got {status} \
                 {body}"
            );
        } else {
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "InvalidParameterValueException"),
                "case {case:?}: expected a 400 InvalidParameterValueException, got {status} {body}"
            );
            // The window's own refusal names the condition by its code; the ratio's names what
            // was asked for, the cap it broke and the two remedies.
            let message = error_message(&body);
            let expected_window_refusal = window_secs < period;
            assert_eq!(
                message == "INVALID_TIME_RANGE",
                expected_window_refusal,
                "case {case:?}: expected the window refusal: {expected_window_refusal}, got \
                 {message:?} in {body}"
            );
            if !expected_window_refusal {
                assert!(
                    message.contains("exceeds the limit of"),
                    "case {case:?}: expected the datapoint-cap refusal, got {message:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_value_no_report_can_be_computed_for_is_refused() {
    let (router, _engine) = build_router_with_engine();
    given_rule(&router, "refusal-rule", &definition(json!({})), None).await;

    // Rows differ along which value is out of range: the number of contributors (below the
    // range, above it, and at each bound), the ranking statistic, and the statistics asked for.
    let cases: Vec<(&str, Value, Option<&str>)> = vec![
        ("the smallest contributor count", json!({"MaxContributorCount": 1}), None),
        ("the largest contributor count", json!({"MaxContributorCount": 100}), None),
        (
            "a contributor count below the range",
            json!({"MaxContributorCount": 0}),
            Some("InvalidParameterValueException"),
        ),
        (
            "a negative contributor count",
            json!({"MaxContributorCount": -1}),
            Some("InvalidParameterValueException"),
        ),
        (
            "a contributor count above the range",
            json!({"MaxContributorCount": 101}),
            Some("InvalidParameterValueException"),
        ),
        ("the Sum ranking", json!({"OrderBy": "Sum"}), None),
        ("the Maximum ranking", json!({"OrderBy": "Maximum"}), None),
        (
            "a ranking the model does not name",
            json!({"OrderBy": "Average"}),
            Some("InvalidParameterValueException"),
        ),
        (
            "a ranking differing from a named one in case",
            json!({"OrderBy": "sum"}),
            Some("InvalidParameterValueException"),
        ),
        (
            "every statistic this operation documents",
            json!({"Metrics": [
                "UniqueContributors", "MaxContributorValue", "SampleCount", "Sum", "Minimum",
                "Maximum", "Average",
            ]}),
            None,
        ),
        (
            "a statistic this operation does not document",
            json!({"Metrics": ["p99"]}),
            Some("InvalidParameterValueException"),
        ),
        (
            "a documented statistic beside one that is not",
            json!({"Metrics": ["Sum", "SampleCount2"]}),
            Some("InvalidParameterValueException"),
        ),
    ];
    for (case, extra, expected_error) in cases {
        let mut request = report_request("refusal-rule", 60, 60, &[]);
        for (member, value) in extra.as_object().expect("the row is an object") {
            request[member.as_str()] = value.clone();
        }
        let (status, body) = call(&router, "GetInsightRuleReport", &request).await;
        match expected_error {
            None => assert_eq!(
                status, 200,
                "case {case:?}: expected the value to be accepted, got {status} {body}"
            ),
            Some(code) => assert_eq!(
                (status, error_code(&body).as_str()),
                (400, code),
                "case {case:?}: expected a 400 {code}, got {status} {body}"
            ),
        }
    }

    // A rule name this store holds no rule for is refused as a bad value for the member that
    // carried it, in the coded form this service states a single-member refusal in. Rows differ
    // along how the name misses.
    for (case, name) in [
        ("a name nothing was created under", "absent-rule".to_string()),
        ("a stored name in another case", "REFUSAL-RULE".to_string()),
        ("a stored name with a suffix", "refusal-rule-2".to_string()),
        ("the longest name the model allows", "n".repeat(128)),
    ] {
        let (status, body) = call(
            &router,
            "GetInsightRuleReport",
            &report_request(&name, 60, 60, &[]),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidParameterValueException"),
            "case {case:?}: expected a 400 InvalidParameterValueException, got {status} {body}"
        );
        assert_eq!(
            error_message(&body),
            format!("RULE_NAME: {name}"),
            "case {case:?}: expected the service's own coded refusal, got {body}"
        );
    }

    // A rule deleted after it collected is gone with everything it collected.
    let (router, engine) = build_router_with_engine();
    given_rule(&router, "deleted-rule", &definition(json!({})), None).await;
    ingest(
        &engine,
        GROUP,
        &[(1, access_log(json!({"ip": "203.0.113.42"})))],
    );
    ok(
        &router,
        "DeleteInsightRules",
        &json!({"RuleNames": ["deleted-rule"]}),
    )
    .await;
    let (status, body) = call(
        &router,
        "GetInsightRuleReport",
        &report_request("deleted-rule", 60, 60, &[]),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "InvalidParameterValueException"),
        "case \"a rule created and then deleted\": expected a 400, got {status} {body}"
    );
    // Re-created under the same name, it starts collecting over: "If you create a rule, delete
    // it, and then re-create it with the same name, historical data from the first time the
    // rule was created might not be available." (`PutInsightRule`)
    given_rule(&router, "deleted-rule", &definition(json!({})), None).await;
    let report = ok(
        &router,
        "GetInsightRuleReport",
        &report_request("deleted-rule", 60, 60, &["SampleCount"]),
    )
    .await;
    assert_eq!(
        reported_contributors(&report),
        Vec::new(),
        "case \"a rule re-created under a deleted name\": expected no contributor, got {report}"
    );
}

#[tokio::test]
async fn an_update_that_changes_the_definition_starts_the_rule_collecting_over() {
    let (router, engine) = build_router_with_engine();
    let keyed_on_ip = definition(json!({}));
    given_rule(&router, "updated-rule", &keyed_on_ip, None).await;
    ingest(
        &engine,
        GROUP,
        &[
            (1, access_log(json!({"ip": "203.0.113.42", "userAgent": "curl"}))),
            (2, access_log(json!({"ip": "203.0.113.42", "userAgent": "curl"}))),
        ],
    );

    // (case, the definition the update carries, the contributors the report must then list)
    let updates: Vec<(&str, String, ReportedContributors)> = vec![
        (
            "a put that leaves the definition as it was",
            keyed_on_ip.clone(),
            vec![(vec!["203.0.113.42".to_string()], 2.0)],
        ),
        (
            "a put that changes which field the rule keys on",
            definition(json!({"Contribution": {"Keys": ["$.userAgent"]}})),
            Vec::new(),
        ),
    ];
    for (case, rule_definition, expected) in updates {
        given_rule(&router, "updated-rule", &rule_definition, None).await;
        let report = ok(
            &router,
            "GetInsightRuleReport",
            &report_request("updated-rule", 60, 60, &["SampleCount"]),
        )
        .await;
        assert_eq!(
            reported_contributors(&report),
            expected,
            "case {case:?}: expected the contributors {expected:?}, got {report}"
        );
    }

    // The rule collects again under its new definition.
    ingest(
        &engine,
        GROUP,
        &[(3, access_log(json!({"ip": "203.0.113.42", "userAgent": "curl"})))],
    );
    let report = ok(
        &router,
        "GetInsightRuleReport",
        &report_request("updated-rule", 60, 60, &["SampleCount"]),
    )
    .await;
    assert_eq!(
        reported_contributors(&report),
        vec![(vec!["curl".to_string()], 1.0)],
        "case \"after the update\": the rule collects under its new keys, got {report}"
    );
}

#[tokio::test]
async fn a_report_never_changes_what_a_later_caller_sees() {
    let (router, engine) = build_router_with_engine();
    given_rule(&router, "read-only-rule", &definition(json!({})), None).await;
    ingest(
        &engine,
        GROUP,
        &[
            (1, access_log(json!({"ip": "203.0.113.42"}))),
            (2, access_log(json!({"ip": "198.51.100.7"}))),
        ],
    );

    // The observations the store holds before and after a run of reports over every window and
    // ranking a caller could ask for.
    let observations = |engine: &Arc<engine::InMemoryEngine>| -> BTreeMap<String, usize> {
        cloudwatch::service_state(&**engine)
            .expect("the CloudWatch store exists")
            .lock()
            .insight_rules
            .iter()
            .map(|(name, rule)| (name.clone(), rule.observations.len()))
            .collect()
    };
    let before = observations(&engine);
    for (period, window, order_by) in [
        (60, 60, "Sum"),
        (60, 3_600, "Maximum"),
        (86_400, 86_400, "Sum"),
        (600, 1_200, "Maximum"),
    ] {
        let mut request = report_request("read-only-rule", period, window, &["SampleCount"]);
        request["OrderBy"] = json!(order_by);
        ok(&router, "GetInsightRuleReport", &request).await;
    }
    let after = observations(&engine);
    assert_eq!(
        after, before,
        "case \"a run of reports\": a report must leave the rule's observations alone, expected \
         {before:?}, got {after:?}"
    );
}
