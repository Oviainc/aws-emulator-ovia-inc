// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the job-execution lifecycle this service serves:
//! `StartCommandExecution` -> `GetPendingJobExecutions` ->
//! `StartNextPendingJobExecution` -> `DescribeJobExecution` ->
//! `UpdateJobExecution`.
//!
//! Every request goes over the wire (restJson1 through the multi-service router),
//! so what is asserted is the HTTP behaviour a device agent sees, and each
//! assertion is made over a SET of inputs rather than one recorded case.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_iotjobsdataplane_iotlaserthingjobmanagerexternalservice::ops::types::JOB_EXECUTION_STATUS_VALUES;
use service_com_amazonaws_iotjobsdataplane_iotlaserthingjobmanagerexternalservice::state::{
    command_registry, MAX_CONCURRENT_COMMAND_EXECUTIONS_PER_THING, MAX_STATUS_DETAIL_NAME_LEN,
    MAX_STATUS_DETAIL_PAIRS, MAX_STATUS_DETAIL_VALUE_LEN, MAX_TERMINAL_EXECUTIONS_PER_THING,
};
use service_com_amazonaws_iotjobsdataplane_iotlaserthingjobmanagerexternalservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 `Authorization` header whose credential scope names this service's
/// signing name, which is how the router picks the service for a request.
const SIGV4: &str = "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/us-east-1/iot-jobs-data/aws4_request, SignedHeaders=host, Signature=0";

/// The two values of the model's `JobExecutionStatus` enum that
/// `GetPendingJobExecutions` documents as "not in a terminal status".
const PENDING_STATUSES: &[&str] = &["QUEUED", "IN_PROGRESS"];

/// Every OTHER value of that enum, taken from the generated catalog
/// `JOB_EXECUTION_STATUS_VALUES` (`src/ops/types/shared_types.rs`, rendered from
/// the Smithy model) rather than hand-listed here.
fn terminal_statuses() -> Vec<&'static str> {
    JOB_EXECUTION_STATUS_VALUES
        .iter()
        .copied()
        .filter(|status| !PENDING_STATUSES.contains(status))
        .collect()
}

/// `stepTimeoutInMinutes`' published bounds, from the member's documentation:
/// "Valid values for this parameter range from 1 to 10080 (1 minute to 7 days)."
const MIN_STEP_TIMEOUT: i64 = 1;
const MAX_STEP_TIMEOUT: i64 = 10_080;

const COMMAND_ARN: &str = "arn:aws:iot:us-east-1:111122223333:command/reboot";

