// SPDX-License-Identifier: CC0-1.0

//! The wire harness every suite in this crate drives the service through: `awsJson1_0`,
//! `POST /` with `X-Amz-Target: GraniteServiceVersion20100801.<OperationName>` (README.md /
//! OPERATIONS.md "Wire protocol").
//!
//! It lives here so each suite under `tests/all/` reaches the service exactly the way the
//! production router does, through ONE definition of the request walk — including the
//! MULTI-SERVICE emulator the alarm-action suites need, because an alarm's action is an SNS
//! topic and a notification is asserted where a client sees it, on the topic's SQS
//! subscription.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// The `X-Amz-Target` prefix this service's model declares.
pub const TARGET_PREFIX: &str = "GraniteServiceVersion20100801";

/// Long enough for a metric alarm's first evaluation to come due.
///
/// `PutMetricAlarm` sets a new alarm to `INSUFFICIENT_DATA` and the evaluated state is owed a
/// span later (`crate::state::alarm_evaluation::EVALUATION_DELAY_SECS`, inside the
/// framework's 250-750 ms window for a transition the clock owes), so a test that wants the
/// EVALUATED state waits past the top of that window.
pub const PAST_EVALUATION_DELAY: Duration = Duration::from_millis(900);

/// Wait for every alarm evaluation started before now to be due.
///
/// A blocking sleep: a suite that drives one router with no concurrent task holds nothing up
/// by blocking its own runtime thread, and nothing it asserts needs the runtime to run
/// meanwhile. A test that waits for the service's own alarm clock to deliver something must
/// await `tokio::time::sleep` instead, so the task gets to run.
pub fn wait_for_evaluation() {
    std::thread::sleep(PAST_EVALUATION_DELAY);
}

/// Wall-clock now as epoch seconds, the clock the evaluation windows are measured against.
pub fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |since| since.as_secs_f64())
}

/// One emulator with this service mounted, reached exactly the way the production router
/// reaches it.
pub fn build_router() -> MultiServiceRouter {
    build_router_with_engine().0
}

/// The same emulator, plus the shared engine it was built on, for a suite that has to look
/// at the store directly (what a write left behind, or what a read must not change).
pub fn build_router_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signature verification is the emulator's concern, not this service's; turning it off
    // keeps the suite independent of how the process was launched.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudwatch_graniteserviceversion20100801::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    (router, engine)
}

/// Publish one managed Contributor Insights rule template into this account's catalog, for the
/// vendor and resource type `resource_arn` names.
///
/// The catalog is the SERVICE's side of the managed rules — "The template name for the managed
/// Contributor Insights rule, as returned by `ListManagedInsightRules`."
/// (`ManagedRule$TemplateName`) — so a suite that needs a template a caller CAN enable publishes
/// it through the store, the way the service publishes one, rather than through any operation.
pub fn publish_managed_rule_template(
    engine: &engine::InMemoryEngine,
    template_name: &str,
    resource_arn: &str,
) {
    use service_com_amazonaws_cloudwatch_graniteserviceversion20100801::state as cloudwatch;
    let template = cloudwatch::ManagedRuleTemplate::for_resource(template_name, resource_arn)
        .unwrap_or_else(|| panic!("{resource_arn} is the ARN a template is published for"));
    cloudwatch::service_state(engine)
        .expect("the CloudWatch store exists")
        .lock()
        .publish_managed_rule_template(template);
}

/// The emulator the end-to-end alarm-action tests drive: CloudWatch, SNS and SQS on ONE
/// engine, because an alarm's action is an SNS topic and the notification is asserted where a
/// client sees it — on the queue subscribed to that topic.
pub struct Emulator {
    /// The router every call below is dispatched through.
    pub router: MultiServiceRouter,
}

/// One emulator with CloudWatch, SNS and SQS mounted on one engine.
pub fn emulator() -> Emulator {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudwatch_graniteserviceversion20100801::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_sns_amazonsimplenotificationservice::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        Arc::clone(&engine),
    ));
    Emulator { router }
}

/// The Region every end-to-end call below is signed for: the one every other call in this
/// harness is signed for too ([`DEFAULT_REGION`]).
pub const REGION: &str = DEFAULT_REGION;

/// One `awsJson` RPC call against the service whose signing name is `signing_name` — the
/// credential scope is what routes it.
pub async fn rpc(
    emulator: &Emulator,
    signing_name: &str,
    target_prefix: &str,
    operation: &str,
    body: &Value,
) -> Value {
    rpc_in_region(emulator, REGION, signing_name, target_prefix, operation, body).await
}

