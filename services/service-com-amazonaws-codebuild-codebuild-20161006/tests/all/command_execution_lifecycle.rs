// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for CodeBuild's sandbox -> command-execution slice, driven over
//! the wire the router serves: `POST /` with
//! `X-Amz-Target: CodeBuild_20161006.<Operation>` on awsJson1_1.
//!
//! The rule under test: `StartCommandExecution` mints ONE command execution of the
//! sandbox the request names (by sandbox id or sandbox ARN), identified by a derived
//! id, carrying the command and command type it was asked for together with that
//! sandbox's id and ARN and the instant it was submitted, in flight and with no
//! result yet — and persists it, so `BatchGetCommandExecutions` reports the same
//! record back under that sandbox. A command execution is SUBMITTED to the sandbox and
//! run in its session, so a sandbox the account does not hold is refused with
//! `ResourceNotFoundException` and one whose session has ENDED — stopped, or timed out —
//! with `InvalidInputException` stating the status it is in, and in both cases nothing
//! is persisted; a sandbox that is still being provisioned takes the submission. Nothing
//! in
//! this emulator executes a sandbox command, so the transition the clock owes an
//! execution ends it in `FAULT` with the reason as its error text and with no exit
//! code and no output.
//!
//! Each suite in `tests/all/` drives the wire itself, so the harness below (the
//! router, `call`, the project body) is this suite's own — the sibling suite's copy is
//! private to it.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_codebuild_codebuild_20161006::ops::types::{CommandExecution, Sandbox};
use service_com_amazonaws_codebuild_codebuild_20161006::state::{
    self, ComAmazonawsCodebuildCodebuild20161006State as CodeBuildState, StoredCommandExecution,
    StoredSandbox,
};
use service_com_amazonaws_codebuild_codebuild_20161006::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below is signed for — deliberately not the emulator's
/// `us-east-1` default, so an ARN assertion proves the region came from the request.
const REGION: &str = "us-west-2";

/// The access key id every request below is signed with.
const ACCESS_KEY_ID: &str = "AKIDCODEBUILDCOMMANDS";

/// Long enough for a command execution's clock-owed terminal transition to have
/// fallen due (the service settles it [`state::COMMAND_EXECUTION_SETTLE_MILLIS`]
/// after `StartCommandExecution`).
const PAST_THE_SETTLE_SPAN: Duration =
    Duration::from_millis(state::COMMAND_EXECUTION_SETTLE_MILLIS as u64 + 400);

/// How far out a SEEDED sandbox's own transition is put, in millis. This suite is
/// about a command execution's clock, so the sandbox it submits to must not settle
/// underneath it; a span this long cannot elapse inside one test.
const SEEDED_SANDBOX_SPAN_MILLIS: i64 = 10 * 60 * 1000;

/// How long ago a SEEDED `Running` sandbox's session went live, in millis: a moment, so
/// the session has the whole of its own wait ahead of it. The sandbox's own wait runs
/// from that instant and the clock reaps it at the end of it, so a session seeded far in
/// the past would be reaped before a command reached it.
const SEEDED_RUNNING_AGE_MILLIS: i64 = 100;

/// The wait a SEEDED sandbox carries: the documented maximum of
/// `Sandbox$timeoutInMinutes` ("from 5 to 2160 (36 hours)"), which maps onto the
/// ceiling of the span the sandbox clock publishes
/// ([`state::MAX_SANDBOX_IDLE_SPAN_MILLIS`]). This suite is about a command execution's
/// clock, so the sandbox it submits to must stay live throughout, whatever the floor of
/// that span is.
const SEEDED_SANDBOX_TIMEOUT_MINUTES: i32 = 2160;

/// Long enough for a sandbox's own clock-owed transition out of `QUEUED` to have
/// fallen due, so the sandbox a command is submitted to is live.
const PAST_THE_SANDBOX_SETTLE_SPAN: Duration =
    Duration::from_millis(state::SANDBOX_SETTLE_MILLIS as u64 + 400);

/// The state a SEEDED sandbox is in by the time a command is submitted to it.
#[derive(Clone, Copy, Debug)]
enum SeededSandbox {
    /// Provisioned and live: its session is what a command execution acts on.
    Running,
    /// Still waiting to be provisioned, so it has no session yet.
    Queued,
    /// Stopped: its session has ended and can never run anything again.
    Ended,
}

/// The members a just-submitted command execution reports nothing for: nothing has
/// run it, so there is no result, no output and no log stream.
const ABSENT_UNTIL_THERE_IS_A_RESULT: &[&str] = &[
    "exitCode",
    "standardOutputContent",
    "standardErrContent",
    "logs",
];