/// One emulator holding one shared engine, driven over HTTP.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        // `StartCommandExecution` starts an execution only for a command that
        // EXISTS. Commands are control-plane resources (`iot:CreateCommand`, a
        // scaffold in its own crate under its own issue), so the shared index
        // they are registered in is seeded here with the one command these tests
        // start — the same write that create makes. The refusal an unregistered
        // command gets is asserted in `command_execution_validation.rs`.
        command_registry(&*engine)
            .expect("the shared IoT command index")
            .lock()
            .register(COMMAND_ARN)
            .expect("register the command these tests start");
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
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    async fn start_command(&self, body: Value) -> (u16, Value) {
        self.call("POST", "/command-executions", Some(body)).await
    }

    /// Start a command on `thing_name` and return its execution id.
    async fn start_on(&self, thing_name: &str) -> String {
        let (status, body) = self
            .start_command(json!({
                "targetArn": format!("arn:aws:iot:us-east-1:111122223333:thing/{thing_name}"),
                "commandArn": COMMAND_ARN,
            }))
            .await;
        assert_eq!(status, 200, "thing {thing_name}: expected 200, got {status} {body}");
        execution_id(&body).to_string()
    }

    async fn pending(&self, thing_name: &str) -> (u16, Value) {
        self.call("GET", &format!("/things/{thing_name}/jobs"), None)
            .await
    }

    async fn start_next(&self, thing_name: &str, body: Value) -> (u16, Value) {
        self.call(
            "PUT",
            &format!("/things/{thing_name}/jobs/$next"),
            Some(body),
        )
        .await
    }

    async fn describe(&self, thing_name: &str, job_id: &str, query: &str) -> (u16, Value) {
        self.call(
            "GET",
            &format!("/things/{thing_name}/jobs/{job_id}{query}"),
            None,
        )
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

fn execution_id(body: &Value) -> &str {
    body["executionId"]
        .as_str()
        .unwrap_or_else(|| panic!("expected an executionId, got {body}"))
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

/// A minted `executionId` satisfies `CommandExecutionId`: len 1..64, pattern
/// `^[a-zA-Z0-9_-]+$`.
fn well_formed_execution_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// RULE: a started command execution is PERSISTED as a queued job execution of the
/// thing its `targetArn` names, whatever its parameters, and the response carries
/// the stored execution's id — so reading it back reports what was stored.
///
/// The rows differ in parameter type and count, in whether a timeout is asked
/// for, and in the thing name's form.
#[tokio::test]
async fn every_started_command_is_persisted_as_a_queued_execution_of_its_thing() {
    let emulator = Emulator::new();

    // (case, thing name, parameters, executionTimeoutSeconds)
    let cases: Vec<(&str, &str, Option<Value>, Option<i64>)> = vec![
        ("no parameters", "laser-1", None, None),
        (
            "one string parameter",
            "laser-2",
            Some(json!({"mode": {"S": "pulse"}})),
            None,
        ),
        (
            "several parameters of several types",
            "laser_3",
            Some(json!({
                "enabled": {"B": true},
                "power": {"I": 42},
                "serial": {"L": 9_007_199_254_740_993_i64},
                "ratio": {"D": 0.5},
                "big": {"UL": "18446744073709551615"},
            })),
            None,
        ),
        (
            "a blob parameter and a timeout",
            "laser:4",
            Some(json!({"firmware": {"BIN": "aGVsbG8="}})),
            Some(300),
        ),
        ("a timeout only", "laser-5", None, Some(1)),
    ];

    for (case, thing_name, parameters, timeout) in cases {
        let mut request = json!({
            "targetArn": format!("arn:aws:iot:us-east-1:111122223333:thing/{thing_name}"),
            "commandArn": COMMAND_ARN,
        });
        if let Some(parameters) = parameters.clone() {
            request["parameters"] = parameters;
        }
        if let Some(timeout) = timeout {
            request["executionTimeoutSeconds"] = json!(timeout);
        }

        let (status, body) = emulator.start_command(request).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} {body}");
        let id = execution_id(&body).to_string();
        assert!(
            well_formed_execution_id(&id),
            "case {case:?}: expected an executionId matching ^[a-zA-Z0-9_-]{{1,64}}$, got {id:?}"
        );

        let (status, pending) = emulator.pending(thing_name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GetPendingJobExecutions, got {status} {pending}"
        );
        assert_eq!(
            job_ids(&pending["queuedJobs"]),
            vec![id.clone()],
            "case {case:?}: expected the new execution queued, got {pending}"
        );
        assert!(
            job_ids(&pending["inProgressJobs"]).is_empty(),
            "case {case:?}: expected no in-progress executions, got {pending}"
        );
        let summary = &pending["queuedJobs"][0];
        assert_eq!(
            summary["versionNumber"], 1,
            "case {case:?}: a freshly queued execution is version 1, got {summary}"
        );
        assert_eq!(
            summary["executionNumber"], 1,
            "case {case:?}: a freshly minted job's first execution is number 1, got {summary}"
        );
        assert!(
            summary["queuedAt"].as_i64().unwrap_or_default() > 0,
            "case {case:?}: queuedAt must be an epoch-seconds timestamp, got {summary}"
        );
        assert!(
            summary["startedAt"].is_null(),
            "case {case:?}: a queued execution has not started, got {summary}"
        );

        // The persisted job document is computed from commandArn + parameters.
        let (status, described) = emulator.describe(thing_name, &id, "").await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from DescribeJobExecution, got {status} {described}"
        );
        let execution = &described["execution"];
        assert_eq!(
            execution["status"], "QUEUED",
            "case {case:?}: expected QUEUED, got {execution}"
        );
        assert!(
            execution["thingName"].is_null(),
            "case {case:?}: the device-facing read does not echo the thing its own URI \
             names (the recorded `$next` and read steps of \
             iotjobsdata-job-execution-queue-poll-and-start carry no thingName), \
             got {execution}"
        );
        let document: Value = serde_json::from_str(
            execution["jobDocument"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected a jobDocument, got {execution}")),
        )
        .unwrap_or_else(|e| panic!("case {case:?}: the job document must be JSON: {e}"));
        assert_eq!(
            document["commandArn"], COMMAND_ARN,
            "case {case:?}: the job document names the command, got {document}"
        );
        assert_eq!(
            document["parameters"],
            parameters.unwrap_or_else(|| json!({})),
            "case {case:?}: the job document carries the request's parameters, got {document}"
        );

        // The timer `executionTimeoutSeconds` starts is reported as the seconds
        // left before the execution would time out.
        match timeout {
            Some(seconds) => {
                let remaining = execution["approximateSecondsBeforeTimedOut"]
                    .as_i64()
                    .unwrap_or(-1);
                assert!(
                    (0..=seconds).contains(&remaining),
                    "case {case:?}: expected at most {seconds} seconds left, got {remaining}"
                );
            }
            None => assert!(
                execution["approximateSecondsBeforeTimedOut"].is_null(),
                "case {case:?}: no timer was asked for, got {execution}"
            ),
        }
    }
}

/// RULE: `clientToken` makes `StartCommandExecution` idempotent — a replay with
/// the same token and the same parameters returns the first call's `executionId`
/// and creates no second execution, a replay with the same token and different
/// parameters is refused 409 `ConflictException`, and calls with distinct (or no)
/// tokens are distinct executions.
#[tokio::test]
async fn client_token_replays_return_the_first_execution_and_differing_replays_conflict() {
    let emulator = Emulator::new();
    let thing = "laser-idem";
    let target = format!("arn:aws:iot:us-east-1:111122223333:thing/{thing}");

    // Each row is one request shape; replaying it under its own token must be a
    // no-op, and reusing that token with any OTHER row's parameters must conflict.
    let rows: Vec<(&str, Value)> = vec![
        ("one parameter", json!({"mode": {"S": "pulse"}})),
        ("another parameter value", json!({"mode": {"S": "continuous"}})),
        ("a parameter of another type", json!({"power": {"I": 7}})),
        ("two parameters", json!({"mode": {"S": "pulse"}, "power": {"I": 7}})),
    ];

    let mut ids = Vec::new();
    for (index, (case, parameters)) in rows.iter().enumerate() {
        let token = format!("tok-{index}");
        let request = json!({
            "targetArn": target,
            "commandArn": COMMAND_ARN,
            "parameters": parameters,
            "clientToken": token,
        });

        let (status, first) = emulator.start_command(request.clone()).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} {first}");
        let (status, replay) = emulator.start_command(request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a replay must succeed, got {status} {replay}"
        );
        assert_eq!(
            execution_id(&first),
            execution_id(&replay),
            "case {case:?}: a replay must return the first executionId"
        );
        ids.push(execution_id(&first).to_string());

        // Same token, different parameters: a conflict, and still no new execution.
        let other = rows[(index + 1) % rows.len()].1.clone();
        let (status, conflict) = emulator
            .start_command(json!({
                "targetArn": target,
                "commandArn": COMMAND_ARN,
                "parameters": other,
                "clientToken": token,
            }))
            .await;
        assert_eq!(
            status, 409,
            "case {case:?}: reusing a token with other parameters is a conflict, got {status} {conflict}"
        );
        assert_eq!(
            error_code(&conflict),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {conflict}"
        );
    }

    let (_, pending) = emulator.pending(thing).await;
    let queued = job_ids(&pending["queuedJobs"]);
    assert_eq!(
        queued, ids,
        "one execution per distinct token, in enqueue order: expected {ids:?}, got {queued:?}"
    );

    // Two calls carrying NO token are two executions, even when identical.
    let request = json!({"targetArn": target, "commandArn": COMMAND_ARN});
    let (_, one) = emulator.start_command(request.clone()).await;
    let (_, two) = emulator.start_command(request).await;
    assert_ne!(
        execution_id(&one),
        execution_id(&two),
        "without a clientToken each call mints its own execution"
    );
    let (_, pending) = emulator.pending(thing).await;
    assert_eq!(
        job_ids(&pending["queuedJobs"]).len(),
        ids.len() + 2,
        "expected {} queued executions, got {pending}",
        ids.len() + 2
    );
}

