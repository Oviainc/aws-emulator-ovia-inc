// SPDX-License-Identifier: CC0-1.0

//! The response DOCUMENT this service answers a 200 with.
//!
//! RULE under test: a success response of this service is a COMPLETE JSON document —
//! it carries every member the operation's output shape models, and a member the
//! service has no value for is JSON `null` rather than a key the body leaves out. So
//! no 200 of this service is ever the empty document `{}`.
//!
//! Every request goes over the wire (restJson1 through the multi-service router), and
//! each assertion is made over a SET of inputs: the member list is the model's, and
//! the rows span which members have a value — none of them, some, all — across all
//! five operations.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_iotjobsdataplane_iotlaserthingjobmanagerexternalservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 `Authorization` header whose credential scope names this service's
/// signing name, which is how the router picks the service for a request.
const SIGV4: &str = "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/us-east-1/iot-jobs-data/aws4_request, SignedHeaders=host, Signature=0";

const COMMAND_ARN: &str = "arn:aws:iot:us-east-1:111122223333:command/reboot";

/// The modelled output members of each operation, from
/// `smithy_query(query="operation", operation="…")` on this service's model
/// (`output` of each operation) — NOT a hand-picked subset:
///
/// * `StartNextPendingJobExecution` -> `{execution}`
/// * `DescribeJobExecution` -> `{execution}`
/// * `UpdateJobExecution` -> `{executionState, jobDocument}`
/// * `GetPendingJobExecutions` -> `{inProgressJobs, queuedJobs}`
/// * `StartCommandExecution` -> `{executionId}`
const START_NEXT_MEMBERS: &[&str] = &["execution"];
const DESCRIBE_MEMBERS: &[&str] = &["execution"];
const UPDATE_MEMBERS: &[&str] = &["executionState", "jobDocument"];
const PENDING_MEMBERS: &[&str] = &["inProgressJobs", "queuedJobs"];
const START_COMMAND_MEMBERS: &[&str] = &["executionId"];

/// One emulator holding one shared engine, driven over HTTP.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // These tests exercise handler behaviour, not request signing.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
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
        // An empty body parses as `Value::Null`, which is exactly the shape this
        // suite refuses: a 200 must carry a JSON object.
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// Start a command on `thing_name` and return the response body plus the
    /// execution id it minted (which is the job id of the execution it queued).
    async fn start_command_on(&self, thing_name: &str) -> (Value, String) {
        let (status, body) = self
            .call(
                "POST",
                "/command-executions",
                Some(json!({
                    "targetArn": format!("arn:aws:iot:us-east-1:111122223333:thing/{thing_name}"),
                    "commandArn": COMMAND_ARN,
                })),
            )
            .await;
        assert_eq!(
            status, 200,
            "thing {thing_name:?}: expected 200 from StartCommandExecution, got {status} {body}"
        );
        let id = body["executionId"]
            .as_str()
            .unwrap_or_else(|| panic!("thing {thing_name:?}: expected an executionId, got {body}"))
            .to_string();
        (body, id)
    }

    async fn pending(&self, thing_name: &str) -> (u16, Value) {
        self.call("GET", &format!("/things/{thing_name}/jobs"), None)
            .await
    }

    async fn start_next(&self, thing_name: &str) -> (u16, Value) {
        self.call("PUT", &format!("/things/{thing_name}/jobs/$next"), None)
            .await
    }

    async fn describe(&self, thing_name: &str, job_id: &str) -> (u16, Value) {
        self.call("GET", &format!("/things/{thing_name}/jobs/{job_id}"), None)
            .await
    }

    async fn update(&self, thing_name: &str, job_id: &str, body: Value) -> (u16, Value) {
        self.call(
            "POST",
            &format!("/things/{thing_name}/jobs/{job_id}"),
            Some(body),
        )
        .await
    }
}

/// Assert the rule for one answer: the body is a JSON object that carries EVERY
/// modelled member, each of the members in `valued` with a value and every other one
/// as JSON `null`.
fn assert_complete_document(
    case: &str,
    operation: &str,
    body: &Value,
    members: &[&str],
    valued: &[&str],
) {
    let object = body.as_object().unwrap_or_else(|| {
        panic!("case {case:?}: {operation} must answer a JSON object, got {body}")
    });
    for member in members {
        let actual = object.get(*member).unwrap_or_else(|| {
            panic!(
                "case {case:?}: {operation}'s response document must carry the modelled member {member:?}, got {body}"
            )
        });
        if valued.contains(member) {
            assert!(
                !actual.is_null(),
                "case {case:?}: {operation} must report a value for {member:?}, got {actual:?}"
            );
        } else {
            assert!(
                actual.is_null(),
                "case {case:?}: {operation} has no value for {member:?}, so it must be reported as null, got {actual:?}"
            );
        }
    }
}