/// One router carrying only this service, over one shared engine — the same assembly
/// `core/router` builds, minus the other services.
fn emulator_on(engine: Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// Call one operation and return its HTTP status and decoded body.
async fn call(router: &MultiServiceRouter, operation: &str, body: Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("CodeBuild_20161006.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential={ACCESS_KEY_ID}/20260101/{REGION}/codebuild/\
                 aws4_request, SignedHeaders=host;x-amz-target, Signature=0f0f"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(&body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let decoded = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, decoded)
}

/// A minimal valid `CreateProject` request for `name`.
fn create_project_body(name: &str) -> Value {
    json!({
        "name": name,
        "source": {"type": "NO_SOURCE", "buildspec": "version: 0.2"},
        "artifacts": {"type": "NO_ARTIFACTS"},
        "environment": {
            "type": "LINUX_CONTAINER",
            "image": "aws/codebuild/standard:7.0",
            "computeType": "BUILD_GENERAL1_SMALL",
        },
        "serviceRole": "arn:aws:iam::000000000000:role/service-role/codebuild",
    })
}

/// Create `name` and start a sandbox of it over the wire, asserting both succeeded.
async fn start_sandbox(router: &MultiServiceRouter, project: &str) -> Value {
    let (status, body) = call(router, "CreateProject", create_project_body(project)).await;
    assert_eq!(
        status, 200,
        "CreateProject {project:?}: expected 200, got {status} with {body}"
    );
    let (status, body) = call(router, "StartSandbox", json!({"projectName": project})).await;
    assert_eq!(
        status, 200,
        "StartSandbox {project:?}: expected 200, got {status} with {body}"
    );
    body["sandbox"].clone()
}

/// Seed a sandbox straight into the store and hand back its (id, ARN).
///
/// A sandbox started over the wire moves on its OWN clock (`QUEUED` until
/// [`state::SANDBOX_SETTLE_MILLIS`], then `RUNNING` until it is stopped or its
/// `timeoutInMinutes` elapses); a test that is about the command execution says which
/// of those states it wants here instead of racing that clock. The wire path itself is
/// covered by [`a_sandbox_started_over_the_wire_runs_commands_until_its_session_ends`].
fn seed_sandbox(
    engine: &engine::InMemoryEngine,
    project: &str,
    id: &str,
    seeded: SeededSandbox,
) -> (String, String) {
    let now = state::now_millis();
    let arn = state::sandbox_arn(REGION, id);
    let ended = matches!(seeded, SeededSandbox::Ended);
    let sandbox = Sandbox {
        id: Some(id.to_string()),
        arn: Some(arn.clone()),
        project_name: Some(project.to_string()),
        request_time: Some(state::datetime(now)),
        timeout_in_minutes: Some(SEEDED_SANDBOX_TIMEOUT_MINUTES),
        status: Some(
            if ended {
                state::SANDBOX_STATUS_STOPPED
            } else {
                state::SANDBOX_STATUS_QUEUED
            }
            .to_string(),
        ),
        // A terminal sandbox is one that has ended — which is what `endTime` records.
        end_time: ended.then(|| state::datetime(now)),
        current_session: Some(state::queued_sandbox_session(
            format!("{id}:session"),
            now,
        )),
        ..Sandbox::default()
    };
    engine.state::<CodeBuildState>().lock().insert_sandbox(
        id.to_string(),
        StoredSandbox {
            sandbox,
            request_millis: now,
            // A `Running` sandbox was provisioned already, so its due instant is in the
            // past and the read materialises the live session; the others must not
            // settle while the test runs.
            settle_due_millis: match seeded {
                SeededSandbox::Running => now - SEEDED_RUNNING_AGE_MILLIS,
                SeededSandbox::Queued | SeededSandbox::Ended => {
                    now + SEEDED_SANDBOX_SPAN_MILLIS
                }
            },
        },
    );
    (id.to_string(), arn)
}

/// Submit `command` to `sandbox_id`, asserting it succeeded, and hand back the
/// execution.
async fn start_command_execution(
    router: &MultiServiceRouter,
    sandbox_id: &str,
    command: &str,
) -> Value {
    let (status, body) = call(
        router,
        "StartCommandExecution",
        json!({"sandboxId": sandbox_id, "command": command}),
    )
    .await;
    assert_eq!(
        status, 200,
        "StartCommandExecution on {sandbox_id:?}: expected 200, got {status} with {body}"
    );
    body["commandExecution"].clone()
}

/// Read command executions back under the sandbox they belong to.
async fn batch_get_command_executions(
    router: &MultiServiceRouter,
    sandbox_id: &str,
    ids: &[String],
) -> Value {
    let (status, body) = call(
        router,
        "BatchGetCommandExecutions",
        json!({"sandboxId": sandbox_id, "commandExecutionIds": ids}),
    )
    .await;
    assert_eq!(
        status, 200,
        "BatchGetCommandExecutions {ids:?}: expected 200, got {status} with {body}"
    );
    body
}

/// The `__type` an error body carries, for asserting the error code a client branches
/// on.
fn error_type(body: &Value) -> String {
    body["__type"].as_str().unwrap_or_default().to_string()
}

/// A string member, for an assertion message that shows what was actually there.
fn text(value: &Value, member: &str) -> String {
    value[member].as_str().unwrap_or_default().to_string()
}

/// RULE: `StartCommandExecution` mints one command execution of the sandbox named —
/// by sandbox id or sandbox ARN — carrying the command and command type it was asked
/// for, that sandbox's id and ARN, the submit instant and no result yet; a later read
/// reports that same record.
///
/// The table spans the command shapes a caller can submit (one line, a multi-line
/// script, quoting and non-ASCII text, a value that looks like an ARN, a long one, a
/// single character), the `type` member present and absent, and both forms of
/// `sandboxId`.
#[tokio::test]
async fn start_command_execution_mints_one_execution_per_command_and_reads_it_back() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(Arc::clone(&engine));

    let long_command = format!("echo {}", "x".repeat(4096));
    // (case, the command, the `type` member asked for, address the sandbox by its ARN)
    let cases: Vec<(&str, String, Option<&str>, bool)> = vec![
        ("a one-line shell command", "echo hello".to_string(), None, false),
        (
            "a multi-line script with the type named",
            "set -euo pipefail\nls -l /tmp\n".to_string(),
            Some("SHELL"),
            false,
        ),
        (
            "quoting and non-ASCII text",
            "printf '%s\\n' \"héllo wörld — ok\"".to_string(),
            None,
            true,
        ),
        // The command is DATA: a value that looks like a sandbox ARN must not be
        // parsed as one.
        (
            "a command that looks like a sandbox ARN",
            state::sandbox_arn(REGION, "not-a-sandbox:0000"),
            None,
            false,
        ),
        ("a long command", long_command, Some("SHELL"), true),
        ("a single character", "x".to_string(), None, false),
    ];

    let mut seen_ids: Vec<String> = Vec::new();
    for (index, (case, command, command_type, by_arn)) in cases.into_iter().enumerate() {
        let (sandbox_id, sandbox_arn) = seed_sandbox(
            &engine,
            "cmd-mint",
            &format!("cmd-mint:{index:04}"),
            SeededSandbox::Running,
        );
        let addressed = if by_arn { &sandbox_arn } else { &sandbox_id };

        let mut request = json!({"sandboxId": addressed, "command": command});
        if let Some(command_type) = command_type {
            request["type"] = json!(command_type);
        }
        let (status, body) = call(&router, "StartCommandExecution", request.clone()).await;
        assert_eq!(
            status, 200,
            "{case}: expected 200, got {status} with {body}"
        );
        let execution = body["commandExecution"].clone();

        let id = text(&execution, "id");
        assert!(
            !id.is_empty() && !seen_ids.contains(&id),
            "{case}: each execution carries an id of its own, got {id:?} \
             already in {seen_ids:?}"
        );
        seen_ids.push(id.clone());
        assert_eq!(
            execution["sandboxId"], sandbox_id.as_str(),
            "{case}: the execution belongs to the sandbox asked for, got {execution}"
        );
        assert_eq!(
            execution["sandboxArn"], sandbox_arn.as_str(),
            "{case}: the execution reports that sandbox's ARN, got {execution}"
        );
        assert_eq!(
            execution["command"], command.as_str(),
            "{case}: the command must be the one submitted, got {execution}"
        );
        // `CommandType` declares exactly one value, so an omitted `type` is a request
        // for that one.
        assert_eq!(
            execution["type"],
            json!(command_type.unwrap_or(state::DEFAULT_COMMAND_TYPE)),
            "{case}: unexpected command type in {execution}"
        );
        assert!(
            execution["submitTime"].is_number(),
            "{case}: submitTime must report when it was submitted, got {execution}"
        );
        assert!(
            execution["startTime"].is_null() && execution["endTime"].is_null(),
            "{case}: a just-submitted execution has neither started nor ended, \
             got {execution}"
        );
        assert_eq!(
            execution["status"],
            json!(state::COMMAND_EXECUTION_STATUS_IN_PROGRESS),
            "{case}: a just-submitted execution is still in flight, got {execution}"
        );
        for member in ABSENT_UNTIL_THERE_IS_A_RESULT {
            assert!(
                execution[member].is_null(),
                "{case}: nothing has run, so {member} must be absent, got {execution}"
            );
        }

        // The round trip: the read path reports the record the write persisted.
        let fetched =
            batch_get_command_executions(&router, &sandbox_id, std::slice::from_ref(&id)).await;
        assert_eq!(
            fetched["commandExecutions"][0], execution,
            "{case}: BatchGetCommandExecutions must report the stored execution, \
             got {fetched}"
        );
        assert_eq!(
            fetched["commandExecutionsNotFound"],
            json!([]),
            "{case}: an execution that exists is not missing, got {fetched}"
        );
    }
}

/// RULE: each `StartCommandExecution` of a sandbox mints a distinct execution, and the
/// store keeps that sandbox's executions in submit order.
#[tokio::test]
async fn each_submission_mints_a_distinct_execution_and_keeps_submit_order() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(Arc::clone(&engine));

    // (the sandbox id, how many commands to submit to it)
    for (sandbox, count) in [("order:one", 1), ("order:three", 3), ("order:five", 5)] {
        let (sandbox_id, _) =
            seed_sandbox(&engine, "cmd-order", sandbox, SeededSandbox::Running);
        let mut ids_in_submit_order: Vec<String> = Vec::new();
        for index in 1..=count {
            let execution =
                start_command_execution(&router, &sandbox_id, &format!("echo {index}")).await;
            let id = text(&execution, "id");
            assert!(
                !ids_in_submit_order.contains(&id),
                "{sandbox}: submission {index} reused the id {id:?} \
                 (already had {ids_in_submit_order:?})"
            );
            ids_in_submit_order.push(id);
        }

        let indexed = engine
            .state::<CodeBuildState>()
            .lock()
            .command_executions_by_sandbox
            .get(&sandbox_id)
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            indexed, ids_in_submit_order,
            "{sandbox}: the store must keep the {count} executions in submit order, \
             got {indexed:?}"
        );

        // Asked for together, they come back in the order asked.
        let fetched =
            batch_get_command_executions(&router, &sandbox_id, &ids_in_submit_order).await;
        let reported: Vec<String> = fetched["commandExecutions"]
            .as_array()
            .expect("a commandExecutions list")
            .iter()
            .map(|execution| text(execution, "id"))
            .collect();
        assert_eq!(
            reported, ids_in_submit_order,
            "{sandbox}: expected {ids_in_submit_order:?}, got {reported:?}"
        );
    }
}