/// RULE: a device's progress through an execution is persisted, so every later
/// read reports the CURRENT status, statusDetails and version: `$next` moves the
/// oldest queued execution to `IN_PROGRESS` (version +1), and each
/// `UpdateJobExecution` applies its status and increments the version again.
#[tokio::test]
async fn the_execution_lifecycle_is_persisted_and_read_back_at_every_step() {
    let emulator = Emulator::new();
    let thing = "laser-1";
    let id = emulator.start_on(thing).await;

    let (status, started) = emulator
        .start_next(
            thing,
            json!({"statusDetails": {"step": "download"}, "stepTimeoutInMinutes": 10}),
        )
        .await;
    assert_eq!(status, 200, "expected 200 from $next, got {status} {started}");
    let execution = &started["execution"];
    assert_eq!(
        execution["jobId"], id.as_str(),
        "$next must return the queued execution, got {execution}"
    );
    assert_eq!(
        execution["status"], "IN_PROGRESS",
        "$next starts the execution, got {execution}"
    );
    assert_eq!(
        execution["versionNumber"], 2,
        "starting the execution is a device update, so version 2, got {execution}"
    );
    assert!(
        execution["startedAt"].as_i64().unwrap_or_default() > 0,
        "$next stamps startedAt, got {execution}"
    );
    assert!(
        execution["jobDocument"].is_string(),
        "the device needs the job document to run the job, got {execution}"
    );
    let remaining = execution["approximateSecondsBeforeTimedOut"]
        .as_i64()
        .unwrap_or(-1);
    assert!(
        (0..=600).contains(&remaining),
        "a 10 minute step timeout leaves at most 600 seconds, got {remaining}"
    );

    // The job moved lists, and the move is what a later reader sees.
    let (_, pending) = emulator.pending(thing).await;
    assert_eq!(
        job_ids(&pending["inProgressJobs"]),
        vec![id.clone()],
        "expected the job in progress, got {pending}"
    );
    assert!(
        job_ids(&pending["queuedJobs"]).is_empty(),
        "expected nothing still queued, got {pending}"
    );

    // Describe reports the current state, with the document opt-out honoured.
    for (case, query, expect_document) in [
        ("default", "", true),
        ("includeJobDocument=true", "?includeJobDocument=true", true),
        ("includeJobDocument=false", "?includeJobDocument=false", false),
    ] {
        let (status, described) = emulator.describe(thing, &id, query).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {described}"
        );
        let execution = &described["execution"];
        assert_eq!(
            execution["status"], "IN_PROGRESS",
            "case {case:?}: expected IN_PROGRESS, got {execution}"
        );
        assert_eq!(
            execution["statusDetails"]["step"], "download",
            "case {case:?}: expected the device's statusDetails, got {execution}"
        );
        assert_eq!(
            execution["versionNumber"], 2,
            "case {case:?}: expected version 2, got {execution}"
        );
        assert_eq!(
            execution["jobDocument"].is_string(),
            expect_document,
            "case {case:?}: jobDocument presence, got {execution}"
        );
    }

    // A read does not advance anything: the next read sees the same version.
    let (_, again) = emulator.describe(thing, &id, "").await;
    assert_eq!(
        again["execution"]["versionNumber"], 2,
        "DescribeJobExecution must not change the execution, got {again}"
    );

    // Reporting progress replaces the details and moves the version.
    let (status, updated) = emulator
        .update(
            thing,
            &id,
            json!({
                "status": "IN_PROGRESS",
                "statusDetails": {"progress": "50"},
                "expectedVersion": 2,
                "includeJobExecutionState": true,
                "includeJobDocument": true,
            }),
        )
        .await;
    assert_eq!(status, 200, "expected 200 from the update, got {status} {updated}");
    assert_eq!(
        updated["executionState"]["versionNumber"], 3,
        "each device update increments the version, got {updated}"
    );
    assert_eq!(
        updated["executionState"]["statusDetails"],
        json!({"progress": "50"}),
        "a supplied statusDetails replaces the stored map, dropping the names it no \
         longer sends, got {updated}"
    );
    assert!(
        updated["jobDocument"].is_string(),
        "includeJobDocument=true returns the document, got {updated}"
    );

    // Finishing it removes it from both pending lists and ends the device's view of
    // it: the read is then refused as terminal.
    let (status, finished) = emulator
        .update(
            thing,
            &id,
            json!({"status": "SUCCEEDED", "expectedVersion": 3, "includeJobExecutionState": true}),
        )
        .await;
    assert_eq!(status, 200, "expected 200 finishing the job, got {status} {finished}");
    assert_eq!(
        finished["executionState"]["status"], "SUCCEEDED",
        "expected SUCCEEDED, got {finished}"
    );
    assert_eq!(
        finished["executionState"]["versionNumber"], 4,
        "expected version 4, got {finished}"
    );

    let (_, pending) = emulator.pending(thing).await;
    assert!(
        job_ids(&pending["queuedJobs"]).is_empty() && job_ids(&pending["inProgressJobs"]).is_empty(),
        "a terminal execution is in neither pending list, got {pending}"
    );
    // And the device-facing read refuses it as finished (410) rather than reporting
    // it or claiming it never existed (404).
    let (status, described) = emulator.describe(thing, &id, "").await;
    assert_eq!(
        status, 410,
        "a finished execution is refused as terminal, got {status} {described}"
    );
    assert_eq!(
        error_code(&described),
        "TerminalStateException",
        "expected TerminalStateException, got {described}"
    );
    assert!(
        described["execution"].is_null(),
        "a refusal carries no execution, got {described}"
    );
}

