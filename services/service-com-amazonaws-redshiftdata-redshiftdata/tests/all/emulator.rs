// SPDX-License-Identifier: CC0-1.0

//! The harness every suite in this crate drives the emulator through, and the
//! helpers that read a response body.
//!
//! Every call goes over the wire: a real `POST /` with `X-Amz-Target:
//! RedshiftData.<Operation>` dispatched through the router, so what a suite
//! asserts is the emulator's HTTP behaviour and not a handler's return value.
//!
//! This module holds NO tests; it exists so that `statement_lifecycle` and
//! `batch_execute_statement` share one harness rather than two copies of it.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_redshiftdata_redshiftdata::EmulatedService;

/// An `Id` in the shape the `UUID` pattern accepts that no statement was ever
/// minted with.
pub const ABSENT_ID: &str = "00000000-0000-0000-0000-000000000000";

/// Comfortably longer than the span after which a submitted statement's terminal
/// status falls due (`state::COMPLETION_SPAN`).
pub const LONGER_THAN_COMPLETION_SPAN: Duration = Duration::from_millis(900);

/// The `WaitTimeSeconds` every test that waits for a statement asks for: inside
/// the modelled 1..30 range and far longer than the engine's own deadline, so a
/// statement's recorded outcome is the engine's verdict and never the request
/// running out of patience on a loaded machine.
pub const WAIT_SECONDS: i32 = 10;

/// A lower bound on that span, in seconds: the transition is owed a non-zero
/// span after submission, so a caller acting immediately still sees `SUBMITTED`.
pub const COMPLETION_SPAN_LOWER_BOUND_SECONDS: f64 = 0.2;

/// One emulator: a router with only this service mounted, over one shared engine.
pub struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    pub fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    /// Call one operation and return its HTTP status and JSON body.
    pub async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("RedshiftData.{operation}"))
            // The SigV4 credential scope is the router's primary routing signal;
            // signatures are not checked here (authentication is off), but the
            // scope still has to name this service the way an SDK would.
            .header(
                "authorization",
                "AWS4-HMAC-SHA256 \
                 Credential=AKIAEMULATORTEST/20260101/us-east-1/redshift-data/aws4_request, \
                 SignedHeaders=host;x-amz-date, Signature=00",
            )
            .body(bytes::Bytes::from(body.to_string()))
            .expect("build the request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let body = response.into_body();
        let parsed = if body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&body).unwrap_or_else(|_| {
                Value::String(String::from_utf8_lossy(body.as_ref()).into_owned())
            })
        };
        (status, parsed)
    }

    /// Call one operation and require a 200.
    pub async fn ok(&self, operation: &str, body: &Value) -> Value {
        let (status, response) = self.call(operation, body).await;
        assert_eq!(
            status, 200,
            "{operation} must succeed for {body}, got {status} {response}"
        );
        response
    }

    /// Submit one statement and wait for it, so the response already reports the
    /// terminal status. `WaitTimeSeconds` is what asks for that.
    ///
    /// The wait is asked generously (well inside the modelled 1..30 range) so
    /// that what decides a statement's recorded outcome is the ENGINE's own
    /// deadline and never this request's patience, however loaded the machine
    /// running the test is.
    pub async fn execute_and_wait(&self, request: &Value) -> Value {
        let mut request = request.clone();
        request["WaitTimeSeconds"] = json!(WAIT_SECONDS);
        self.ok("ExecuteStatement", &request).await
    }

    /// Submit `sql` against `my-cluster`/`dev`, wait for it, and return its `Id`.
    pub async fn run(&self, sql: &str) -> String {
        let response = self
            .execute_and_wait(&json!({
                "ClusterIdentifier": "my-cluster",
                "Database": "dev",
                "DbUser": "awsuser",
                "Sql": sql,
            }))
            .await;
        assert_eq!(
            response["Status"], "FINISHED",
            "a waited-for statement must report its terminal status: {sql} -> {response}"
        );
        id_of(&response)
    }

    /// Submit a batch and wait for it, so the response already reports the
    /// terminal status.
    pub async fn batch_and_wait(&self, request: &Value) -> Value {
        let mut request = request.clone();
        request["WaitTimeSeconds"] = json!(WAIT_SECONDS);
        self.ok("BatchExecuteStatement", &request).await
    }
}

/// The `Id` a response carries.
pub fn id_of(response: &Value) -> String {
    response["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("response carries no Id: {response}"))
        .to_string()
}

/// The wire error code of an error body (`__type` without its namespace).
pub fn error_code(response: &Value) -> String {
    let raw = response["__type"]
        .as_str()
        .or_else(|| response["code"].as_str())
        .or_else(|| response["Code"].as_str())
        .unwrap_or_else(|| panic!("error body carries no __type: {response}"));
    raw.rsplit('#').next().unwrap_or(raw).to_string()
}

/// A long-typed member, defaulted the way the model defaults it when the wire
/// omits it.
pub fn number(value: &Value, member: &str) -> i64 {
    value.get(member).and_then(Value::as_i64).unwrap_or(0)
}

/// A boolean member, defaulted to false when the wire omits it.
pub fn flag(value: &Value, member: &str) -> bool {
    value.get(member).and_then(Value::as_bool).unwrap_or(false)
}

/// A timestamp member, in seconds since the epoch.
pub fn timestamp(value: &Value, member: &str) -> f64 {
    value
        .get(member)
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("{member} is not a timestamp: {value}"))
}

/// A list member, defaulted to empty when the wire omits it.
pub fn list(value: &Value, member: &str) -> Vec<Value> {
    value
        .get(member)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}
