// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the LIFE of a CodeBuild sandbox session, driven over the wire
//! the router serves: `POST /` with `X-Amz-Target: CodeBuild_20161006.<Operation>` on
//! awsJson1_1.
//!
//! The rules under test:
//!
//! * a sandbox that leaves the queue is provisioned and becomes a live session — it
//!   reports `RUNNING` with `currentSession.currentPhase` `RUN_SANDBOX` and STAYS there
//!   for as long as its own `timeoutInMinutes` has not elapsed;
//! * once that documented wait ("How long, in minutes, from 5 to 2160 (36 hours), for
//!   CodeBuild to wait before timing out this sandbox if it does not get marked as
//!   completed") has elapsed, the sandbox is `TIMED_OUT` and has ended;
//! * `StopSandbox` ends a sandbox that has not ended on its own, from whichever phase
//!   it is in, and leaves one that already ended as it was.
//!
//! Each suite in `tests/all/` drives the wire itself, so the harness below is this
//! suite's own.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_codebuild_codebuild_20161006::ops::types::Sandbox;
use service_com_amazonaws_codebuild_codebuild_20161006::state::{
    self, ComAmazonawsCodebuildCodebuild20161006State as CodeBuildState, StoredSandbox,
};
use service_com_amazonaws_codebuild_codebuild_20161006::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below is signed for.
const REGION: &str = "us-west-2";

/// The access key id every request below is signed with.
const ACCESS_KEY_ID: &str = "AKIDCODEBUILDSESSION";

/// Long enough for a sandbox's clock-owed move out of `QUEUED` to have fallen due.
const PAST_THE_SETTLE_SPAN: Duration =
    Duration::from_millis(state::SANDBOX_SETTLE_MILLIS as u64 + 400);

/// Milliseconds in a minute — a `timeoutInMinutes` is measured in minutes and the
/// store in epoch millis.
const MILLIS_PER_MINUTE: i64 = 60_000;

/// One router carrying only this service, over the engine the caller holds.
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

/// Create a minimal NO_SOURCE project, with `extra`'s members merged in.
async fn create_project(router: &MultiServiceRouter, name: &str, extra: Value) {
    let mut body = json!({
        "name": name,
        "source": {"type": "NO_SOURCE", "buildspec": "version: 0.2"},
        "artifacts": {"type": "NO_ARTIFACTS"},
        "environment": {
            "type": "LINUX_CONTAINER",
            "image": "aws/codebuild/standard:7.0",
            "computeType": "BUILD_GENERAL1_SMALL",
        },
        "serviceRole": "arn:aws:iam::000000000000:role/service-role/codebuild",
    });
    if let (Value::Object(target), Value::Object(members)) = (&mut body, extra) {
        target.extend(members);
    }
    let (status, response) = call(router, "CreateProject", body).await;
    assert_eq!(
        status, 200,
        "CreateProject {name:?}: expected 200, got {status} with {response}"
    );
}

/// Start a sandbox of `project`, asserting it succeeded, and hand back its id.
async fn start_sandbox(router: &MultiServiceRouter, project: &str) -> String {
    let (status, response) = call(router, "StartSandbox", json!({"projectName": project})).await;
    assert_eq!(
        status, 200,
        "StartSandbox {project:?}: expected 200, got {status} with {response}"
    );
    text(&response["sandbox"], "id")
}

/// Read one sandbox back, asserting it is reported.
async fn read_sandbox(router: &MultiServiceRouter, id: &str) -> Value {
    let (status, body) = call(router, "BatchGetSandboxes", json!({"ids": [id]})).await;
    assert_eq!(
        status, 200,
        "BatchGetSandboxes {id:?}: expected 200, got {status} with {body}"
    );
    assert!(
        body["sandboxes"][0].is_object(),
        "BatchGetSandboxes {id:?}: expected the sandbox back, got {body}"
    );
    body["sandboxes"][0].clone()
}

