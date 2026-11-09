// SPDX-License-Identifier: CC0-1.0

//! What an invocation PRINTS drives the log-metric-filter -> alarm -> SNS -> SQS chain, through
//! the WHOLE mounted service set — the one `core/router/src/lib.rs` assembles for the binary.
//!
//! The RULE under test: **every invocation of a function writes that run's log to the
//! function's own CloudWatch Logs group — the group its `LoggingConfig$LogGroup` names, else
//! the default `/aws/lambda/<function name>` — as one event per line, the runtime's `START`,
//! `END` and `REPORT RequestId:` framing around each line the handler printed; each metric
//! filter on that group counts every event its pattern matches into its own CloudWatch metric,
//! so `GetMetricStatistics` reports `Sum` == the number of matching lines the runs wrote, and
//! an alarm over that metric whose threshold the count crosses transitions to ALARM, records a
//! succeeded Action against each topic its `AlarmActions` names, and publishes the notification
//! to that topic, which fans it out to the topic's CURRENT subscriptions only — a subscription
//! removed before the transition receives nothing. A filter whose pattern no line matches
//! leaves its metric empty, so an alarm over THAT metric never transitions and publishes
//! nothing.**
//!
//! Six services have to agree for that to hold — Lambda runs the handler and delivers the run's
//! log, CloudWatch Logs holds the group and extracts the metric data, CloudWatch evaluates the
//! alarm and executes its action, SNS fans the notification out, SQS holds what arrived, and
//! CloudFormation provisions the function and the declared group — and each hop is an
//! in-process call addressed the way a client's own scoped call is addressed, so only the
//! production service set can show the whole chain. The crate-local suites pin the links
//! separately:
//! `services/service-com-amazonaws-lambda-awsgirapiservice/tests/all/invocation_log_delivery.rs`
//! pins what one invocation leaves in the group over a table of handlers and roles,
//! `core/router/tests/all/logs_metric_filter_extraction.rs` pins the extraction over a table of
//! filter forms, and `core/router/tests/all/cloudwatch_alarm_action_reaches_its_subscribed_queue.rs`
//! pins the delivery of an alarm action. What only this suite can reach is the chain from the
//! handler's own `print` to the queue a topic fans out to.
//!
//! The rows differ along what the rule generalises over: WHERE the function logs (the default
//! group, created out of band as a client that wants its filter in place before the first run
//! creates it, and a group a stack declares and the function's `LoggingConfig` names), the FORM
//! of the filter pattern (a JSON pattern over a member of the printed document, and an
//! unstructured term), how many lines ONE invocation prints and how many of them the pattern
//! matches, and how many invocations were made. Every expected count is computed from the row,
//! never from a recording.

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::cloudformation_firehose_stream_stabilisation::wait_for_stack_status;
use crate::cloudwatch_alarm_action_reaches_its_subscribed_queue::{cw, drain, drain_until};
use crate::query_wire::{element, form_encode, outputs_of};

/// The Region every request below is signed for and every ARN names.
const REGION: &str = "us-east-1";

/// The `StackStatus` a completed create reports.
const CREATE_COMPLETE: &str = "CREATE_COMPLETE";

/// How long one invocation of this handler is given, as the function's own `Timeout`.
const FUNCTION_TIMEOUT: u64 = 30;

/// How long to wait between polls of a clock-owed transition, and how many polls to make.
const POLL: Duration = Duration::from_millis(50);
const POLLS: usize = 400;

/// The fixed framing the runtime writes around every run's own output, and the fixed field
/// names of the closing report. Source: the lines Lambda writes for an invocation — `START
/// RequestId: <id> Version: <version>`, `END RequestId: <id>` and `REPORT RequestId: <id>` with
/// the run's duration and the function's memory size as named fields.
const START_PREFIX: &str = "START RequestId: ";
const END_PREFIX: &str = "END RequestId: ";
const REPORT_PREFIX: &str = "REPORT RequestId: ";
const REPORT_FIELDS: &[&str] = &["Duration:", "Billed Duration:", "Memory Size:"];

/// The namespace the rows' metric filters publish into: a namespace of this suite's own, so
/// nothing another suite publishes lands in the series an alarm here evaluates.
const NAMESPACE: &str = "Custom/LambdaLogChain";

