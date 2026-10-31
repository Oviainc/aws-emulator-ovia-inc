// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the WIRE FORM of the Contributor Insights read models: what
//! `DescribeInsightRules` reports as a rule's `Schema`, and how `GetInsightRuleReport` names
//! its aggregation and lays out its series.
//!
//! The rules under test:
//!
//! 1. **`InsightRule$Schema` is a string naming the schema the rule is written against — the
//!    schema's name and version joined by `/` — so a rule a caller created reports
//!    `CloudWatchLogRule/1` and a rule `PutManagedInsightRules` enabled reports
//!    `ServiceLogRule/1`, on every read path and for every rule a page holds.** A client tells
//!    a rule it wrote from a rule the service manages by the name before the `/`, which the
//!    JSON object text that DEFINES the schema would never match.
//! 2. **A report names its aggregation by the per-datapoint statistic that aggregation
//!    computes** — the occurrence count (`SampleCount`) for a rule aggregating on counts and
//!    the total (`Sum`) for one summing the field its contribution names — rather than by the
//!    keyword the definition spells it with.
//! 3. **A report's series holds one datapoint per `Period` of `[StartTime, EndTime)`**, each
//!    carrying exactly the statistics the request asked for, a period holding no observation
//!    reporting each of them at zero.
//!
//! The rows differ along what the rules generalise over: how a rule was created (by
//! `PutInsightRule` or by `PutManagedInsightRules`), how many rules a read reports and how they
//! are paged, what the definition aggregates on, whether the rule has collected anything, and
//! the `Period` and window the report is asked over. The per-rule FOLD itself — which
//! contributor an event belongs to and what each statistic of a period comes to — is tabled in
//! the `insight_rule_report` suite.

use crate::harness::{build_router, build_router_with_engine, ok, publish_managed_rule_template};
use serde_json::{json, Value};
use service_com_amazonaws_cloudwatch_graniteserviceversion20100801::state as cloudwatch;
use smithy_server_runtime::MultiServiceRouter;

/// The instant the report windows below are measured from: a whole minute AND a whole five
/// minutes past the epoch, so each datapoint's anchored timestamp is predictable for every
/// period a row asks for.
const ANCHOR_SECS: i64 = 1_767_225_600;

/// The log group the rules below read.
const GROUP: &str = "/app/access-logs";

/// The schema NAMES a Contributor Insights rule can be written against, with the kind of rule
/// each belongs to: "For rules that you create, this is always `{"Name": "CloudWatchLogRule",
/// "Version": 1}`. For managed rules, this is `{"Name": "ServiceLogRule", "Version": 1}`"
/// (`smithy_query(query="doc",shape="InsightRule",member="Schema")`).
const SCHEMA_NAMES: [&str; 2] = ["CloudWatchLogRule", "ServiceLogRule"];

/// The version every one of those schemas is at. See [`SCHEMA_NAMES`].
const SCHEMA_VERSION: &str = "1";

/// The `Schema` a rule a caller created reports.
const CREATED_RULE_SCHEMA: &str = "CloudWatchLogRule/1";

/// The `Schema` a managed rule reports.
const MANAGED_RULE_SCHEMA: &str = "ServiceLogRule/1";

