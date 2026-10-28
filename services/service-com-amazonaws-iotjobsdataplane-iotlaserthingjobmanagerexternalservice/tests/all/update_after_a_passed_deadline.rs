// SPDX-License-Identifier: CC0-1.0

//! What `UpdateJobExecution` answers a device that reports AFTER its own deadline.
//!
//! The rule under test: the status both of this operation's state refusals are
//! decided from is the execution's status at the CURRENT instant, so the `TIMED_OUT`
//! transition a timer already made due is in force before the device's `status` is
//! considered ("If the job execution status is not set to a terminal state before
//! this timer expires ... the job execution status will be automatically set to
//! TIMED_OUT" — `UpdateJobExecutionRequest$stepTimeoutInMinutes`, and the same for
//! the timer a command start arms). A late update therefore meets a FINISHED
//! execution: it is refused 409 `InvalidStateTransitionException` naming the status
//! the clock decided, and it writes nothing at all — the status stays `TIMED_OUT`,
//! `versionNumber` does not move, `lastUpdatedAt` stays at the DEADLINE the move was
//! owed at, and neither the `statusDetails` nor the `stepTimeoutInMinutes` the late
//! request carried reaches the record.
//!
//! The table spans which timer armed the deadline (the command start's
//! `executionTimeoutSeconds`, the device's own `stepTimeoutInMinutes`, both at once),
//! how far past that deadline the report arrives (exactly due, a second, an hour, a
//! week), and the stage the execution was in when the deadline passed (`QUEUED`,
//! never taken up, and `IN_PROGRESS`). Each row is then driven with EVERY value of
//! the model's `JobExecutionStatus` catalog and with each optional member of the
//! request (`statusDetails`, `stepTimeoutInMinutes`, `expectedVersion`,
//! `includeJobExecutionState`), since the refusal depends on none of them.
//!
//! The counterpart test keeps the table honest: the same requests against the same
//! rows, with the deadline still to COME, are accepted and do write.
//!
//! Every request goes over the wire (restJson1 through the multi-service router), so
//! what is asserted is the HTTP behaviour a device agent sees.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_iotjobsdataplane_iotlaserthingjobmanagerexternalservice::ops::types::JOB_EXECUTION_STATUS_VALUES;
use service_com_amazonaws_iotjobsdataplane_iotlaserthingjobmanagerexternalservice::state::{
    command_registry, iot_commands::CommandFacts, now_epoch_seconds, service_state,
};
use service_com_amazonaws_iotjobsdataplane_iotlaserthingjobmanagerexternalservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 `Authorization` header whose credential scope names this service's
/// signing name, which is how the router picks the service for a request.
const SIGV4: &str = "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/us-east-1/iot-jobs-data/aws4_request, SignedHeaders=host, Signature=0";

const COMMAND_ARN: &str = "arn:aws:iot:us-east-1:111122223333:command/reboot";

/// The status the clock moves an execution to when a timer armed on it expires, and
/// the one the refusal must name whatever the device reported.
const STATUS_TIMED_OUT: &str = "TIMED_OUT";

/// How far BEYOND a row's own distance the counterpart test puts the deadline, in
/// seconds, so a deadline "still to come" cannot pass while the test's own requests
/// run.
const SECONDS_OF_HEADROOM: i64 = 300;