/// RULE: the terminal transition is owed by the CLOCK, so it is materialised from the
/// stored instants when the execution is read — once, and identically on every later
/// read. Nothing here runs a sandbox container, so the execution ends in CodeBuild's
/// build-system failure status with the reason as its error text, and with no exit
/// code and no output.
#[tokio::test]
async fn a_command_execution_settles_once_from_stored_state_and_never_changes_again() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(Arc::clone(&engine));
    let (sandbox_id, _) = seed_sandbox(
        &engine,
        "cmd-settle",
        "cmd-settle:0001",
        SeededSandbox::Running,
    );

    let submitted = start_command_execution(&router, &sandbox_id, "echo ok").await;
    let id = text(&submitted, "id");

    // Before the span elapses the caller still sees the transient state.
    let early = batch_get_command_executions(&router, &sandbox_id, std::slice::from_ref(&id)).await;
    assert_eq!(
        early["commandExecutions"][0]["status"],
        json!(state::COMMAND_EXECUTION_STATUS_IN_PROGRESS),
        "an execution read straight after submitting it is still in flight, got {early}"
    );

    tokio::time::sleep(PAST_THE_SETTLE_SPAN).await;

    let settled =
        batch_get_command_executions(&router, &sandbox_id, std::slice::from_ref(&id)).await;
    let record = settled["commandExecutions"][0].clone();
    assert_eq!(
        record["status"],
        json!(state::COMMAND_EXECUTION_STATUS_FAULT),
        "an execution no sandbox shell ran must end in FAULT, got {record}"
    );
    assert_eq!(
        record["standardErrContent"],
        json!(state::COMMAND_NOT_EXECUTED_MESSAGE),
        "the execution must say why it produced nothing, got {record}"
    );
    assert!(
        record["exitCode"].is_null() && record["standardOutputContent"].is_null(),
        "nothing exited and nothing wrote to stdout, got {record}"
    );
    assert!(
        record["endTime"].is_number() && record["endTime"] == record["startTime"],
        "the attempt was made and given up at one instant, so no run duration is \
         reported, got {record}"
    );
    assert_eq!(
        record["submitTime"], submitted["submitTime"],
        "settling must not re-date the submission, got {record}"
    );
    assert_eq!(
        record["command"], submitted["command"],
        "settling must not change the command submitted, got {record}"
    );

    // Read again: identical, because it was computed once from the stored instants.
    let reread = batch_get_command_executions(&router, &sandbox_id, &[id]).await;
    assert_eq!(
        reread["commandExecutions"][0], record,
        "a settled execution must never be re-fabricated per call, got {reread}"
    );
}

