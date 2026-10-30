// SPDX-License-Identifier: CC0-1.0

//! Ingesting log events into a log group a Contributor Insights rule names makes them
//! contributors — in CloudWatch.
//!
//! The rule under test: **a log event stored by `PutLogEvents` into a log group an ENABLED
//! Contributor Insights rule names is evaluated by that rule as it arrives, so
//! `GetInsightRuleReport` over a window of at least one `Period` reports one contributor per
//! distinct combination of the values the rule's `Contribution.Keys` extract, with a series of
//! `Period`-wide datapoints strictly ascending in `Timestamp`; an event ingested into a group
//! no rule names, or arriving while the rule is DISABLED, contributes nothing.** Two crates
//! have to agree for that to hold (CloudWatch Logs ingests, CloudWatch answers for the
//! report), so the suite lives here: a crate-local test can only ever see one side of it.
//!
//! The rows differ along what the rule generalises over: the log format, how many keys a
//! contributor is made of, what the rule aggregates on, whether a filter admits the event,
//! and which log group it was ingested into. The per-rule fold itself is tabled much more
//! widely in the CloudWatch crate's own `insight_rule_report` suite.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The region every request here is signed for.
const REGION: &str = "us-east-1";

/// CloudWatch Logs' `X-Amz-Target` prefix and the signing name its credential scope carries.
const LOGS_TARGET_PREFIX: &str = "Logs_20140328";
const LOGS_SIGNING_NAME: &str = "logs";

/// CloudWatch's `X-Amz-Target` prefix and signing name (`sigv4Name: monitoring`).
const CLOUDWATCH_TARGET_PREFIX: &str = "GraniteServiceVersion20100801";
const CLOUDWATCH_SIGNING_NAME: &str = "monitoring";

/// The period the reports below aggregate their datapoints over, and the window they ask about:
/// a window of several periods, so the series is one datapoint per period of it.
const PERIOD_SECS: i64 = 60;

/// The emulator with the two services this suite exercises, mounted on ONE engine exactly as
/// `core/router/src/lib.rs` mounts them.
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudwatchlogs_logs_20140328::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_cloudwatch_graniteserviceversion20100801::EmulatedService::new(
            engine,
        ),
    );
    router
}

/// The SigV4 header a request carries, naming the signing name the router routes on.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/aws4_request, \
         SignedHeaders=host;x-amz-target, Signature=0123456789abcdef"
    )
}

/// Call one operation of either service over its awsJson wire.
async fn call(
    router: &MultiServiceRouter,
    signing_name: &str,
    target_prefix: &str,
    content_type: &str,
    operation: &str,
    request: &Value,
) -> (u16, Value) {
    let http_request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header("content-type", content_type)
        .header("x-amz-target", format!("{target_prefix}.{operation}"))
        .header("authorization", authorization(signing_name))
        .body(Bytes::from(
            serde_json::to_vec(request).expect("the request serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(http_request).await;
    let status = response.status().as_u16();
    let parsed = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, parsed)
}

/// Call one CloudWatch Logs operation, requiring it to succeed.
async fn logs(router: &MultiServiceRouter, case: &str, operation: &str, request: &Value) -> Value {
    let (status, parsed) = call(
        router,
        LOGS_SIGNING_NAME,
        LOGS_TARGET_PREFIX,
        "application/x-amz-json-1.1",
        operation,
        request,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from logs {operation}, got {status} with body {parsed}"
    );
    parsed
}

/// Call one CloudWatch operation, requiring it to succeed.
async fn cloudwatch(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    request: &Value,
) -> Value {
    let (status, parsed) = call(
        router,
        CLOUDWATCH_SIGNING_NAME,
        CLOUDWATCH_TARGET_PREFIX,
        "application/x-amz-json-1.0",
        operation,
        request,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CloudWatch {operation}, got {status} with body \
         {parsed}"
    );
    parsed
}

/// Epoch seconds, the unit the report's window is expressed in.
fn now_secs() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("the clock is after the Unix epoch")
            .as_secs(),
    )
    .expect("the epoch seconds fit in an i64")
}

/// A log group and a stream inside it, both created.
async fn group_with_stream(router: &MultiServiceRouter, case: &str, group: &str, stream: &str) {
    logs(router, case, "CreateLogGroup", &json!({"logGroupName": group})).await;
    logs(
        router,
        case,
        "CreateLogStream",
        &json!({"logGroupName": group, "logStreamName": stream}),
    )
    .await;
}