/// The alarm's period in seconds, and the threshold a count of matching lines has to cross:
/// any matching line in a period breaches it, and a period with none does not.
const ALARM_PERIOD: i64 = 60;
const ALARM_THRESHOLD: f64 = 0.0;

/// The service principal SNS performs its delivery to a subscribed queue as, which is what that
/// queue's own policy has to allow.
const SNS_PRINCIPAL: &str = "sns.amazonaws.com";

/// How long a row waits for the chain to complete, and how often it looks again. The evaluation
/// is owed by the clock rather than by a call, so the transition is seconds away; the deadline
/// is generous only so a failure is a failure rather than a hang.
const CHAIN_DEADLINE: Duration = Duration::from_secs(90);
const LOOK_AGAIN: Duration = Duration::from_millis(200);

/// How long a row watches the quiet alarm for a transition it must NOT make, once the subject
/// has fired: long enough to span several of the alarm clock's own wake-ups.
const QUIET_WATCH: Duration = Duration::from_secs(3);

/// The handler every row deploys: it prints one `ERROR` document per line the event asks for
/// and one `INFO` document, each carrying the event's marker, and answers how many of the
/// `ERROR` lines it printed. It imports nothing but `json`, so the run makes no call of its own
/// and the only thing that reaches CloudWatch Logs is what the runtime delivered.
const HANDLER_SOURCE: &str = r#"
import json


def handler(event, context):
    printed = 0
    for index in range(int(event["lines"])):
        print(json.dumps({"level": "ERROR", "marker": event["marker"], "line": index}))
        printed += 1
    print(json.dumps({"level": "INFO", "marker": event["marker"]}))
    return {"printed": printed}
"#;

/// The line the handler prints for one `ERROR` document: what python's `json.dumps` writes for
/// it, with that function's default separators (`", "` between members and `": "` between a
/// member's name and its value).
fn error_line(marker: &str, line: usize) -> String {
    format!("{{\"level\": \"ERROR\", \"marker\": \"{marker}\", \"line\": {line}}}")
}

/// One answered request: its status and its body verbatim.
struct Answered {
    status: u16,
    body: Bytes,
}

impl Answered {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }

    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }
}

/// The emulator as it is ASSEMBLED, on one engine: the group the run writes to, the metric the
/// filter extracts, the alarm that evaluates it and the queue the notification lands in are all
/// the mounted services' own.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = aws_emulator_router::build_router(engine);
    // The suite exercises the services, not SigV4: each call carries a credential scope so the
    // router can name the target service, and the signature itself is not under test here.
    router.set_request_authentication(false);
    router
}

/// A SigV4 `Authorization` whose credential scope names `signing_name`, which is what routes
/// the request.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/{signing_name}/\
         aws4_request, SignedHeaders=host, Signature=0000"
    )
}

/// Dispatch one request as it arrives on the wire.
async fn dispatch(
    router: &MultiServiceRouter,
    method: &str,
    host: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Bytes,
) -> Answered {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{path}"))
        .header("host", host);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let response = router
        .dispatch(builder.body(body).expect("a well-formed request"))
        .await;
    let (parts, body) = response.into_parts();
    Answered {
        status: parts.status.as_u16(),
        body,
    }
}

/// Call one CloudFormation operation over its awsQuery wire.
async fn cloudformation(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(&str, &str)],
) -> Answered {
    let mut body = format!("Action={action}&Version=2010-05-15");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    dispatch(
        router,
        "POST",
        &format!("cloudformation.{REGION}.amazonaws.com"),
        "/",
        &[
            ("content-type", "application/x-www-form-urlencoded"),
            ("authorization", &authorization("cloudformation")),
        ],
        Bytes::from(body),
    )
    .await
}

