// SPDX-License-Identifier: CC0-1.0

//! The harness every suite in this target shares: one service mounted on one shared
//! engine, reached over the wire.
//!
//! A test drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so a
//! response that is not computed from the shared store cannot pass. Keeping that walk
//! here means a suite holds only the rules it is about.

use std::sync::Arc;

use serde_json::Value;

/// The region the tests' SigV4 credential scope names, so an ARN built from the request
/// is distinguishable from one built from a hardcoded default.
pub const REGION: &str = "eu-west-2";

/// One service mounted on one shared engine, reached over the wire.
pub struct Logs {
    router: smithy_server_runtime::MultiServiceRouter,
}

impl Logs {
    pub fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = smithy_server_runtime::MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_cloudwatchlogs_logs_20140328::EmulatedService::new(engine),
        );
        Self { router }
    }

    /// Call one operation, returning its HTTP status and parsed JSON body.
    pub async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("Logs_20140328.{operation}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/logs/aws4_request, \
                     SignedHeaders=host;x-amz-target, Signature=0123456789abcdef"
                ),
            )
            .body(bytes::Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a valid request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// Call one operation that must succeed, returning its response body.
    pub async fn ok(&self, operation: &str, body: &Value) -> Value {
        let (status, parsed) = self.call(operation, body).await;
        assert_eq!(
            status, 200,
            "{operation} {body}: expected HTTP 200, got {status} {parsed}"
        );
        parsed
    }

    /// Call one operation that must be refused as a client error, returning its wire
    /// error code.
    pub async fn err(&self, operation: &str, body: &Value) -> String {
        let (status, parsed) = self.call(operation, body).await;
        assert_eq!(
            status, 400,
            "{operation} {body}: expected HTTP 400, got {status} {parsed}"
        );
        error_code(&parsed)
    }

    /// Call one operation that must be refused as a client error, returning its wire
    /// error code AND the message it reports — for a refusal whose message the service
    /// builds from the request, so the message carries the rule.
    pub async fn refusal(&self, operation: &str, body: &Value) -> (String, String) {
        let (status, parsed) = self.call(operation, body).await;
        assert_eq!(
            status, 400,
            "{operation} {body}: expected HTTP 400, got {status} {parsed}"
        );
        let message = parsed
            .get("message")
            .or_else(|| parsed.get("Message"))
            .and_then(Value::as_str)
            .unwrap_or_else(|| {
                panic!("{operation} {body}: a refusal carries a message, got {parsed}")
            })
            .to_string();
        (error_code(&parsed), message)
    }

    /// The one log group the service reports under `identifier` (a name or either ARN
    /// form), or `None` once it is gone. Every suite that reads a log group's state back
    /// goes through this, so what it asserts is what a client can observe.
    pub async fn describe_group(&self, identifier: &str) -> Option<Value> {
        let page = self
            .ok(
                "DescribeLogGroups",
                &serde_json::json!({"logGroupIdentifiers": [identifier]}),
            )
            .await;
        let groups = page
            .get("logGroups")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("DescribeLogGroups must return logGroups, got {page}"))
            .clone();
        assert!(
            groups.len() <= 1,
            "case {identifier}: at most one log group is reported under one identifier, got {groups:?}"
        );
        groups.into_iter().next()
    }

    /// Both ARN forms a log group carries: `arn` (with the trailing `:*`) and
    /// `logGroupArn` (without it).
    pub async fn group_arns(&self, name: &str) -> (String, String) {
        let group = self
            .describe_group(name)
            .await
            .unwrap_or_else(|| panic!("case {name}: the log group must exist"));
        let member = |key: &str| {
            group
                .get(key)
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {name}: a log group carries {key}, got {group}"))
                .to_string()
        };
        (member("arn"), member("logGroupArn"))
    }
}

/// The wire error code of an awsJson error body (`__type`, which may carry a namespace).
pub fn error_code(body: &Value) -> String {
    let raw = body
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("an error body must carry __type, got {body}"));
    raw.rsplit(['#', '/']).next().unwrap_or(raw).to_string()
}

/// Wall-clock milliseconds, the unit every CloudWatch Logs timestamp member carries.
pub fn now_ms() -> i64 {
    i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("the clock is after the Unix epoch")
            .as_millis(),
    )
    .expect("the epoch milliseconds fit in an i64")
}

/// An in-window base timestamp: one hour before the ingestion clock. `PutLogEvents`
/// admits each event against a clock-relative ingestion window (14 days in the past to
/// 2 hours in the future), so a suite whose subject is NOT that window stamps its events
/// from this base plus a small offset and stays comfortably inside it. The window itself
/// is the subject of `tests/all/ingestion_window.rs`.
pub fn base_ms() -> i64 {
    now_ms() - 60 * 60 * 1_000
}

/// A log group plus one stream, both created, with a retention in days when one is
/// given. The whole of this target's core loop starts from this pair, so it lives here
/// rather than once per suite.
pub async fn group_with_stream(
    logs: &Logs,
    group: &str,
    stream: &str,
    retention_in_days: Option<i64>,
) {
    logs.ok("CreateLogGroup", &serde_json::json!({"logGroupName": group}))
        .await;
    if let Some(days) = retention_in_days {
        logs.ok(
            "PutRetentionPolicy",
            &serde_json::json!({"logGroupName": group, "retentionInDays": days}),
        )
        .await;
    }
    logs.ok(
        "CreateLogStream",
        &serde_json::json!({"logGroupName": group, "logStreamName": stream}),
    )
    .await;
}
