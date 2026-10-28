// SPDX-License-Identifier: CC0-1.0

//! The clock-driven `TIMED_OUT` transition, as the device-facing plane answers it.
//!
//! The rule under test: a timer armed on a PENDING execution moves it to `TIMED_OUT`
//! by itself once its deadline passes ("If the job execution status is not set to
//! another terminal state before the time expires, it will be automatically set to
//! TIMED_OUT" — `iot`: `TimeoutConfig$inProgressTimeoutInMinutes`), so EVERY
//! operation of this plane answers from the status the deadline already decided,
//! whether or not some other call happened in between:
//!
//! * `GetPendingJobExecutions` reports an execution past its deadline in neither
//!   list, on a request that is nothing but a read;
//! * `DescribeJobExecution` refuses it 410 `TerminalStateException`;
//! * `UpdateJobExecution` refuses it 409 `InvalidStateTransitionException`;
//! * `StartNextPendingJobExecution` does not hand it out and does not re-arm it: the
//!   next execution whose deadline has NOT passed is the one started.
//!
//! The move is recorded in the shared store (the control plane reads the same
//! record), and it is stamped at the DEADLINE rather than at the call that
//! materialised it.
//!
//! Every row drives the real wire path — restJson1 through the multi-service router —
//! and the table spans which timer is armed (the step timer, the execution /
//! in-progress timer, both), how far from its deadline the record is (an hour past,
//! a second past, exactly due, still to come, none armed), and the status the
//! execution is in (`QUEUED`, `IN_PROGRESS`, already finished).

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_iotjobsdataplane_iotlaserthingjobmanagerexternalservice::state::{
    command_registry, iot_commands::CommandFacts, now_epoch_seconds, service_state,
    JobExecutionRecord, STATUS_IN_PROGRESS, STATUS_QUEUED,
};
use service_com_amazonaws_iotjobsdataplane_iotlaserthingjobmanagerexternalservice::EmulatedService;
use service_common::iot_job_executions::STATUS_TIMED_OUT;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 `Authorization` header whose credential scope names this service's
/// signing name, which is how the router picks the service for a request.
const SIGV4: &str = "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/us-east-1/iot-jobs-data/aws4_request, SignedHeaders=host, Signature=0";

const COMMAND_ARN: &str = "arn:aws:iot:us-east-1:111122223333:command/reboot";

/// A terminal value of the model's `JobExecutionStatus` that a device reported
/// before any deadline passed, for the row that proves a FINISHED execution is left
/// exactly as it finished.
const STATUS_SUCCEEDED: &str = "SUCCEEDED";

/// One emulator holding one shared engine, driven over HTTP.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        // An execution starts only for a command that EXISTS; commands are the
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

    /// Start a command execution on `thing` and answer the minted id, which is the
    /// `jobId` the device's own operations name it by.
    async fn start_command_on(&self, thing: &str) -> String {
        let (status, body) = self
            .call(
                "POST",
                "/command-executions",
                Some(json!({
                    "targetArn": format!("arn:aws:iot:us-east-1:111122223333:thing/{thing}"),
                    "commandArn": COMMAND_ARN,
                })),
            )
            .await;
        assert_eq!(
            status, 200,
            "starting a command on {thing} must answer 200, got {status} {body}"
        );
        body["executionId"]
            .as_str()
            .unwrap_or_else(|| panic!("expected an executionId, got {body}"))
            .to_string()
    }

    async fn pending(&self, thing: &str) -> (u16, Value) {
        self.call("GET", &format!("/things/{thing}/jobs"), None).await
    }

    async fn describe(&self, thing: &str, job_id: &str) -> (u16, Value) {
        self.call("GET", &format!("/things/{thing}/jobs/{job_id}"), None)
            .await
    }

    async fn start_next(&self, thing: &str, body: Value) -> (u16, Value) {
        self.call("PUT", &format!("/things/{thing}/jobs/$next"), Some(body))
            .await
    }

    async fn update(&self, thing: &str, job_id: &str, body: Value) -> (u16, Value) {
        self.call("POST", &format!("/things/{thing}/jobs/{job_id}"), Some(body))
            .await
    }

    /// Put one stored execution into the state a row asks about: the status a device
    /// drove it to and the deadlines its timers were armed with. This is the state a
    /// live service reaches by waiting, without a test that sleeps for it.
    fn with_stored_execution<F>(&self, thing: &str, job_id: &str, mutate: F)
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
    fn stored(&self, thing: &str, job_id: &str) -> (String, i64) {
        let store = service_state(&*self.engine).expect("the shared job-execution store");
        let store = store.lock();
        store
            .execution_index(thing, job_id, None)
            .and_then(|index| store.record(thing, index))
            .map(|record| (record.status.clone(), record.last_updated_at))
            .unwrap_or_else(|| panic!("{thing} must hold the execution {job_id}"))
    }
}

