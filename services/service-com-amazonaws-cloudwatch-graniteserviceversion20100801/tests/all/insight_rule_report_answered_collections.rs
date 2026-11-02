// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for WHICH MEMBERS a `GetInsightRuleReport` success answer carries.
//!
//! The rule under test: **every collection a report reports is ANSWERED on the 200 — present
//! and carrying an empty collection when the report has nothing in it, never left out — so a
//! client can read, length and iterate each one and tell "the report has none" from "the
//! service did not answer this".** That covers the rule-attribute series, which no request this
//! operation accepts asks for and which is therefore always empty, as much as the key labels,
//! the contributors and the datapoint series, which a report may or may not have entries for.
//!
//! The rows differ along what the rule generalises over: how the rule was created (by
//! `PutInsightRule` or by `PutManagedInsightRules`), what its definition aggregates on, whether
//! it is enabled, whether it has evaluated any log event, the `Period` and the window the report
//! is asked over, which statistics `Metrics` asks for (none at all, the member omitted, one, and
//! every statistic the operation documents), how many contributors are asked for and how they
//! are ranked. The recorded case — an idle rule over several periods with no statistic asked
//! for — is one row among them.
//!
//! It also pins that the ANCHORING a report's buckets are computed with is total over every
//! instant a request can name: the window a caller asks about reaches the bucket arithmetic
//! unchanged, so an instant outside the epoch seconds the arithmetic holds is refused for the
//! series it asks for rather than met with an overflow.
//!
//! The log events are evaluated through the SAME entry point the CloudWatch Logs side of this
//! emulator calls on ingest (`state::insight_rules::record_log_events`).

use crate::harness::{build_router_with_engine, call, error_code, ok, publish_managed_rule_template};
use serde_json::{json, Value};
use service_com_amazonaws_cloudwatch_graniteserviceversion20100801::state as cloudwatch;
use smithy_server_runtime::MultiServiceRouter;

/// The instant the windows below are measured from: a whole minute AND a whole five minutes past
/// the epoch, so every period a row asks for divides the window the same way.
const ANCHOR_SECS: i64 = 1_767_225_600;

/// The log group the rules below read.
const GROUP: &str = "/app/access-logs";

/// The resource a managed rule is enabled for.
const MANAGED_RESOURCE_ARN: &str = "arn:aws:dynamodb:us-east-1:123456789012:table/Orders";

/// The template name of the managed rule a row enables.
const MANAGED_TEMPLATE: &str = "ServiceLogRule";

/// Every member of `GetInsightRuleReportOutput` whose target is a LIST, from
/// `smithy_query(query="shape",shape="GetInsightRuleReportOutput")` on this service's model:
/// the rule-attribute series, the key labels, the contributors and the datapoint series. These
/// are the members the rule under test is about — each one answered, empty rather than absent
/// when the report has nothing for it.
const COLLECTION_MEMBERS: [&str; 4] = [
    "RuleAttributes",
    "KeyLabels",
    "Contributors",
    "MetricDatapoints",
];

/// The statistics `Metrics` may ask for, as this operation documents them
/// (`smithy_query(query="doc",shape="GetInsightRuleReportInput",member="Metrics")`).
const DOCUMENTED_METRICS: [&str; 7] = [
    "UniqueContributors",
    "MaxContributorValue",
    "SampleCount",
    "Sum",
    "Minimum",
    "Maximum",
    "Average",
];

/// One report a row asks for, and the rule it is asked about.
struct Row {
    /// What this row is, named in every assertion.
    case: &'static str,
    /// Members merged over the base rule definition.
    overrides: Value,
    /// The state the rule is put in after it is created, when the row names one.
    state: Option<&'static str>,
    /// Whether the rule is a managed rule the service enabled rather than one a caller wrote.
    managed: bool,
    /// The log events ingested into [`GROUP`], as `(seconds after [`ANCHOR_SECS`], message)`.
    events: Vec<(i64, String)>,
    /// The period the datapoints are aggregated over.
    period: i64,
    /// How wide the reported window is.
    window_secs: i64,
    /// What `Metrics` asks for: `None` leaves the member out of the request entirely.
    metrics: Option<Vec<&'static str>>,
    /// What `MaxContributorCount` asks for, when the row names one.
    max_contributor_count: Option<i64>,
    /// What `OrderBy` ranks the contributors by, when the row names one.
    order_by: Option<&'static str>,
}

