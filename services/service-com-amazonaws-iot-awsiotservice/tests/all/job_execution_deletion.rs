// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of `DeleteJobExecution`: the control plane removes ONE
//! execution of one job on one device from the SHARED job-execution store
//! (`service_common::iot_job_executions`) the IoT Jobs data plane serves.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a
//! restJson1 `DELETE /things/{thingName}/jobs/{jobId}/executionNumber/{executionNumber}`
//! signed for the `iot` signing name — after creating the job through `PUT /jobs/{jobId}`,
//! and reads the result back through `GET /things/{thingName}/jobs/{jobId}`
//! (`DescribeJobExecution`), so what a delete claims and what a reader sees cannot
//! disagree.
//!
//! The rules under test:
//!
//! * the triple `(thingName, jobId, executionNumber)` addresses exactly ONE stored
//!   execution, and the delete removes that one and nothing else: every other execution
//!   of the same job, and every other execution of the same device, stays readable;
//! * an execution that has NOT finished (a non-terminal status) is refused 409
//!   `InvalidStateTransitionException` unless `force` is set, and the refusal removes
//!   nothing; with `force` it is removed whatever its status;
//! * a `jobId` nobody created, and a triple the store holds no execution for, are
//!   refused 404 `ResourceNotFoundException`, each refusal naming the resource it is
//!   about;
//! * an execution whose armed timer the clock has already passed is `TIMED_OUT` — the
//!   transition that was due at the deadline — hence finished, hence deletable without
//!   `force`.
//!
//! Each table spans the class its rule generalises over — every `JobExecutionStatus` an
//! execution can be stored in, the `force` flag, which execution of a re-targeted job is
//! addressed, which half of the triple is unknown, and which timer is armed — rather than
//! one recorded request.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_iot_awsiotservice::EmulatedService;
use service_common::iot_job_executions::{
    now_epoch_seconds, JobExecutionRecord, JobExecutionStore, STATUS_IN_PROGRESS, STATUS_QUEUED,
    STATUS_TIMED_OUT,
};
use smithy_server_runtime::MultiServiceRouter;

/// The account id every emulated ARN carries (`emulator_server::EMULATOR_ACCOUNT_ID`).
const ACCOUNT_ID: &str = "000000000000";
/// The region the harness signs for.
const REGION: &str = "us-east-1";

/// The terminal `JobExecutionStatus` values `DeleteJobExecution$force` lists as the
/// states an execution can be deleted in without forcing ("SUCCEEDED", "FAILED",
/// "REJECTED", "REMOVED" or "CANCELED"), plus `TIMED_OUT`, which the shape's enum
/// carries and `TimeoutConfig$inProgressTimeoutInMinutes` documents as "another terminal
/// state".
const TERMINAL_STATUSES: &[&str] = &[
    "SUCCEEDED",
    "FAILED",
    "REJECTED",
    "REMOVED",
    "CANCELED",
    STATUS_TIMED_OUT,
];

/// A SigV4 `Authorization` header whose credential scope names the `iot` signing name
/// and `region`: the scope is how the router picks the claiming service.
fn signed_for_iot(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTESTIOT/20260101/{region}/iot/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=0000"
    )
}

/// One response, reduced to what a client branches on.
#[derive(Debug)]
struct Reply {
    status: u16,
    error_code: Option<String>,
    body: Value,
}

impl Reply {
    fn message(&self) -> String {
        self.body
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }
}