/// RULE: the response members `UpdateJobExecution` documents as opt-in
/// (`includeJobExecutionState`, `includeJobDocument`, each defaulting to false)
/// are present exactly when the request asks for them.
#[tokio::test]
async fn update_returns_only_the_opt_in_members_the_request_asked_for() {
    let emulator = Emulator::new();
    let thing = "laser-opt";

    for (case, state_flag, document_flag) in [
        ("neither asked for", None, None),
        ("state only", Some(true), None),
        ("document only", None, Some(true)),
        ("both", Some(true), Some(true)),
        ("both explicitly off", Some(false), Some(false)),
    ] {
        let id = emulator.start_on(thing).await;
        let mut request = json!({"status": "IN_PROGRESS"});
        if let Some(flag) = state_flag {
            request["includeJobExecutionState"] = json!(flag);
        }
        if let Some(flag) = document_flag {
            request["includeJobDocument"] = json!(flag);
        }

        let (status, body) = emulator.update(thing, &id, request).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} {body}");
        assert_eq!(
            body["executionState"].is_object(),
            state_flag == Some(true),
            "case {case:?}: executionState presence, got {body}"
        );
        assert_eq!(
            body["jobDocument"].is_string(),
            document_flag == Some(true),
            "case {case:?}: jobDocument presence, got {body}"
        );

        // Clean up so the next row's `$next` and lists are unambiguous.
        let (status, _) = emulator
            .update(thing, &id, json!({"status": "SUCCEEDED"}))
            .await;
        assert_eq!(status, 200, "case {case:?}: expected to be able to finish the job");
    }
}

/// RULE: the pending lists report the thing's non-terminal executions in
/// enqueue order, and `$next` serves them oldest-first — so the device drains its
/// queue in the order the executions were created.
#[tokio::test]
async fn pending_executions_are_reported_and_served_oldest_first() {
    let emulator = Emulator::new();
    let thing = "laser-queue";

    let mut ids = Vec::new();
    for _ in 0..4 {
        ids.push(emulator.start_on(thing).await);
    }

    let (_, pending) = emulator.pending(thing).await;
    assert_eq!(
        job_ids(&pending["queuedJobs"]),
        ids,
        "queuedJobs must be in enqueue order, got {pending}"
    );

    // `$next` resolves to the oldest queued execution, and an already-started one
    // is served again before any queued one (both are "pending").
    let (_, first) = emulator.start_next(thing, json!({})).await;
    assert_eq!(
        first["execution"]["jobId"], ids[0].as_str(),
        "expected the oldest queued execution, got {first}"
    );
    let (_, again) = emulator.start_next(thing, json!({})).await;
    assert_eq!(
        again["execution"]["jobId"], ids[0].as_str(),
        "an in-progress execution is still the next pending one, got {again}"
    );
    assert_eq!(
        again["execution"]["startedAt"], first["execution"]["startedAt"],
        "resuming an execution keeps its original startedAt, got {again}"
    );

    // `$next` also resolves for DescribeJobExecution, without starting anything.
    let (status, described) = emulator.describe(thing, "$next", "").await;
    assert_eq!(status, 200, "expected 200 describing $next, got {status} {described}");
    assert_eq!(
        described["execution"]["jobId"], ids[0].as_str(),
        "$next describes the next pending execution, got {described}"
    );

    // Finish it and the next-oldest takes over.
    let (status, _) = emulator
        .update(thing, &ids[0], json!({"status": "SUCCEEDED"}))
        .await;
    assert_eq!(status, 200, "expected to be able to finish the first execution");
    let (_, second) = emulator.start_next(thing, json!({})).await;
    assert_eq!(
        second["execution"]["jobId"], ids[1].as_str(),
        "expected the next-oldest execution, got {second}"
    );

    let (_, pending) = emulator.pending(thing).await;
    assert_eq!(
        job_ids(&pending["inProgressJobs"]),
        vec![ids[1].clone()],
        "expected one in-progress execution, got {pending}"
    );
    assert_eq!(
        job_ids(&pending["queuedJobs"]),
        vec![ids[2].clone(), ids[3].clone()],
        "expected the rest still queued in order, got {pending}"
    );
}