/// One AWS Lambda call over its `restJson1` wire.
async fn lambda(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Answered {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    dispatch(
        router,
        method,
        &format!("lambda.{REGION}.amazonaws.com"),
        path,
        &[
            ("content-type", "application/json"),
            ("authorization", &authorization("lambda")),
        ],
        Bytes::from(payload),
    )
    .await
}

/// One Amazon CloudWatch Logs call over its `awsJson1_1` wire, which must answer 200.
async fn logs(router: &MultiServiceRouter, case: &str, operation: &str, body: Value) -> Value {
    let answered = dispatch(
        router,
        "POST",
        &format!("logs.{REGION}.amazonaws.com"),
        "/",
        &[
            ("content-type", "application/x-amz-json-1.1"),
            ("authorization", &authorization("logs")),
            ("x-amz-target", &format!("Logs_20140328.{operation}")),
        ],
        Bytes::from(body.to_string()),
    )
    .await;
    assert_eq!(
        answered.status, 200,
        "case {case:?}: expected HTTP 200 from CloudWatch Logs {operation}, got {} with {:?}",
        answered.status,
        answered.text()
    );
    answered.json()
}

/// One Amazon SQS call over its `awsJson1_0` wire, which must answer 200.
async fn sqs(router: &MultiServiceRouter, case: &str, operation: &str, body: Value) -> Value {
    let answered = dispatch(
        router,
        "POST",
        &format!("sqs.{REGION}.amazonaws.com"),
        "/",
        &[
            ("content-type", "application/x-amz-json-1.0"),
            ("authorization", &authorization("sqs")),
            ("x-amz-target", &format!("AmazonSQS.{operation}")),
        ],
        Bytes::from(body.to_string()),
    )
    .await;
    assert_eq!(
        answered.status, 200,
        "case {case:?}: expected HTTP 200 from SQS {operation}, got {} with {:?}",
        answered.status,
        answered.text()
    );
    answered.json()
}

/// One SNS call over the `awsQuery` form wire that service serves, answering the status and the
/// body as text.
async fn sns_call(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2010-03-31");
    for (name, value) in params {
        body.push('&');
        body.push_str(name);
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let answered = dispatch(
        router,
        "POST",
        &format!("sns.{REGION}.amazonaws.com"),
        "/",
        &[
            ("content-type", "application/x-www-form-urlencoded"),
            ("authorization", &authorization("sns")),
        ],
        Bytes::from(body),
    )
    .await;
    (answered.status, answered.text())
}

/// The same call, which must answer 200.
async fn sns(
    router: &MultiServiceRouter,
    case: &str,
    action: &str,
    params: &[(&str, &str)],
) -> String {
    let (status, text) = sns_call(router, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from SNS {action}, got {status} with {text}"
    );
    text
}

/// A queue of this emulator subscribed to `topic`, granting the SNS service principal the send
/// its delivery makes — "you must ensure that the Amazon SQS queue policy allows the Amazon SNS
/// topic to send messages to the queue". Answers the queue's url and the ARN of the
/// subscription, which is the handle `Unsubscribe` takes.
async fn given_subscribed_queue(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    topic: &str,
) -> (String, String) {
    let created = sqs(router, case, "CreateQueue", json!({"QueueName": name})).await;
    let url = created["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no QueueUrl in {created}"))
        .to_string();
    let attributes = sqs(
        router,
        case,
        "GetQueueAttributes",
        json!({"QueueUrl": url, "AttributeNames": ["QueueArn"]}),
    )
    .await;
    let queue_arn = attributes["Attributes"]["QueueArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no QueueArn in {attributes}"))
        .to_string();
    let policy = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": "AllowSNSDelivery",
            "Effect": "Allow",
            "Principal": {"Service": SNS_PRINCIPAL},
            "Action": "sqs:SendMessage",
            "Resource": queue_arn,
            "Condition": {"ArnEquals": {"aws:SourceArn": topic}},
        }],
    })
    .to_string();
    sqs(
        router,
        case,
        "SetQueueAttributes",
        json!({"QueueUrl": url, "Attributes": {"Policy": policy}}),
    )
    .await;
    let subscribed = sns(
        router,
        case,
        "Subscribe",
        &[
            ("TopicArn", topic),
            ("Protocol", "sqs"),
            ("Endpoint", &queue_arn),
        ],
    )
    .await;
    (url, element(&subscribed, "SubscriptionArn"))
}