/// One emulator holding one shared engine, driven over HTTP.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        // An execution starts only for a command that EXISTS, and commands are the
        // control plane's resource, so the shared index is seeded with the one
        // command these rows start.
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

    /// Start a command execution on `thing`, arming the command timer when
    /// `execution_timeout_seconds` is given, and answer the minted id — the `jobId`
    /// the device's own operations name the execution by.
    async fn start_command_on(
        &self,
        thing: &str,
        execution_timeout_seconds: Option<i64>,
    ) -> String {
        let mut body = json!({
            "targetArn": format!("arn:aws:iot:us-east-1:111122223333:thing/{thing}"),
            "commandArn": COMMAND_ARN,
        });
        if let Some(seconds) = execution_timeout_seconds {
            body["executionTimeoutSeconds"] = json!(seconds);
        }
        let (status, started) = self.call("POST", "/command-executions", Some(body)).await;
        assert_eq!(
            status, 200,
            "starting a command on {thing} must answer 200, got {status} {started}"
        );
        started["executionId"]
            .as_str()
            .unwrap_or_else(|| panic!("expected an executionId, got {started}"))
            .to_string()
    }

    async fn describe(&self, thing: &str, job_id: &str) -> (u16, Value) {
        self.call("GET", &format!("/things/{thing}/jobs/{job_id}"), None)
            .await
    }

    async fn update(&self, thing: &str, job_id: &str, body: Value) -> (u16, Value) {
        self.call("POST", &format!("/things/{thing}/jobs/{job_id}"), Some(body))
            .await
    }

    /// Move every deadline ARMED on the stored execution to `instant`, which is how
    /// a live service reaches the state a row describes — time passing — without a
    /// test that sleeps for it. A timer no request armed stays unarmed.
    fn move_armed_deadlines_to(&self, thing: &str, job_id: &str, instant: i64) {
        let store = service_state(&*self.engine).expect("the shared job-execution store");
        let mut store = store.lock();
        let index = store
            .execution_index(thing, job_id, None)
            .unwrap_or_else(|| panic!("{thing} must hold the execution {job_id}"));
        let record = store
            .record_mut(thing, index)
            .expect("the index came from this store");
        record.step_timeout_expires_at = record.step_timeout_expires_at.map(|_| instant);
        record.execution_timeout_expires_at = record.execution_timeout_expires_at.map(|_| instant);
    }

    /// What the SHARED store holds for an execution — the record the IoT control
    /// plane reads too: (status, `lastUpdatedAt`, `versionNumber`, `statusDetails`,
    /// whether any timer is still armed).
    fn stored(&self, thing: &str, job_id: &str) -> StoredExecution {
        let store = service_state(&*self.engine).expect("the shared job-execution store");
        let store = store.lock();
        store
            .execution_index(thing, job_id, None)
            .and_then(|index| store.record(thing, index))
            .map(|record| StoredExecution {
                status: record.status.clone(),
                last_updated_at: record.last_updated_at,
                version_number: record.version_number,
                // The stored details are a `BTreeMap`, so its keys are already in
                // one order whatever order they were written in.
                status_detail_names: record.status_details.keys().cloned().collect(),
                has_armed_timer: record.step_timeout_expires_at.is_some()
                    || record.execution_timeout_expires_at.is_some(),
            })
            .unwrap_or_else(|| panic!("{thing} must hold the execution {job_id}"))
    }
}

/// The part of a stored execution these rules are about.
#[derive(Debug, Clone, PartialEq, Eq)]
struct StoredExecution {
    status: String,
    last_updated_at: i64,
    version_number: i64,
    status_detail_names: Vec<String>,
    has_armed_timer: bool,
}

/// The wire error code of a modeled failure (restJson1 carries it in `__type`).
fn error_code(body: &Value) -> &str {
    body["__type"].as_str().unwrap_or("")
}

/// One row: an execution whose timers were armed by real requests, taken up or not,
/// whose soonest deadline then sits `past_by` seconds in the past (or, in the
/// counterpart test, that many seconds in the future).
struct LateReportCase {
    case: &'static str,
    /// One device per row, so the rows cannot interfere with each other.
    thing: &'static str,
    /// `executionTimeoutSeconds` the command start arms the execution timer with.
    execution_timeout_seconds: Option<i64>,
    /// `stepTimeoutInMinutes` the device arms the step timer with, through an
    /// `IN_PROGRESS` update that also takes the execution up.
    step_timeout_minutes: Option<i64>,
    /// How far from the deadline the device's report arrives, in seconds: `0` is a
    /// deadline exactly due.
    distance_seconds: i64,
}