/// The `__type` an error body carries, for asserting the error code a client branches on.
fn error_type(body: &Value) -> String {
    body["__type"].as_str().unwrap_or_default().to_string()
}

/// A string member, for an assertion message that shows what was actually there.
fn text(value: &Value, member: &str) -> String {
    value[member].as_str().unwrap_or_default().to_string()
}

/// The (phaseType, phaseStatus) pairs a session reports, in order.
fn phase_pairs(session: &Value) -> Vec<(String, String)> {
    session["phases"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|phase| (text(phase, "phaseType"), text(phase, "phaseStatus")))
        .collect()
}

/// Persist a sandbox whose session went live `age_minutes` ago, carrying
/// `timeout_minutes` — the way `StartSandbox` persists one, but with the instants
/// placed in the past so a wait AWS measures in minutes is observable in a test.
fn seed_live_sandbox(
    engine: &engine::InMemoryEngine,
    project: &str,
    id: &str,
    timeout_minutes: Option<i32>,
    age_minutes: i64,
) {
    let started = state::now_millis() - age_minutes * MILLIS_PER_MINUTE;
    let requested = started - state::SANDBOX_SETTLE_MILLIS;
    engine.state::<CodeBuildState>().lock().insert_sandbox(
        id.to_string(),
        StoredSandbox {
            sandbox: Sandbox {
                id: Some(id.to_string()),
                project_name: Some(project.to_string()),
                request_time: Some(state::datetime(requested)),
                status: Some("QUEUED".to_string()),
                timeout_in_minutes: timeout_minutes,
                current_session: Some(state::queued_sandbox_session(
                    format!("{id}-session"),
                    requested,
                )),
                ..Sandbox::default()
            },
            request_millis: requested,
            settle_due_millis: started,
        },
    );
}

