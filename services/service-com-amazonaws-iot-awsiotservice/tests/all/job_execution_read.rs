// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of `DescribeJobExecution`: the control plane reports the
//! job execution a job created on a device, computed from the SHARED job-execution
//! store (`service_common::iot_job_executions`) the IoT Jobs data plane serves.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a
//! restJson1 `GET /things/{thingName}/jobs/{jobId}` signed for the `iot` signing name —
//! after creating the job through `PUT /jobs/{jobId}`, so what is read back is what a
//! writer persisted.
//!
//! The rules under test:
//!
//! * the reported `execution` is the STORED record projected onto the model's
//!   `JobExecution` — `jobId`, `status`, `statusDetails`, `thingArn`, `queuedAt`,
//!   `startedAt`, `lastUpdatedAt`, `executionNumber`, `versionNumber` and
//!   `approximateSecondsBeforeTimedOut` (reported only while a timer is armed and the
//!   execution has not finished) — and reading it does not change it;
//! * `executionNumber` selects one of a job's executions on that device, and a request
//!   that names none reports the LATEST, as the member documents;
//! * a `jobId`/`thingName` pair the store holds no execution for is refused 404
//!   `ResourceNotFoundException`, whichever half is unknown;
//! * a FINISHED execution is REPORTED (this model declares no terminal-state error),
//!   and an execution whose armed timer the clock has already passed is reported
//!   `TIMED_OUT` — the transition that was due at the deadline — while a twin whose
//!   timer was re-armed into the future is untouched.
//!
//! Each table spans the class a rule generalises over — how many devices a job
//! targets, which optional members the job carries, the region the request is signed
//! for, every `JobExecutionStatus` an execution can be read in, which timer is armed
//! and how far from its deadline — rather than one recorded request.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_iot_awsiotservice::EmulatedService;
use service_common::iot_job_executions::{
    now_epoch_seconds, JobExecutionStore, STATUS_IN_PROGRESS, STATUS_QUEUED, STATUS_TIMED_OUT,
};
use smithy_server_runtime::MultiServiceRouter;

/// The account id every emulated ARN carries (`emulator_server::EMULATOR_ACCOUNT_ID`).
const ACCOUNT_ID: &str = "000000000000";
/// The region the harness signs for unless a case names another.
const REGION: &str = "us-east-1";

/// A SigV4 `Authorization` header whose credential scope names the `iot` signing name
/// and `region`: the scope is how the router picks the claiming service, and how the
/// service picks the region its ARNs are scoped to.
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
    /// One member of the reported `execution`.
    fn member(&self, name: &str) -> &Value {
        &self.body["execution"][name]
    }

    fn message(&self) -> String {
        self.body
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }
}

/// The emulator under test: ONE shared engine with the IoT service mounted on it. The
/// engine handle is kept so a case can seed and read the SHARED job-execution store
/// the control plane writes into and the data plane serves.
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

    async fn call(&self, method: &str, path: &str, body: Option<&Value>, region: &str) -> Reply {
        let payload = body.map_or_else(Vec::new, |value| {
            serde_json::to_vec(value).expect("a request body serializes")
        });
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://iot.{region}.amazonaws.com{path}"))
            .header("authorization", signed_for_iot(region))
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
        self.call(
            "POST",
            &format!("/things/{thing_name}"),
            Some(&json!({})),
            REGION,
        )
        .await
    }

    async fn create_job_in(&self, job_id: &str, request: &Value, region: &str) -> Reply {
        self.call("PUT", &format!("/jobs/{job_id}"), Some(request), region)
            .await
    }

    async fn create_job(&self, job_id: &str, request: &Value) -> Reply {
        self.create_job_in(job_id, request, REGION).await
    }

    /// `DescribeJobExecution` for a (thing, job) pair, signed for `region`, with the
    /// `executionNumber` query the row asks for.
    async fn describe_in(
        &self,
        thing_name: &str,
        job_id: &str,
        execution_number: Option<i64>,
        region: &str,
    ) -> Reply {
        let query = execution_number
            .map(|number| format!("?executionNumber={number}"))
            .unwrap_or_default();
        self.call(
            "GET",
            &format!("/things/{thing_name}/jobs/{job_id}{query}"),
            None,
            region,
        )
        .await
    }

    async fn describe(&self, thing_name: &str, job_id: &str) -> Reply {
        self.describe_in(thing_name, job_id, None, REGION).await
    }

    /// Mutate the stored execution of (thing, job) in the SHARED store, so a case can
    /// put a record in a state only the device-facing plane reaches (a status, a
    /// device-reported detail map, an armed timer) and then read it back through this
    /// service.
    fn with_stored_execution<F>(&self, thing_name: &str, job_id: &str, mutate: F)
    where
        F: FnOnce(&mut ::service_common::iot_job_executions::JobExecutionRecord),
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

    /// The status the SHARED store holds for (thing, job) — what the device polling
    /// the Jobs data plane would see.
    fn stored_status(&self, thing_name: &str, job_id: &str) -> String {
        let store = self.engine.state::<JobExecutionStore>();
        let store = store.lock();
        store
            .execution_index(thing_name, job_id, None)
            .and_then(|index| store.record(thing_name, index))
            .map(|record| record.status.clone())
            .unwrap_or_default()
    }
}