/// The same call with the request signed for `region`: the credential scope is what every
/// handler reads the request's Region from, so this is the seam for anything reported — or
/// addressed — per Region.
pub async fn rpc_in_region(
    emulator: &Emulator,
    region: &str,
    signing_name: &str,
    target_prefix: &str,
    operation: &str,
    body: &Value,
) -> Value {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_name}.{region}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("{target_prefix}.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTTESTTEST/20260101/{region}/{signing_name}/\
                 aws4_request, SignedHeaders=host, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = emulator.router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed: Value = if payload.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&payload).unwrap_or(Value::Null)
    };
    assert_eq!(
        status, 200,
        "{operation}: expected HTTP 200, got {status} with body {parsed}"
    );
    parsed
}

/// One awsQuery form call against SNS, the protocol that service serves, requiring a 200.
pub async fn sns(emulator: &Emulator, action: &str, params: &[(&str, &str)]) -> String {
    sns_in_region(emulator, REGION, action, params).await
}

/// The same SNS call with the request signed for `region`, requiring a 200.
pub async fn sns_in_region(
    emulator: &Emulator,
    region: &str,
    action: &str,
    params: &[(&str, &str)],
) -> String {
    let (status, text) = sns_call_in_region(emulator, region, action, params).await;
    assert_eq!(status, 200, "{action}: expected HTTP 200, got {status} / {text}");
    text
}

/// The same call with its HTTP status and body as answered — the seam for a suite that drives
/// SNS into a REFUSAL and reads what it said.
pub async fn sns_call(emulator: &Emulator, action: &str, params: &[(&str, &str)]) -> (u16, String) {
    sns_call_in_region(emulator, REGION, action, params).await
}

/// The same, with the request signed for `region`: the credential scope is what every handler
/// reads the request's Region from.
pub async fn sns_call_in_region(
    emulator: &Emulator,
    region: &str,
    action: &str,
    params: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2010-03-31");
    for (name, value) in params {
        body.push('&');
        body.push_str(name);
        body.push('=');
        for byte in value.bytes() {
            match byte {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                    body.push(char::from(byte));
                }
                other => body.push_str(&format!("%{other:02X}")),
            }
        }
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("sns.{region}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTTESTTEST/20260101/{region}/sns/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(body))
        .expect("the test request is well formed");
    let response = emulator.router.dispatch(request).await;
    let status = response.status().as_u16();
    let text = String::from_utf8_lossy(response.body()).to_string();
    (status, text)
}

/// The text of the first `<tag>…</tag>` element of an awsQuery response.
pub fn element(body: &str, tag: &str) -> String {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = body
        .find(&open)
        .unwrap_or_else(|| panic!("no <{tag}> in {body}"))
        + open.len();
    let end = body[start..]
        .find(&close)
        .unwrap_or_else(|| panic!("no </{tag}> in {body}"));
    body[start..start + end].to_string()
}

/// The queue policy AWS documents for an SQS queue subscribed to an SNS topic: the service
/// principal `sns.amazonaws.com`, allowed `sqs:SendMessage` on that queue.
pub fn grant_sns_delivery(queue_arn: &str) -> String {
    ::serde_json::json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": "AllowSNSDelivery",
            "Effect": "Allow",
            "Principal": {"Service": "sns.amazonaws.com"},
            "Action": "sqs:SendMessage",
            "Resource": queue_arn,
        }],
    })
    .to_string()
}

/// A queue called `name`: its url and its ARN, with no policy of its own.
///
/// The SNS-action suites all need one — a notification is asserted where a client sees it, on
/// the queue subscribed to the alarm's topic — so the setup lives here once.
pub async fn given_queue(emulator: &Emulator, name: &str) -> (String, String) {
    let created = rpc(
        emulator,
        "sqs",
        "AmazonSQS",
        "CreateQueue",
        &::serde_json::json!({"QueueName": name}),
    )
    .await;
    let url = created["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateQueue {name}: no QueueUrl in {created}"))
        .to_string();
    let attributes = rpc(
        emulator,
        "sqs",
        "AmazonSQS",
        "GetQueueAttributes",
        &::serde_json::json!({"QueueUrl": url, "AttributeNames": ["QueueArn"]}),
    )
    .await;
    let arn = attributes["Attributes"]["QueueArn"]
        .as_str()
        .unwrap_or_else(|| panic!("GetQueueAttributes {name}: no QueueArn in {attributes}"))
        .to_string();
    (url, arn)
}

/// Grant the SNS service principal the send on the queue at `url`, which is what a
/// subscribed queue needs to receive anything ([`grant_sns_delivery`]).
pub async fn grant_sns_delivery_on(emulator: &Emulator, url: &str, queue_arn: &str) {
    rpc(
        emulator,
        "sqs",
        "AmazonSQS",
        "SetQueueAttributes",
        &::serde_json::json!({
            "QueueUrl": url,
            "Attributes": {"Policy": grant_sns_delivery(queue_arn)},
        }),
    )
    .await;
}