/// A rule definition in the Contributor Insights rule syntax, with `overrides` merged over a
/// `JSON` rule over one log group keyed on one field.
fn definition(overrides: Value) -> String {
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

/// Create the rule `name` with `rule_definition`.
async fn given_rule(router: &MultiServiceRouter, name: &str, rule_definition: &str) {
    ok(
        router,
        "PutInsightRule",
        &json!({"RuleName": name, "RuleDefinition": rule_definition}),
    )
    .await;
}

/// Enable the managed rule of `template` for `resource_arn`, and answer the name the rule the
/// service created for that pair carries.
async fn given_managed_rule(
    router: &MultiServiceRouter,
    engine: &engine::InMemoryEngine,
    template: &str,
    resource_arn: &str,
) -> String {
    publish_managed_rule_template(engine, template, resource_arn);
    let enabled = ok(
        router,
        "PutManagedInsightRules",
        &json!({
            "ManagedRules": [{"TemplateName": template, "ResourceARN": resource_arn}],
        }),
    )
    .await;
    assert_eq!(
        enabled.get("Failures").and_then(Value::as_array).map(Vec::len),
        Some(0),
        "case \"a managed rule\": enabling one managed rule must report no failure, got {enabled}"
    );
    cloudwatch::managed_insight_rule_name(template, resource_arn)
}

/// Ingest log events into `GROUP`, through the SAME seam the CloudWatch Logs side of this
/// emulator calls when a `PutLogEvents` batch is stored: each event is `(seconds after
/// [`ANCHOR_SECS`], message)`.
fn ingest(engine: &engine::InMemoryEngine, events: &[(i64, String)]) {
    let stored: Vec<(i64, String)> = events
        .iter()
        .map(|(offset_secs, message)| ((ANCHOR_SECS + offset_secs) * 1_000, message.clone()))
        .collect();
    cloudwatch::insight_rules::record_log_events(engine, GROUP, &stored);
}

/// The rules `DescribeInsightRules` reports, page by page, as `(Name, Schema)` pairs in the
/// order they were reported, with `max_results` rules per page when the row names a size.
async fn paged_schemas(
    router: &MultiServiceRouter,
    max_results: Option<usize>,
) -> Vec<(String, String)> {
    let mut reported: Vec<(String, String)> = Vec::new();
    let mut next_token: Option<String> = None;
    loop {
        let mut request = json!({});
        if let Some(size) = max_results {
            request["MaxResults"] = json!(size);
        }
        if let Some(token) = &next_token {
            request["NextToken"] = json!(token);
        }
        let page = ok(router, "DescribeInsightRules", &request).await;
        let rules = page
            .get("InsightRules")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        reported.extend(
            rules
                .iter()
                .map(|rule| (member(rule, "Name"), member(rule, "Schema"))),
        );
        next_token = page
            .get("NextToken")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        if next_token.is_none() {
            return reported;
        }
        assert!(
            reported.len() < 100,
            "paging must terminate, got {reported:?}"
        );
    }
}

/// One string member of a reported shape, or the empty string when it carries none.
fn member(value: &Value, name: &str) -> String {
    value
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// A `GetInsightRuleReport` request over `[ANCHOR_SECS, ANCHOR_SECS + window_secs)`.
fn report_request(name: &str, period: i64, window_secs: i64, metrics: &[&str]) -> Value {
    json!({
        "RuleName": name,
        "StartTime": ANCHOR_SECS,
        "EndTime": ANCHOR_SECS + window_secs,
        "Period": period,
        "Metrics": metrics,
    })
}

/// The datapoints a report's series carries.
fn datapoints(report: &Value) -> Vec<Value> {
    report
        .get("MetricDatapoints")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

#[tokio::test]
async fn every_reported_rule_carries_the_compact_schema_of_its_kind() {
    // Rows differ along how many rules the store holds, how they were created, and how large a
    // page the read asks for — including the single-rule page the model's lower bound allows,
    // which is the first page of a paginated read, and no bound at all.
    let resource_arn = "arn:aws:dynamodb:us-east-1:123456789012:table/Orders";
    for (case, created, managed, page_size) in [
        ("one rule a caller created, read in one page", 1, false, None),
        ("several rules in one page", 3, false, None),
        ("the first page of a paginated read", 3, false, Some(1)),
        ("two rules in a page of two", 2, false, Some(2)),
        ("a managed rule beside the rules a caller created", 2, true, None),
        ("a managed rule on the paginated path", 2, true, Some(1)),
        ("a managed rule alone", 0, true, None),
    ] {
        let (router, engine) = build_router_with_engine();
        let mut expected: Vec<(String, String)> = Vec::new();
        for index in 0..created {
            let name = format!("wire-rule-{index}");
            given_rule(&router, &name, &definition(json!({}))).await;
            expected.push((name, CREATED_RULE_SCHEMA.to_string()));
        }
        if managed {
            let name = given_managed_rule(&router, &engine, "ServiceLogRule", resource_arn).await;
            expected.push((name, MANAGED_RULE_SCHEMA.to_string()));
        }
        expected.sort();

        let reported = paged_schemas(&router, page_size).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: expected each rule's Schema as the compact schema of its kind, got \
             {reported:?}"
        );
        // The form itself: a schema the member carries is the schema's NAME and VERSION joined
        // by `/`, never the JSON object text that defines it.
        for (name, schema) in &reported {
            let (schema_name, version) = schema.split_once('/').unwrap_or_default();
            assert!(
                SCHEMA_NAMES.contains(&schema_name) && version == SCHEMA_VERSION,
                "case {case:?}: rule {name:?} must report one of {SCHEMA_NAMES:?} at version \
                 {SCHEMA_VERSION}, got {schema:?}"
            );
            assert!(
                !schema.contains('{'),
                "case {case:?}: rule {name:?} must report the schema and not the document that \
                 defines it, got {schema:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_report_names_its_aggregation_by_the_statistic_that_aggregation_computes() {
    // Rows differ along what the definition aggregates on — the two values the syntax names and
    // the omitted member, which counts — and whether the rule has collected anything, so both
    // the idle and the exercised report path are covered.
    let summing = json!({
        "Contribution": {"Keys": ["$.ip"], "ValueOf": "$.requestBytes"},
        "AggregateOn": "Sum",
    });
    let cases: Vec<(&str, Value, &str)> = vec![
        ("a definition that counts occurrences", json!({"AggregateOn": "Count"}), "SampleCount"),
        ("a definition naming no aggregation, which counts", json!({}), "SampleCount"),
        ("a definition summing the field its contribution names", summing, "Sum"),
    ];
    for (index, (case, overrides, expected)) in cases.into_iter().enumerate() {
        for (traffic, events) in [
            ("no contributor traffic", Vec::new()),
            (
                "traffic the rule evaluated",
                vec![(1, json!({"ip": "203.0.113.42", "requestBytes": 128}).to_string())],
            ),
        ] {
            let case = format!("{case} / {traffic}");
            let (router, engine) = build_router_with_engine();
            let name = format!("aggregation-rule-{index}");
            given_rule(&router, &name, &definition(overrides.clone())).await;
            ingest(&engine, &events);

            let report = ok(
                &router,
                "GetInsightRuleReport",
                &report_request(&name, 60, 60, &["SampleCount"]),
            )
            .await;
            assert_eq!(
                report.get("AggregationStatistic").and_then(Value::as_str),
                Some(expected),
                "case {case:?}: expected AggregationStatistic {expected:?}, got {report}"
            );
        }
    }

    // A managed rule is enabled by the service rather than written by a caller, and it counts
    // too: "The rules can be enabled, disabled, and deleted using EnableInsightRules,
    // DisableInsightRules, and DeleteInsightRules." (`PutManagedInsightRules`) — so it answers
    // for a report like any other rule of this account.
    let (router, engine) = build_router_with_engine();
    let name = given_managed_rule(
        &router,
        &engine,
        "ServiceLogRule",
        "arn:aws:dynamodb:us-east-1:123456789012:table/Orders",
    )
    .await;
    let report = ok(
        &router,
        "GetInsightRuleReport",
        &report_request(&name, 60, 60, &["SampleCount"]),
    )
    .await;
    assert_eq!(
        report.get("AggregationStatistic").and_then(Value::as_str),
        Some("SampleCount"),
        "case \"a managed rule\": expected the occurrence-count statistic, got {report}"
    );
}

#[tokio::test]
async fn the_series_spans_every_period_of_the_window_asked_about() {
    // Rows differ along the period the datapoints are aggregated over, how many periods the
    // window spans, and which statistics the request asks for — one period, the twelve a
    // five-minute period spans an hour in, an hour of one-minute periods, and the widest period
    // a report may be asked at.
    let cases: Vec<(&str, i64, i64, Vec<&str>)> = vec![
        ("one period, one statistic", 60, 60, vec!["UniqueContributors"]),
        ("twelve five-minute periods of an hour", 300, 3_600, vec!["UniqueContributors"]),
        ("an hour of one-minute periods", 60, 3_600, vec!["SampleCount", "Sum"]),
        (
            "every statistic this operation documents",
            600,
            3_600,
            vec![
                "UniqueContributors",
                "MaxContributorValue",
                "SampleCount",
                "Sum",
                "Minimum",
                "Maximum",
                "Average",
            ],
        ),
        ("the widest period a report may be asked at", 86_400, 172_800, vec!["SampleCount"]),
    ];
    for (index, (case, period, window_secs, metrics)) in cases.into_iter().enumerate() {
        // The idle rule: a window of periods none of which holds an observation.
        let (router, engine) = build_router_with_engine();
        let idle = format!("idle-rule-{index}");
        given_rule(&router, &idle, &definition(json!({}))).await;
        let report = ok(
            &router,
            "GetInsightRuleReport",
            &report_request(&idle, period, window_secs, &metrics),
        )
        .await;
        let series = datapoints(&report);
        let expected_points = usize::try_from(window_secs / period).expect("a short window");
        assert_eq!(
            series.len(),
            expected_points,
            "case {case:?}: expected one datapoint per period of the window, {expected_points} \
             of them, got {report}"
        );
        for (step, point) in series.iter().enumerate() {
            let at = i64::try_from(step).expect("a short window") * period;
            #[allow(clippy::cast_precision_loss)]
            let expected_timestamp = (ANCHOR_SECS + at) as f64;
            assert_eq!(
                point.get("Timestamp").and_then(Value::as_f64),
                Some(expected_timestamp),
                "case {case:?}: expected the datapoint at {at} seconds into the window anchored \
                 at {expected_timestamp}, got {point}"
            );
            for metric in &metrics {
                assert_eq!(
                    point.get(*metric).and_then(Value::as_f64),
                    Some(0.0),
                    "case {case:?}: a period holding no observation reports {metric} at zero, \
                     got {point}"
                );
            }
            // Only the statistics the request asked for are carried.
            for absent in ["UniqueContributors", "SampleCount", "Sum", "Average"] {
                assert_eq!(
                    point.get(absent).is_some(),
                    metrics.contains(&absent),
                    "case {case:?}: {absent} is carried exactly when it was asked for, got \
                     {point}"
                );
            }
        }

        // The same window for a rule that collected one event in its first period: the series
        // is the same length, and the observation is reported in the period it fell in.
        let exercised = format!("exercised-rule-{index}");
        given_rule(&router, &exercised, &definition(json!({}))).await;
        ingest(
            &engine,
            &[(0, json!({"ip": "203.0.113.42"}).to_string())],
        );
        let report = ok(
            &router,
            "GetInsightRuleReport",
            &report_request(&exercised, period, window_secs, &["UniqueContributors"]),
        )
        .await;
        let series = datapoints(&report);
        assert_eq!(
            series.len(),
            expected_points,
            "case {case:?}: an exercised rule reports the same periods, got {report}"
        );
        let unique: Vec<Option<f64>> = series
            .iter()
            .map(|point| point.get("UniqueContributors").and_then(Value::as_f64))
            .collect();
        let mut expected_unique = vec![Some(0.0); expected_points];
        expected_unique[0] = Some(1.0);
        assert_eq!(
            unique, expected_unique,
            "case {case:?}: expected the contributor in the first period only, got {unique:?} \
             in {report}"
        );
    }
}

#[tokio::test]
async fn a_window_that_is_not_period_aligned_opens_on_the_period_it_starts_in() {
    // A datapoint is anchored at a multiple of the period, so a window whose StartTime is not
    // opens on the period that instant falls in — which is the period an observation at
    // StartTime is bucketed into. Rows differ along how far into its period the window starts
    // and along the period itself.
    let (router, _engine) = build_router_with_engine();
    given_rule(&router, "unaligned-rule", &definition(json!({}))).await;
    for (case, offset_secs, period, window_secs) in [
        ("a window starting on a period boundary", 0, 60, 120),
        ("a window starting half a period in", 30, 60, 120),
        ("a window starting one second before the next period", 59, 60, 60),
        ("a window starting one second into a five-minute period", 1, 300, 600),
        ("a window starting inside the narrowest period a report allows", 7, 60, 120),
    ] {
        let start_secs = ANCHOR_SECS + offset_secs;
        let end_secs = start_secs + window_secs;
        let report = ok(
            &router,
            "GetInsightRuleReport",
            &json!({
                "RuleName": "unaligned-rule",
                "StartTime": start_secs,
                "EndTime": end_secs,
                "Period": period,
                "Metrics": ["SampleCount"],
            }),
        )
        .await;
        let timestamps: Vec<f64> = datapoints(&report)
            .iter()
            .filter_map(|point| point.get("Timestamp").and_then(Value::as_f64))
            .collect();
        // The periods of [StartTime, EndTime), counted from the instant the series opens at.
        let anchored_start = start_secs - start_secs.rem_euclid(period);
        let expected_points = usize::try_from((end_secs - anchored_start + period - 1) / period)
            .expect("a short window");
        assert_eq!(
            timestamps.len(),
            expected_points,
            "case {case:?}: expected {expected_points} datapoints over the window's periods, got \
             {timestamps:?} in {report}"
        );
        #[allow(clippy::cast_precision_loss)]
        let expected_first = anchored_start as f64;
        assert_eq!(
            timestamps.first().copied(),
            Some(expected_first),
            "case {case:?}: the series must open on the period StartTime falls in, got \
             {timestamps:?}"
        );
        #[allow(clippy::cast_precision_loss)]
        let window_end = end_secs as f64;
        #[allow(clippy::cast_precision_loss)]
        let period_width = period as f64;
        let last = timestamps.last().copied().unwrap_or(f64::NAN);
        assert!(
            last < window_end && window_end <= last + period_width,
            "case {case:?}: the series must end on the last period before {window_end}, got \
             {last}"
        );
    }
}

#[tokio::test]
async fn a_rule_updated_through_put_insight_rule_still_reports_the_created_schema() {
    // A rule's schema is the one the KIND of rule is written against, so neither a state change
    // nor a new definition moves it: `PutInsightRule` requires the definition to declare the
    // CloudWatchLogRule schema, and the rule reports that schema whatever else the definition
    // says.
    let router = build_router();
    given_rule(&router, "updated-rule", &definition(json!({}))).await;
    for operation in ["EnableInsightRules", "DisableInsightRules"] {
        ok(&router, operation, &json!({"RuleNames": ["updated-rule"]})).await;
        let reported = paged_schemas(&router, None).await;
        assert_eq!(
            reported,
            vec![("updated-rule".to_string(), CREATED_RULE_SCHEMA.to_string())],
            "case {operation:?}: a state change must leave the rule's Schema alone, got \
             {reported:?}"
        );
    }
    given_rule(
        &router,
        "updated-rule",
        &definition(json!({"Contribution": {"Keys": ["$.userAgent"]}})),
    )
    .await;
    let reported = paged_schemas(&router, None).await;
    assert_eq!(
        reported,
        vec![("updated-rule".to_string(), CREATED_RULE_SCHEMA.to_string())],
        "case \"an updated definition\": the rule still reports the schema of its kind, got \
         {reported:?}"
    );
}