/// The emulator under test: ONE shared engine with the IoT service mounted on it. The
/// engine handle is kept so a case can seed the SHARED job-execution store with a record
/// only the device-facing plane would otherwise write (a status, an extra execution of a
/// re-targeted job, an armed timer).
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    async fn call(&self, method: &str, path: &str, body: Option<&Value>) -> Reply {
        let payload = body.map_or_else(Vec::new, |value| {
            serde_json::to_vec(value).expect("a request body serializes")
        });
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://iot.{REGION}.amazonaws.com{path}"))
            .header("authorization", signed_for_iot(REGION))
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(payload))
            .expect("a well-formed request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_code = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Reply {
            status,
            error_code,
            body,
        }
    }

    async fn create_thing(&self, thing_name: &str) -> Reply {
        self.call("POST", &format!("/things/{thing_name}"), Some(&json!({})))
            .await
    }

    async fn create_job(&self, job_id: &str, request: &Value) -> Reply {
        self.call("PUT", &format!("/jobs/{job_id}"), Some(request))
            .await
    }

    /// `DeleteJobExecution` for one triple, with the `force` query the row asks for.
    async fn delete(
        &self,
        thing_name: &str,
        job_id: &str,
        execution_number: i64,
        force: bool,
    ) -> Reply {
        let query = if force { "?force=true" } else { "" };
        self.call(
            "DELETE",
            &format!(
                "/things/{thing_name}/jobs/{job_id}/executionNumber/{execution_number}{query}"
            ),
            None,
        )
        .await
    }

    /// `DescribeJobExecution` for one triple — the implemented reader this suite proves
    /// the delete through.
    async fn describe(&self, thing_name: &str, job_id: &str, execution_number: i64) -> Reply {
        self.call(
            "GET",
            &format!("/things/{thing_name}/jobs/{job_id}?executionNumber={execution_number}"),
            None,
        )
        .await
    }

    /// Mutate the stored execution of (thing, job) in the SHARED store, so a case can put
    /// a record in a state only the device-facing plane reaches.
    fn with_stored_execution<F>(&self, thing_name: &str, job_id: &str, mutate: F)
    where
        F: FnOnce(&mut JobExecutionRecord),
    {
        let store = self.engine.state::<JobExecutionStore>();
        let mut store = store.lock();
        let index = store
            .execution_index(thing_name, job_id, None)
            .unwrap_or_else(|| panic!("{thing_name} must hold an execution of {job_id}"));
        mutate(
            store
                .record_mut(thing_name, index)
                .expect("the index came from this store"),
        );
    }

    /// Enqueue ANOTHER execution of `job_id` on `thing_name`, numbered as a re-target
    /// would number it, so a case can address one execution of a device that holds
    /// several of the same job.
    fn add_execution(&self, thing_name: &str, job_id: &str, status: &str) -> i64 {
        let store = self.engine.state::<JobExecutionStore>();
        let mut store = store.lock();
        let execution_number = store.next_execution_number(thing_name, job_id);
        let mut record = JobExecutionRecord::queued(
            job_id.to_string(),
            thing_name.to_string(),
            Some("{}".to_string()),
            now_epoch_seconds(),
        );
        record.execution_number = execution_number;
        record.status = status.to_string();
        store
            .try_insert_execution(record)
            .expect("the store admits another execution");
        execution_number
    }

    /// How many executions the SHARED store holds for `thing_name` — what the device
    /// polling the Jobs data plane would be offered.
    fn stored_count(&self, thing_name: &str) -> usize {
        let store = self.engine.state::<JobExecutionStore>();
        let store = store.lock();
        store.executions_for(thing_name).len()
    }
}

/// The ARN a `targets` entry names a device by.
fn target_arn(thing_name: &str) -> String {
    format!("arn:aws:iot:{REGION}:{ACCOUNT_ID}:thing/{thing_name}")
}

/// Create `job_id` targeting every device of `things`, each getting execution number 1.
async fn seed_job(emulator: &Emulator, job_id: &str, things: &[&str]) {
    for thing in things {
        emulator.create_thing(thing).await;
    }
    let created = emulator
        .create_job(
            job_id,
            &json!({
                "targets": things.iter().map(|t| target_arn(t)).collect::<Vec<String>>(),
                "document": r#"{"operation":"reboot"}"#,
            }),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "creating {job_id} must answer 200, got {} {:?}",
        created.status, created.body
    );
}

