// SPDX-License-Identifier: CC0-1.0

//! The harness every suite in this crate drives the emulator through, and the
//! helpers that read a response body.
//!
//! Every call goes over the wire: a real `POST /` with `X-Amz-Target:
//! RedshiftData.<Operation>` dispatched through the router, so what a suite
//! asserts is the emulator's HTTP behaviour and not a handler's return value.
//!
//! This module holds NO tests; it exists so that every suite in this crate
//! shares ONE harness rather than a copy of it each.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_redshiftdata_redshiftdata::state::MAX_ENGINE_DEADLINE;
use service_com_amazonaws_redshiftdata_redshiftdata::EmulatedService;

/// An `Id` in the shape the `UUID` pattern accepts that no statement was ever
/// minted with.
pub const ABSENT_ID: &str = "00000000-0000-0000-0000-000000000000";

/// Comfortably longer than a trivial statement's execution takes, so a
/// statement submitted without a wait has certainly reported its outcome by the
/// time this has passed.
pub const LONGER_THAN_A_QUICK_STATEMENT: Duration = Duration::from_millis(900);

/// A statement whose execution takes long enough to be observed WHILE it runs,
/// so the transient `SUBMITTED` state and the state-dependent behaviour that
/// goes with it (`CancelStatement` stops it, `GetStatementResult` refuses) are
/// observable without racing the engine. It counts rows in the engine rather
/// than sleeping, so the engine's own step budget and deadline bound it.
pub const SLOW_SQL: &str = "WITH RECURSIVE counter(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM \
                            counter WHERE x < 3000000) SELECT count(*) FROM counter";

/// A statement that runs until a bound stops it: it counts for ever and emits a
/// single row, so neither the row cap nor the byte cap can end it and the only
/// thing that can is the engine's own execution bound.
pub const UNBOUNDED_SQL: &str = "WITH RECURSIVE counter(x) AS (SELECT 1 UNION ALL SELECT x + 1 \
                                FROM counter) SELECT count(*) FROM counter";

/// The longest wait the model admits: `WaitTimeSeconds` carries the range 1..30,
/// and its documentation states "The maximum value is 30 seconds."
pub const LONGEST_MODELLED_WAIT_SECONDS: i32 = 30;

/// The `WaitTimeSeconds` every test that waits for a statement asks for: the
/// most patience the model admits, which is strictly longer than the engine's
/// own execution bound, so a statement's recorded outcome is the engine's
/// verdict and never the request running out of patience on a loaded machine.
///
/// A waited request answers as soon as the execution reports, so asking for the
/// longest wait costs a statement that finishes sooner nothing.
pub const WAIT_SECONDS: i32 = LONGEST_MODELLED_WAIT_SECONDS;

/// A wait no longer than the engine's own execution bound cannot see a statement
/// that bound stops: the execution is armed after the request starts waiting, so
/// it reports at the earliest when that wait is already over, and the waiting
/// caller reads the non-terminal status instead. Which bound stops a statement
/// first otherwise depends on the instruction rate the host sustains, so a
/// waiting suite would pass on one machine and fail on another. Checked here so
/// the two bounds cannot drift back together unnoticed.
const _: () = assert!(
    WAIT_SECONDS > 0 && MAX_ENGINE_DEADLINE.as_secs() < WAIT_SECONDS.unsigned_abs() as u64,
    "the wait a waiting test asks for must outlast the engine's own execution bound"
);

/// The shortest wait the model admits (`WaitTimeSeconds`, range 1..30).
pub const SHORTEST_MODELLED_WAIT_SECONDS: i32 = 1;

/// A statement the engine runs to completion as soon as it picks it up.
pub const QUICK_SQL: &str = "SELECT 1";

/// What the engine decides for a submission, whatever its caller waited for.
#[derive(Clone, Copy, Debug)]
pub enum Verdict {
    /// Every statement of the submission ran to completion.
    Finished,
    /// One of the engine's execution bounds stopped it, so it failed carrying
    /// the engine's own interruption message.
    Interrupted,
}