/// RULE: a sandbox the account does not hold is refused with
/// `ResourceNotFoundException`, a sandbox whose session has ENDED with
/// `InvalidInputException` stating the status it is in, and a refused submission persists
/// nothing. A sandbox still being provisioned is neither: it takes the submission, which
/// is `a_queued_sandbox_takes_a_command_submission`'s subject.
#[tokio::test]
async fn start_command_execution_refuses_a_sandbox_it_cannot_run_a_command_in() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(Arc::clone(&engine));
    let (held_id, _) = seed_sandbox(
        &engine,
        "cmd-refuse",
        "cmd-refuse:0001",
        SeededSandbox::Running,
    );
    let (ended_id, ended_arn) = seed_sandbox(
        &engine,
        "cmd-refuse",
        "cmd-refuse:ended",
        SeededSandbox::Ended,
    );

    let unknown_uuid = "cmd-refuse:00000000-0000-8000-8000-000000000000".to_string();
    // (case, the sandboxId asked for)
    let unknown: Vec<(&str, String)> = vec![
        ("an id nothing started", unknown_uuid.clone()),
        (
            "the ARN of an id nothing started",
            state::sandbox_arn(REGION, &unknown_uuid),
        ),
        (
            "a project name rather than a sandbox id",
            "cmd-refuse".to_string(),
        ),
        ("a held id in the wrong case", held_id.to_uppercase()),
        (
            "a prefix of a held id",
            held_id[..held_id.len() - 1].to_string(),
        ),
    ];
    for (case, sandbox_id) in unknown {
        let (status, body) = call(
            &router,
            "StartCommandExecution",
            json!({"sandboxId": sandbox_id, "command": "echo hello"}),
        )
        .await;
        assert_eq!(status, 400, "{case}: expected 400, got {status} with {body}");
        assert!(
            error_type(&body).contains("ResourceNotFoundException"),
            "{case}: expected ResourceNotFoundException, got {body}"
        );
    }

    // (case, the sandboxId asked for, the status the refusal must state)
    let not_live: Vec<(&str, String, &str)> = vec![
        (
            "a sandbox that has been stopped, by id",
            ended_id.clone(),
            state::SANDBOX_STATUS_STOPPED,
        ),
        (
            "a sandbox that has been stopped, by ARN",
            ended_arn,
            state::SANDBOX_STATUS_STOPPED,
        ),
    ];
    for (case, sandbox_id, status_named) in not_live {
        let (status, body) = call(
            &router,
            "StartCommandExecution",
            json!({"sandboxId": sandbox_id, "command": "echo hello"}),
        )
        .await;
        assert_eq!(status, 400, "{case}: expected 400, got {status} with {body}");
        assert!(
            error_type(&body).contains("InvalidInputException"),
            "{case}: expected InvalidInputException, got {body}"
        );
        // CodeBuild states the rule and the status the sandbox is in, and quotes
        // nothing of the request.
        assert_eq!(
            text(&body, "message"),
            format!("Command execution cannot be started if sandbox status is {status_named}"),
            "{case}: expected CodeBuild's sentence for a sandbox that cannot take a \
             command, got {body}"
        );
    }

    let handle = engine.state::<CodeBuildState>();
    let guard = handle.lock();
    assert!(
        guard.command_executions.is_empty() && guard.command_executions_by_sandbox.is_empty(),
        "a refused StartCommandExecution must persist nothing, got {:?}",
        guard.command_executions.keys().collect::<Vec<_>>()
    );
}