/// RULE: a status in the model's terminal set ends the execution — it leaves both
/// pending lists, its read is refused 410 `TerminalStateException` and it accepts no
/// further update (409 `InvalidStateTransitionException`) — while a pending status
/// keeps it in the list its status names.
#[tokio::test]
async fn each_modeled_status_lands_the_execution_in_the_right_list() {
    let emulator = Emulator::new();
    let terminal = terminal_statuses();
    assert_eq!(
        terminal.len() + PENDING_STATUSES.len(),
        JOB_EXECUTION_STATUS_VALUES.len(),
        "every modeled status must be classified, got {terminal:?} beside {PENDING_STATUSES:?}"
    );

    for status in &terminal {
        let thing = format!("laser-{}", status.to_lowercase().replace('_', "-"));
        let id = emulator.start_on(&thing).await;
        let (code, body) = emulator
            .update(&thing, &id, json!({"status": status}))
            .await;
        assert_eq!(code, 200, "case {status:?}: expected 200, got {code} {body}");

        let (_, pending) = emulator.pending(&thing).await;
        assert!(
            job_ids(&pending["queuedJobs"]).is_empty()
                && job_ids(&pending["inProgressJobs"]).is_empty(),
            "case {status:?}: a terminal execution is in neither list, got {pending}"
        );
        let (code, described) = emulator.describe(&thing, &id, "").await;
        assert_eq!(
            code, 410,
            "case {status:?}: a finished execution is refused as terminal, got {code} {described}"
        );
        assert_eq!(
            error_code(&described),
            "TerminalStateException",
            "case {status:?}: expected TerminalStateException, got {described}"
        );
        // The refusal states the condition it refused on; it carries nothing from
        // the request, so it is one message for every terminal status.
        assert_eq!(
            described["message"].as_str(),
            Some(
                "Job Execution has reached terminal state. It is neither IN_PROGRESS nor QUEUED"
            ),
            "case {status:?}: expected the terminal-state message, got {described}"
        );

        // `$next` has nothing left to start: a 200 with no execution.
        let (code, next) = emulator.start_next(&thing, json!({})).await;
        assert_eq!(
            code, 200,
            "case {status:?}: a thing with no pending job answers 200, got {code} {next}"
        );
        assert!(
            next["execution"].is_null(),
            "case {status:?}: expected no execution, got {next}"
        );

        // And it accepts no further update, whatever status is asked for.
        for next_status in JOB_EXECUTION_STATUS_VALUES {
            let (code, refused) = emulator
                .update(&thing, &id, json!({"status": next_status}))
                .await;
            assert_eq!(
                code, 409,
                "case {status:?} -> {next_status:?}: expected 409, got {code} {refused}"
            );
            assert_eq!(
                error_code(&refused),
                "InvalidStateTransitionException",
                "case {status:?} -> {next_status:?}: expected InvalidStateTransitionException, got {refused}"
            );
            // The refusal names the job, the thing and the status the execution
            // finished in — all three from the request and the stored record — so a
            // refused update is visibly one that changed nothing.
            let expected_message =
                format!("Job {id} for {thing} has already finished with status {status}");
            assert_eq!(
                refused["message"].as_str(),
                Some(expected_message.as_str()),
                "case {status:?} -> {next_status:?}: expected {expected_message:?}, got {refused}"
            );
            let (code, described) = emulator.describe(&thing, &id, "").await;
            assert_eq!(
                code, 410,
                "case {status:?} -> {next_status:?}: the refused update left it finished, got {code} {described}"
            );
        }
    }

    for status in PENDING_STATUSES {
        let thing = format!("laser-pending-{}", status.to_lowercase().replace('_', "-"));
        let id = emulator.start_on(&thing).await;
        let (code, body) = emulator
            .update(&thing, &id, json!({"status": status}))
            .await;
        assert_eq!(code, 200, "case {status:?}: expected 200, got {code} {body}");
        let (_, pending) = emulator.pending(&thing).await;
        let list = if *status == "QUEUED" {
            "queuedJobs"
        } else {
            "inProgressJobs"
        };
        assert_eq!(
            job_ids(&pending[list]),
            vec![id.clone()],
            "case {status:?}: expected the execution in {list}, got {pending}"
        );
    }
}

