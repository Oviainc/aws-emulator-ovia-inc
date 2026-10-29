// SPDX-License-Identifier: CC0-1.0

//! The harness the device-facing suites of this test crate share.
//!
//! Every suite that drives this plane needs the same four things: an engine holding
//! the one command its executions are started from, an HTTP call through the
//! multi-service router (so what is asserted is the wire behaviour a device agent
//! sees), a way to put a stored execution into the state a row asks about, and a way
//! to read the SHARED record back the way the IoT control plane reading the same
//! record would. They live here once, and each suite keeps only its own table and
//! its own assertions.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_iotjobsdataplane_iotlaserthingjobmanagerexternalservice::state::{
    command_registry, iot_commands::CommandFacts, service_state, JobExecutionRecord,
};
use service_com_amazonaws_iotjobsdataplane_iotlaserthingjobmanagerexternalservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 `Authorization` header whose credential scope names this service's
/// signing name, which is how the router picks the service for a request.
const SIGV4: &str = "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/us-east-1/iot-jobs-data/aws4_request, SignedHeaders=host, Signature=0";

/// The command every execution these suites start runs: a control-plane resource, so
/// the shared index is seeded with it by [`Emulator::new`].
pub(crate) const COMMAND_ARN: &str = "arn:aws:iot:us-east-1:111122223333:command/reboot";

/// A terminal value of the model's `JobExecutionStatus` that a device reported
/// before any deadline passed, for the rows that prove an execution which finished
/// on its own is left exactly as it finished.
pub(crate) const STATUS_SUCCEEDED: &str = "SUCCEEDED";

