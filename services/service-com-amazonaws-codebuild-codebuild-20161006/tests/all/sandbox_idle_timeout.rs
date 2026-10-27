// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for CodeBuild reaping an IDLE sandbox, driven over the wire the
//! router serves: `POST /` with `X-Amz-Target: CodeBuild_20161006.<Operation>` on
//! awsJson1_1, against the wall clock and with nothing reaching into the store.
//!
//! The rule under test: a provisioned sandbox nobody touches is reaped by the clock at
//! the end of the wait its project configured (`Sandbox$timeoutInMinutes`, "How long, in
//! minutes, from 5 to 2160 (36 hours), for CodeBuild to wait before timing out this
//! sandbox if it does not get marked as completed"), with no `StopSandbox` call, and
//! ends in the SAME terminal status an explicit `StopSandbox` gives it — so a client
//! that provisions a sandbox and leaves CodeBuild to reap it can watch that happen
//! through an independent `BatchGetSandboxes` read, and a sandbox whose configured wait
//! has not run out is still live at the same moment.
//!
//! Each suite in `tests/all/` drives the wire itself, so the harness below is this
//! suite's own.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_codebuild_codebuild_20161006::state;
use service_com_amazonaws_codebuild_codebuild_20161006::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below is signed for.
const REGION: &str = "us-west-2";

/// The access key id every request below is signed with.
const ACCESS_KEY_ID: &str = "AKIDCODEBUILDREAP";

/// The documented floor of `Sandbox$timeoutInMinutes` ("from 5 to 2160 (36 hours)"),
/// which is the shortest wait a project can configure for a sandbox.
const SHORTEST_DOCUMENTED_WAIT_MINUTES: i32 = 5;

/// How long a sandbox configured with [`SHORTEST_DOCUMENTED_WAIT_MINUTES`] stays live:
/// that wait on the compressed clock this emulator reaps a sandbox on, clamped to the
/// bounds that clock publishes.
fn shortest_live_span() -> Duration {
    let millis = (i64::from(SHORTEST_DOCUMENTED_WAIT_MINUTES) * 60_000
        / state::SANDBOX_IDLE_CLOCK_SCALE)
        .clamp(
            state::MIN_SANDBOX_IDLE_SPAN_MILLIS,
            state::MAX_SANDBOX_IDLE_SPAN_MILLIS,
        );
    Duration::from_millis(millis as u64)
}

/// How long the poll below gives the clock to reap the sandbox: its live span, the
/// provisioning span that precedes it and slack for a loaded machine.
fn reap_budget() -> Duration {
    shortest_live_span()
        + Duration::from_millis(state::SANDBOX_SETTLE_MILLIS as u64)
        + Duration::from_secs(10)
}

/// How often the polls below read the sandbox back.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// One router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::new(engine::InMemoryEngine::new())));
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
            "image": "aws/codebuild/amazonlinux2-x86_64-standard:5.0",
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

/// Start a sandbox of `project`, asserting it succeeded, and hand back the record.
async fn start_sandbox(router: &MultiServiceRouter, project: &str) -> Value {
    let (status, response) = call(router, "StartSandbox", json!({"projectName": project})).await;
    assert_eq!(
        status, 200,
        "StartSandbox {project:?}: expected 200, got {status} with {response}"
    );
    response["sandbox"].clone()
}

/// Read one sandbox back through `BatchGetSandboxes`, asserting it is reported.
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

/// A string member, for an assertion message that shows what was actually there.
fn text(value: &Value, member: &str) -> String {
    value[member].as_str().unwrap_or_default().to_string()
}