/// Poll `GetFunctionConfiguration` until Lambda first reports the function `Active`, which is
/// the state an invocation needs. The span is that service's own, so nothing here sleeps a fixed
/// span on its behalf.
async fn wait_until_active(router: &MultiServiceRouter, case: &str, name: &str) {
    for _ in 0..POLLS {
        let reported = lambda(
            router,
            "GET",
            &format!("/2015-03-31/functions/{name}/configuration"),
            None,
        )
        .await;
        if reported.json().get("State").and_then(Value::as_str) == Some("Active") {
            return;
        }
        tokio::time::sleep(POLL).await;
    }
    panic!("case {case:?}: Lambda never reported {name} Active");
}

/// Every message CloudWatch Logs holds in `group`, polled until the group holds at least
/// `at_least` of them or the polls run out.
async fn messages_in(
    router: &MultiServiceRouter,
    case: &str,
    group: &str,
    at_least: usize,
) -> Vec<String> {
    let mut held: Vec<String> = Vec::new();
    for _ in 0..POLLS {
        let answered = logs(
            router,
            case,
            "FilterLogEvents",
            json!({ "logGroupName": group }),
        )
        .await;
        held = answered
            .get("events")
            .and_then(Value::as_array)
            .map(|events| {
                events
                    .iter()
                    .filter_map(|event| event.get("message").and_then(Value::as_str))
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        if held.len() >= at_least {
            return held;
        }
        tokio::time::sleep(POLL).await;
    }
    held
}

/// Wall-clock now as epoch seconds, the clock the series' windows are measured against.
fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |since| since.as_secs_f64())
}

/// The `Sum` the series `metric` holds over the window around `at`, totalled over the period
/// windows it falls in, and how many datapoints carried it.
async fn reported_sum(
    router: &MultiServiceRouter,
    case: &str,
    metric: &str,
    at: f64,
) -> (f64, usize) {
    #[allow(clippy::cast_precision_loss)]
    let span = 2.0 * ALARM_PERIOD as f64;
    let answered = cw(
        router,
        case,
        REGION,
        "GetMetricStatistics",
        &json!({
            "Namespace": NAMESPACE,
            "MetricName": metric,
            "StartTime": at - span,
            "EndTime": at + span,
            "Period": ALARM_PERIOD,
            "Statistics": ["Sum"],
        }),
    )
    .await;
    let datapoints = answered["Datapoints"].as_array().cloned().unwrap_or_default();
    let sum = datapoints
        .iter()
        .map(|point| point["Sum"].as_f64().unwrap_or_default())
        .sum();
    (sum, datapoints.len())
}