/// One emulator holding one shared engine, driven over HTTP.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        // An execution starts only for a command that EXISTS; commands are the
        // control plane's resource (`iot:CreateCommand`, a scaffold in its own crate
        // under its own issue), so the shared index is seeded with the one command
        // these suites start — the same write that create makes. The refusal an
        // unregistered command gets is asserted in `command_execution_validation.rs`.
        command_registry(&*engine)
            .expect("the shared IoT command index")
            .lock()
            .register(COMMAND_ARN, CommandFacts::default())
            .expect("register the command these tests start");
        let mut router = MultiServiceRouter::new();
        // These tests exercise handler behaviour, not request signing.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    async fn call(&self, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        let payload = body.map_or_else(Bytes::new, |body| Bytes::from(body.to_string()));
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://data.jobs.iot.us-east-1.amazonaws.com{path}"))
            .header("authorization", SIGV4)
            .header("content-type", "application/json")
            .body(payload)
            .expect("build the request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// Start a command execution on `thing`, optionally with the
    /// `executionTimeoutSeconds` timer the start arms, and answer the minted id —
    /// the `jobId` the device's own operations name the execution by.
    pub(crate) async fn start_command_on(
        &self,
        thing: &str,
        timeout_seconds: Option<i64>,
    ) -> String {
        let mut body = json!({
            "targetArn": format!("arn:aws:iot:us-east-1:111122223333:thing/{thing}"),
            "commandArn": COMMAND_ARN,
        });
        if let Some(seconds) = timeout_seconds {
            body["executionTimeoutSeconds"] = json!(seconds);
        }
        let (status, answered) = self.call("POST", "/command-executions", Some(body)).await;
        assert_eq!(
            status, 200,
            "starting a command on {thing} must answer 200, got {status} {answered}"
        );
        answered["executionId"]
            .as_str()
            .unwrap_or_else(|| panic!("expected an executionId, got {answered}"))
            .to_string()
    }

    pub(crate) async fn pending(&self, thing: &str) -> (u16, Value) {
        self.call("GET", &format!("/things/{thing}/jobs"), None).await
    }

    /// `DescribeJobExecution` on `job_id` (which may be the `$next` label), with
    /// `query` carrying this read's `includeJobDocument` / `executionNumber` or
    /// empty for a request that sends neither.
    pub(crate) async fn describe(&self, thing: &str, job_id: &str, query: &str) -> (u16, Value) {
        self.call("GET", &format!("/things/{thing}/jobs/{job_id}{query}"), None)
            .await
    }

    pub(crate) async fn start_next(&self, thing: &str, body: Value) -> (u16, Value) {
        self.call("PUT", &format!("/things/{thing}/jobs/$next"), Some(body))
            .await
    }

    pub(crate) async fn update(&self, thing: &str, job_id: &str, body: Value) -> (u16, Value) {
        self.call("POST", &format!("/things/{thing}/jobs/{job_id}"), Some(body))
            .await
    }

    /// Put one stored execution into the state a row asks about: the status a device
    /// drove it to and the deadlines its timers were armed with. This is the state a
    /// live service reaches by waiting, without a test that sleeps for it.
    pub(crate) fn with_stored_execution<F>(&self, thing: &str, job_id: &str, mutate: F)
    where
        F: FnOnce(&mut JobExecutionRecord),
    {
        let store = service_state(&*self.engine).expect("the shared job-execution store");
        let mut store = store.lock();
        let index = store
            .execution_index(thing, job_id, None)
            .unwrap_or_else(|| panic!("{thing} must hold the execution {job_id}"));
        mutate(
            store
                .record_mut(thing, index)
                .expect("the index came from this store"),
        );
    }

    /// The status and `lastUpdatedAt` the SHARED store holds — what the IoT control
    /// plane, reading the same record, would report.
    pub(crate) fn stored(&self, thing: &str, job_id: &str) -> (String, i64) {
        let record = self.stored_execution(thing, job_id);
        (record.status, record.last_updated_at)
    }

    /// The whole of the SHARED record — the one the IoT control plane reads too — for
    /// a suite that asserts on more of it than the status: the version a device sees,
    /// the stamps, and the deadlines the record's timers still carry.
    pub(crate) fn stored_execution(&self, thing: &str, job_id: &str) -> StoredExecution {
        let store = service_state(&*self.engine).expect("the shared job-execution store");
        let store = store.lock();
        store
            .execution_index(thing, job_id, None)
            .and_then(|index| store.record(thing, index))
            .map(|record| StoredExecution {
                status: record.status.clone(),
                version_number: record.version_number,
                started_at: record.started_at,
                last_updated_at: record.last_updated_at,
                step_timeout_expires_at: record.step_timeout_expires_at,
                execution_timeout_expires_at: record.execution_timeout_expires_at,
            })
            .unwrap_or_else(|| panic!("{thing} must hold the execution {job_id}"))
    }
}

/// A stored execution as these suites assert on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StoredExecution {
    pub(crate) status: String,
    pub(crate) version_number: i64,
    pub(crate) started_at: Option<i64>,
    pub(crate) last_updated_at: i64,
    pub(crate) step_timeout_expires_at: Option<i64>,
    pub(crate) execution_timeout_expires_at: Option<i64>,
}

impl StoredExecution {
    /// The instant this record would move to `TIMED_OUT` at: the SOONEST armed
    /// timer's deadline, or `None` while no timer is armed.
    pub(crate) fn deadline(&self) -> Option<i64> {
        [self.step_timeout_expires_at, self.execution_timeout_expires_at]
            .into_iter()
            .flatten()
            .min()
    }
}

/// The wire error code of a modeled failure (restJson1 carries it in `__type`).
pub(crate) fn error_code(body: &Value) -> &str {
    body["__type"].as_str().unwrap_or("")
}

/// The `jobId`s of one pending list, in the order the service returned them.
pub(crate) fn job_ids(list: &Value) -> Vec<String> {
    list.as_array()
        .unwrap_or_else(|| panic!("expected a list of summaries, got {list}"))
        .iter()
        .map(|summary| summary["jobId"].as_str().unwrap_or_default().to_string())
        .collect()
}