/// RULE: an `expectedVersion` that is not the stored version rejects the update 400
/// `InvalidRequestException` and leaves the execution untouched; the matching version
/// succeeds. (The guard's whole table lives in `job_execution_state_machine.rs`.)
#[tokio::test]
async fn an_unexpected_version_rejects_the_update_and_changes_nothing() {
    let emulator = Emulator::new();
    let thing = "laser-version";
    let id = emulator.start_on(thing).await;

    // The stored version is 1 before any device update.
    for stale in [0_i64, 2, 3, 99] {
        let (status, refused) = emulator
            .update(
                thing,
                &id,
                json!({"status": "IN_PROGRESS", "expectedVersion": stale}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case expectedVersion={stale}: expected 400, got {status} {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "InvalidRequestException",
            "case expectedVersion={stale}: expected InvalidRequestException, got {refused}"
        );
        let (_, described) = emulator.describe(thing, &id, "").await;
        assert_eq!(
            described["execution"]["versionNumber"], 1,
            "case expectedVersion={stale}: the rejected update must change nothing, got {described}"
        );
        assert_eq!(
            described["execution"]["status"], "QUEUED",
            "case expectedVersion={stale}: the rejected update must change nothing, got {described}"
        );
    }

    let (status, accepted) = emulator
        .update(
            thing,
            &id,
            json!({"status": "IN_PROGRESS", "expectedVersion": 1, "includeJobExecutionState": true}),
        )
        .await;
    assert_eq!(status, 200, "the matching version is accepted, got {status} {accepted}");
    assert_eq!(
        accepted["executionState"]["versionNumber"], 2,
        "the accepted update increments the version, got {accepted}"
    );

    // The old expected version is now stale, so the same request is refused.
    let (status, refused) = emulator
        .update(
            thing,
            &id,
            json!({"status": "IN_PROGRESS", "expectedVersion": 1}),
        )
        .await;
    assert_eq!(
        status, 400,
        "replaying a consumed expectedVersion is refused, got {status} {refused}"
    );
}

/// RULE: an execution the request names but the store does not hold is 404
/// `ResourceNotFoundException`, while a thing with no executions still REPORTS
/// (empty lists, 200) — absence of a job is not absence of the thing.
#[tokio::test]
async fn unknown_executions_are_404_but_a_thing_with_none_still_reports() {
    let emulator = Emulator::new();
    let known = "laser-known";
    let id = emulator.start_on(known).await;

    // (case, thing, job id, describe query, extra update members)
    let missing: Vec<(&str, &str, String, &str, Value)> = vec![
        (
            "unknown thing and job",
            "unknown-thing",
            "does-not-exist".to_string(),
            "",
            json!({}),
        ),
        (
            "unknown job on a known thing",
            known,
            "does-not-exist".to_string(),
            "",
            json!({}),
        ),
        (
            "known job on another thing",
            "laser-other",
            id.clone(),
            "",
            json!({}),
        ),
        (
            "$next on a thing with no jobs",
            "laser-empty",
            "$next".to_string(),
            "",
            json!({}),
        ),
        (
            "an executionNumber that does not exist",
            known,
            id.clone(),
            "?executionNumber=7",
            json!({"executionNumber": 7}),
        ),
    ];

    for (case, thing, job_id, query, update_extra) in missing {
        let (status, body) = emulator.describe(thing, &job_id, query).await;
        assert_eq!(status, 404, "case {case:?}: expected 404, got {status} {body}");
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {body}"
        );
        // The refusal's message is built from the two request members that name the
        // execution, so it states which job on which thing was not found.
        let expected_message = format!("Job {job_id} not found for {thing}");
        assert_eq!(
            body["message"].as_str(),
            Some(expected_message.as_str()),
            "case {case:?}: expected the message {expected_message:?}, got {body}"
        );

        let mut request = json!({"status": "SUCCEEDED"});
        for (name, value) in update_extra.as_object().into_iter().flatten() {
            request[name.as_str()] = value.clone();
        }
        let (status, body) = emulator.update(thing, &job_id, request).await;
        if job_id == "$next" {
            // `$next` is not a `JobId`, so UpdateJobExecution's own pattern
            // refuses it before the handler runs.
            assert_eq!(
                status, 400,
                "case {case:?}: $next is not a valid jobId for an update, got {status} {body}"
            );
        } else {
            assert_eq!(
                status, 404,
                "case {case:?}: expected 404 from the update, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {case:?}: expected ResourceNotFoundException, got {body}"
            );
            assert_eq!(
                body["message"].as_str(),
                Some(expected_message.as_str()),
                "case {case:?}: the update's refusal follows the same message rule, got {body}"
            );
        }
    }

    // The known execution is still exactly where it was.
    let (status, described) = emulator.describe(known, &id, "?executionNumber=1").await;
    assert_eq!(status, 200, "case known execution: expected 200, got {status} {described}");
    assert_eq!(
        described["execution"]["versionNumber"], 1,
        "none of the failed lookups touched the execution, got {described}"
    );

    for thing in ["laser-empty", "unknown-thing"] {
        let (status, pending) = emulator.pending(thing).await;
        assert_eq!(
            status, 200,
            "case {thing:?}: a thing with no executions answers 200, got {status} {pending}"
        );
        assert_eq!(
            pending["queuedJobs"],
            json!([]),
            "case {thing:?}: expected an empty queuedJobs list, got {pending}"
        );
        assert_eq!(
            pending["inProgressJobs"],
            json!([]),
            "case {thing:?}: expected an empty inProgressJobs list, got {pending}"
        );
        let (status, next) = emulator.start_next(thing, json!({})).await;
        assert_eq!(
            status, 200,
            "case {thing:?}: $next on an empty queue answers 200, got {status} {next}"
        );
        assert!(
            next["execution"].is_null(),
            "case {thing:?}: expected no execution, got {next}"
        );
    }
}

/// RULE: `targetArn` must name an IoT thing and `commandArn` an IoT command;
/// anything else is refused 400 `ValidationException` and nothing is persisted.
#[tokio::test]
async fn start_command_execution_refuses_arns_that_name_no_thing_or_command() {
    let emulator = Emulator::new();

    // (case, targetArn, commandArn)
    let refused: Vec<(&str, &str, &str)> = vec![
        ("target is not an ARN", "laser-1", COMMAND_ARN),
        (
            "target has too few fields",
            "arn:aws:iot:thing/laser-1",
            COMMAND_ARN,
        ),
        (
            "target names another service",
            "arn:aws:s3:us-east-1:111122223333:thing/laser-1",
            COMMAND_ARN,
        ),
        (
            "target names another resource type",
            "arn:aws:iot:us-east-1:111122223333:job/laser-1",
            COMMAND_ARN,
        ),
        (
            "target names no thing",
            "arn:aws:iot:us-east-1:111122223333:thing/",
            COMMAND_ARN,
        ),
        (
            "target thing name has an illegal character",
            "arn:aws:iot:us-east-1:111122223333:thing/laser 1",
            COMMAND_ARN,
        ),
        (
            "command is not an ARN",
            "arn:aws:iot:us-east-1:111122223333:thing/laser-1",
            "reboot",
        ),
        (
            "command names another resource type",
            "arn:aws:iot:us-east-1:111122223333:thing/laser-1",
            "arn:aws:iot:us-east-1:111122223333:thing/laser-1",
        ),
    ];

    for (case, target_arn, command_arn) in refused {
        let (status, body) = emulator
            .start_command(json!({"targetArn": target_arn, "commandArn": command_arn}))
            .await;
        assert_eq!(status, 400, "case {case:?}: expected 400, got {status} {body}");
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body}"
        );
    }

    // The accepted forms: every partition, region and account, and a thing name
    // using each character class `ThingName` allows.
    for (case, target_arn, thing) in [
        (
            "aws partition",
            "arn:aws:iot:us-east-1:111122223333:thing/laser-1",
            "laser-1",
        ),
        (
            "another partition and region",
            "arn:aws-cn:iot:cn-north-1:444455556666:thing/laser_2",
            "laser_2",
        ),
        (
            "a colon in the thing name",
            "arn:aws:iot:eu-west-1:111122223333:thing/plant:laser:3",
            "plant:laser:3",
        ),
    ] {
        let (status, body) = emulator
            .start_command(json!({"targetArn": target_arn, "commandArn": COMMAND_ARN}))
            .await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} {body}");
        let (status, pending) = emulator.pending(thing).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} {pending}");
        assert_eq!(
            job_ids(&pending["queuedJobs"]),
            vec![execution_id(&body).to_string()],
            "case {case:?}: expected the execution queued on {thing}, got {pending}"
        );
    }
}