/// The base row: an enabled rule a caller wrote, with no traffic, reported over an hour of
/// five-minute periods with no statistic asked for — the recorded shape of the request.
fn row() -> Row {
    Row {
        case: "",
        overrides: json!({}),
        state: None,
        managed: false,
        events: Vec::new(),
        period: 300,
        window_secs: 3_600,
        metrics: Some(Vec::new()),
        max_contributor_count: None,
        order_by: None,
    }
}

/// A rule definition in the Contributor Insights rule syntax, with `overrides` merged over a
/// `JSON` rule over one log group keyed on one field.
fn definition(overrides: &Value) -> String {
    let mut rule = json!({
        "Schema": {"Name": "CloudWatchLogRule", "Version": 1},
        "LogGroupNames": [GROUP],
        "LogFormat": "JSON",
        "Contribution": {"Keys": ["$.ip"]},
    });
    let members = rule.as_object_mut().expect("the base rule is an object");
    for (member, value) in overrides.as_object().expect("the overrides are an object") {
        members.insert(member.clone(), value.clone());
    }
    serde_json::to_string(&rule).expect("the definition serializes")
}

/// A contributor's log event: one JSON record carrying the keyed field and a value to sum.
fn event(ip: &str, request_bytes: i64) -> String {
    json!({"ip": ip, "requestBytes": request_bytes}).to_string()
}

/// Stand the row's rule up and answer the report it asks for.
async fn reported(router: &MultiServiceRouter, engine: &engine::InMemoryEngine, row: &Row) -> Value {
    let name = if row.managed {
        publish_managed_rule_template(engine, MANAGED_TEMPLATE, MANAGED_RESOURCE_ARN);
        ok(
            router,
            "PutManagedInsightRules",
            &json!({
                "ManagedRules": [{
                    "TemplateName": MANAGED_TEMPLATE,
                    "ResourceARN": MANAGED_RESOURCE_ARN,
                }],
            }),
        )
        .await;
        cloudwatch::managed_insight_rule_name(MANAGED_TEMPLATE, MANAGED_RESOURCE_ARN)
    } else {
        let name = "answered-collections-rule".to_string();
        ok(
            router,
            "PutInsightRule",
            &json!({"RuleName": name, "RuleDefinition": definition(&row.overrides)}),
        )
        .await;
        name
    };
    if let Some(state) = row.state {
        ok(router, state, &json!({"RuleNames": [name]})).await;
    }
    let stored: Vec<(i64, String)> = row
        .events
        .iter()
        .map(|(offset_secs, message)| ((ANCHOR_SECS + offset_secs) * 1_000, message.clone()))
        .collect();
    cloudwatch::insight_rules::record_log_events(engine, GROUP, &stored);

    let mut request = json!({
        "RuleName": name,
        "StartTime": ANCHOR_SECS,
        "EndTime": ANCHOR_SECS + row.window_secs,
        "Period": row.period,
    });
    if let Some(metrics) = &row.metrics {
        request["Metrics"] = json!(metrics);
    }
    if let Some(count) = row.max_contributor_count {
        request["MaxContributorCount"] = json!(count);
    }
    if let Some(order_by) = row.order_by {
        request["OrderBy"] = json!(order_by);
    }
    ok(router, "GetInsightRuleReport", &request).await
}