/// RULE: a sandbox is a live session, not a job with an outcome — it reports `RUNNING`
/// in the `RUN_SANDBOX` phase from the instant it is provisioned until the
/// `timeoutInMinutes` it carries has elapsed, and only then is it `TIMED_OUT` and
/// ended.
///
/// The table spans that documented wait: the published bounds of
/// `Sandbox$timeoutInMinutes` ("from 5 to 2160 (36 hours)"), a value between them, and
/// the project default a sandbox carries when the member was never set — each just
/// inside the wait, exactly at it, and past it.
#[tokio::test]
async fn a_live_sandbox_runs_until_its_documented_timeout_elapses() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(Arc::clone(&engine));
    create_project(&router, "timeout-proj", json!({})).await;

    // (case, the sandbox's timeoutInMinutes, minutes since it went live, expected status)
    let cases: Vec<(&str, Option<i32>, i64, &str)> = vec![
        ("the documented minimum, a minute in", Some(5), 1, "RUNNING"),
        ("the documented minimum, at the wait", Some(5), 5, "TIMED_OUT"),
        (
            "the documented minimum, a minute past it",
            Some(5),
            6,
            "TIMED_OUT",
        ),
        ("a value between the bounds, inside", Some(480), 479, "RUNNING"),
        (
            "a value between the bounds, past it",
            Some(480),
            481,
            "TIMED_OUT",
        ),
        (
            "the documented maximum, a minute short of 36 hours",
            Some(2160),
            2159,
            "RUNNING",
        ),
        (
            "the documented maximum, at 36 hours",
            Some(2160),
            2160,
            "TIMED_OUT",
        ),
        (
            "the documented maximum, a day past it",
            Some(2160),
            2160 + 24 * 60,
            "TIMED_OUT",
        ),
        // No member at all: the record falls back to the 60 minutes AWS documents as
        // the project build-timeout default, which `CreateProject` stores.
        ("no timeout member, inside the default", None, 59, "RUNNING"),
        ("no timeout member, past the default", None, 61, "TIMED_OUT"),
    ];

    for (index, (case, timeout_minutes, age_minutes, expected)) in cases.iter().enumerate() {
        let id = format!("timeout-proj:case-{index:02}");
        seed_live_sandbox(
            &engine,
            "timeout-proj",
            &id,
            *timeout_minutes,
            *age_minutes,
        );

        let record = read_sandbox(&router, &id).await;
        let actual = text(&record, "status");
        assert_eq!(
            &actual, expected,
            "case {case:?}: expected status {expected}, got {actual:?} in {record}"
        );

        let session = record["currentSession"].clone();
        assert_eq!(
            text(&session, "status"),
            *expected,
            "case {case:?}: the session must report the sandbox's status, got {session}"
        );

        if *expected == "RUNNING" {
            assert_eq!(
                text(&session, "currentPhase"),
                "RUN_SANDBOX",
                "case {case:?}: a live session sits in RUN_SANDBOX, got {session}"
            );
            assert!(
                record["endTime"].is_null(),
                "case {case:?}: a live sandbox has not ended, got {record}"
            );
            assert!(
                record["startTime"].is_number(),
                "case {case:?}: a live sandbox reports when it started, got {record}"
            );
            assert_eq!(
                phase_pairs(&session),
                vec![
                    ("SUBMITTED".to_string(), "SUCCEEDED".to_string()),
                    ("QUEUED".to_string(), "SUCCEEDED".to_string()),
                    ("PROVISIONING".to_string(), "SUCCEEDED".to_string()),
                    ("RUN_SANDBOX".to_string(), String::new()),
                ],
                "case {case:?}: provisioning must have succeeded and the run be open, \
                 got {session}"
            );
        } else {
            assert_eq!(
                text(&session, "currentPhase"),
                "COMPLETED",
                "case {case:?}: an ended session sits in COMPLETED, got {session}"
            );
            let started = record["startTime"].as_f64().unwrap_or_default();
            let ended = record["endTime"].as_f64().unwrap_or_default();
            // awsJson timestamps are epoch SECONDS, so the wait is seconds / 60.
            let waited_minutes = ((ended - started) / 60.0).round() as i64;
            let documented = i64::from(timeout_minutes.unwrap_or(60));
            assert_eq!(
                waited_minutes, documented,
                "case {case:?}: must time out exactly {documented} minutes after it \
                 went live, got {waited_minutes} in {record}"
            );
            assert_eq!(
                phase_pairs(&session),
                vec![
                    ("SUBMITTED".to_string(), "SUCCEEDED".to_string()),
                    ("QUEUED".to_string(), "SUCCEEDED".to_string()),
                    ("PROVISIONING".to_string(), "SUCCEEDED".to_string()),
                    ("RUN_SANDBOX".to_string(), "TIMED_OUT".to_string()),
                    ("COMPLETED".to_string(), String::new()),
                ],
                "case {case:?}: the run phase must carry the timeout, got {session}"
            );
        }

        // Computed from the stored instants, so a second read reports the same record.
        let reread = read_sandbox(&router, &id).await;
        assert_eq!(
            reread, record,
            "case {case:?}: a sandbox must not be re-fabricated per call, got {reread}"
        );
    }
}