/// Subscribe the queue `queue_arn` to the topic `topic_arn`.
pub async fn subscribe_queue(emulator: &Emulator, topic_arn: &str, queue_arn: &str) {
    sns(
        emulator,
        "Subscribe",
        &[
            ("TopicArn", topic_arn),
            ("Protocol", "sqs"),
            ("Endpoint", queue_arn),
        ],
    )
    .await;
}

/// Force one alarm into `state`, which is what fires the actions configured for it.
pub async fn set_alarm_state(emulator: &Emulator, name: &str, state: &str) {
    rpc(
        emulator,
        "monitoring",
        TARGET_PREFIX,
        "SetAlarmState",
        &::serde_json::json!({
            "AlarmName": name,
            "StateValue": state,
            "StateReason": "driven by the test",
        }),
    )
    .await;
}

/// Every message waiting on the queue at `url`, as `ReceiveMessage` reports them.
pub async fn received_messages(emulator: &Emulator, url: &str) -> Vec<Value> {
    let received = rpc(
        emulator,
        "sqs",
        "AmazonSQS",
        "ReceiveMessage",
        &::serde_json::json!({"QueueUrl": url, "MaxNumberOfMessages": 10}),
    )
    .await;
    received["Messages"].as_array().cloned().unwrap_or_default()
}

/// The Region the suite's requests are signed for unless they say otherwise.
pub const DEFAULT_REGION: &str = "us-east-1";

/// Call one operation and return its HTTP status and parsed body.
pub async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    call_in_region(router, DEFAULT_REGION, operation, body).await
}

/// Call one operation with the request signed for `region`, and return its HTTP status and
/// parsed body. The credential scope is what a handler reads the request's Region from, so
/// this is the seam for anything that is reported per Region.
pub async fn call_in_region(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let (status, _headers, parsed) = call_with_headers_in_region(router, region, operation, body).await;
    (status, parsed)
}

/// The same call, with the response HEADERS as well.
///
/// The seam for an answer that carries something its body does not — the header
/// `PutMetricData` reports a dropped entity in, say.
pub async fn call_with_headers(
    router: &MultiServiceRouter,
    operation: &str,
    body: &Value,
) -> (u16, http::HeaderMap, Value) {
    call_with_headers_in_region(router, DEFAULT_REGION, operation, body).await
}

/// The same, with the request signed for `region`.
pub async fn call_with_headers_in_region(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: &Value,
) -> (u16, http::HeaderMap, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
        // The SigV4 credential scope is what routes the request to the `monitoring`
        // signing name.
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{region}/monitoring/aws4_request, \
                 SignedHeaders=host;x-amz-date, Signature=deadbeef"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let headers = response.headers().clone();
    let payload = response.into_body();
    let parsed = if payload.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&payload).unwrap_or(Value::Null)
    };
    (status, headers, parsed)
}

/// Call one operation, requiring a 200, and return its body.
pub async fn ok(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, parsed) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "{operation}: expected HTTP 200, got {status} with body {parsed}"
    );
    parsed
}

/// The text a client reads off a refusal body.
///
/// The modelled error shapes spell the member `message` (`ResourceNotFound`,
/// `DashboardNotFoundError`) or `Message` (`ResourceNotFoundException`), and a refusal the
/// service answers with its error code alone leaves the member out, which a client reads as
/// the empty string — so one reader answers for every refusal this service makes.
pub fn error_message(body: &Value) -> String {
    ["message", "Message"]
        .iter()
        .find_map(|member| body.get(*member).and_then(Value::as_str))
        .unwrap_or_default()
        .to_string()
}

/// The wire error code of a failure body (`__type` on the awsJson protocols), without any
/// namespace prefix.
pub fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .map(|name| name.rsplit('#').next().unwrap_or(name).to_string())
        .unwrap_or_default()
}

/// A role ARN of the form a log alarm's scheduled query names
/// (`ScheduledQueryConfiguration$ScheduledQueryRoleARN`).
pub const QUERY_ROLE: &str = "arn:aws:iam::000000000000:role/cwl-scheduled-query";

/// Which kind of alarm a case creates and reads back: the three `AlarmType` values this
/// service stores (`smithy_query(query="shape",shape="AlarmType")`).
///
/// Every suite that asserts a rule holding for an alarm of ANY kind generalises over this,
/// so the three writers and the one reader live here once rather than per suite.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlarmKind {
    /// An alarm on a metric, written by `PutMetricAlarm`.
    Metric,
    /// An alarm on other alarms' states, written by `PutCompositeAlarm`.
    Composite,
    /// An alarm on a CloudWatch Logs scheduled query, written by `PutLogAlarm`.
    Log,
}