/// RULE: a FINISHED execution is deleted without `force`, whatever terminal status it
/// finished in, and the delete removes exactly the addressed execution — the same job's
/// execution on another device stays readable.
#[tokio::test]
async fn a_finished_execution_is_deleted_without_force() {
    let emulator = Emulator::new();

    // One row per terminal status, so the rule is asserted over the whole class rather
    // than the one status a recording happened to carry.
    let things: Vec<String> = TERMINAL_STATUSES
        .iter()
        .map(|status| format!("done-{}", status.to_lowercase()))
        .collect();
    let things: Vec<&str> = things.iter().map(String::as_str).collect();
    let job_id = "terminal-fanout";
    // Plus a device whose execution is left QUEUED: nothing below may touch it.
    let mut targets = things.clone();
    targets.push("terminal-bystander");
    seed_job(&emulator, job_id, &targets).await;

    for (thing, status) in things.iter().zip(TERMINAL_STATUSES) {
        emulator.with_stored_execution(thing, job_id, |record| {
            record.status = (*status).to_string();
        });
    }

    for (thing, status) in things.iter().zip(TERMINAL_STATUSES) {
        let case = format!("{status} execution of {thing}");
        let deleted = emulator.delete(thing, job_id, 1, false).await;
        assert_eq!(
            (deleted.status, deleted.error_code.as_deref()),
            (200, None),
            "case {case:?}: deleting a finished execution must answer 200, got {} {:?} {:?}",
            deleted.status,
            deleted.error_code,
            deleted.body
        );
        let read = emulator.describe(thing, job_id, 1).await;
        assert_eq!(
            (read.status, read.error_code.as_deref()),
            (404, Some("ResourceNotFoundException")),
            "case {case:?}: the deleted execution must read back 404, got {} {:?} {:?}",
            read.status,
            read.error_code,
            read.body
        );
        assert_eq!(
            emulator.stored_count(thing),
            0,
            "case {case:?}: the shared store must hold no execution for {thing}, got {}",
            emulator.stored_count(thing)
        );
        // The bystander is the SAME job on another device: a delete of one execution is
        // not a delete of the job.
        let bystander = emulator.describe("terminal-bystander", job_id, 1).await;
        assert_eq!(
            (
                bystander.status,
                bystander.body["execution"]["status"].as_str()
            ),
            (200, Some(STATUS_QUEUED)),
            "case {case:?}: the untouched device's execution must still read 200 QUEUED, \
             got {} {:?}",
            bystander.status,
            bystander.body
        );
    }
}

/// One row of the `force` table.
struct ForceCase {
    case: &'static str,
    thing: &'static str,
    /// The status the stored execution is in when the delete arrives.
    status: &'static str,
    /// What the request sends for `force`.
    force: bool,
    /// The status the delete must answer with.
    expected_status: u16,
}