/// RULE: the store a request grows is BOUNDED. A device admits at most
/// `MAX_CONCURRENT_COMMAND_EXECUTIONS_PER_THING` command executions that have not
/// reached a terminal status — a further `StartCommandExecution` is refused with
/// the modeled `ServiceQuotaExceededException` and stores nothing (the limit over a
/// set of devices and drain states is asserted in
/// `command_execution_concurrency.rs`) — and a thing keeps at most
/// `MAX_TERMINAL_EXECUTIONS_PER_THING` finished executions, the oldest evicted
/// first as new ones are enqueued.
#[tokio::test]
async fn the_execution_store_is_bounded_per_thing() {
    let emulator = Emulator::new();

    let thing = "laser-quota";
    for index in 0..MAX_CONCURRENT_COMMAND_EXECUTIONS_PER_THING {
        let (status, body) = emulator
            .start_command(json!({
                "targetArn": format!("arn:aws:iot:us-east-1:111122223333:thing/{thing}"),
                "commandArn": COMMAND_ARN,
            }))
            .await;
        assert_eq!(
            status, 200,
            "case execution {index}: expected 200 below the quota, got {status} {body}"
        );
    }

    let (status, refused) = emulator
        .start_command(json!({
            "targetArn": format!("arn:aws:iot:us-east-1:111122223333:thing/{thing}"),
            "commandArn": COMMAND_ARN,
        }))
        .await;
    assert_eq!(
        status, 402,
        "case over the quota: expected 402, got {status} {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "ServiceQuotaExceededException",
        "case over the quota: expected ServiceQuotaExceededException, got {refused}"
    );
    let (_, pending) = emulator.pending(thing).await;
    assert_eq!(
        job_ids(&pending["queuedJobs"]).len(),
        MAX_CONCURRENT_COMMAND_EXECUTIONS_PER_THING,
        "case over the quota: the refused request must store nothing"
    );

    // Finished executions are retained only up to their own bound.
    let thing = "laser-retention";
    let mut finished = Vec::new();
    for index in 0..=MAX_TERMINAL_EXECUTIONS_PER_THING {
        let id = emulator.start_on(thing).await;
        let (status, body) = emulator
            .update(thing, &id, json!({"status": "SUCCEEDED"}))
            .await;
        assert_eq!(
            status, 200,
            "case finished {index}: expected 200, got {status} {body}"
        );
        finished.push(id);
    }
    // Every one of them is still HELD while the bound is not exceeded: a finished
    // execution the store holds is refused as terminal (410), not as absent (404).
    let (status, described) = emulator.describe(thing, &finished[0], "").await;
    assert_eq!(
        status, 410,
        "case at the retention bound: the oldest finished execution is still held, got {status} {described}"
    );

    // Enqueueing one more evicts the OLDEST finished execution and keeps the rest.
    emulator.start_on(thing).await;
    let (status, gone) = emulator.describe(thing, &finished[0], "").await;
    assert_eq!(
        status, 404,
        "case past the retention bound: the oldest finished execution is evicted, got {status} {gone}"
    );
    for (index, id) in finished.iter().enumerate().skip(1) {
        let (status, kept) = emulator.describe(thing, id, "").await;
        assert_eq!(
            status, 410,
            "case finished {index}: a newer finished execution is kept, got {status} {kept}"
        );
    }
}

/// RULE: `stepTimeoutInMinutes` is honoured inside its published range (1..10080)
/// and refused 400 outside it; `-1` cancels the step timer on
/// `UpdateJobExecution`, which is the only operation that documents it.
#[tokio::test]
async fn the_step_timeout_is_honoured_inside_its_published_range_and_refused_outside() {
    let emulator = Emulator::new();

    for minutes in [MIN_STEP_TIMEOUT, 10, MAX_STEP_TIMEOUT] {
        let thing = format!("laser-step-{minutes}");
        emulator.start_on(&thing).await;
        let (status, started) = emulator
            .start_next(&thing, json!({"stepTimeoutInMinutes": minutes}))
            .await;
        assert_eq!(
            status, 200,
            "case stepTimeoutInMinutes={minutes}: expected 200, got {status} {started}"
        );
        let remaining = started["execution"]["approximateSecondsBeforeTimedOut"]
            .as_i64()
            .unwrap_or(-1);
        let expected = minutes * 60;
        assert!(
            (expected - 60..=expected).contains(&remaining),
            "case stepTimeoutInMinutes={minutes}: expected about {expected} seconds left, got {remaining}"
        );
    }

    // Each published bound, and one step past it on each side.
    for minutes in [MIN_STEP_TIMEOUT - 1, MAX_STEP_TIMEOUT + 1, -1, -100] {
        let thing = "laser-step-bad";
        let id = emulator.start_on(thing).await;
        let (status, refused) = emulator
            .start_next(thing, json!({"stepTimeoutInMinutes": minutes}))
            .await;
        assert_eq!(
            status, 400,
            "case stepTimeoutInMinutes={minutes}: expected 400 from $next, got {status} {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "InvalidRequestException",
            "case stepTimeoutInMinutes={minutes}: expected InvalidRequestException, got {refused}"
        );

        // On UpdateJobExecution the same values are refused, except -1, which
        // the member documents as "cancel the current step timer".
        let (status, body) = emulator
            .update(
                thing,
                &id,
                json!({"status": "IN_PROGRESS", "stepTimeoutInMinutes": minutes}),
            )
            .await;
        if minutes == -1 {
            assert_eq!(
                status, 200,
                "case stepTimeoutInMinutes=-1: cancelling the step timer is valid, got {status} {body}"
            );
            let (_, described) = emulator.describe(thing, &id, "").await;
            assert!(
                described["execution"]["approximateSecondsBeforeTimedOut"].is_null(),
                "case stepTimeoutInMinutes=-1: the step timer must be cancelled, got {described}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case stepTimeoutInMinutes={minutes}: expected 400 from the update, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "InvalidRequestException",
                "case stepTimeoutInMinutes={minutes}: expected InvalidRequestException, got {body}"
            );
        }

        // Clear the queue for the next row.
        let (status, _) = emulator
            .update(thing, &id, json!({"status": "SUCCEEDED"}))
            .await;
        assert_eq!(
            status, 200,
            "case stepTimeoutInMinutes={minutes}: expected to be able to finish the job"
        );
    }
}

