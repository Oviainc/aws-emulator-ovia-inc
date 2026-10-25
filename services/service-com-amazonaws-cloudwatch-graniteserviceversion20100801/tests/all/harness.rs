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
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("{target_prefix}.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTTESTTEST/20260101/{REGION}/{signing_name}/\
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

/// One awsQuery form call against SNS, the protocol that service serves.
pub async fn sns(emulator: &Emulator, action: &str, params: &[(&str, &str)]) -> String {
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
        .header("host", format!("sns.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTTESTTEST/20260101/{REGION}/sns/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(body))
        .expect("the test request is well formed");
    let response = emulator.router.dispatch(request).await;
    let status = response.status().as_u16();
    let text = String::from_utf8_lossy(response.body()).to_string();
    assert_eq!(status, 200, "{action}: expected HTTP 200, got {status} / {text}");
    text
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
    let payload = response.into_body();
    let parsed = if payload.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&payload).unwrap_or(Value::Null)
    };
    (status, parsed)
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

/// The wire error code of a failure body (`__type` on the awsJson protocols), without any
/// namespace prefix.
pub fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .map(|name| name.rsplit('#').next().unwrap_or(name).to_string())
        .unwrap_or_default()
}