/// RULE: an execution that has not finished is refused 409
/// `InvalidStateTransitionException` and left in place unless `force` is set, as `force`
/// documents ("Otherwise, you can only delete a job execution which is in a terminal
/// state … or an exception will occur"); a finished one needs no forcing and `force` on
/// any execution deletes it.
#[tokio::test]
async fn an_unfinished_execution_needs_force() {
    let emulator = Emulator::new();

    // The rows span both non-terminal statuses and a terminal one, each with and without
    // `force`.
    let cases: &[ForceCase] = &[
        ForceCase {
            case: "queued, unforced",
            thing: "force-queued-no",
            status: STATUS_QUEUED,
            force: false,
            expected_status: 409,
        },
        ForceCase {
            case: "queued, forced",
            thing: "force-queued-yes",
            status: STATUS_QUEUED,
            force: true,
            expected_status: 200,
        },
        ForceCase {
            case: "in progress, unforced",
            thing: "force-running-no",
            status: STATUS_IN_PROGRESS,
            force: false,
            expected_status: 409,
        },
        ForceCase {
            case: "in progress, forced",
            thing: "force-running-yes",
            status: STATUS_IN_PROGRESS,
            force: true,
            expected_status: 200,
        },
        ForceCase {
            case: "already succeeded, unforced",
            thing: "force-done-no",
            status: "SUCCEEDED",
            force: false,
            expected_status: 200,
        },
        ForceCase {
            case: "already succeeded, forced",
            thing: "force-done-yes",
            status: "SUCCEEDED",
            force: true,
            expected_status: 200,
        },
    ];

    let job_id = "force-fanout";
    let targets: Vec<&str> = cases.iter().map(|case| case.thing).collect();
    seed_job(&emulator, job_id, &targets).await;
    for case in cases {
        emulator.with_stored_execution(case.thing, job_id, |record| {
            record.status = case.status.to_string();
        });
    }

    for case in cases {
        let label = case.case;
        let deleted = emulator.delete(case.thing, job_id, 1, case.force).await;
        assert_eq!(
            deleted.status, case.expected_status,
            "case {label:?}: expected HTTP {}, got {} {:?}",
            case.expected_status, deleted.status, deleted.body
        );
        if case.expected_status == 409 {
            assert_eq!(
                deleted.error_code.as_deref(),
                Some("InvalidStateTransitionException"),
                "case {label:?}: expected InvalidStateTransitionException, got {:?} {:?}",
                deleted.error_code,
                deleted.body
            );
            let message = deleted.message();
            assert!(
                message.contains(case.status)
                    && message.contains(case.thing)
                    && message.contains(job_id),
                "case {label:?}: the refusal must name the status {}, the thing {} and the \
                 job {job_id}, got {message:?}",
                case.status,
                case.thing
            );
            // A refusal removes nothing: the execution is still there, in the status it
            // was refused in.
            let read = emulator.describe(case.thing, job_id, 1).await;
            assert_eq!(
                (read.status, read.body["execution"]["status"].as_str()),
                (200, Some(case.status)),
                "case {label:?}: a refused delete must leave the execution readable in {}, \
                 got {} {:?}",
                case.status,
                read.status,
                read.body
            );
        } else {
            let read = emulator.describe(case.thing, job_id, 1).await;
            assert_eq!(
                (read.status, read.error_code.as_deref()),
                (404, Some("ResourceNotFoundException")),
                "case {label:?}: the deleted execution must read back 404, got {} {:?} {:?}",
                read.status,
                read.error_code,
                read.body
            );
        }
    }
}

/// RULE: `executionNumber` addresses ONE execution of the named job on the named device,
/// so a device holding several executions of the same job keeps the ones the request did
/// not name.
#[tokio::test]
async fn only_the_addressed_execution_number_is_deleted() {
    let emulator = Emulator::new();
    let job_id = "retargeted-job";
    let thing = "retargeted-thing";
    seed_job(&emulator, job_id, &[thing]).await;

    // Three executions of ONE job on ONE device, as re-targeting it numbers them, each
    // finished so none needs forcing.
    emulator.with_stored_execution(thing, job_id, |record| {
        record.status = "SUCCEEDED".to_string();
    });
    let second = emulator.add_execution(thing, job_id, "FAILED");
    let third = emulator.add_execution(thing, job_id, "CANCELED");
    assert_eq!(
        (second, third),
        (2, 3),
        "re-targeting must number the executions 2 and 3, got {second} and {third}"
    );

    // Deleting the MIDDLE one first: the delete must not be read as "the latest".
    for (case, number, remaining) in [
        ("the middle execution", second, vec![1, third]),
        ("the latest execution", third, vec![1]),
        ("the first execution", 1, vec![]),
    ] {
        let deleted = emulator.delete(thing, job_id, number, false).await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: deleting execution {number} must answer 200, got {} {:?}",
            deleted.status, deleted.body
        );
        let read = emulator.describe(thing, job_id, number).await;
        assert_eq!(
            (read.status, read.error_code.as_deref()),
            (404, Some("ResourceNotFoundException")),
            "case {case:?}: execution {number} must read back 404, got {} {:?} {:?}",
            read.status,
            read.error_code,
            read.body
        );
        for kept in &remaining {
            let kept_read = emulator.describe(thing, job_id, *kept).await;
            assert_eq!(
                (
                    kept_read.status,
                    kept_read.body["execution"]["executionNumber"].as_i64()
                ),
                (200, Some(*kept)),
                "case {case:?}: execution {kept} must still read 200, got {} {:?}",
                kept_read.status,
                kept_read.body
            );
        }
        assert_eq!(
            emulator.stored_count(thing),
            remaining.len(),
            "case {case:?}: the shared store must hold {} execution(s) for {thing}, got {}",
            remaining.len(),
            emulator.stored_count(thing)
        );
    }
}