/// The wire error code of a modeled failure (restJson1 carries it in `__type`).
fn error_code(body: &Value) -> &str {
    body["__type"].as_str().unwrap_or("")
}

/// The `jobId`s of one pending list, in the order the service returned them.
fn job_ids(list: &Value) -> Vec<String> {
    list.as_array()
        .unwrap_or_else(|| panic!("expected a list of summaries, got {list}"))
        .iter()
        .map(|summary| summary["jobId"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// One row of the timer table: the record a device drove to `status` with the
/// deadlines `step_timeout_in` / `execution_timeout_in` seconds from now, and the
/// status the clock has therefore already decided for it.
struct TimerCase {
    case: &'static str,
    /// The device this row's execution belongs to — one per row, so the rows cannot
    /// interfere with each other.
    thing: &'static str,
    status: &'static str,
    /// `step_timeout_expires_at` as an offset in seconds from the seeding instant;
    /// a negative offset is a deadline that has already passed, `0` one that is
    /// exactly due.
    step_timeout_in: Option<i64>,
    /// `execution_timeout_expires_at` as the same kind of offset: the timer
    /// `StartCommandExecution$executionTimeoutSeconds` and a job's
    /// `inProgressTimeoutInMinutes` both arm.
    execution_timeout_in: Option<i64>,
    /// The status every operation of this plane must answer from.
    expected_status: &'static str,
}

impl TimerCase {
    /// True when the clock owes this row the `TIMED_OUT` transition.
    fn times_out(&self) -> bool {
        self.expected_status == STATUS_TIMED_OUT
    }

    /// The deadline the move is owed at: the SOONEST armed timer.
    fn deadline(&self, seeded_at: i64) -> Option<i64> {
        [self.step_timeout_in, self.execution_timeout_in]
            .into_iter()
            .flatten()
            .min()
            .map(|offset| seeded_at + offset)
    }
}

/// The table every test in this file runs: one row per combination of armed timer,
/// distance from the deadline and stored status.
fn timer_cases() -> Vec<TimerCase> {
    vec![
        TimerCase {
            case: "a queued execution's execution timer passed an hour ago",
            thing: "laser-queued-exec-past",
            status: STATUS_QUEUED,
            step_timeout_in: None,
            execution_timeout_in: Some(-3_600),
            expected_status: STATUS_TIMED_OUT,
        },
        TimerCase {
            case: "a queued execution's step timer passed a second ago",
            thing: "laser-queued-step-past",
            status: STATUS_QUEUED,
            step_timeout_in: Some(-1),
            execution_timeout_in: None,
            expected_status: STATUS_TIMED_OUT,
        },
        TimerCase {
            case: "an in-progress execution's step timer is exactly due",
            thing: "laser_in-progress-due",
            status: STATUS_IN_PROGRESS,
            step_timeout_in: Some(0),
            execution_timeout_in: None,
            expected_status: STATUS_TIMED_OUT,
        },
        TimerCase {
            case: "both timers armed, the sooner one passed",
            thing: "plant:laser:both",
            status: STATUS_IN_PROGRESS,
            step_timeout_in: Some(-30),
            execution_timeout_in: Some(600),
            expected_status: STATUS_TIMED_OUT,
        },
        TimerCase {
            case: "both timers armed, neither has passed",
            thing: "laser-both-future",
            status: STATUS_IN_PROGRESS,
            step_timeout_in: Some(600),
            execution_timeout_in: Some(1_200),
            expected_status: STATUS_IN_PROGRESS,
        },
        TimerCase {
            case: "a queued execution whose timer is still to come",
            thing: "laser-queued-future",
            status: STATUS_QUEUED,
            step_timeout_in: None,
            execution_timeout_in: Some(3_600),
            expected_status: STATUS_QUEUED,
        },
        TimerCase {
            case: "no timer is armed at all",
            thing: "laser-no-timer",
            status: STATUS_QUEUED,
            step_timeout_in: None,
            execution_timeout_in: None,
            expected_status: STATUS_QUEUED,
        },
        TimerCase {
            case: "an execution that finished before its deadline",
            thing: "laser-finished",
            status: STATUS_SUCCEEDED,
            step_timeout_in: Some(-60),
            execution_timeout_in: Some(-60),
            expected_status: STATUS_SUCCEEDED,
        },
    ]
}

/// Start one command execution per row and seed the row's status and deadlines,
/// answering (row, execution id, the instant the deadlines are offset from).
async fn seeded(emulator: &Emulator, cases: &[TimerCase]) -> (Vec<String>, i64) {
    let mut ids = Vec::new();
    for case in cases {
        ids.push(emulator.start_command_on(case.thing).await);
    }
    let seeded_at = now_epoch_seconds();
    for (case, id) in cases.iter().zip(&ids) {
        emulator.with_stored_execution(case.thing, id, |record| {
            record.status = case.status.to_string();
            record.started_at = (case.status != STATUS_QUEUED).then_some(seeded_at - 1_000);
            record.step_timeout_expires_at =
                case.step_timeout_in.map(|offset| seeded_at + offset);
            record.execution_timeout_expires_at =
                case.execution_timeout_in.map(|offset| seeded_at + offset);
        });
    }
    (ids, seeded_at)
}

/// RULE: `GetPendingJobExecutions` reports the thing's executions that are not in a
/// terminal status, read at the CURRENT instant — so an execution whose armed timer
/// has passed is `TIMED_OUT` and in neither list, on a request that is nothing but a
/// read, while one whose deadline is still to come keeps its pending partition.
///
/// The transition is RECORDED, not invented per read: the shared store holds it
/// afterwards, stamped at the deadline it was owed at rather than at this call.
#[tokio::test]
async fn a_pending_listing_alone_materialises_every_deadline_that_has_passed() {
    let emulator = Emulator::new();
    let cases = timer_cases();
    let (ids, seeded_at) = seeded(&emulator, &cases).await;

    for (case, id) in cases.iter().zip(&ids) {
        let label = case.case;
        let (status, listed) = emulator.pending(case.thing).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the listing must answer 200, got {status} {listed}"
        );

        let queued = job_ids(&listed["queuedJobs"]);
        let in_progress = job_ids(&listed["inProgressJobs"]);
        let reported_in = |partition: &str| -> Vec<String> {
            if case.expected_status == partition {
                vec![id.clone()]
            } else {
                Vec::new()
            }
        };
        let expected_queued = reported_in(STATUS_QUEUED);
        let expected_in_progress = reported_in(STATUS_IN_PROGRESS);
        assert_eq!(
            (queued.clone(), in_progress.clone()),
            (expected_queued, expected_in_progress),
            "case {label:?}: expected the execution reported for status {}, got {listed}",
            case.expected_status
        );

        let (stored_status, stored_last_updated) = emulator.stored(case.thing, id);
        assert_eq!(
            stored_status, case.expected_status,
            "case {label:?}: the shared store must hold {}, got {stored_status}",
            case.expected_status
        );
        if case.times_out() {
            let deadline = case
                .deadline(seeded_at)
                .unwrap_or_else(|| panic!("case {label:?}: a timed-out row arms a timer"));
            assert_eq!(
                stored_last_updated, deadline,
                "case {label:?}: the move is stamped at the deadline {deadline}, got {stored_last_updated}"
            );
        }

        // Reading again changes nothing further: the materialised status is stable.
        let (_, again) = emulator.pending(case.thing).await;
        assert_eq!(
            (job_ids(&again["queuedJobs"]), job_ids(&again["inProgressJobs"])),
            (queued, in_progress),
            "case {label:?}: a second read must report the same lists, got {again}"
        );
        let (stored_again, last_updated_again) = emulator.stored(case.thing, id);
        assert_eq!(
            (stored_again.as_str(), last_updated_again),
            (case.expected_status, stored_last_updated),
            "case {label:?}: a second read must leave the record alone, got {stored_again} at {last_updated_again}"
        );
    }
}

/// RULE: the same materialised status decides every OTHER device-facing answer — a
/// read of an execution past its deadline is the model's `TerminalStateException`
/// (410), and an update of one is `InvalidStateTransitionException` (409), the two
/// refusals a finished execution already answers with; an execution whose deadline
/// has not passed is still served.
#[tokio::test]
async fn a_read_and_an_update_answer_the_refusals_a_passed_deadline_earns() {
    let emulator = Emulator::new();
    let cases = timer_cases();
    let (ids, _) = seeded(&emulator, &cases).await;

    for (case, id) in cases.iter().zip(&ids) {
        let label = case.case;

        let (status, described) = emulator.describe(case.thing, id).await;
        if case.expected_status == STATUS_QUEUED || case.expected_status == STATUS_IN_PROGRESS {
            assert_eq!(
                (status, described["execution"]["status"].as_str()),
                (200, Some(case.expected_status)),
                "case {label:?}: a live execution must be reported, got {status} {described}"
            );
        } else {
            assert_eq!(
                (status, error_code(&described)),
                (410, "TerminalStateException"),
                "case {label:?}: a finished execution must be refused as terminal, got {status} {described}"
            );
        }

        // The update is made LAST for each row, since an accepted one moves the
        // record: a pending row accepts it, a row the clock finished refuses it.
        let (status, updated) = emulator
            .update(case.thing, id, json!({"status": "SUCCEEDED"}))
            .await;
        if case.expected_status == STATUS_QUEUED || case.expected_status == STATUS_IN_PROGRESS {
            assert_eq!(
                status, 200,
                "case {label:?}: a live execution accepts the device's update, got {status} {updated}"
            );
        } else {
            assert_eq!(
                (status, error_code(&updated)),
                (409, "InvalidStateTransitionException"),
                "case {label:?}: an execution the clock finished must refuse the update, got {status} {updated}"
            );
            let (stored_status, _) = emulator.stored(case.thing, id);
            assert_eq!(
                stored_status, case.expected_status,
                "case {label:?}: the refused update must leave the status alone, got {stored_status}"
            );
        }
    }
}

/// RULE: "pending" is read at the current instant, so `StartNextPendingJobExecution`
/// hands out — and re-arms — only an execution whose deadline has NOT passed: one
/// past its deadline is `TIMED_OUT`, hence not pending, and the next execution behind
/// it is the one started.
///
/// The rows differ in how many of the thing's executions are past their deadline
/// (the first of two, both of two, none of one), which is what decides whether the
/// device is handed the execution behind, nothing at all, or the one it polled for.
#[tokio::test]
async fn the_next_pending_execution_skips_every_deadline_that_has_passed() {
    let emulator = Emulator::new();

    // (case, thing, how many executions to enqueue, how many of them are expired)
    let rows: [(&str, &str, usize, usize); 4] = [
        ("the first of two queues has expired", "laser-skip-one", 2, 1),
        ("both queued executions have expired", "laser-skip-both", 2, 2),
        ("nothing has expired", "laser-skip-none", 1, 0),
        ("the only queued execution has expired", "laser-skip-only", 1, 1),
    ];

    for (case, thing, enqueued, expired) in rows {
        let mut ids = Vec::new();
        for _ in 0..enqueued {
            ids.push(emulator.start_command_on(thing).await);
        }
        let seeded_at = now_epoch_seconds();
        for id in ids.iter().take(expired) {
            emulator.with_stored_execution(thing, id, |record| {
                record.execution_timeout_expires_at = Some(seeded_at - 5);
            });
        }

        let (status, started) = emulator
            .start_next(thing, json!({"stepTimeoutInMinutes": 10}))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: $next must answer 200, got {status} {started}"
        );
        let expected = ids.get(expired).cloned();
        assert_eq!(
            started["execution"]["jobId"].as_str().map(str::to_string),
            expected,
            "case {case:?}: expected $next to start {expected:?}, got {started}"
        );

        // Every expired execution is TIMED_OUT in the store, stamped at its own
        // deadline — the poll neither handed it out nor re-armed its timer.
        for id in ids.iter().take(expired) {
            let (stored_status, last_updated) = emulator.stored(thing, id);
            assert_eq!(
                (stored_status.as_str(), last_updated),
                (STATUS_TIMED_OUT, seeded_at - 5),
                "case {case:?}: {id} must be timed out at its deadline, got {stored_status} at {last_updated}"
            );
        }
    }
}
