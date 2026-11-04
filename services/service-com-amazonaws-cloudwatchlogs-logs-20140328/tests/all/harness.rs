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

use service_com_amazonaws_cloudwatchlogs_logs_20140328::ops::types::{Anomaly, LogEvent};
use service_com_amazonaws_cloudwatchlogs_logs_20140328::state::{
    ComAmazonawsCloudwatchlogsLogs20140328State, ANOMALY_STATE_ACTIVE,
    EXPORT_TASK_COMPLETION_DELAY_MS, EXPORT_TASK_RUNNING_DELAY_MS,
};

/// The region the tests' SigV4 credential scope names, so an ARN built from the request
/// is distinguishable from one built from a hardcoded default.
pub const REGION: &str = "eu-west-2";

/// One service mounted on one shared engine, reached over the wire.
pub struct Logs {
    router: smithy_server_runtime::MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Logs {
    pub fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = smithy_server_runtime::MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_cloudwatchlogs_logs_20140328::EmulatedService::new(Arc::clone(
                &engine,
            )),
        );
        Self { router, engine }
    }

    /// The ONE shared engine this service is mounted on — the store every request through
    /// [`Logs::call`] reads and writes.
    ///
    /// A suite reaches for this only to stand up a resource whose CREATING operation is
    /// still a scaffold stub, so there is no wire call that can put one in the store;
    /// everything it then asserts goes over the wire like any client's call.
    pub fn engine(&self) -> &Arc<engine::InMemoryEngine> {
        &self.engine
    }

    /// Call one operation, returning its HTTP status and parsed JSON body.
    pub async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        self.call_in_region(REGION, operation, body).await
    }

    /// Call one operation with the SigV4 credential scope naming `region`, for a rule that is
    /// about which Region a request was made in rather than about its body.
    pub async fn call_in_region(
        &self,
        region: &str,
        operation: &str,
        body: &Value,
    ) -> (u16, Value) {
        let (status, _headers, payload) = self.call_raw_in_region(region, operation, body).await;
        let parsed = if payload.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&payload).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// One RAW exchange: the status, the response headers and the body bytes exactly as the
    /// service answered them.
    ///
    /// Every other call here reads the body as JSON. One operation's answer is not JSON —
    /// `StartLiveTail` serves `application/vnd.amazon.eventstream` frames — so its suite
    /// reads the bytes and the content type through this.
    pub async fn call_raw(
        &self,
        operation: &str,
        body: &Value,
    ) -> (u16, http::HeaderMap, bytes::Bytes) {
        self.call_raw_in_region(REGION, operation, body).await
    }

    /// [`Logs::call_raw`], with the credential scope naming `region`.
    pub async fn call_raw_in_region(
        &self,
        region: &str,
        operation: &str,
        body: &Value,
    ) -> (u16, http::HeaderMap, bytes::Bytes) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("Logs_20140328.{operation}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/logs/aws4_request, \
                     SignedHeaders=host;x-amz-target, Signature=0123456789abcdef"
                ),
            )
            .body(bytes::Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a valid request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let headers = response.headers().clone();
        (status, headers, response.into_body())
    }

    /// Call one operation that must succeed, returning its response body.
    pub async fn ok(&self, operation: &str, body: &Value) -> Value {
        self.ok_in_region(REGION, operation, body).await
    }

    /// [`Logs::ok`], with the credential scope naming `region`.
    pub async fn ok_in_region(&self, region: &str, operation: &str, body: &Value) -> Value {
        let (status, parsed) = self.call_in_region(region, operation, body).await;
        assert_eq!(
            status, 200,
            "{operation} {body} in {region}: expected HTTP 200, got {status} {parsed}"
        );
        parsed
    }

    /// Call one operation that must be refused as a client error, returning its wire
    /// error code.
    pub async fn err(&self, operation: &str, body: &Value) -> String {
        self.err_in_region(REGION, operation, body).await
    }

    /// [`Logs::err`], with the credential scope naming `region`.
    pub async fn err_in_region(&self, region: &str, operation: &str, body: &Value) -> String {
        let (status, parsed) = self.call_in_region(region, operation, body).await;
        assert_eq!(
            status, 400,
            "{operation} {body} in {region}: expected HTTP 400, got {status} {parsed}"
        );
        error_code(&parsed)
    }

    /// Call one operation that must be refused as a client error, returning its wire
    /// error code AND the message it reports — for a refusal whose message the service
    /// builds from the request, so the message carries the rule.
    pub async fn refusal(&self, operation: &str, body: &Value) -> (String, String) {
        self.refusal_in_region(REGION, operation, body).await
    }

    /// [`Logs::refusal`], with the credential scope naming `region`.
    pub async fn refusal_in_region(
        &self,
        region: &str,
        operation: &str,
        body: &Value,
    ) -> (String, String) {
        let (status, parsed) = self.call_in_region(region, operation, body).await;
        assert_eq!(
            status, 400,
            "{operation} {body} in {region}: expected HTTP 400, got {status} {parsed}"
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

/// Room for a test's own scheduling on top of a span the export-task clock owes, so a sleep
/// that has to land past one of those instants does even on a loaded host.
const EXPORT_SCHEDULING_SLACK_MS: i64 = 1_000;

/// A span of the clock as a [`std::time::Duration`].
fn span(millis: i64) -> std::time::Duration {
    std::time::Duration::from_millis(
        u64::try_from(millis).expect("a span of the clock is not negative"),
    )
}

/// How long to wait to catch a started export task mid-export: past the instant it begins
/// moving data and well before the instant it completes.
///
/// Every export suite waits on the same two spans the create arms, so both waits are derived
/// from those constants here rather than restated — or, worse, hardcoded — once per suite.
pub fn mid_export() -> std::time::Duration {
    span(EXPORT_TASK_RUNNING_DELAY_MS + EXPORT_SCHEDULING_SLACK_MS)
}

/// How long to wait before treating a started export task as finished: past both the spans the
/// create armed. See [`mid_export`].
pub fn past_completion() -> std::time::Duration {
    span(
        EXPORT_TASK_RUNNING_DELAY_MS + EXPORT_TASK_COMPLETION_DELAY_MS + EXPORT_SCHEDULING_SLACK_MS,
    )
}

/// An in-window base timestamp: one hour before the ingestion clock. `PutLogEvents`
/// admits each event against a clock-relative ingestion window (14 days in the past to
/// 2 hours in the future), so a suite whose subject is NOT that window stamps its events
/// from this base plus a small offset and stays comfortably inside it. The window itself
/// is the subject of `tests/all/ingestion_window.rs`.
pub fn base_ms() -> i64 {
    now_ms() - 60 * 60 * 1_000
}

/// A `logEvents` batch for `PutLogEvents`, built from `(timestamp, message)` pairs in the
/// chronological order a batch must be written in. Shared here because every suite that
/// writes events needs it.
pub fn batch(events: &[(i64, &str)]) -> Value {
    Value::Array(
        events
            .iter()
            .map(|(timestamp, message)| serde_json::json!({"timestamp": timestamp, "message": message}))
            .collect(),
    )
}

/// The `message` values of a `GetLogEvents` page, in the order returned. Shared here
/// because every suite that reads events back compares them by message.
pub fn messages(page: &Value) -> Vec<String> {
    page.get("events")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("GetLogEvents must return events, got {page}"))
        .iter()
        .map(|event| {
            event
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("every event carries a message, got {event}"))
                .to_string()
        })
        .collect()
}