/// RULE: a command execution is submitted to a sandbox whose session has not ended, so
/// the whole of a sandbox's pre-terminal life takes one: the identifier may be the
/// sandbox's id or its ARN, the sandbox may still be being provisioned or already live,
/// and the submission is persisted and listed under that sandbox either way.
///
/// The rows differ along the two things that could decide the answer and must not — the
/// form of the identifier, and how far along the sandbox's own clock is.
#[tokio::test]
async fn a_queued_sandbox_takes_a_command_submission() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(Arc::clone(&engine));
    let (queued_id, queued_arn) = seed_sandbox(
        &engine,
        "cmd-queued",
        "cmd-queued:0001",
        SeededSandbox::Queued,
    );
    let (running_id, running_arn) = seed_sandbox(
        &engine,
        "cmd-queued",
        "cmd-queued:0002",
        SeededSandbox::Running,
    );

    // (case, the sandboxId asked for, the sandbox the execution must belong to, the
    // status that sandbox is in when the command is submitted)
    let accepted: Vec<(&str, String, String, &str)> = vec![
        (
            "a sandbox still being provisioned, by id",
            queued_id.clone(),
            queued_id.clone(),
            state::SANDBOX_STATUS_QUEUED,
        ),
        (
            "a sandbox still being provisioned, by ARN",
            queued_arn,
            queued_id.clone(),
            state::SANDBOX_STATUS_QUEUED,
        ),
        (
            "a live sandbox, by id",
            running_id.clone(),
            running_id.clone(),
            state::SANDBOX_STATUS_RUNNING,
        ),
        (
            "a live sandbox, by ARN",
            running_arn,
            running_id.clone(),
            state::SANDBOX_STATUS_RUNNING,
        ),
    ];

    for (case, asked_for, owner, status_named) in accepted {
        let (status, body) = call(
            &router,
            "StartCommandExecution",
            json!({"sandboxId": asked_for, "command": format!("echo {case}")}),
        )
        .await;
        assert_eq!(status, 200, "{case}: expected 200, got {status} with {body}");
        let execution = body["commandExecution"].clone();
        assert_eq!(
            execution["sandboxId"],
            json!(owner),
            "{case}: the execution must belong to the sandbox it was submitted to, got \
             {execution}"
        );
        // The sandbox it was submitted to is in the status this row is about: the
        // submission is what is being tested, not a sandbox that settled underneath it.
        let (_, sandboxes) = call(&router, "BatchGetSandboxes", json!({"ids": [owner.clone()]})).await;
        assert_eq!(
            sandboxes["sandboxes"][0]["status"],
            json!(status_named),
            "{case}: expected the sandbox to be {status_named}, got {sandboxes}"
        );
        // And it is listed under that sandbox, which is what a client polls.
        let (_, listed) = call(
            &router,
            "ListCommandExecutionsForSandbox",
            json!({"sandboxId": owner}),
        )
        .await;
        let id = text(&execution, "id");
        let reported: Vec<String> = listed["commandExecutions"]
            .as_array()
            .map(|executions| executions.iter().map(|one| text(one, "id")).collect())
            .unwrap_or_default();
        assert!(
            reported.contains(&id),
            "{case}: the submitted execution must be listed for its sandbox, got {listed}"
        );
    }
}