/// RULE: a job execution's `statusDetails` is bounded — at most
/// `MAX_STATUS_DETAIL_PAIRS` name/value pairs, each name and value within its own
/// maximum length — and a map past any of those bounds is refused 400
/// `InvalidRequestException` with the execution left untouched, on BOTH operations
/// that accept details. The bound is on the map the REQUEST carries, because a
/// supplied map replaces the stored one, so repeated updates accumulate nothing.
#[tokio::test]
async fn status_details_are_bounded_and_an_over_bound_map_changes_nothing() {
    let emulator = Emulator::new();

    // Accepted: the pair count at its bound, and a name and a value of exactly
    // the maximum length.
    let full_map: Value = (0..MAX_STATUS_DETAIL_PAIRS)
        .map(|index| (format!("key-{index}"), json!("v")))
        .collect::<serde_json::Map<String, Value>>()
        .into();
    let longest_name = "n".repeat(MAX_STATUS_DETAIL_NAME_LEN);
    let longest_value = "v".repeat(MAX_STATUS_DETAIL_VALUE_LEN);

    for (case, details) in [
        ("the pair count at its bound", full_map),
        (
            "a name of exactly the maximum length",
            json!({longest_name: "v"}),
        ),
        (
            "a value of exactly the maximum length",
            json!({"step": longest_value}),
        ),
    ] {
        let thing = "laser-details-ok";
        let id = emulator.start_on(thing).await;
        let (status, body) = emulator
            .update(
                thing,
                &id,
                json!({"status": "IN_PROGRESS", "statusDetails": details, "includeJobExecutionState": true}),
            )
            .await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} {body}");
        assert_eq!(
            body["executionState"]["statusDetails"], details,
            "case {case:?}: expected the details to be stored, got {body}"
        );
        let (status, _) = emulator
            .update(thing, &id, json!({"status": "SUCCEEDED"}))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected to be able to finish the job"
        );
    }

    // Refused: one pair too many, a name one character too long, a value one
    // character too long.
    let too_many: Value = (0..=MAX_STATUS_DETAIL_PAIRS)
        .map(|index| (format!("key-{index}"), json!("v")))
        .collect::<serde_json::Map<String, Value>>()
        .into();
    let over_long_name = "n".repeat(MAX_STATUS_DETAIL_NAME_LEN + 1);
    let over_long_value = "v".repeat(MAX_STATUS_DETAIL_VALUE_LEN + 1);

    for (case, details) in [
        ("one pair too many", too_many),
        ("a name one character too long", json!({over_long_name: "v"})),
        (
            "a value one character too long",
            json!({"step": over_long_value}),
        ),
    ] {
        let thing = "laser-details-refused";
        let id = emulator.start_on(thing).await;

        // Both operations that accept `statusDetails` refuse it.
        let (status, refused) = emulator
            .start_next(thing, json!({"statusDetails": details}))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 from $next, got {status} {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "InvalidRequestException",
            "case {case:?}: expected InvalidRequestException, got {refused}"
        );

        let (status, refused) = emulator
            .update(
                thing,
                &id,
                json!({"status": "IN_PROGRESS", "statusDetails": details}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 from the update, got {status} {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "InvalidRequestException",
            "case {case:?}: expected InvalidRequestException, got {refused}"
        );

        // Neither refusal touched the execution.
        let (_, described) = emulator.describe(thing, &id, "").await;
        assert_eq!(
            described["execution"]["status"], "QUEUED",
            "case {case:?}: the refused calls must change nothing, got {described}"
        );
        assert_eq!(
            described["execution"]["versionNumber"], 1,
            "case {case:?}: the refused calls must change nothing, got {described}"
        );
        assert!(
            described["execution"]["statusDetails"].is_null(),
            "case {case:?}: the refused calls must store no details, got {described}"
        );

        let (status, _) = emulator
            .update(thing, &id, json!({"status": "SUCCEEDED"}))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected to be able to finish the job"
        );
    }

    // Details cannot ACCUMULATE across updates at all: each supplied map replaces
    // the stored one, so a record holds exactly what the last write sent and the
    // bound is a bound on one request's map.
    let thing = "laser-details-accumulate";
    let id = emulator.start_on(thing).await;
    for index in 0..MAX_STATUS_DETAIL_PAIRS {
        let (status, body) = emulator
            .update(
                thing,
                &id,
                json!({"status": "IN_PROGRESS", "statusDetails": {format!("key-{index}"): "v"}}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case write {index}: expected 200, got {status} {body}"
        );
        let (_, described) = emulator.describe(thing, &id, "").await;
        assert_eq!(
            described["execution"]["statusDetails"],
            json!({format!("key-{index}"): "v"}),
            "case write {index}: the record holds exactly the map just written, got {described}"
        );
    }
    // One map of one pair too many is still refused, and changes nothing.
    let one_pair_too_many: Value = (0..=MAX_STATUS_DETAIL_PAIRS)
        .map(|index| (format!("over-{index}"), json!("v")))
        .collect::<serde_json::Map<String, Value>>()
        .into();
    let (status, refused) = emulator
        .update(
            thing,
            &id,
            json!({"status": "IN_PROGRESS", "statusDetails": one_pair_too_many}),
        )
        .await;
    assert_eq!(
        status, 400,
        "case one map past the bound: expected 400, got {status} {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "InvalidRequestException",
        "case one map past the bound: expected InvalidRequestException, got {refused}"
    );
    let (_, described) = emulator.describe(thing, &id, "").await;
    assert_eq!(
        described["execution"]["statusDetails"]
            .as_object()
            .map_or(0, serde_json::Map::len),
        1,
        "case one map past the bound: the refused write changes nothing, got {described}"
    );
}