/// The rows the rule is asserted over.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "an idle rule over an hour of five-minute periods, no statistic asked for",
            ..row()
        },
        Row {
            case: "the member omitted from the request instead of asked for empty",
            metrics: None,
            ..row()
        },
        Row {
            case: "one statistic over one one-minute period",
            period: 60,
            window_secs: 60,
            metrics: Some(vec!["SampleCount"]),
            ..row()
        },
        Row {
            case: "every statistic the operation documents",
            metrics: Some(DOCUMENTED_METRICS.to_vec()),
            ..row()
        },
        Row {
            case: "a rule that evaluated events from several contributors",
            events: vec![
                (0, event("203.0.113.42", 128)),
                (30, event("203.0.113.42", 256)),
                (900, event("198.51.100.7", 64)),
            ],
            metrics: Some(vec!["UniqueContributors", "SampleCount"]),
            ..row()
        },
        Row {
            case: "a rule summing the field its contribution names, ranked by Maximum",
            overrides: json!({
                "Contribution": {"Keys": ["$.ip"], "ValueOf": "$.requestBytes"},
                "AggregateOn": "Sum",
            }),
            events: vec![(0, event("203.0.113.42", 128)), (60, event("198.51.100.7", 4))],
            metrics: Some(vec!["Sum", "Maximum"]),
            max_contributor_count: Some(1),
            order_by: Some("Maximum"),
            ..row()
        },
        Row {
            case: "a rule keyed on two fields, with every contributor asked for",
            overrides: json!({"Contribution": {"Keys": ["$.ip", "$.requestBytes"]}}),
            events: vec![(0, event("203.0.113.42", 128)), (0, event("203.0.113.42", 256))],
            max_contributor_count: Some(100),
            ..row()
        },
        Row {
            case: "a disabled rule, which collects nothing while it is disabled",
            state: Some("DisableInsightRules"),
            events: vec![(0, event("203.0.113.42", 128))],
            ..row()
        },
        Row {
            case: "a rule disabled and enabled again",
            state: Some("EnableInsightRules"),
            events: vec![(0, event("203.0.113.42", 128))],
            metrics: Some(vec!["Average"]),
            ..row()
        },
        Row {
            case: "a managed rule the service enabled",
            managed: true,
            ..row()
        },
        Row {
            case: "the widest period a report may be asked at",
            period: 86_400,
            window_secs: 172_800,
            metrics: Some(vec!["SampleCount"]),
            ..row()
        },
    ]
}

#[tokio::test]
async fn every_collection_a_report_reports_is_answered_rather_than_left_out() {
    for row in rows() {
        let case = row.case;
        let (router, engine) = build_router_with_engine();
        let report = reported(&router, &engine, &row).await;
        for member in COLLECTION_MEMBERS {
            let answered = report.get(member);
            assert!(
                answered.is_some_and(Value::is_array),
                "case {case:?}: {member} must be answered as an array, got {answered:?} in \
                 {report}"
            );
        }
        // The rule-attribute series: `Metrics` is the only member of the request that asks for
        // anything beyond the contributor series, and every value it may carry names a
        // per-datapoint statistic, so no request this operation serves asks for a rule
        // attribute — which is an EMPTY series and not an absent member.
        assert_eq!(
            report.get("RuleAttributes"),
            Some(&json!([])),
            "case {case:?}: a report nothing asked a rule attribute of carries the empty \
             series, got {report}"
        );
    }
}

#[tokio::test]
async fn an_answered_empty_collection_is_the_report_having_none() {
    // The same member, on a report that HAS entries and on one that does not: an empty array
    // means the report found none, which is only readable because the member is answered either
    // way.
    for (case, events, expected_contributors) in [
        ("a rule with no traffic at all", Vec::new(), 0),
        (
            "a rule with one contributor",
            vec![(0, event("203.0.113.42", 128))],
            1,
        ),
        (
            "a rule with two contributors",
            vec![(0, event("203.0.113.42", 128)), (60, event("198.51.100.7", 64))],
            2,
        ),
    ] {
        let (router, engine) = build_router_with_engine();
        let report = reported(
            &router,
            &engine,
            &Row {
                case,
                events,
                metrics: Some(vec!["UniqueContributors"]),
                ..row()
            },
        )
        .await;
        let contributors = report
            .get("Contributors")
            .and_then(Value::as_array)
            .map(Vec::len);
        assert_eq!(
            contributors,
            Some(expected_contributors),
            "case {case:?}: expected {expected_contributors} contributors in an answered array, \
             got {contributors:?} in {report}"
        );
        let labels = report.get("KeyLabels").and_then(Value::as_array).cloned();
        assert_eq!(
            labels,
            Some(vec![json!("$.ip")]),
            "case {case:?}: expected the rule's keys as an answered KeyLabels array, got {report}"
        );
        assert_eq!(
            report.get("RuleAttributes"),
            Some(&json!([])),
            "case {case:?}: expected the rule-attribute series answered empty whatever the rule \
             collected, got {report}"
        );
    }
}