/// RULE: over the wire, a command execution can be started on a sandbox at any point
/// before its session has ended — while it is still being provisioned and once it is
/// live — and not after `StopSandbox` has ended it.
#[tokio::test]
async fn a_sandbox_started_over_the_wire_runs_commands_until_its_session_ends() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(Arc::clone(&engine));
    let sandbox = start_sandbox(&router, "cmd-wire").await;
    let sandbox_id = text(&sandbox, "id");

    // A sandbox that is still being provisioned takes the submission: the sandbox queues
    // the command and runs it in the session it is bringing up.
    let queued = start_command_execution(&router, &sandbox_id, "echo queued").await;
    assert_eq!(
        queued["sandboxId"], sandbox_id.as_str(),
        "a sandbox still in the queue: the execution belongs to it, got {queued}"
    );

    // Once its provisioning span has elapsed the session is live and takes commands too.
    tokio::time::sleep(PAST_THE_SANDBOX_SETTLE_SPAN).await;
    let execution = start_command_execution(&router, &sandbox_id, "echo live").await;
    let id = text(&execution, "id");
    assert_eq!(
        execution["sandboxId"], sandbox_id.as_str(),
        "the execution belongs to the sandbox it was submitted to, got {execution}"
    );
    let fetched =
        batch_get_command_executions(&router, &sandbox_id, std::slice::from_ref(&id)).await;
    assert_eq!(
        fetched["commandExecutions"][0]["id"],
        json!(id),
        "a live sandbox's execution must be readable back, got {fetched}"
    );

    let (status, body) = call(&router, "StopSandbox", json!({"id": sandbox_id})).await;
    assert_eq!(
        status, 200,
        "StopSandbox {sandbox_id:?}: expected 200, got {status} with {body}"
    );

    let (status, body) = call(
        &router,
        "StartCommandExecution",
        json!({"sandboxId": sandbox_id, "command": "echo too late"}),
    )
    .await;
    assert_eq!(
        status, 400,
        "a sandbox that has been stopped: expected 400, got {status} with {body}"
    );
    assert!(
        error_type(&body).contains("InvalidInputException"),
        "a sandbox that has been stopped: expected InvalidInputException, got {body}"
    );
    assert_eq!(
        text(&body, "message"),
        format!(
            "Command execution cannot be started if sandbox status is {}",
            state::SANDBOX_STATUS_STOPPED
        ),
        "a sandbox that has been stopped: the refusal must state its status, got {body}"
    );

    // The execution started while it was live is still there: stopping the sandbox
    // refuses NEW commands rather than discarding what it already ran.
    let after_stop =
        batch_get_command_executions(&router, &sandbox_id, std::slice::from_ref(&id)).await;
    assert_eq!(
        after_stop["commandExecutions"][0]["id"],
        json!(id),
        "the execution submitted while the sandbox was live must survive the stop, \
         got {after_stop}"
    );
}