/// Record, in the store, that the detector under `arn` has REPORTED the anomaly `anomaly`
/// of the pattern `pattern` — what a detection pass over the watched group's events writes.
///
/// No REQUEST reports an anomaly ("The anomaly detector uses pattern recognition to find
/// anomalies"), so every suite whose subject is what a client then does with a finding —
/// listing it, suppressing it, deleting the detector that holds it — seeds it here, as the
/// modelled `Anomaly` shape, and asserts the rest over the wire.
pub fn report_anomaly(logs: &Logs, arn: &str, anomaly: &str, pattern: &str) {
    let state = logs
        .engine()
        .state::<ComAmazonawsCloudwatchlogsLogs20140328State>();
    let mut store = state.lock();
    let detector = store
        .anomaly_detectors
        .get_mut(arn)
        .unwrap_or_else(|| panic!("the detector under {arn} is stored"));
    let first_seen = now_ms() - 60 * 1_000;
    detector.anomalies.insert(
        anomaly.to_string(),
        Anomaly {
            anomaly_id: anomaly.to_string(),
            pattern_id: pattern.to_string(),
            anomaly_detector_arn: arn.to_string(),
            pattern_string: "ERROR <*> while handling <*>".to_string(),
            first_seen,
            last_seen: first_seen + 30 * 1_000,
            description: "an unusual pattern".to_string(),
            active: true,
            state: ANOMALY_STATE_ACTIVE.to_string(),
            log_samples: vec![LogEvent {
                timestamp: Some(first_seen),
                message: Some("ERROR 42 while handling /orders".to_string()),
            }],
            ..Anomaly::default()
        },
    );
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