/// Ingest `messages`, each stamped at its own instant, as one `PutLogEvents` batch.
async fn ingest(
    router: &MultiServiceRouter,
    case: &str,
    group: &str,
    stream: &str,
    events: &[(i64, String)],
) {
    let log_events: Vec<Value> = events
        .iter()
        .map(|(at_secs, message)| json!({"timestamp": at_secs * 1_000, "message": message}))
        .collect();
    logs(
        router,
        case,
        "PutLogEvents",
        &json!({
            "logGroupName": group,
            "logStreamName": stream,
            "logEvents": log_events,
        }),
    )
    .await;
}

/// Create a Contributor Insights rule over `group`, in `state` when the case names one.
async fn given_rule(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    definition: &Value,
    state: Option<&str>,
) {
    let mut request = json!({
        "RuleName": name,
        "RuleDefinition": definition.to_string(),
    });
    if let Some(state) = state {
        request["RuleState"] = json!(state);
    }
    cloudwatch(router, case, "PutInsightRule", &request).await;
}

/// A rule definition in the Contributor Insights rule syntax over `group`, with `contribution`
/// as its `Contribution` and the members `overrides` adds.
fn definition(group: &str, contribution: Value, overrides: Value) -> Value {
    let mut rule = json!({
        "Schema": {"Name": "CloudWatchLogRule", "Version": 1},
        "LogGroupNames": [group],
        "LogFormat": "JSON",
        "Contribution": contribution,
    });
    for (member, value) in overrides.as_object().expect("the overrides are an object") {
        rule[member.as_str()] = value.clone();
    }
    rule
}

/// The report for `rule` over the window `[start_secs, end_secs)`.
async fn report(
    router: &MultiServiceRouter,
    case: &str,
    rule: &str,
    start_secs: i64,
    end_secs: i64,
    metrics: &[&str],
) -> Value {
    cloudwatch(
        router,
        case,
        "GetInsightRuleReport",
        &json!({
            "RuleName": rule,
            "StartTime": start_secs,
            "EndTime": end_secs,
            "Period": PERIOD_SECS,
            "Metrics": metrics,
        }),
    )
    .await
}

/// What [`reported_contributors`] answers: each contributor's keys, joined, and its aggregate
/// value, in the order the report listed them.
type ReportedContributors = Vec<(String, f64)>;

/// The `(keys joined, ApproximateAggregateValue)` pairs a report lists, in order.
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
                                .filter_map(Value::as_str)
                                .collect::<Vec<_>>()
                                .join(",")
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