/// One row of a caller-patience table: the statements submitted, the patience
/// the submit asks for, the statuses that submit's own response may report, and
/// the verdict the engine must reach for it.
///
/// Shared by every suite that asserts what `WaitTimeSeconds` decides, so the
/// submit operations that carry it are driven from one shape rather than a copy
/// of it each.
pub struct PatienceRow {
    pub label: &'static str,
    /// The statements submitted: exactly one for `ExecuteStatement`, one or more
    /// for `BatchExecuteStatement`.
    pub sqls: &'static [&'static str],
    /// The `WaitTimeSeconds` the submit carries, or `None` for a submit that
    /// names no wait at all.
    pub wait: Option<i32>,
    /// The statuses the submit's own response may report. A submit that names no
    /// wait reports the status the submission has at that instant; a wait
    /// SHORTER than the service's whole execution bound reports whatever it has
    /// reached by the time that wait expires; only a wait that covers the
    /// service's execution bound is sure to carry the verdict.
    pub answered: &'static [&'static str],
    pub verdict: Verdict,
}

/// The verdict the engine reached, asserted on the record read back: the status
/// it settled at, and the engine's own reason when one of its execution bounds
/// stopped it.
pub fn assert_verdict(row: &PatienceRow, described: &Value) {
    match row.verdict {
        Verdict::Finished => {
            assert_eq!(
                described["Status"], "FINISHED",
                "case {}: a submission the engine completed must report FINISHED, got \
                 {described}",
                row.label
            );
            assert!(
                described["Error"].is_null(),
                "case {}: a submission that ran reports no error, got {described}",
                row.label
            );
        }
        Verdict::Interrupted => {
            assert_eq!(
                described["Status"], "FAILED",
                "case {}: a submission the engine's bound stopped must report FAILED, got \
                 {described}",
                row.label
            );
            let error = described["Error"].as_str().unwrap_or_default();
            assert!(
                error.to_ascii_lowercase().contains("interrupt"),
                "case {}: the engine's own interruption must say why it stopped, got {described}",
                row.label
            );
        }
    }
}

/// A lower bound, in seconds, on how long a statement that produced a result
/// ran for: its `UpdatedAt` is the instant its execution finished, so it is at
/// or after the instant it was submitted.
pub const RUN_TIME_LOWER_BOUND_SECONDS: f64 = 0.0;

/// The statuses a statement's lifecycle ends at.
pub const TERMINAL_STATUSES: [&str; 3] = ["FINISHED", "FAILED", "ABORTED"];

/// How long [`wait_for_terminal`] gives a submitted statement: the engine's own
/// execution bound and room to spare, so a poll gives up because a statement
/// never terminated and never because the machine running the test was busy.
const TERMINAL_STATUS_BUDGET: Duration =
    MAX_ENGINE_DEADLINE.saturating_add(Duration::from_secs(30));

/// How long to wait between two polls of a statement's status.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

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
    /// The wait asked for is [`WAIT_SECONDS`], the modelled ceiling, which
    /// OUTLASTS the engine's own execution bound — so what decides a statement's
    /// recorded outcome is the ENGINE's deadline and never this request's
    /// patience, however loaded the machine running the test is.
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

    /// Submit `request` — which carries the connection and the statements — with
    /// the patience `row` asks for, check the submit's OWN answer against the
    /// statuses that row admits, and answer the `Id` the service recorded it
    /// under.
    pub async fn submit_with_patience(
        &self,
        operation: &str,
        request: &Value,
        row: &PatienceRow,
    ) -> String {
        let mut request = request.clone();
        if let Some(wait) = row.wait {
            request["WaitTimeSeconds"] = json!(wait);
        }
        let response = self.ok(operation, &request).await;
        let answered = response["Status"].as_str().unwrap_or_default().to_string();
        assert!(
            row.answered.contains(&answered.as_str()),
            "case {}: a submit asking to wait {:?} must be answered with one of {:?}, got \
             {response}",
            row.label,
            row.wait,
            row.answered
        );
        id_of(&response)
    }
}

/// Poll `id` until it reports a terminal status, and answer the record it
/// reported. Gives up once [`TERMINAL_STATUS_BUDGET`] has passed: an execution
/// runs independently of the request that submitted it, so a statement that
/// never reaches a decision is a defect and not a slow machine.
pub async fn wait_for_terminal(emulator: &Emulator, case: &str, id: &str) -> Value {
    let started = Instant::now();
    loop {
        let described = emulator.ok("DescribeStatement", &json!({"Id": id})).await;
        let status = described["Status"].as_str().unwrap_or_default().to_string();
        if TERMINAL_STATUSES.contains(&status.as_str()) {
            return described;
        }
        assert!(
            started.elapsed() < TERMINAL_STATUS_BUDGET,
            "case {case}: {id} reported {status:?} for {:?} and never reached a terminal \
             status: {described}",
            started.elapsed()
        );
        tokio::time::sleep(POLL_INTERVAL).await;
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