/// One row of the read table: the case, the sandbox the read is scoped to, the ids
/// asked for, the ids expected back in order, and the ids expected under
/// `commandExecutionsNotFound`.
type ReadCase = (&'static str, String, Vec<String>, Vec<String>, Vec<String>);

/// RULE: `BatchGetCommandExecutions` resolves each entry of `commandExecutionIds`
/// against the sandbox `sandboxId` names, in the order asked, and reports every entry
/// that sandbox holds nothing for under `commandExecutionsNotFound` as the caller
/// wrote it — so an execution of ANOTHER sandbox is not found under this one.
#[tokio::test]
async fn batch_get_command_executions_resolves_ids_within_their_sandbox() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(Arc::clone(&engine));
    let (first_sandbox, first_sandbox_arn) = seed_sandbox(
        &engine,
        "cmd-read",
        "cmd-read:0001",
        SeededSandbox::Running,
    );
    let (second_sandbox, _) = seed_sandbox(
        &engine,
        "cmd-read",
        "cmd-read:0002",
        SeededSandbox::Running,
    );

    let first = text(
        &start_command_execution(&router, &first_sandbox, "echo one").await,
        "id",
    );
    let second = text(
        &start_command_execution(&router, &first_sandbox, "echo two").await,
        "id",
    );
    let elsewhere = text(
        &start_command_execution(&router, &second_sandbox, "echo elsewhere").await,
        "id",
    );
    let unknown = "00000000-0000-8000-8000-000000000000".to_string();

    let cases: Vec<ReadCase> = vec![
        (
            "one id",
            first_sandbox.clone(),
            vec![first.clone()],
            vec![first.clone()],
            vec![],
        ),
        (
            "the sandbox named by its ARN",
            first_sandbox_arn.clone(),
            vec![first.clone()],
            vec![first.clone()],
            vec![],
        ),
        (
            "submit order",
            first_sandbox.clone(),
            vec![first.clone(), second.clone()],
            vec![first.clone(), second.clone()],
            vec![],
        ),
        (
            "reversed",
            first_sandbox.clone(),
            vec![second.clone(), first.clone()],
            vec![second.clone(), first.clone()],
            vec![],
        ),
        (
            "the same id twice",
            first_sandbox.clone(),
            vec![first.clone(), first.clone()],
            vec![first.clone(), first.clone()],
            vec![],
        ),
        (
            "a known and an unknown id",
            first_sandbox.clone(),
            vec![first.clone(), unknown.clone()],
            vec![first.clone()],
            vec![unknown.clone()],
        ),
        (
            "only unknown ids",
            first_sandbox.clone(),
            vec![unknown.clone()],
            vec![],
            vec![unknown.clone()],
        ),
        // An execution is addressed WITHIN its sandbox, so another sandbox's id does
        // not resolve under this one.
        (
            "an execution of another sandbox",
            first_sandbox.clone(),
            vec![elsewhere.clone(), first.clone()],
            vec![first.clone()],
            vec![elsewhere.clone()],
        ),
        (
            "every id under a sandbox nothing started",
            "cmd-read:nothing".to_string(),
            vec![first.clone(), elsewhere.clone()],
            vec![],
            vec![first.clone(), elsewhere.clone()],
        ),
    ];

    for (case, sandbox_id, asked, expected_found, expected_missing) in cases {
        let body = batch_get_command_executions(&router, &sandbox_id, &asked).await;
        let reported: Vec<String> = body["commandExecutions"]
            .as_array()
            .expect("a commandExecutions list")
            .iter()
            .map(|execution| text(execution, "id"))
            .collect();
        assert_eq!(
            reported, expected_found,
            "{case}: expected {expected_found:?}, got {reported:?}"
        );
        assert_eq!(
            body["commandExecutionsNotFound"],
            json!(expected_missing),
            "{case}: expected {expected_missing:?} missing, got {body}"
        );
    }
}

/// RULE: the command-execution store is bounded at its growth site —
/// `StartCommandExecution` mints a record per call, so one past the ceiling evicts the
/// oldest execution that has already reached a terminal status, pruning the
/// per-sandbox index with it.
#[tokio::test]
async fn the_command_execution_store_is_bounded_at_its_growth_site() {
    let ceiling = i64::try_from(state::MAX_COMMAND_EXECUTIONS).expect("the ceiling fits in i64");
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let handle = engine.state::<CodeBuildState>();
    {
        let mut guard = handle.lock();
        for index in 0..=ceiling {
            let id = format!("bounded-{index:08}");
            guard.insert_command_execution(
                id.clone(),
                StoredCommandExecution {
                    command_execution: CommandExecution {
                        id: Some(id),
                        sandbox_id: Some("bounded:0001".to_string()),
                        // Terminal: `endTime` is set, so it is evictable.
                        end_time: Some(state::datetime(index)),
                        ..CommandExecution::default()
                    },
                    submit_millis: index,
                    settle_due_millis: index,
                },
            );
        }
    }

    let guard = handle.lock();
    assert_eq!(
        guard.command_executions.len(),
        state::MAX_COMMAND_EXECUTIONS,
        "one execution past the ceiling must evict one, got {}",
        guard.command_executions.len()
    );
    assert!(
        !guard.command_executions.contains_key("bounded-00000000"),
        "the oldest terminal execution is the one evicted, got {:?}",
        guard.command_executions.keys().next()
    );
    let indexed = guard
        .command_executions_by_sandbox
        .get("bounded:0001")
        .map_or(0, Vec::len);
    assert_eq!(
        indexed,
        state::MAX_COMMAND_EXECUTIONS,
        "eviction must prune the per-sandbox index too, got {indexed}"
    );
}