/// One row of the table: a rule over its own log group, the events ingested into it, and the
/// contributors the report must then list.
struct Row {
    /// What this row varies, named in every assertion.
    case: &'static str,
    /// The rule's `Contribution`.
    contribution: Value,
    /// The other definition members the row sets (`AggregateOn`, `LogFormat`).
    overrides: Value,
    /// The messages ingested, each `(seconds before the window's end, message)`.
    messages: Vec<(i64, String)>,
    /// The contributors the report must list, in order.
    contributors: Vec<(&'static str, f64)>,
}

#[tokio::test]
async fn an_ingested_log_event_contributes_to_the_rules_over_its_log_group() {
    let router = build_router();
    // The window: five periods wide and ending in the future, so every event the rows ingest
    // falls inside it whatever second the suite runs at.
    let end_secs = now_secs() + PERIOD_SECS;
    let start_secs = end_secs - 5 * PERIOD_SECS;

    let rows: Vec<Row> = vec![
        Row {
            case: "a rule keyed on one field, counting occurrences",
            contribution: json!({"Keys": ["$.ip"]}),
            overrides: json!({}),
            messages: vec![
                (90, json!({"ip": "203.0.113.42", "requestBytes": 128}).to_string()),
                (80, json!({"ip": "203.0.113.42", "requestBytes": 128}).to_string()),
                (70, json!({"ip": "203.0.113.42", "requestBytes": 128}).to_string()),
                (60, json!({"ip": "198.51.100.7", "requestBytes": 64}).to_string()),
            ],
            contributors: vec![("203.0.113.42", 3.0), ("198.51.100.7", 1.0)],
        },
        Row {
            case: "a rule keyed on two fields",
            contribution: json!({"Keys": ["$.ip", "$.httpMethod"]}),
            overrides: json!({}),
            messages: vec![
                (90, json!({"ip": "203.0.113.42", "httpMethod": "PUT"}).to_string()),
                (80, json!({"ip": "203.0.113.42", "httpMethod": "PUT"}).to_string()),
                (70, json!({"ip": "203.0.113.42", "httpMethod": "GET"}).to_string()),
            ],
            contributors: vec![("203.0.113.42,PUT", 2.0), ("203.0.113.42,GET", 1.0)],
        },
        Row {
            case: "a rule summing the field its contribution names",
            contribution: json!({"Keys": ["$.ip"], "ValueOf": "$.requestBytes"}),
            overrides: json!({"AggregateOn": "Sum"}),
            messages: vec![
                (90, json!({"ip": "203.0.113.42", "requestBytes": 100}).to_string()),
                (80, json!({"ip": "203.0.113.42", "requestBytes": 300}).to_string()),
                (70, json!({"ip": "198.51.100.7", "requestBytes": 50}).to_string()),
            ],
            contributors: vec![("203.0.113.42", 400.0), ("198.51.100.7", 50.0)],
        },
        Row {
            case: "a rule whose filter admits only some of the events",
            contribution: json!({
                "Keys": ["$.ip"],
                "Filters": [{"Match": "$.httpMethod", "In": ["PUT"]}],
            }),
            overrides: json!({}),
            messages: vec![
                (90, json!({"ip": "203.0.113.42", "httpMethod": "PUT"}).to_string()),
                (80, json!({"ip": "203.0.113.42", "httpMethod": "GET"}).to_string()),
            ],
            contributors: vec![("203.0.113.42", 1.0)],
        },
        Row {
            case: "a CLF rule keyed on a field its Fields member names",
            contribution: json!({"Keys": ["IpAddress"]}),
            overrides: json!({"LogFormat": "CLF", "Fields": {"1": "IpAddress"}}),
            messages: vec![
                (90, "203.0.113.42 - - [01/Jan/2026] 200".to_string()),
                (80, "203.0.113.42 - - [01/Jan/2026] 404".to_string()),
                (70, "192.0.2.9 - - [01/Jan/2026] 200".to_string()),
            ],
            contributors: vec![("203.0.113.42", 2.0), ("192.0.2.9", 1.0)],
        },
        Row {
            case: "a message that is not in the rule's log format",
            contribution: json!({"Keys": ["$.ip"]}),
            overrides: json!({}),
            messages: vec![(90, "ERROR something went wrong".to_string())],
            contributors: Vec::new(),
        },
    ];

    for (index, row) in rows.into_iter().enumerate() {
        let case = row.case;
        let group = format!("/insights/forms-{index}");
        let rule = format!("insights-forms-{index}");
        group_with_stream(&router, case, &group, "stream").await;
        given_rule(
            &router,
            case,
            &rule,
            &definition(&group, row.contribution, row.overrides),
            Some("ENABLED"),
        )
        .await;

        let events: Vec<(i64, String)> = row
            .messages
            .iter()
            .map(|(before_end, message)| (end_secs - before_end, message.clone()))
            .collect();
        ingest(&router, case, &group, "stream", &events).await;

        let answered = report(
            &router,
            case,
            &rule,
            start_secs,
            end_secs,
            &["UniqueContributors", "SampleCount"],
        )
        .await;
        let expected: Vec<(String, f64)> = row
            .contributors
            .iter()
            .map(|(keys, value)| ((*keys).to_string(), *value))
            .collect();
        let got = reported_contributors(&answered);
        assert_eq!(
            got, expected,
            "case {case:?}: expected the contributors {expected:?}, got {got:?} in {answered}"
        );

        // The series is strictly ascending by timestamp, with no instant repeated.
        let timestamps: Vec<f64> = answered
            .get("MetricDatapoints")
            .and_then(Value::as_array)
            .map(|points| {
                points
                    .iter()
                    .filter_map(|point| point.get("Timestamp").and_then(Value::as_f64))
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            timestamps.len(),
            answered
                .get("MetricDatapoints")
                .and_then(Value::as_array)
                .map_or(0, Vec::len),
            "case {case:?}: every datapoint carries a Timestamp, got {answered}"
        );
        assert!(
            timestamps.windows(2).all(|pair| pair[0] < pair[1]),
            "case {case:?}: the series must be strictly ascending by Timestamp, got {timestamps:?}"
        );
        // The series covers the window the request named, whether or not every period of it
        // holds an observation: it opens on the period the window starts in, ends inside the
        // window, and steps by exactly one period.
        #[allow(clippy::cast_precision_loss)]
        let period_width = PERIOD_SECS as f64;
        #[allow(clippy::cast_precision_loss)]
        let window_start = start_secs as f64;
        #[allow(clippy::cast_precision_loss)]
        let window_end = end_secs as f64;
        let first = timestamps.first().copied().unwrap_or(f64::NAN);
        let last = timestamps.last().copied().unwrap_or(f64::NAN);
        assert!(
            first <= window_start && window_start < first + period_width,
            "case {case:?}: the series must open on the period {window_start} falls in, got \
             {first} in {answered}"
        );
        assert!(
            last < window_end && window_end <= last + period_width,
            "case {case:?}: the series must end on the last period before {window_end}, got \
             {last} in {answered}"
        );
        assert!(
            timestamps
                .windows(2)
                .all(|pair| (pair[1] - pair[0] - period_width).abs() <= f64::EPSILON),
            "case {case:?}: the series must step by one period, got {timestamps:?}"
        );
    }
}

#[tokio::test]
async fn an_event_outside_a_rule_s_reach_contributes_nothing_to_it() {
    let router = build_router();
    let end_secs = now_secs() + PERIOD_SECS;
    let start_secs = end_secs - 5 * PERIOD_SECS;
    const NAMED: &str = "/insights/named";
    const OTHER: &str = "/insights/other";
    let case = "reach";
    group_with_stream(&router, case, NAMED, "stream").await;
    group_with_stream(&router, case, OTHER, "stream").await;

    // One rule over the named group only, and one DISABLED rule over it as well, so the same
    // ingestion answers both halves of the rule.
    given_rule(
        &router,
        case,
        "enabled-rule",
        &definition(NAMED, json!({"Keys": ["$.ip"]}), json!({})),
        Some("ENABLED"),
    )
    .await;
    given_rule(
        &router,
        case,
        "disabled-rule",
        &definition(NAMED, json!({"Keys": ["$.ip"]}), json!({})),
        Some("DISABLED"),
    )
    .await;

    let message = json!({"ip": "203.0.113.42"}).to_string();
    for group in [NAMED, OTHER] {
        ingest(
            &router,
            case,
            group,
            "stream",
            &[(end_secs - 90, message.clone())],
        )
        .await;
    }

    // (case, the rule, the contributors it must list)
    let expectations: Vec<(&str, &str, ReportedContributors)> = vec![
        (
            "the enabled rule over the group the events reached",
            "enabled-rule",
            vec![("203.0.113.42".to_string(), 1.0)],
        ),
        (
            "a rule that was disabled while the events arrived",
            "disabled-rule",
            Vec::new(),
        ),
    ];
    for (case, rule, expected) in expectations {
        let answered = report(&router, case, rule, start_secs, end_secs, &["SampleCount"]).await;
        let got = reported_contributors(&answered);
        assert_eq!(
            got, expected,
            "case {case:?}: expected the contributors {expected:?}, got {got:?} in {answered}"
        );
    }

    // Enabling the rule makes it collect what arrives from then on — and nothing it missed.
    cloudwatch(
        &router,
        "enabled later",
        "EnableInsightRules",
        &json!({"RuleNames": ["disabled-rule"]}),
    )
    .await;
    ingest(
        &router,
        "enabled later",
        NAMED,
        "stream",
        &[(end_secs - 30, message.clone())],
    )
    .await;
    let answered = report(
        &router,
        "enabled later",
        "disabled-rule",
        start_secs,
        end_secs,
        &["SampleCount"],
    )
    .await;
    assert_eq!(
        reported_contributors(&answered),
        vec![("203.0.113.42".to_string(), 1.0)],
        "case \"enabled later\": only the event that arrived while enabled counts, got {answered}"
    );

    // A window shorter than one period is refused rather than answered with an empty report.
    let (status, body) = call(
        &router,
        CLOUDWATCH_SIGNING_NAME,
        CLOUDWATCH_TARGET_PREFIX,
        "application/x-amz-json-1.0",
        "GetInsightRuleReport",
        &json!({
            "RuleName": "enabled-rule",
            "StartTime": end_secs - PERIOD_SECS + 1,
            "EndTime": end_secs,
            "Period": PERIOD_SECS,
        }),
    )
    .await;
    assert_eq!(
        (status, body.get("__type").and_then(Value::as_str)),
        (400, Some("InvalidParameterValueException")),
        "case \"a window shorter than one period\": expected a 400 refusal, got {status} {body}"
    );
}