/// RULE: `StopSandbox` ends a sandbox that has not ended on its own — whichever phase
/// it was in takes the `STOPPED` status, the record reports when it ended, and the
/// outcome persists; a sandbox that already ended keeps the outcome it reached, and an
/// id no sandbox answers to is refused with `ResourceNotFoundException` (HTTP 400).
#[tokio::test]
async fn stop_sandbox_ends_a_sandbox_from_whichever_phase_it_is_in() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(Arc::clone(&engine));
    create_project(&router, "stop-proj", json!({})).await;

    // Stopped while still queued: the queue is the phase that takes the status.
    let queued = start_sandbox(&router, "stop-proj").await;
    let (status, body) = call(&router, "StopSandbox", json!({"id": queued})).await;
    assert_eq!(
        status, 200,
        "stopping a queued sandbox: expected 200, got {status} with {body}"
    );
    let stopped_from_the_queue = body["sandbox"].clone();
    assert_eq!(
        text(&stopped_from_the_queue, "status"),
        "STOPPED",
        "stopping a queued sandbox: expected STOPPED, got {stopped_from_the_queue}"
    );
    assert_eq!(
        phase_pairs(&stopped_from_the_queue["currentSession"]),
        vec![
            ("SUBMITTED".to_string(), "SUCCEEDED".to_string()),
            ("QUEUED".to_string(), "STOPPED".to_string()),
            ("COMPLETED".to_string(), String::new()),
        ],
        "stopping a queued sandbox: the queue must take the status, got \
         {stopped_from_the_queue}"
    );

    // Stopped while live: the run phase is the one that takes the status.
    let live = start_sandbox(&router, "stop-proj").await;
    tokio::time::sleep(PAST_THE_SETTLE_SPAN).await;
    let running = read_sandbox(&router, &live).await;
    assert_eq!(
        text(&running, "status"),
        "RUNNING",
        "a sandbox past its provisioning span must be live before it is stopped, \
         got {running}"
    );

    let (status, body) = call(&router, "StopSandbox", json!({"id": live.clone()})).await;
    assert_eq!(
        status, 200,
        "stopping a live sandbox: expected 200, got {status} with {body}"
    );
    let stopped = body["sandbox"].clone();
    assert_eq!(
        text(&stopped, "status"),
        "STOPPED",
        "stopping a live sandbox: expected STOPPED, got {stopped}"
    );
    assert!(
        stopped["endTime"].is_number(),
        "stopping a live sandbox: it must report when it ended, got {stopped}"
    );
    assert_eq!(
        phase_pairs(&stopped["currentSession"]),
        vec![
            ("SUBMITTED".to_string(), "SUCCEEDED".to_string()),
            ("QUEUED".to_string(), "SUCCEEDED".to_string()),
            ("PROVISIONING".to_string(), "SUCCEEDED".to_string()),
            ("RUN_SANDBOX".to_string(), "STOPPED".to_string()),
            ("COMPLETED".to_string(), String::new()),
        ],
        "stopping a live sandbox: the run phase must take the status, got {stopped}"
    );

    // Persisted: the read path reports the stop, and the clock never revives it.
    let fetched = read_sandbox(&router, &live).await;
    assert_eq!(
        fetched, stopped,
        "a stopped sandbox must be the record the stop persisted, got {fetched}"
    );

    // Stopping it again changes nothing: it has already ended.
    let (status, body) = call(&router, "StopSandbox", json!({"id": live})).await;
    assert_eq!(
        status, 200,
        "stopping it twice: expected 200, got {status} with {body}"
    );
    assert_eq!(
        body["sandbox"], stopped,
        "stopping it twice must report the same record, got {body}"
    );

    // A sandbox the clock already timed out keeps that outcome.
    seed_live_sandbox(&engine, "stop-proj", "stop-proj:timed-out", Some(5), 9);
    let (status, body) = call(
        &router,
        "StopSandbox",
        json!({"id": "stop-proj:timed-out"}),
    )
    .await;
    assert_eq!(
        status, 200,
        "stopping a timed-out sandbox: expected 200, got {status} with {body}"
    );
    assert_eq!(
        text(&body["sandbox"], "status"),
        "TIMED_OUT",
        "a sandbox the clock already ended keeps its outcome, got {body}"
    );

    // (case, the id asked for)
    for (case, id) in [
        ("an id never minted", "stop-proj:00000000-0000-8000-8000-000000000000"),
        ("an id of another project", "other-proj:1"),
        ("the project name alone", "stop-proj"),
    ] {
        let (status, body) = call(&router, "StopSandbox", json!({"id": id})).await;
        assert_eq!(
            status, 400,
            "{case}: expected 400, got {status} with {body}"
        );
        assert!(
            error_type(&body).contains("ResourceNotFoundException"),
            "{case}: expected ResourceNotFoundException, got {body}"
        );
        assert!(
            text(&body, "message").contains(id),
            "{case}: the refusal must name the id asked for, got {body}"
        );
    }
}