impl AlarmKind {
    /// The `DescribeAlarms` output member alarms of this kind are reported under.
    pub fn member(self) -> &'static str {
        match self {
            Self::Metric => "MetricAlarms",
            Self::Composite => "CompositeAlarms",
            Self::Log => "LogAlarms",
        }
    }

    /// The modelled `AlarmType` value a reader asks for this kind of alarm by: "If you omit
    /// this parameter, only metric alarms are returned" (`DescribeAlarmsInput$AlarmTypes`),
    /// so a reader of any other kind has to name it.
    pub fn alarm_type(self) -> &'static str {
        match self {
            Self::Metric => "MetricAlarm",
            Self::Composite => "CompositeAlarm",
            Self::Log => "LogAlarm",
        }
    }

    /// Create an alarm of this kind named `name`, carrying `actions_enabled` when the case
    /// states one and the documented default otherwise.
    pub async fn create(
        self,
        router: &MultiServiceRouter,
        name: &str,
        actions_enabled: Option<bool>,
    ) {
        let (operation, mut request) = match self {
            Self::Metric => ("PutMetricAlarm", metric_alarm_request(name)),
            Self::Composite => {
                // A composite alarm's rule references other alarms, so the case's own child
                // alarm is created first and the rule names that one.
                let child = format!("{name}-child");
                ok(router, "PutMetricAlarm", &metric_alarm_request(&child)).await;
                (
                    "PutCompositeAlarm",
                    ::serde_json::json!({
                        "AlarmName": name,
                        "AlarmRule": format!("ALARM(\"{child}\")"),
                    }),
                )
            }
            Self::Log => ("PutLogAlarm", log_alarm_request(name)),
        };
        if let (Some(enabled), Value::Object(members)) = (actions_enabled, &mut request) {
            members.insert(
                "ActionsEnabled".to_string(),
                ::serde_json::Value::Bool(enabled),
            );
        }
        ok(router, operation, &request).await;
    }
}

/// A static-threshold metric alarm request for `name`.
pub fn metric_alarm_request(name: &str) -> Value {
    ::serde_json::json!({
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

/// `request` with `patch` merged in: every member the patch names replaces the one the request
/// carried, and a member the patch names `null` is REMOVED — an alarm carries `Statistic` or
/// `ExtendedStatistic`, never both, so a case that names one has to drop the other.
///
/// Every table-driven suite that varies one request across rows merges its rows this way, so the
/// merge lives here once rather than beside each base request.
pub fn merged(mut request: Value, patch: &Value) -> Value {
    let members = request
        .as_object_mut()
        .expect("the request being patched is a JSON object");
    for (member, value) in patch.as_object().cloned().unwrap_or_default() {
        if value.is_null() {
            members.remove(&member);
        } else {
            members.insert(member, value);
        }
    }
    request
}

/// Whether two JSON values are the same value, comparing numbers numerically so an integral
/// double and an integer are not reported as a difference.
pub fn same_json_value(asked: &Value, reported: &Value) -> bool {
    match (asked.as_f64(), reported.as_f64()) {
        (Some(asked), Some(reported)) => (asked - reported).abs() <= f64::EPSILON,
        _ => asked == reported,
    }
}

/// A log alarm request for `name`: every member `PutLogAlarm` requires and nothing else.
pub fn log_alarm_request(name: &str) -> Value {
    ::serde_json::json!({
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

/// The one alarm of `kind` that `DescribeAlarms` reports for `name`, or `None` when it
/// reports none.
pub async fn described_alarm(
    router: &MultiServiceRouter,
    kind: AlarmKind,
    name: &str,
) -> Option<Value> {
    let body = ok(
        router,
        "DescribeAlarms",
        &::serde_json::json!({"AlarmNames": [name], "AlarmTypes": [kind.alarm_type()]}),
    )
    .await;
    body.get(kind.member())
        .and_then(Value::as_array)
        .and_then(|alarms| alarms.first().cloned())
}

/// The history items of type `item_type` recorded for `name`, as `DescribeAlarmHistory`
/// reports them.
///
/// The kind is named: a history reader, like an alarm reader, reports metric alarms unless it
/// is told otherwise (`DescribeAlarmHistoryInput$AlarmTypes`).
pub async fn alarm_history_items(
    router: &MultiServiceRouter,
    kind: AlarmKind,
    name: &str,
    item_type: &str,
) -> Vec<Value> {
    let body = ok(
        router,
        "DescribeAlarmHistory",
        &::serde_json::json!({
            "AlarmName": name,
            "AlarmTypes": [kind.alarm_type()],
            "HistoryItemType": item_type,
        }),
    )
    .await;
    body.get("AlarmHistoryItems")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// A timestamp member of a described alarm, as the epoch seconds the awsJson protocols carry.
pub fn alarm_stamp(alarm: &Value, member: &str) -> Option<f64> {
    alarm.get(member).and_then(Value::as_f64)
}