/// Poll `id` until its status is no longer `want`, giving up after `budget`. Returns
/// the record read last and what its status was.
async fn poll_until_status_leaves(
    router: &MultiServiceRouter,
    id: &str,
    want: &str,
    budget: Duration,
) -> (String, Value) {
    let deadline = Instant::now() + budget;
    loop {
        let record = read_sandbox(router, id).await;
        let status = text(&record, "status");
        if status != want || Instant::now() >= deadline {
            return (status, record);
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// RULE: a sandbox nobody touches is reaped by the clock at the end of the wait its
/// project configured, with no `StopSandbox` call, and ends in the same terminal status
/// an explicit stop gives it — observable through an independent `BatchGetSandboxes`
/// read, which is also where the transient `QUEUED` and live `RUNNING` statuses are
/// observable before it.
///
/// Two projects, read in the same window, span what the wait is taken from: one
/// configures the documented floor of `Sandbox$timeoutInMinutes` and is reaped inside
/// the window; one configures no wait at all, so it falls back to the project
/// build-timeout default AWS documents (60 minutes) and is still live when the first
/// has been reaped. Nothing here reaches into the store: every status comes off the
/// wire.
#[tokio::test]
async fn an_idle_sandbox_is_reaped_stopped_at_the_end_of_the_wait_its_project_configured() {
    let router = emulator();
    create_project(
        &router,
        "reap-soonest",
        json!({"timeoutInMinutes": SHORTEST_DOCUMENTED_WAIT_MINUTES}),
    )
    .await;
    create_project(&router, "reap-default", json!({})).await;

    let reaped_soon = start_sandbox(&router, "reap-soonest").await;
    let still_live = start_sandbox(&router, "reap-default").await;
    let reaped_id = text(&reaped_soon, "id");
    let live_id = text(&still_live, "id");

    // The two rows this test reads in one window.
    let case = "the documented floor of the configured wait";
    let longer_case = "the documented project default";

    // The sandbox carries the wait its project configured, so the reap below is keyed
    // to a configured value and not to a span every sandbox shares.
    assert_eq!(
        reaped_soon["timeoutInMinutes"], SHORTEST_DOCUMENTED_WAIT_MINUTES,
        "StartSandbox must report the wait its project configured, got {reaped_soon}"
    );
    assert_eq!(
        still_live["timeoutInMinutes"],
        state::DEFAULT_BUILD_TIMEOUT_MINUTES,
        "a project that configured no wait gives its sandbox the documented default, \
         got {still_live}"
    );

    // Provisioning first: the sandbox leaves the queue and its session goes live, which
    // a caller can SEE before anything reaps it.
    let (queued_then, record) = poll_until_status_leaves(
        &router,
        &reaped_id,
        "QUEUED",
        Duration::from_millis(state::SANDBOX_SETTLE_MILLIS as u64) + Duration::from_secs(10),
    )
    .await;
    assert_eq!(
        queued_then, "RUNNING",
        "case {case:?}: a sandbox out of the queue is a live session, got {record}"
    );
    assert!(
        record["endTime"].is_null(),
        "case {case:?}: a live sandbox has not ended, got {record}"
    );

    // Then the reap, with no StopSandbox and no StartCommandExecution in between.
    let (terminal_status, terminal) =
        poll_until_status_leaves(&router, &reaped_id, "RUNNING", reap_budget()).await;
    assert_eq!(
        terminal_status, "STOPPED",
        "case {case:?}: expected the idle sandbox to be reaped STOPPED, got \
         {terminal_status:?} in {terminal}"
    );
    assert!(
        terminal["endTime"].is_number(),
        "case {case:?}: a reaped sandbox reports when it ended, got {terminal}"
    );
    let session = terminal["currentSession"].clone();
    assert_eq!(
        text(&session, "status"),
        "STOPPED",
        "case {case:?}: the session must report the status the reap gave it, got {session}"
    );
    assert_eq!(
        text(&session, "currentPhase"),
        "COMPLETED",
        "case {case:?}: a reaped session sits in COMPLETED, got {session}"
    );

    // The longer wait is NOT over in the same window: the reap follows the configured
    // value rather than firing for every sandbox alike.
    let live = read_sandbox(&router, &live_id).await;
    assert_eq!(
        text(&live, "status"),
        "RUNNING",
        "case {longer_case:?}: a sandbox whose configured wait has not run out is still \
         live when the shortest-wait one has been reaped, got {live}"
    );

    // The clock's reap is the outcome an explicit stop produces: a sibling sandbox of
    // the same project, stopped by the caller, reports the same terminal status.
    let stopped_by_hand = start_sandbox(&router, "reap-soonest").await;
    let (status, body) = call(
        &router,
        "StopSandbox",
        json!({"id": text(&stopped_by_hand, "id")}),
    )
    .await;
    assert_eq!(
        status, 200,
        "StopSandbox: expected 200, got {status} with {body}"
    );
    assert_eq!(
        text(&body["sandbox"], "status"),
        terminal_status,
        "case {:?}: an explicitly stopped sandbox must report the status the reap gave \
         the idle one ({terminal_status}), got {body}",
        "the control: stopped by the caller"
    );
}