/// RULE: `StartNextPendingJobExecution` answers 200 with the modelled `execution`
/// member always present — the started execution when the thing has pending work,
/// JSON `null` when it has none — never an empty document.
///
/// The rows differ in WHY there is or is not something to start (no execution at all,
/// a queued one, one already in progress, only a finished one) and in the form of the
/// thing name.
#[tokio::test]
async fn start_next_pending_job_execution_always_carries_the_execution_member() {
    let emulator = Emulator::new();

    // (case, thing name, how many commands to start, whether to drive it first,
    //  whether an execution is expected back)
    let rows: &[(&str, &str, usize, Drive, bool)] = &[
        ("a thing with no execution at all", "laser-none", 0, Drive::None, false),
        ("a thing with a queued execution", "laser-queued", 1, Drive::None, true),
        (
            "a thing whose execution is already in progress",
            "laser_in-progress",
            1,
            Drive::Start,
            true,
        ),
        (
            "a thing whose only execution has finished",
            "plant:laser:3",
            1,
            Drive::Finish,
            false,
        ),
        ("a thing with two queued executions", "laser-two", 2, Drive::None, true),
    ];

    for (case, thing, commands, drive, expect_execution) in rows {
        let mut job_id = String::new();
        for _ in 0..*commands {
            job_id = emulator.start_command_on(thing).await.1;
        }
        match drive {
            Drive::None => {}
            Drive::Start => {
                let (status, body) = emulator.start_next(thing).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected 200 while arranging the case, got {status} {body}"
                );
            }
            Drive::Finish => {
                let (status, body) = emulator.start_next(thing).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected 200 while arranging the case, got {status} {body}"
                );
                let (status, body) = emulator
                    .update(thing, &job_id, json!({"status": "SUCCEEDED"}))
                    .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected 200 while arranging the case, got {status} {body}"
                );
            }
        }

        let (status, body) = emulator.start_next(thing).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from $next, got {status} {body}"
        );
        let valued: &[&str] = if *expect_execution { &["execution"] } else { &[] };
        assert_complete_document(
            case,
            "StartNextPendingJobExecution",
            &body,
            START_NEXT_MEMBERS,
            valued,
        );
    }
}

/// How far an execution is driven before the case's own call.
enum Drive {
    /// Left queued.
    None,
    /// Moved to `IN_PROGRESS` through `$next`.
    Start,
    /// Started and then updated to a terminal status.
    Finish,
}

/// RULE: `UpdateJobExecution` answers 200 with BOTH modelled members present —
/// `executionState` and `jobDocument` carry a value exactly when the request's
/// `includeJobExecutionState` / `includeJobDocument` asked for them, and JSON `null`
/// otherwise.
///
/// The rows are the whole include-flag matrix (absent, explicitly false, each one
/// alone, both), crossed with the status the update reports.
#[tokio::test]
async fn update_job_execution_always_carries_both_modelled_members() {
    let emulator = Emulator::new();

    // (case, the flags the request carries, the status it reports)
    let rows: &[(&str, Option<bool>, Option<bool>, &str)] = &[
        ("neither flag sent", None, None, "IN_PROGRESS"),
        ("both flags explicitly false", Some(false), Some(false), "IN_PROGRESS"),
        ("the execution state only", Some(true), None, "IN_PROGRESS"),
        ("the job document only", None, Some(true), "QUEUED"),
        ("both members", Some(true), Some(true), "IN_PROGRESS"),
        ("both members on a terminal update", Some(true), Some(true), "SUCCEEDED"),
        ("the state only on a terminal update", Some(true), Some(false), "FAILED"),
        ("neither flag on a terminal update", None, None, "REJECTED"),
    ];

    for (index, (case, include_state, include_document, status_value)) in rows.iter().enumerate() {
        // One fresh thing per row, so no row depends on another's execution.
        let thing = format!("laser-update-{index}");
        let (_, job_id) = emulator.start_command_on(&thing).await;

        let mut request = json!({"status": status_value});
        if let Some(flag) = include_state {
            request["includeJobExecutionState"] = json!(flag);
        }
        if let Some(flag) = include_document {
            request["includeJobDocument"] = json!(flag);
        }

        let (status, body) = emulator.update(&thing, &job_id, request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from UpdateJobExecution, got {status} {body}"
        );

        let mut valued: Vec<&str> = Vec::new();
        if *include_state == Some(true) {
            valued.push("executionState");
        }
        if *include_document == Some(true) {
            // Every execution this service mints carries the command's job
            // document, so an asked-for document always has a value here.
            valued.push("jobDocument");
        }
        assert_complete_document(case, "UpdateJobExecution", &body, UPDATE_MEMBERS, &valued);
    }
}

/// RULE: the same completeness holds for the other three operations — the reporting
/// reads and the create — whose handlers determine every modelled member, so each
/// answers a document carrying all of them with a value.
#[tokio::test]
async fn every_other_operation_answers_a_complete_document() {
    let emulator = Emulator::new();

    // StartCommandExecution: the minted id.
    let (created, job_id) = emulator.start_command_on("laser-complete").await;
    assert_complete_document(
        "a started command execution",
        "StartCommandExecution",
        &created,
        START_COMMAND_MEMBERS,
        START_COMMAND_MEMBERS,
    );

    // GetPendingJobExecutions: both lists, for a thing with work and for one the
    // store has never seen (where "none" is the empty list, not a missing member).
    for (case, thing) in [
        ("a thing with a queued execution", "laser-complete"),
        ("a thing the store has never seen", "laser-unknown"),
    ] {
        let (status, body) = emulator.pending(thing).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GetPendingJobExecutions, got {status} {body}"
        );
        assert_complete_document(
            case,
            "GetPendingJobExecutions",
            &body,
            PENDING_MEMBERS,
            PENDING_MEMBERS,
        );
    }

    // DescribeJobExecution: the execution, by its own id and through `$next`.
    for (case, requested_id) in [
        ("by job id", job_id.as_str()),
        ("through $next", "$next"),
    ] {
        let (status, body) = emulator.describe("laser-complete", requested_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from DescribeJobExecution, got {status} {body}"
        );
        assert_complete_document(
            case,
            "DescribeJobExecution",
            &body,
            DESCRIBE_MEMBERS,
            DESCRIBE_MEMBERS,
        );
    }
}