/// The ARN a `targets` entry names a device by.
fn target_arn(region: &str, thing_name: &str) -> String {
    format!("arn:aws:iot:{region}:{ACCOUNT_ID}:thing/{thing_name}")
}

/// One row of the read-back table.
struct CreateCase {
    case: &'static str,
    job_id: &'static str,
    /// The devices the job targets; EVERY one of them must report its own execution.
    things: &'static [&'static str],
    /// The `document` member, when the row carries one.
    document: Option<&'static str>,
    /// `timeoutConfig.inProgressTimeoutInMinutes`, when the row carries one. A QUEUED
    /// execution still reports no remaining time: "The timer is started when the job
    /// execution status is set to IN_PROGRESS" (`TimeoutConfig`).
    in_progress_timeout_minutes: Option<i64>,
    /// The region the row is signed for; the reported `thingArn` must be scoped to it.
    region: &'static str,
}

/// RULE: `DescribeJobExecution` reports the execution `CreateJob` created on the named
/// device — queued, at version 1 and execution number 1, carrying the thing's ARN —
/// and reading it leaves it exactly as it was.
#[tokio::test]
async fn a_created_execution_is_read_back_from_the_store() {
    let emulator = Emulator::new();

    // The rows differ in how many devices one job targets, in whether it carries a
    // document or a document source, in whether it sets an in-progress timeout, and in
    // the region the request is signed for.
    let cases: &[CreateCase] = &[
        CreateCase {
            case: "one device, minimal request",
            job_id: "read-minimal",
            things: &["reader-a"],
            document: Some(r#"{"operation":"reboot"}"#),
            in_progress_timeout_minutes: None,
            region: REGION,
        },
        CreateCase {
            case: "three devices, one in-progress timeout",
            job_id: "read-three-targets",
            things: &["reader-b", "reader-c", "reader-d"],
            document: Some(r#"{"operation":"install","version":"7"}"#),
            in_progress_timeout_minutes: Some(10),
            region: REGION,
        },
        CreateCase {
            case: "no document at all, another region",
            job_id: "read-from-source",
            things: &["reader-e"],
            document: None,
            in_progress_timeout_minutes: Some(10_080),
            region: "eu-west-2",
        },
    ];

    for case in cases {
        let label = case.case;
        for thing in case.things {
            emulator.create_thing(thing).await;
        }
        let mut request = json!({
            "targets": case
                .things
                .iter()
                .map(|thing| target_arn(case.region, thing))
                .collect::<Vec<String>>(),
        });
        match case.document {
            Some(document) => request["document"] = json!(document),
            None => {
                request["documentSource"] =
                    json!("https://s3.us-east-1.amazonaws.com/firmware/v1");
            }
        }
        if let Some(minutes) = case.in_progress_timeout_minutes {
            request["timeoutConfig"] = json!({"inProgressTimeoutInMinutes": minutes});
        }
        let before = now_epoch_seconds();
        let created = emulator
            .create_job_in(case.job_id, &request, case.region)
            .await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: creating the job must answer 200, got {} {:?}",
            created.status, created.body
        );
        let after = now_epoch_seconds();

        for thing in case.things {
            let read = emulator
                .describe_in(thing, case.job_id, None, case.region)
                .await;
            assert_eq!(
                read.status, 200,
                "case {label:?}: describing {thing}'s execution must answer 200, got {} {:?}",
                read.status, read.body
            );
            assert_eq!(
                (
                    read.member("jobId").as_str(),
                    read.member("status").as_str(),
                    read.member("versionNumber").as_i64(),
                    read.member("executionNumber").as_i64(),
                ),
                (Some(case.job_id), Some(STATUS_QUEUED), Some(1), Some(1)),
                "case {label:?}: expected {thing}'s created execution, got {:?}",
                read.body
            );
            assert_eq!(
                read.member("thingArn").as_str(),
                Some(target_arn(case.region, thing).as_str()),
                "case {label:?}: thingArn must name {thing} in {}, got {:?}",
                case.region,
                read.body
            );
            let queued_at = read.member("queuedAt").as_f64().unwrap_or_else(|| {
                panic!("case {label:?}: the execution reported no queuedAt: {:?}", read.body)
            });
            assert!(
                queued_at >= before as f64 && queued_at <= after as f64,
                "case {label:?}: queuedAt {queued_at} is outside [{before}, {after}], \
                 got {:?}",
                read.body
            );
            assert_eq!(
                read.member("lastUpdatedAt").as_f64(),
                Some(queued_at),
                "case {label:?}: a never-updated execution was last updated when it was \
                 queued, got {:?}",
                read.body
            );
            assert!(
                read.member("startedAt").is_null()
                    && read.member("statusDetails").is_null()
                    && read.member("forceCanceled").is_null(),
                "case {label:?}: a queued execution has no start, details or force-cancel, \
                 got {:?}",
                read.body
            );
            // The job's in-progress timer is armed by the START, not by the create, so
            // nothing is counting down yet whatever `timeoutConfig` the job carried.
            assert!(
                read.member("approximateSecondsBeforeTimedOut").is_null(),
                "case {label:?}: a queued execution reports no remaining time, got {:?}",
                read.body
            );

            // An explicit executionNumber of the one execution reads the same record;
            // one the device does not hold is not found.
            let numbered = emulator
                .describe_in(thing, case.job_id, Some(1), case.region)
                .await;
            assert_eq!(
                (numbered.status, numbered.body.clone()),
                (read.status, read.body.clone()),
                "case {label:?}: executionNumber=1 must read the same record, got {:?}",
                numbered.body
            );
            let missing = emulator
                .describe_in(thing, case.job_id, Some(2), case.region)
                .await;
            assert_eq!(
                (missing.status, missing.error_code.as_deref()),
                (404, Some("ResourceNotFoundException")),
                "case {label:?}: executionNumber=2 does not exist, got {} {:?}",
                missing.status,
                missing.body
            );

            // A read must not change what a later caller sees.
            let again = emulator
                .describe_in(thing, case.job_id, None, case.region)
                .await;
            assert_eq!(
                again.body, read.body,
                "case {label:?}: two reads of {thing}'s execution must agree, got {:?} \
                 then {:?}",
                read.body, again.body
            );
        }
    }
}

/// One row of the status/timer table: the record the device-facing plane would have
/// left behind, and what this service must report for it.
struct StoredCase {
    case: &'static str,
    thing: &'static str,
    status: &'static str,
    /// The device-reported `statusDetails` pairs, if any.
    details: &'static [(&'static str, &'static str)],
    /// `step_timeout_expires_at`, as an offset in seconds from the read's instant.
    step_timeout_in: Option<i64>,
    /// `execution_timeout_expires_at`, as an offset in seconds from the read's instant.
    execution_timeout_in: Option<i64>,
    /// The `versionNumber` the record carries.
    version_number: i64,
    /// Whether the record has been started.
    started: bool,
    /// The remaining-time window the read must report, when it reports one at all:
    /// `None` means the member must be absent.
    expected_remaining: Option<(i64, i64)>,
}

/// RULE: every member of the reported `execution` comes from the stored record — the
/// status the device left it in, its detail map, its version, its start, and the
/// seconds left before the SOONEST armed timer would time it out (absent once the
/// execution has finished, as `approximateSecondsBeforeTimedOut` documents).
#[tokio::test]
async fn the_reported_execution_follows_the_stored_record() {
    let emulator = Emulator::new();

    // The rows span the statuses an execution can be read in (both pending ones and
    // four terminal ones), the presence and size of the detail map, which of the two
    // timers is armed, and the version the device advanced it to.
    let cases: &[StoredCase] = &[
        StoredCase {
            case: "queued, untouched",
            thing: "stored-queued",
            status: STATUS_QUEUED,
            details: &[],
            step_timeout_in: None,
            execution_timeout_in: None,
            version_number: 1,
            started: false,
            expected_remaining: None,
        },
        StoredCase {
            case: "in progress with the job's in-progress timer",
            thing: "stored-in-progress",
            status: STATUS_IN_PROGRESS,
            details: &[("stage", "download")],
            step_timeout_in: None,
            execution_timeout_in: Some(600),
            version_number: 2,
            started: true,
            expected_remaining: Some((540, 600)),
        },
        StoredCase {
            case: "in progress, the step timer is sooner",
            thing: "stored-step-timer",
            status: STATUS_IN_PROGRESS,
            details: &[("stage", "verify"), ("attempt", "3")],
            step_timeout_in: Some(60),
            execution_timeout_in: Some(600),
            version_number: 5,
            started: true,
            expected_remaining: Some((30, 60)),
        },
        StoredCase {
            case: "queued with a step timer armed",
            thing: "stored-queued-timer",
            status: STATUS_QUEUED,
            details: &[],
            step_timeout_in: Some(120),
            execution_timeout_in: None,
            version_number: 1,
            started: false,
            expected_remaining: Some((90, 120)),
        },
        StoredCase {
            case: "succeeded",
            thing: "stored-succeeded",
            status: "SUCCEEDED",
            details: &[("stage", "done")],
            step_timeout_in: None,
            execution_timeout_in: Some(600),
            version_number: 4,
            started: true,
            expected_remaining: None,
        },
        StoredCase {
            case: "failed",
            thing: "stored-failed",
            status: "FAILED",
            details: &[("reason", "checksum")],
            step_timeout_in: Some(600),
            execution_timeout_in: None,
            version_number: 3,
            started: true,
            expected_remaining: None,
        },
        StoredCase {
            case: "rejected before it ever started",
            thing: "stored-rejected",
            status: "REJECTED",
            details: &[],
            step_timeout_in: None,
            execution_timeout_in: None,
            version_number: 2,
            started: false,
            expected_remaining: None,
        },
        StoredCase {
            case: "canceled",
            thing: "stored-canceled",
            status: "CANCELED",
            details: &[("reason", "operator")],
            step_timeout_in: None,
            execution_timeout_in: None,
            version_number: 6,
            started: true,
            expected_remaining: None,
        },
    ];

    for case in cases {
        let label = case.case;
        let job_id = format!("stored-{}", case.thing);
        emulator.create_thing(case.thing).await;
        let created = emulator
            .create_job(
                &job_id,
                &json!({"targets": [target_arn(REGION, case.thing)], "document": "{}"}),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: creating the job must answer 200, got {} {:?}",
            created.status, created.body
        );

        let now = now_epoch_seconds();
        let started_at = now - 30;
        emulator.with_stored_execution(case.thing, &job_id, |record| {
            record.status = case.status.to_string();
            record.version_number = case.version_number;
            record.last_updated_at = now - 5;
            if case.started {
                record.started_at = Some(started_at);
            }
            record.step_timeout_expires_at = case.step_timeout_in.map(|offset| now + offset);
            record.execution_timeout_expires_at =
                case.execution_timeout_in.map(|offset| now + offset);
            for (name, value) in case.details {
                record
                    .status_details
                    .insert((*name).to_string(), (*value).to_string());
            }
        });

        let read = emulator.describe(case.thing, &job_id).await;
        assert_eq!(
            read.status, 200,
            "case {label:?}: describing the stored execution must answer 200, got {} {:?}",
            read.status, read.body
        );
        assert_eq!(
            (
                read.member("status").as_str(),
                read.member("versionNumber").as_i64(),
                read.member("lastUpdatedAt").as_f64(),
            ),
            (Some(case.status), Some(case.version_number), Some((now - 5) as f64)),
            "case {label:?}: expected the stored status, version and update time, got {:?}",
            read.body
        );
        assert_eq!(
            read.member("startedAt").as_f64(),
            case.started.then_some(started_at as f64),
            "case {label:?}: startedAt must follow the record, got {:?}",
            read.body
        );
        if case.details.is_empty() {
            assert!(
                read.member("statusDetails").is_null(),
                "case {label:?}: an execution with no detail pair reports no map, got {:?}",
                read.body
            );
        } else {
            for (name, value) in case.details {
                assert_eq!(
                    read.member("statusDetails")["detailsMap"][*name].as_str(),
                    Some(*value),
                    "case {label:?}: expected statusDetails {name}={value}, got {:?}",
                    read.body
                );
            }
        }
        let remaining = read.member("approximateSecondsBeforeTimedOut").as_i64();
        match case.expected_remaining {
            None => assert!(
                remaining.is_none(),
                "case {label:?}: expected no remaining time, got {remaining:?} in {:?}",
                read.body
            ),
            Some((low, high)) => {
                let remaining = remaining.unwrap_or_else(|| {
                    panic!(
                        "case {label:?}: expected a remaining time in [{low}, {high}], got {:?}",
                        read.body
                    )
                });
                assert!(
                    (low..=high).contains(&remaining),
                    "case {label:?}: remaining {remaining} is outside [{low}, {high}], \
                     got {:?}",
                    read.body
                );
            }
        }
    }
}

/// One row of the timeout table: where the armed deadline sits relative to the read.
struct TimeoutCase {
    case: &'static str,
    thing: &'static str,
    /// The status the record is in before the read.
    status: &'static str,
    /// `step_timeout_expires_at` as an offset in seconds from the read (negative is
    /// already past).
    step_timeout_in: Option<i64>,
    /// `execution_timeout_expires_at` as an offset in seconds from the read.
    execution_timeout_in: Option<i64>,
    /// The status the read must report.
    expected_status: &'static str,
}

/// RULE: a timer armed on a PENDING execution moves it to `TIMED_OUT` by itself once
/// its deadline passes — "If the job execution status is not set to another terminal
/// state before the time expires, it will be automatically set to TIMED_OUT"
/// (`TimeoutConfig$inProgressTimeoutInMinutes`) — so a read reports the status the
/// clock already decided, records it in the shared store the device polls, and leaves
/// an execution whose deadline has NOT passed (or that already finished) alone.
#[tokio::test]
async fn an_execution_whose_timer_expired_is_reported_timed_out() {
    let emulator = Emulator::new();

    let cases: &[TimeoutCase] = &[
        TimeoutCase {
            case: "the step timer passed a second ago",
            thing: "timer-step-past",
            status: STATUS_IN_PROGRESS,
            step_timeout_in: Some(-1),
            execution_timeout_in: None,
            expected_status: STATUS_TIMED_OUT,
        },
        TimeoutCase {
            case: "the in-progress timer passed an hour ago",
            thing: "timer-execution-past",
            status: STATUS_IN_PROGRESS,
            step_timeout_in: None,
            execution_timeout_in: Some(-3_600),
            expected_status: STATUS_TIMED_OUT,
        },
        TimeoutCase {
            case: "a queued execution's step timer passed",
            thing: "timer-queued-past",
            status: STATUS_QUEUED,
            step_timeout_in: Some(-10),
            execution_timeout_in: None,
            expected_status: STATUS_TIMED_OUT,
        },
        TimeoutCase {
            case: "both timers armed, the sooner one passed",
            thing: "timer-sooner-past",
            status: STATUS_IN_PROGRESS,
            step_timeout_in: Some(-5),
            execution_timeout_in: Some(600),
            expected_status: STATUS_TIMED_OUT,
        },
        TimeoutCase {
            case: "the step timer was re-armed into the future",
            thing: "timer-reset",
            status: STATUS_IN_PROGRESS,
            step_timeout_in: Some(600),
            execution_timeout_in: None,
            expected_status: STATUS_IN_PROGRESS,
        },
        TimeoutCase {
            case: "no timer is armed at all",
            thing: "timer-none",
            status: STATUS_IN_PROGRESS,
            step_timeout_in: None,
            execution_timeout_in: None,
            expected_status: STATUS_IN_PROGRESS,
        },
        TimeoutCase {
            case: "an execution that finished before its deadline",
            thing: "timer-finished",
            status: "SUCCEEDED",
            step_timeout_in: Some(-60),
            execution_timeout_in: Some(-60),
            expected_status: "SUCCEEDED",
        },
    ];

    // ONE job targeting every device of the table, so each row is the same job on a
    // different device: the one whose timer expired times out while its twins — one
    // with the timer re-armed, one with none — survive.
    let job_id = "timer-fanout";
    for case in cases {
        emulator.create_thing(case.thing).await;
    }
    let created = emulator
        .create_job(
            job_id,
            &json!({
                "targets": cases
                    .iter()
                    .map(|case| target_arn(REGION, case.thing))
                    .collect::<Vec<String>>(),
                "document": r#"{"operation":"install"}"#,
                "timeoutConfig": {"inProgressTimeoutInMinutes": 10},
            }),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "creating the fan-out job must answer 200, got {} {:?}",
        created.status, created.body
    );

    let now = now_epoch_seconds();
    for case in cases {
        emulator.with_stored_execution(case.thing, job_id, |record| {
            record.status = case.status.to_string();
            record.started_at = Some(now - 1_000);
            record.step_timeout_expires_at = case.step_timeout_in.map(|offset| now + offset);
            record.execution_timeout_expires_at =
                case.execution_timeout_in.map(|offset| now + offset);
        });
    }

    for case in cases {
        let label = case.case;
        let read = emulator.describe(case.thing, job_id).await;
        assert_eq!(
            read.status, 200,
            "case {label:?}: describing the execution must answer 200, got {} {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.member("status").as_str(),
            Some(case.expected_status),
            "case {label:?}: expected status {}, got {:?}",
            case.expected_status,
            read.body
        );
        // The move is recorded, not invented per read: the shared store the device
        // polls holds the same status, and a second read agrees with the first.
        assert_eq!(
            emulator.stored_status(case.thing, job_id),
            case.expected_status,
            "case {label:?}: the shared store must hold {}, got {:?}",
            case.expected_status,
            emulator.stored_status(case.thing, job_id)
        );
        let again = emulator.describe(case.thing, job_id).await;
        assert_eq!(
            again.member("status").as_str(),
            Some(case.expected_status),
            "case {label:?}: timing out is idempotent, got {:?}",
            again.body
        );
        if case.expected_status == STATUS_TIMED_OUT {
            // A timed-out execution has finished, so it reports no remaining time and
            // is stamped at the deadline the clock passed, not at the read.
            let deadline = case
                .step_timeout_in
                .into_iter()
                .chain(case.execution_timeout_in)
                .min()
                .map(|offset| now + offset);
            assert_eq!(
                read.member("lastUpdatedAt").as_f64(),
                deadline.map(|at| at as f64),
                "case {label:?}: expected the deadline as lastUpdatedAt, got {:?}",
                read.body
            );
            assert!(
                read.member("approximateSecondsBeforeTimedOut").is_null(),
                "case {label:?}: a finished execution reports no remaining time, got {:?}",
                read.body
            );
        }
    }
}

/// RULE: a `jobId`/`thingName` pair the store holds no execution for is refused 404
/// `ResourceNotFoundException`, whichever half of the pair is the unknown one, and the
/// message names both.
#[tokio::test]
async fn a_pair_with_no_execution_is_not_found() {
    let emulator = Emulator::new();
    emulator.create_thing("targeted-thing").await;
    emulator.create_thing("untargeted-thing").await;
    let created = emulator
        .create_job(
            "existing-job",
            &json!({"targets": [target_arn(REGION, "targeted-thing")], "document": "{}"}),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "creating the job must answer 200, got {} {:?}",
        created.status, created.body
    );

    // (case, thing, job, executionNumber, the job itself exists)
    let cases: &[(&str, &str, &str, Option<i64>, bool)] = &[
        (
            "a job nobody created",
            "targeted-thing",
            "no-such-job",
            None,
            false,
        ),
        (
            "a registered thing the job never targeted",
            "untargeted-thing",
            "existing-job",
            None,
            true,
        ),
        (
            "a thing that was never registered",
            "no-such-thing",
            "existing-job",
            None,
            true,
        ),
        (
            "an execution number the device never had",
            "targeted-thing",
            "existing-job",
            Some(9),
            true,
        ),
        (
            "neither half exists",
            "no-such-thing",
            "no-such-job",
            Some(1),
            false,
        ),
    ];

    for (case, thing, job, execution_number, job_exists) in cases {
        let read = emulator
            .describe_in(thing, job, *execution_number, REGION)
            .await;
        assert_eq!(
            (read.status, read.error_code.as_deref()),
            (404, Some("ResourceNotFoundException")),
            "case {case:?}: expected 404 ResourceNotFoundException, got {} {:?} {:?}",
            read.status,
            read.error_code,
            read.body
        );
        let message = read.message();
        // The two conditions are distinguishable, so each refusal is about the resource
        // it is about: a job nobody created is reported as the missing JOB and says
        // nothing of the thing, while an existing job with no execution on the thing
        // names the pair it found none for.
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
        assert!(
            read.body.get("execution").is_none(),
            "case {case:?}: a refusal reports no execution, got {:?}",
            read.body
        );
    }

    // The targeted device's own execution is still readable, so a refused lookup of a
    // neighbouring pair changed nothing.
    let read = emulator.describe("targeted-thing", "existing-job").await;
    assert_eq!(
        (read.status, read.member("status").as_str()),
        (200, Some(STATUS_QUEUED)),
        "the targeted device's execution must still be readable, got {} {:?}",
        read.status,
        read.body
    );
}