/// Poll the series until it reports a `Sum` of `wanted` or the deadline passes, and answer what
/// it reported last. The extraction happens on ingest, which the invocation's own log delivery
/// drives, so the series is read until it has what the runs wrote rather than once.
async fn polled_sum(router: &MultiServiceRouter, case: &str, metric: &str, at: f64, wanted: f64) -> f64 {
    let deadline = Instant::now() + CHAIN_DEADLINE;
    loop {
        let (sum, _) = reported_sum(router, case, metric, at).await;
        if (sum - wanted).abs() < f64::EPSILON || Instant::now() >= deadline {
            return sum;
        }
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// Create the alarm `name` over the series `metric`, with `topic` as its one alarm action.
async fn given_alarm(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    metric: &str,
    topic: &str,
) {
    cw(
        router,
        case,
        REGION,
        "PutMetricAlarm",
        &json!({
            "AlarmName": name,
            "Namespace": NAMESPACE,
            "MetricName": metric,
            "Statistic": "Sum",
            "Period": ALARM_PERIOD,
            "EvaluationPeriods": 1,
            "Threshold": ALARM_THRESHOLD,
            "ComparisonOperator": "GreaterThanThreshold",
            "TreatMissingData": "notBreaching",
            "AlarmActions": [topic],
        }),
    )
    .await;
}

/// What `DescribeAlarms` reports as `name`'s state.
async fn alarm_state(router: &MultiServiceRouter, case: &str, name: &str) -> String {
    let described = cw(
        router,
        case,
        REGION,
        "DescribeAlarms",
        &json!({"AlarmNames": [name]}),
    )
    .await;
    described["MetricAlarms"][0]["StateValue"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// The `Action` items the alarm's history holds, each as its parsed `HistoryData`, polled until
/// it holds `wanted` of them or the deadline passes.
async fn polled_action_items(
    router: &MultiServiceRouter,
    case: &str,
    alarm: &str,
    wanted: usize,
) -> Vec<Value> {
    let deadline = Instant::now() + CHAIN_DEADLINE;
    loop {
        let history = cw(
            router,
            case,
            REGION,
            "DescribeAlarmHistory",
            &json!({
                "AlarmName": alarm,
                "HistoryItemType": "Action",
                "AlarmTypes": ["MetricAlarm"],
            }),
        )
        .await;
        let items: Vec<Value> = history["AlarmHistoryItems"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|item| {
                let data = item["HistoryData"].as_str().unwrap_or_default();
                serde_json::from_str(data).unwrap_or_else(|error| {
                    panic!("case {case:?}: HistoryData is not JSON ({error}): {data}")
                })
            })
            .collect();
        if items.len() >= wanted || Instant::now() >= deadline {
            return items;
        }
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// The two approximate counters a client reads a queue's depth from.
async fn queue_depth(router: &MultiServiceRouter, case: &str, url: &str) -> (String, String) {
    let attributes = sqs(
        router,
        case,
        "GetQueueAttributes",
        json!({
            "QueueUrl": url,
            "AttributeNames": [
                "ApproximateNumberOfMessages",
                "ApproximateNumberOfMessagesNotVisible",
            ],
        }),
    )
    .await;
    let read = |name: &str| {
        attributes["Attributes"][name]
            .as_str()
            .unwrap_or_default()
            .to_string()
    };
    (
        read("ApproximateNumberOfMessages"),
        read("ApproximateNumberOfMessagesNotVisible"),
    )
}

/// How a row spells the filter pattern its metric filter counts with.
#[derive(Debug, Clone, Copy)]
enum Pattern {
    /// A JSON pattern over a member of the printed document, which matches only the `ERROR`
    /// lines the handler printed.
    ErrorLevel,
    /// An unstructured term: the marker itself, which every line the handler printed carries.
    Marker,
}

impl Pattern {
    /// The `filterPattern` this spelling states for the row's marker.
    fn spelled(self, marker: &str) -> String {
        match self {
            Self::ErrorLevel => r#"{ $.level = "ERROR" }"#.to_string(),
            Self::Marker => marker.to_string(),
        }
    }

    /// How many lines of ONE invocation's log this pattern matches, for a handler that printed
    /// `lines` `ERROR` documents and one `INFO` document, each carrying the marker: the
    /// `ERROR` lines alone for the JSON pattern over `level`, and every printed line for the
    /// term the marker spells. The runtime's own framing lines are neither a JSON document nor
    /// carriers of the marker, so they match neither.
    fn matched_per_invocation(self, lines: usize) -> usize {
        match self {
            Self::ErrorLevel => lines,
            Self::Marker => lines + 1,
        }
    }
}

/// One row: where the function logs, how its filter is spelled, how many lines one invocation
/// prints and how many invocations are made.
struct Row {
    case: &'static str,
    /// The group the stack DECLARES and the function's `LoggingConfig` names, or `None` for the
    /// default `/aws/lambda/<function name>`, which the row creates out of band.
    declared_group: Option<&'static str>,
    pattern: Pattern,
    /// How many `ERROR` documents one invocation prints.
    lines: usize,
    /// How many invocations are made.
    invocations: usize,
}

const ROWS: &[Row] = &[
    Row {
        case: "the default group, a JSON pattern over the printed level, one line per \
               invocation and five invocations",
        declared_group: None,
        pattern: Pattern::ErrorLevel,
        lines: 1,
        invocations: 5,
    },
    Row {
        case: "a declared group the function's LoggingConfig names, an unstructured marker \
               term, two lines per invocation and two invocations",
        declared_group: Some("/custom/lambda-log-chain"),
        pattern: Pattern::Marker,
        lines: 2,
        invocations: 2,
    },
];

/// The template one row deploys: the function, and the log group it logs to where the row
/// declares one.
fn template(row: &Row) -> String {
    let mut properties = json!({
        "Runtime": "python3.12",
        "Handler": "index.handler",
        "Timeout": FUNCTION_TIMEOUT,
        "InlineCode": HANDLER_SOURCE,
    });
    let mut resources = serde_json::Map::new();
    if let Some(group) = row.declared_group {
        resources.insert(
            "Group".to_string(),
            json!({
                "Type": "AWS::Logs::LogGroup",
                "Properties": { "LogGroupName": group },
            }),
        );
        properties["LoggingConfig"] = json!({ "LogGroup": { "Ref": "Group" } });
    }
    resources.insert(
        "Logger".to_string(),
        json!({
            "Type": "AWS::Serverless::Function",
            "Properties": properties,
        }),
    );
    json!({
        "Transform": "AWS::Serverless-2016-10-31",
        "Resources": Value::Object(resources),
        "Outputs": { "FunctionName": { "Value": { "Ref": "Logger" } } },
    })
    .to_string()
}

/// Deploy one row's stack and answer the function name it published.
async fn given_function(router: &MultiServiceRouter, case: &str, stack: &str, row: &Row) -> String {
    let body = template(row);
    let created = cloudformation(
        router,
        "CreateStack",
        &[
            ("StackName", stack),
            ("TemplateBody", &body),
            // The macro is acknowledged with CAPABILITY_AUTO_EXPAND, and the execution role
            // the transform synthesizes for the function with CAPABILITY_IAM.
            ("Capabilities.member.1", "CAPABILITY_IAM"),
            ("Capabilities.member.2", "CAPABILITY_AUTO_EXPAND"),
        ],
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: expected HTTP 200 from CreateStack, got {} with {:?}",
        created.status,
        created.text()
    );
    let (reached, described) = wait_for_stack_status(router, stack, CREATE_COMPLETE).await;
    assert_eq!(
        reached, CREATE_COMPLETE,
        "case {case:?}: expected the stack to reach {CREATE_COMPLETE}, got {described}"
    );
    let function = outputs_of(&described)
        .get("FunctionName")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: the stack published no function name"));
    wait_until_active(router, case, &function).await;
    function
}

/// Store one metric filter on `group`, counting every event its pattern matches into `metric`.
async fn given_metric_filter(
    router: &MultiServiceRouter,
    case: &str,
    group: &str,
    name: &str,
    pattern: &str,
    metric: &str,
) {
    logs(
        router,
        case,
        "PutMetricFilter",
        json!({
            "logGroupName": group,
            "filterName": name,
            "filterPattern": pattern,
            "metricTransformations": [{
                "metricName": metric,
                "metricNamespace": NAMESPACE,
                "metricValue": "1",
            }],
        }),
    )
    .await;
}

#[tokio::test]
async fn what_an_invocation_prints_drives_the_metric_filter_alarm_and_subscription_chain() {
    // ONE emulator for every row: each row deploys its own stack, group, filters, alarms, topic
    // and queues, so what a row reads is still that row's alone.
    let router = emulator();
    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let marker = format!("logchainmarker{index}");
        let metric = format!("MatchedLines{index}");
        let quiet_metric = format!("UnmatchedLines{index}");
        let subject_alarm = format!("log-chain-{index}");
        let quiet_alarm = format!("log-chain-quiet-{index}");

        // 1. The function, and the group its runs log to: the one the stack declared, or the
        //    default one, created out of band the way a client that wants its filter in place
        //    before the first run creates it.
        let function = given_function(&router, case, &format!("log-chain-{index}"), row).await;
        let group = match row.declared_group {
            Some(declared) => declared.to_string(),
            None => {
                let default = format!("/aws/lambda/{function}");
                logs(
                    &router,
                    case,
                    "CreateLogGroup",
                    json!({ "logGroupName": default }),
                )
                .await;
                default
            }
        };
        assert_eq!(
            lambda(
                &router,
                "GET",
                &format!("/2015-03-31/functions/{function}/configuration"),
                None,
            )
            .await
            .json()["LoggingConfig"]["LogGroup"]
                .as_str(),
            Some(group.as_str()),
            "case {case:?}: expected Lambda to report {group:?} as the function's log group"
        );

        // 2. The two filters on that group: the one whose pattern the printed lines match, and
        //    the quiet one whose pattern nothing prints — identical but for the pattern, so the
        //    difference between them is the matching alone.
        given_metric_filter(
            &router,
            case,
            &group,
            "count-matching-lines",
            &row.pattern.spelled(&marker),
            &metric,
        )
        .await;
        given_metric_filter(
            &router,
            case,
            &group,
            "count-nothing",
            &format!("absent{marker}"),
            &quiet_metric,
        )
        .await;

        // 3. The topic, the two queues subscribed to it, and the alarms over the two metrics.
        //    One subscription is REMOVED before anything is printed, so the chain reaching the
        //    topic must stop exactly there for it.
        let topic = element(
            &sns(&router, case, "CreateTopic", &[("Name", &format!("log-chain-{index}"))]).await,
            "TopicArn",
        );
        let quiet_topic = element(
            &sns(
                &router,
                case,
                "CreateTopic",
                &[("Name", &format!("log-chain-quiet-{index}"))],
            )
            .await,
            "TopicArn",
        );
        let (subscribed_url, _subscribed) =
            given_subscribed_queue(&router, case, &format!("log-chain-kept-{index}"), &topic).await;
        let (removed_url, removed_subscription) =
            given_subscribed_queue(&router, case, &format!("log-chain-removed-{index}"), &topic)
                .await;
        let (quiet_url, _quiet_subscription) = given_subscribed_queue(
            &router,
            case,
            &format!("log-chain-quiet-{index}"),
            &quiet_topic,
        )
        .await;
        given_alarm(&router, case, &subject_alarm, &metric, &topic).await;
        given_alarm(&router, case, &quiet_alarm, &quiet_metric, &quiet_topic).await;

        sns(
            &router,
            case,
            "Unsubscribe",
            &[("SubscriptionArn", &removed_subscription)],
        )
        .await;
        let (status, text) = sns_call(
            &router,
            "GetSubscriptionAttributes",
            &[("SubscriptionArn", &removed_subscription)],
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: a removed subscription must resolve to nothing, got {status} with \
             {text}"
        );
        assert!(
            text.contains("NotFound"),
            "case {case:?}: the refusal must name the NotFound error, got {text}"
        );

        // 4. The invocations. Each one prints the row's lines and answers how many it printed.
        let at = now_secs();
        for invocation in 0..row.invocations {
            let invoked = lambda(
                &router,
                "POST",
                &format!("/2015-03-31/functions/{function}/invocations"),
                Some(json!({ "marker": marker, "lines": row.lines })),
            )
            .await;
            assert_eq!(
                invoked.status, 200,
                "case {case:?}: expected HTTP 200 from invocation {invocation}, got {} with {:?}",
                invoked.status,
                invoked.text()
            );
            assert_eq!(
                invoked.json()["printed"].as_u64(),
                Some(row.lines as u64),
                "case {case:?}: invocation {invocation} must report the lines it printed, got {:?}",
                invoked.text()
            );
        }

        // 5. What the runs left in the group: every line the handler printed, as an event of its
        //    own, framed by the runtime's own opening and closing lines — one framing per run.
        let printed_lines: Vec<String> =
            (0..row.lines).map(|line| error_line(&marker, line)).collect();
        let framing_per_run = 3;
        let wanted = row.invocations * (row.lines + 1 + framing_per_run);
        let held = messages_in(&router, case, &group, wanted).await;
        for printed in &printed_lines {
            let written = held.iter().filter(|message| *message == printed).count();
            assert_eq!(
                written, row.invocations,
                "case {case:?}: expected {group:?} to hold the printed line {printed:?} once per \
                 invocation ({}), got {written} of {held:?}",
                row.invocations
            );
        }
        for prefix in [START_PREFIX, END_PREFIX, REPORT_PREFIX] {
            let framed = held
                .iter()
                .filter(|message| message.starts_with(prefix))
                .count();
            assert_eq!(
                framed, row.invocations,
                "case {case:?}: expected one {prefix:?} line per invocation ({}), got {framed} \
                 of {held:?}",
                row.invocations
            );
        }
        let report = held
            .iter()
            .find(|message| message.starts_with(REPORT_PREFIX))
            .unwrap_or_else(|| panic!("case {case:?}: no report line in {held:?}"));
        for field in REPORT_FIELDS {
            assert!(
                report.contains(field),
                "case {case:?}: the report line must carry the {field:?} field, got {report:?}"
            );
        }

        // 6. What the filters made of those events: the matching lines counted into the
        //    subject's metric, and nothing at all in the quiet one.
        #[allow(clippy::cast_precision_loss)]
        let expected_sum =
            (row.invocations * row.pattern.matched_per_invocation(row.lines)) as f64;
        let sum = polled_sum(&router, case, &metric, at, expected_sum).await;
        assert!(
            (sum - expected_sum).abs() < f64::EPSILON,
            "case {case:?}: expected the metric to report Sum {expected_sum} — one per matching \
             line — got {sum}"
        );
        let (quiet_sum, quiet_points) = reported_sum(&router, case, &quiet_metric, at).await;
        assert_eq!(
            quiet_points, 0,
            "case {case:?}: a filter whose pattern nothing matched must publish no datapoint, \
             got {quiet_points} carrying {quiet_sum}"
        );

        // 7. The chain: the evaluation the clock owes moves the alarm, its action publishes to
        //    the topic, and the queue still subscribed receives the notification — with no call
        //    forcing any of it.
        let delivered = drain_until(&router, case, REGION, &subscribed_url, 1).await;
        assert_eq!(
            delivered.len(),
            1,
            "case {case:?}: the alarm's action must reach the subscribed queue exactly once, got \
             {delivered:?}"
        );
        let notification = &delivered[0];
        assert_eq!(
            notification["AlarmName"].as_str(),
            Some(subject_alarm.as_str()),
            "case {case:?}: the notification must name the alarm, got {notification}"
        );
        assert_eq!(
            notification["NewStateValue"].as_str(),
            Some("ALARM"),
            "case {case:?}: the notification must carry the state moved to, got {notification}"
        );
        assert_eq!(
            notification["Trigger"]["MetricName"].as_str(),
            Some(metric.as_str()),
            "case {case:?}: the Trigger must report the filter's metric, got {notification}"
        );
        let state = alarm_state(&router, case, &subject_alarm).await;
        assert_eq!(
            state, "ALARM",
            "case {case:?}: the alarm the printed lines breached must report ALARM, got {state:?}"
        );

        // ... and the action is recorded against the topic as one that was executed.
        let items = polled_action_items(&router, case, &subject_alarm, 1).await;
        assert_eq!(
            items.len(),
            1,
            "case {case:?}: the transition's one action must be recorded, got {items:?}"
        );
        assert_eq!(
            items[0]["notificationResource"].as_str(),
            Some(topic.as_str()),
            "case {case:?}: the item must name the action's target, got {}",
            items[0]
        );
        assert_eq!(
            items[0]["actionState"].as_str(),
            Some("Succeeded"),
            "case {case:?}: the recorded action must not be a failed one, got {}",
            items[0]
        );

        // 8. The removed subscription received nothing — the chain stopped exactly at delivery,
        //    with the queue's own policy still granting the send.
        let withheld = drain(&router, case, REGION, &removed_url).await;
        assert!(
            withheld.is_empty(),
            "case {case:?}: the unsubscribed queue must receive nothing, got {withheld:?}"
        );
        let (visible, in_flight) = queue_depth(&router, case, &removed_url).await;
        assert_eq!(
            (visible.as_str(), in_flight.as_str()),
            ("0", "0"),
            "case {case:?}: the unsubscribed queue must report no message at all, got \
             {visible:?} and {in_flight:?}"
        );

        // 9. And the quiet alarm, whose filter matched nothing, never transitioned: its state
        //    stays OK and its own subscribed queue stays empty, however many wake-ups pass.
        tokio::time::sleep(QUIET_WATCH).await;
        let quiet_state = alarm_state(&router, case, &quiet_alarm).await;
        assert_eq!(
            quiet_state, "OK",
            "case {case:?}: an alarm over a metric nothing published to must stay OK, got \
             {quiet_state:?}"
        );
        let quiet_delivered = drain(&router, case, REGION, &quiet_url).await;
        assert!(
            quiet_delivered.is_empty(),
            "case {case:?}: an alarm that never fired may deliver nothing, got \
             {quiet_delivered:?}"
        );
    }
}