/// RULE: a triple the store holds no execution for is refused 404
/// `ResourceNotFoundException` — and the refusal is about the resource that is missing:
/// a `jobId` nobody created is reported as the missing JOB, while an existing job with no
/// execution for the named device and number names the pair it found none for.
#[tokio::test]
async fn a_triple_with_no_execution_is_not_found() {
    let emulator = Emulator::new();
    emulator.create_thing("delete-untargeted").await;
    seed_job(&emulator, "delete-existing-job", &["delete-targeted"]).await;
    emulator.with_stored_execution("delete-targeted", "delete-existing-job", |record| {
        record.status = "SUCCEEDED".to_string();
    });

    // (case, thing, job, executionNumber, the job itself exists)
    let cases: &[(&str, &str, &str, i64, bool)] = &[
        (
            "a job nobody created",
            "delete-targeted",
            "no-such-job",
            1,
            false,
        ),
        (
            "a registered thing the job never targeted",
            "delete-untargeted",
            "delete-existing-job",
            1,
            true,
        ),
        (
            "a thing that was never registered",
            "no-such-thing",
            "delete-existing-job",
            1,
            true,
        ),
        (
            "an execution number the device never had",
            "delete-targeted",
            "delete-existing-job",
            7,
            true,
        ),
        (
            "neither half exists",
            "no-such-thing",
            "no-such-job",
            2,
            false,
        ),
    ];

    for (case, thing, job, number, job_exists) in cases {
        let refused = emulator.delete(thing, job, *number, false).await;
        assert_eq!(
            (refused.status, refused.error_code.as_deref()),
            (404, Some("ResourceNotFoundException")),
            "case {case:?}: expected 404 ResourceNotFoundException, got {} {:?} {:?}",
            refused.status,
            refused.error_code,
            refused.body
        );
        let message = refused.message();
        if *job_exists {
            assert!(
                message.contains(job) && message.contains(thing),
                "case {case:?}: the refusal must name {job} and {thing}, got {message:?}"
            );
        } else {
            assert!(
                message.contains(job) && !message.contains(thing),
                "case {case:?}: the refusal must name the missing job {job} and not the \
                 thing {thing}, got {message:?}"
            );
        }
    }

    // A forced delete of a triple that addresses nothing is the same 404: `force` is
    // about an unfinished execution, not about one that does not exist.
    let forced = emulator
        .delete("delete-targeted", "delete-existing-job", 7, true)
        .await;
    assert_eq!(
        (forced.status, forced.error_code.as_deref()),
        (404, Some("ResourceNotFoundException")),
        "a forced delete of a missing execution must still be 404, got {} {:?} {:?}",
        forced.status,
        forced.error_code,
        forced.body
    );

    // Every refusal above left the one real execution alone.
    let read = emulator
        .describe("delete-targeted", "delete-existing-job", 1)
        .await;
    assert_eq!(
        (read.status, read.body["execution"]["status"].as_str()),
        (200, Some("SUCCEEDED")),
        "the targeted device's execution must still be readable, got {} {:?}",
        read.status,
        read.body
    );
}