/// The table both tests run.
fn late_report_cases() -> Vec<LateReportCase> {
    vec![
        LateReportCase {
            case: "the command start's timer, exactly due, never taken up",
            thing: "laser-exec-due",
            execution_timeout_seconds: Some(60),
            step_timeout_minutes: None,
            distance_seconds: 0,
        },
        LateReportCase {
            case: "the command start's timer, a second past, never taken up",
            thing: "laser-exec-second",
            execution_timeout_seconds: Some(1),
            step_timeout_minutes: None,
            distance_seconds: 1,
        },
        LateReportCase {
            case: "the device's own step timer, an hour past, in progress",
            thing: "laser-step-hour",
            execution_timeout_seconds: None,
            step_timeout_minutes: Some(1),
            distance_seconds: 3_600,
        },
        LateReportCase {
            case: "both timers armed, a week past, in progress",
            thing: "plant:laser:both",
            execution_timeout_seconds: Some(600),
            step_timeout_minutes: Some(10_080),
            distance_seconds: 604_800,
        },
        LateReportCase {
            case: "the maximum step timeout, a minute past, in progress",
            thing: "laser_step-max",
            execution_timeout_seconds: None,
            step_timeout_minutes: Some(10_080),
            distance_seconds: 60,
        },
    ]
}

/// The update bodies every row is driven with: every value of the model's
/// `JobExecutionStatus` catalog, and then the same report carrying each optional
/// member of the request — none of which the refusal depends on.
fn late_update_bodies(stored_version: i64) -> Vec<(String, Value)> {
    let mut bodies: Vec<(String, Value)> = JOB_EXECUTION_STATUS_VALUES
        .iter()
        .map(|status| {
            (
                format!("status={status}"),
                json!({"status": status, "includeJobExecutionState": true}),
            )
        })
        .collect();
    bodies.push((
        "status=SUCCEEDED with statusDetails".to_string(),
        json!({"status": "SUCCEEDED", "statusDetails": {"stage": "late"}}),
    ));
    bodies.push((
        "status=IN_PROGRESS resetting the step timer".to_string(),
        json!({"status": "IN_PROGRESS", "stepTimeoutInMinutes": 10}),
    ));
    bodies.push((
        "status=IN_PROGRESS cancelling the step timer".to_string(),
        json!({"status": "IN_PROGRESS", "stepTimeoutInMinutes": -1}),
    ));
    bodies.push((
        "status=FAILED naming the stored version".to_string(),
        json!({"status": "FAILED", "expectedVersion": stored_version}),
    ));
    bodies
}