#[tokio::test]
async fn the_bucket_anchoring_is_total_over_every_instant_a_request_can_name() {
    // A bucket opens at a multiple of the period at or below the instant it holds. Rows differ
    // along the instant — the epoch, a whole period, part way into a period, instants before the
    // epoch, and the extremes of the domain a timestamp can carry — and along the period.
    for (case, at_secs, period) in [
        ("the epoch", 0.0_f64, 60_i64),
        ("part way into a period", 1_767_225_607.0, 60),
        ("one second before the epoch", -1.0, 60),
        ("a whole period before the epoch", -61.0, 60),
        ("the narrowest period", 1_767_225_607.9, 1),
        ("a day-wide period", 1_767_225_607.0, 86_400),
        ("an instant below the domain", f64::MIN, 60),
        ("an instant above the domain", f64::MAX, 60),
        ("an instant below the domain at a day-wide period", -9.3e18, 86_400),
    ] {
        let anchor = cloudwatch::insight_rules::period_start(at_secs, period);
        #[allow(clippy::cast_possible_truncation)]
        let secs = at_secs.floor() as i64;
        assert!(
            anchor <= secs,
            "case {case:?}: a bucket must open at or before the instant it holds, got {anchor} \
             for {secs}"
        );
        let into_period = secs.saturating_sub(anchor);
        assert!(
            into_period < period,
            "case {case:?}: the instant must fall inside the bucket that opens at {anchor}, got \
             {into_period} seconds into a period of {period}"
        );
        assert!(
            anchor == i64::MIN || anchor.rem_euclid(period) == 0,
            "case {case:?}: a bucket opens at a multiple of the period, or at the bound of the \
             domain, got {anchor}"
        );
    }
}

#[tokio::test]
async fn a_window_outside_the_domain_is_refused_and_the_service_keeps_serving() {
    // The instants reach the bucket arithmetic unchanged, so a window no series could be
    // enumerated over is REFUSED for its size — the modelled parameter fault — and the next
    // caller is served as before.
    let (router, engine) = build_router_with_engine();
    let report = reported(&router, &engine, &row()).await;
    assert_eq!(
        report.get("RuleAttributes"),
        Some(&json!([])),
        "case \"before the extreme window\": expected the answered empty series, got {report}"
    );
    for (case, start_secs, end_secs) in [
        ("a start instant below the domain", -9.3e18, 0.0_f64),
        ("an end instant above the domain", 0.0, 9.3e18),
        ("both instants outside the domain", -9.3e18, 9.3e18),
    ] {
        let (status, body) = call(
            &router,
            "GetInsightRuleReport",
            &json!({
                "RuleName": "answered-collections-rule",
                "StartTime": start_secs,
                "EndTime": end_secs,
                "Period": 60,
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the window to be refused as a parameter fault, got {status} \
             with {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValueException",
            "case {case:?}: expected the modelled parameter refusal, got {body}"
        );
    }
    let again = ok(
        &router,
        "GetInsightRuleReport",
        &json!({
            "RuleName": "answered-collections-rule",
            "StartTime": ANCHOR_SECS,
            "EndTime": ANCHOR_SECS + 3_600,
            "Period": 300,
        }),
    )
    .await;
    assert_eq!(
        again.get("RuleAttributes"),
        Some(&json!([])),
        "case \"after the extreme windows\": the next report must be answered as before, got \
         {again}"
    );
}

#[tokio::test]
async fn two_reports_of_one_window_answer_the_same_members() {
    // A report READS: asking twice answers the same body, so the answered-empty series is not
    // something the first read consumed.
    let (router, engine) = build_router_with_engine();
    let row = Row {
        case: "a report read twice",
        events: vec![(0, event("203.0.113.42", 128))],
        metrics: Some(vec!["SampleCount"]),
        ..row()
    };
    let first = reported(&router, &engine, &row).await;
    let second = ok(
        &router,
        "GetInsightRuleReport",
        &json!({
            "RuleName": "answered-collections-rule",
            "StartTime": ANCHOR_SECS,
            "EndTime": ANCHOR_SECS + row.window_secs,
            "Period": row.period,
            "Metrics": ["SampleCount"],
        }),
    )
    .await;
    assert_eq!(
        first, second,
        "case {:?}: a second report of the same window must answer the same members, got \
         {first} then {second}",
        row.case
    );
}