/// One row of the timer table.
struct TimerCase {
    case: &'static str,
    thing: &'static str,
    /// `step_timeout_expires_at` as an offset in seconds from the delete's instant.
    step_timeout_in: Option<i64>,
    /// `execution_timeout_expires_at` as an offset in seconds from the delete's instant.
    execution_timeout_in: Option<i64>,
    /// Whether the clock has already finished the execution, so an unforced delete
    /// removes it.
    deletable_unforced: bool,
}

/// RULE: an execution whose armed timer the clock has already passed is `TIMED_OUT` —
/// the transition that was due at its own deadline — so it is finished and deletable
/// without `force`, while a twin whose deadline has not passed still needs forcing.
#[tokio::test]
async fn an_execution_whose_timer_expired_is_deletable_without_force() {
    let emulator = Emulator::new();
    let job_id = "timer-delete";

    let cases: &[TimerCase] = &[
        TimerCase {
            case: "the step timer passed a second ago",
            thing: "td-step-past",
            step_timeout_in: Some(-1),
            execution_timeout_in: None,
            deletable_unforced: true,
        },
        TimerCase {
            case: "the in-progress timer passed an hour ago",
            thing: "td-execution-past",
            step_timeout_in: None,
            execution_timeout_in: Some(-3_600),
            deletable_unforced: true,
        },
        TimerCase {
            case: "both timers armed, the sooner one passed",
            thing: "td-sooner-past",
            step_timeout_in: Some(-5),
            execution_timeout_in: Some(600),
            deletable_unforced: true,
        },
        TimerCase {
            case: "the step timer was re-armed into the future",
            thing: "td-reset",
            step_timeout_in: Some(600),
            execution_timeout_in: None,
            deletable_unforced: false,
        },
        TimerCase {
            case: "no timer is armed at all",
            thing: "td-none",
            step_timeout_in: None,
            execution_timeout_in: None,
            deletable_unforced: false,
        },
    ];

    let targets: Vec<&str> = cases.iter().map(|case| case.thing).collect();
    seed_job(&emulator, job_id, &targets).await;

    let now = now_epoch_seconds();
    for case in cases {
        emulator.with_stored_execution(case.thing, job_id, |record| {
            record.status = STATUS_IN_PROGRESS.to_string();
            record.started_at = Some(now - 1_000);
            record.step_timeout_expires_at = case.step_timeout_in.map(|offset| now + offset);
            record.execution_timeout_expires_at =
                case.execution_timeout_in.map(|offset| now + offset);
        });
    }

    for case in cases {
        let label = case.case;
        let thing = case.thing;
        let deleted = emulator.delete(thing, job_id, 1, false).await;
        let expected = if case.deletable_unforced { 200 } else { 409 };
        assert_eq!(
            deleted.status, expected,
            "case {label:?}: expected HTTP {expected}, got {} {:?}",
            deleted.status, deleted.body
        );
        if case.deletable_unforced {
            assert_eq!(
                emulator.stored_count(thing),
                0,
                "case {label:?}: the timed-out execution must be gone, got {} stored",
                emulator.stored_count(thing)
            );
        } else {
            let message = deleted.message();
            assert!(
                message.contains(STATUS_IN_PROGRESS),
                "case {label:?}: the refusal must name the status {STATUS_IN_PROGRESS}, \
                 got {message:?}"
            );
            // Forcing it is what deletes an execution the clock has not finished.
            let forced = emulator.delete(thing, job_id, 1, true).await;
            assert_eq!(
                forced.status, 200,
                "case {label:?}: a forced delete must answer 200, got {} {:?}",
                forced.status, forced.body
            );
            assert_eq!(
                emulator.stored_count(thing),
                0,
                "case {label:?}: the forced delete must leave nothing, got {} stored",
                emulator.stored_count(thing)
            );
        }
    }
}