/// Put one row's execution into the state it describes and answer its id, the
/// deadline its timers now carry and the record as the store holds it.
async fn seed(
    emulator: &Emulator,
    case: &LateReportCase,
    deadline_offset: i64,
) -> (String, i64, StoredExecution) {
    let label = case.case;
    let id = emulator
        .start_command_on(case.thing, case.execution_timeout_seconds)
        .await;
    if let Some(minutes) = case.step_timeout_minutes {
        // The device arms its own step timer by reporting IN_PROGRESS, which is also
        // what takes the execution up — both through the real handler.
        let (status, taken_up) = emulator
            .update(
                case.thing,
                &id,
                json!({"status": "IN_PROGRESS", "stepTimeoutInMinutes": minutes}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: arming the step timer must answer 200, got {status} {taken_up}"
        );
    }
    let deadline = now_epoch_seconds() + deadline_offset;
    emulator.move_armed_deadlines_to(case.thing, &id, deadline);
    let before = emulator.stored(case.thing, &id);
    assert!(
        before.has_armed_timer,
        "case {label:?}: the row must arm a timer, got {before:?}"
    );
    (id, deadline, before)
}

/// RULE: both state refusals of `UpdateJobExecution` are decided from the status the
/// execution has at the CURRENT instant, so an armed timer whose deadline has passed
/// has already carried the execution to `TIMED_OUT` — a device reporting after it
/// meets a FINISHED execution. The report is refused 409
/// `InvalidStateTransitionException`, whose message names the status the clock
/// decided and not the one the device reported, and nothing is written: the status,
/// `versionNumber`, `statusDetails` and the `lastUpdatedAt` the move was stamped with
/// at its deadline all stay as the clock left them.
#[tokio::test]
async fn a_report_that_arrives_after_its_deadline_is_refused_and_writes_nothing() {
    let emulator = Emulator::new();

    for case in late_report_cases() {
        let label = case.case;
        let (id, deadline, before) = seed(&emulator, &case, -case.distance_seconds).await;

        // The clock owed the move at the deadline, so this is the record every later
        // caller must see, whichever operation materialises it.
        let timed_out = StoredExecution {
            status: STATUS_TIMED_OUT.to_string(),
            last_updated_at: deadline,
            version_number: before.version_number,
            status_detail_names: before.status_detail_names.clone(),
            // `TIMED_OUT` is terminal: the deadlines are spent.
            has_armed_timer: false,
        };

        for (request, body) in late_update_bodies(before.version_number) {
            let (status, refused) = emulator.update(case.thing, &id, body.clone()).await;
            assert_eq!(
                status, 409,
                "case {label:?} {request}: a report after the deadline must be refused 409, got {status} {refused}"
            );
            assert_eq!(
                error_code(&refused),
                "InvalidStateTransitionException",
                "case {label:?} {request}: expected InvalidStateTransitionException, got {refused}"
            );
            // The message is built from the request's `jobId` and `thingName` and
            // from the status the record HOLDS — the one the clock decided.
            let wanted = format!(
                "Job {id} for {thing} has already finished with status {STATUS_TIMED_OUT}",
                thing = case.thing
            );
            assert_eq!(
                refused["message"].as_str(),
                Some(wanted.as_str()),
                "case {label:?} {request}: expected {wanted:?}, got {refused}"
            );

            let after = emulator.stored(case.thing, &id);
            assert_eq!(
                after, timed_out,
                "case {label:?} {request}: the refused report must leave {timed_out:?}, got {after:?}"
            );
        }

        // And the reader of the same record agrees it is finished, so the device
        // cannot resume the execution by polling for it either.
        let (status, described) = emulator.describe(case.thing, &id).await;
        assert_eq!(
            (status, error_code(&described)),
            (410, "TerminalStateException"),
            "case {label:?}: the execution the clock finished must read as terminal, got {status} {described}"
        );
    }
}

/// RULE, from the other side: the deadline is what decides this, and nothing else —
/// the SAME rows and the same report, with the soonest deadline still to COME, are
/// accepted and do write: the status the device reported is stored, `versionNumber`
/// advances by one, the `statusDetails` the request carried replace the stored map
/// and `lastUpdatedAt` is stamped at the instant of the call.
#[tokio::test]
async fn the_same_report_before_its_deadline_is_accepted_and_does_write() {
    let emulator = Emulator::new();

    for case in late_report_cases() {
        let label = case.case;
        // The same distance from the deadline as the refused rows, on the other side
        // of it, and never zero (a deadline exactly due is already owed the move).
        let (id, deadline, before) =
            seed(&emulator, &case, case.distance_seconds + SECONDS_OF_HEADROOM).await;

        let called_at = now_epoch_seconds();
        let (status, accepted) = emulator
            .update(
                case.thing,
                &id,
                json!({
                    "status": "SUCCEEDED",
                    "statusDetails": {"stage": "done"},
                    "expectedVersion": before.version_number,
                    "includeJobExecutionState": true,
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: a report before the deadline {deadline} must be accepted, got {status} {accepted}"
        );
        assert_eq!(
            (
                accepted["executionState"]["status"].as_str(),
                accepted["executionState"]["versionNumber"].as_i64(),
                accepted["executionState"]["statusDetails"]["stage"].as_str(),
            ),
            (
                Some("SUCCEEDED"),
                Some(before.version_number + 1),
                Some("done")
            ),
            "case {label:?}: expected the reported state back, got {accepted}"
        );

        let after = emulator.stored(case.thing, &id);
        assert_eq!(
            (
                after.status.as_str(),
                after.version_number,
                after.status_detail_names.clone()
            ),
            (
                "SUCCEEDED",
                before.version_number + 1,
                vec!["stage".to_string()]
            ),
            "case {label:?}: expected the device's report stored, got {after:?}"
        );
        assert!(
            after.last_updated_at >= called_at && after.last_updated_at < deadline,
            "case {label:?}: an accepted report is stamped at the call and not at the deadline {deadline}, got {}",
            after.last_updated_at
        );

        // The status the DEVICE reported is the one the execution finished in, so
        // the reader refuses it as terminal for the device's status and not for a
        // timeout nothing reached.
        let (status, described) = emulator.describe(case.thing, &id).await;
        assert_eq!(
            (status, error_code(&described)),
            (410, "TerminalStateException"),
            "case {label:?}: the finished execution must read as terminal, got {status} {described}"
        );
    }
}