/// RULE: `ListCommandExecutionsForSandbox` reports the executions recorded against the
/// sandbox the request names — by sandbox id or sandbox ARN — in the order
/// `sortOrder` asks for (newest first unless `ASCENDING` is asked for, the default the
/// sibling `ListBuildsForProject$sortOrder` documents), a page at a time, and an
/// execution of another sandbox is never reported under this one.
#[tokio::test]
async fn list_command_executions_for_sandbox_pages_the_sandboxs_own_executions() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(Arc::clone(&engine));
    let (sandbox, sandbox_arn) =
        seed_sandbox(&engine, "cmd-list", "cmd-list:0001", SeededSandbox::Running);
    let (other, _) = seed_sandbox(&engine, "cmd-list", "cmd-list:0002", SeededSandbox::Running);

    // Submitted oldest first, which is the order the per-sandbox index keeps.
    let mut submitted = Vec::new();
    for command in ["echo one", "echo two", "echo three"] {
        submitted.push(text(
            &start_command_execution(&router, &sandbox, command).await,
            "id",
        ));
    }
    let elsewhere = text(
        &start_command_execution(&router, &other, "echo elsewhere").await,
        "id",
    );
    let newest_first: Vec<String> = submitted.iter().rev().cloned().collect();

    // (case, the request, the ids expected back in order)
    let cases: Vec<(&str, Value, Vec<String>)> = vec![
        (
            "no sortOrder: newest first",
            json!({"sandboxId": sandbox}),
            newest_first.clone(),
        ),
        (
            "ASCENDING: oldest first",
            json!({"sandboxId": sandbox, "sortOrder": "ASCENDING"}),
            submitted.clone(),
        ),
        (
            "DESCENDING: newest first",
            json!({"sandboxId": sandbox, "sortOrder": "DESCENDING"}),
            newest_first.clone(),
        ),
        (
            "the sandbox named by its ARN",
            json!({"sandboxId": sandbox_arn, "sortOrder": "ASCENDING"}),
            submitted.clone(),
        ),
        (
            "one per page",
            json!({"sandboxId": sandbox, "sortOrder": "ASCENDING", "maxResults": 1}),
            submitted[..1].to_vec(),
        ),
        (
            "two per page",
            json!({"sandboxId": sandbox, "sortOrder": "ASCENDING", "maxResults": 2}),
            submitted[..2].to_vec(),
        ),
        (
            "the other sandbox reports only its own",
            json!({"sandboxId": other}),
            vec![elsewhere.clone()],
        ),
        (
            "a sandbox nothing was submitted to",
            json!({"sandboxId": "cmd-list:nothing"}),
            vec![],
        ),
    ];

    for (case, request, expected) in cases {
        let (status, body) = call(&router, "ListCommandExecutionsForSandbox", request).await;
        assert_eq!(status, 200, "{case}: expected 200, got {status} with {body}");
        let reported: Vec<String> = body["commandExecutions"]
            .as_array()
            .expect("a commandExecutions list")
            .iter()
            .map(|execution| text(execution, "id"))
            .collect();
        assert_eq!(
            reported, expected,
            "{case}: expected {expected:?}, got {reported:?}"
        );
    }

    // The page token walks the rest of the listing and stops: every execution is
    // reported exactly once, in order, and the last page carries no token.
    let mut walked = Vec::new();
    let mut token = Value::Null;
    loop {
        let mut request = json!({
            "sandboxId": sandbox,
            "sortOrder": "ASCENDING",
            "maxResults": 2,
        });
        if let (Value::Object(members), Value::String(_)) = (&mut request, &token) {
            members.insert("nextToken".to_string(), token.clone());
        }
        let (status, body) = call(&router, "ListCommandExecutionsForSandbox", request).await;
        assert_eq!(
            status, 200,
            "paging: expected 200, got {status} with {body}"
        );
        for execution in body["commandExecutions"]
            .as_array()
            .expect("a commandExecutions list")
        {
            walked.push(text(execution, "id"));
        }
        token = body["nextToken"].clone();
        if !token.is_string() {
            break;
        }
    }
    assert_eq!(
        walked, submitted,
        "paging must walk every execution once, in order, got {walked:?}"
    );
}
