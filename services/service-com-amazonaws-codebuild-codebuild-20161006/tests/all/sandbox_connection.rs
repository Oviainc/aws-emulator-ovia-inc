// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `StartSandboxConnection`, driven over the wire the router
//! serves: `POST /` with `X-Amz-Target: CodeBuild_20161006.<Operation>` on awsJson1_1.
//!
//! The rules under test:
//!
//! * a connection is a session INTO a sandbox's live session, so every call against a
//!   live sandbox starts its OWN Session Manager session — a session id no other
//!   connection carries, its own opaque token, and the regional data-channel URL that
//!   session id belongs to — and that session is persisted rather than fabricated per
//!   call;
//! * the token is in force only while the sandbox session it connects to is: it stops
//!   being in force at the end of the wait `Sandbox$timeoutInMinutes` documents;
//! * a sandbox whose session is not in its run phase has nothing to connect to, and is
//!   refused naming the phase its own session is sitting in;
//! * an id (or ARN) no sandbox of the account answers to is refused as invalid input.
//!
//! Each suite in `tests/all/` drives the wire itself, so the harness below is this
//! suite's own.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_codebuild_codebuild_20161006::ops::types::Sandbox;
use service_com_amazonaws_codebuild_codebuild_20161006::state::{
    self, ComAmazonawsCodebuildCodebuild20161006State as CodeBuildState, StoredSandbox,
    StoredSandboxConnection,
};
use service_com_amazonaws_codebuild_codebuild_20161006::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region a request below is signed for unless the case names another.
const REGION: &str = "us-west-2";

/// The access key id every request below is signed with.
const ACCESS_KEY_ID: &str = "AKIDCODEBUILDCONNECT";

/// Long enough for a sandbox's clock-owed move out of `QUEUED` to have fallen due.
const PAST_THE_SETTLE_SPAN: Duration =
    Duration::from_millis(state::SANDBOX_SETTLE_MILLIS as u64 + 400);

/// Milliseconds in a minute — a `timeoutInMinutes` is measured in minutes and the store
/// in epoch millis.
const MILLIS_PER_MINUTE: i64 = 60_000;

/// How long a sandbox carrying `timeout_minutes` stays live: the wait AWS documents in
/// minutes, on the compressed clock this emulator reaps a sandbox on and inside the
/// bounds that clock publishes. A record carrying no member falls back to the project
/// build-timeout default AWS documents.
fn live_span_millis(timeout_minutes: Option<i32>) -> i64 {
    let minutes = i64::from(timeout_minutes.unwrap_or(state::DEFAULT_BUILD_TIMEOUT_MINUTES));
    (minutes * MILLIS_PER_MINUTE / state::SANDBOX_IDLE_CLOCK_SCALE).clamp(
        state::MIN_SANDBOX_IDLE_SPAN_MILLIS,
        state::MAX_SANDBOX_IDLE_SPAN_MILLIS,
    )
}

/// The shortest a Session Manager token may read and still be the opaque, encrypted
/// value `SSMSession$tokenValue` documents ("An encrypted token value containing
/// session and caller information") rather than an identifier.
const SHORTEST_OPAQUE_TOKEN: usize = 128;

/// One router carrying only this service, over the engine the caller holds.
fn emulator_on(engine: Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// Call one operation, signed for `region`, and return its HTTP status and decoded body.
async fn call_in(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("CodeBuild_20161006.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential={ACCESS_KEY_ID}/20260101/{region}/codebuild/\
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

/// Call one operation signed for [`REGION`].
async fn call(router: &MultiServiceRouter, operation: &str, body: Value) -> (u16, Value) {
    call_in(router, REGION, operation, body).await
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

/// Start a sandbox of `project`, asserting it succeeded, and hand back the record.
async fn start_sandbox(router: &MultiServiceRouter, project: &str) -> Value {
    let (status, response) = call(router, "StartSandbox", json!({"projectName": project})).await;
    assert_eq!(
        status, 200,
        "StartSandbox {project:?}: expected 200, got {status} with {response}"
    );
    response["sandbox"].clone()
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

/// `StartSandboxConnection` for `sandbox_id`, signed for `region`.
async fn start_connection(
    router: &MultiServiceRouter,
    region: &str,
    sandbox_id: &str,
) -> (u16, Value) {
    call_in(
        router,
        region,
        "StartSandboxConnection",
        json!({"sandboxId": sandbox_id}),
    )
    .await
}

/// A string member, for an assertion message that shows what was actually there.
fn text(value: &Value, member: &str) -> String {
    value[member].as_str().unwrap_or_default().to_string()
}

/// The `__type` an error body carries, for asserting the error code a client branches on.
fn error_type(body: &Value) -> String {
    body["__type"].as_str().unwrap_or_default().to_string()
}

/// The connection the store holds under `session_id`, if any.
fn stored_connection(
    engine: &Arc<engine::InMemoryEngine>,
    session_id: &str,
) -> Option<StoredSandboxConnection> {
    let state = engine.state::<CodeBuildState>();
    let guard = state.lock();
    guard.sandbox_connections.get(session_id).cloned()
}

/// Every session id the store holds for `sandbox_id`, whether still in force or not.
fn connection_ids_of(engine: &Arc<engine::InMemoryEngine>, sandbox_id: &str) -> BTreeSet<String> {
    let state = engine.state::<CodeBuildState>();
    let guard = state.lock();
    guard
        .sandbox_connections
        .iter()
        .filter(|(_, held)| held.sandbox_id == sandbox_id)
        .map(|(id, _)| id.clone())
        .collect()
}

/// Every session id of `sandbox_id` whose token is still in force at `at_millis`.
fn live_connection_ids_at(
    engine: &Arc<engine::InMemoryEngine>,
    sandbox_id: &str,
    at_millis: i64,
) -> BTreeSet<String> {
    let state = engine.state::<CodeBuildState>();
    let guard = state.lock();
    guard
        .live_sandbox_connections(sandbox_id, at_millis)
        .into_iter()
        .filter_map(|held| held.ssm_session.session_id.clone())
        .collect()
}

/// How many connections the store holds in total.
fn connection_count(engine: &Arc<engine::InMemoryEngine>) -> usize {
    let state = engine.state::<CodeBuildState>();
    let guard = state.lock();
    guard.sandbox_connections.len()
}

/// Persist a sandbox whose session went live `age_millis` ago, carrying
/// `timeout_minutes` — the way `StartSandbox` persists one, but with the instants placed
/// in the past, so a test reads a sandbox of any age without waiting for one. Returns
/// the instant its session went live at.
fn seed_live_sandbox(
    engine: &Arc<engine::InMemoryEngine>,
    project: &str,
    id: &str,
    timeout_minutes: Option<i32>,
    age_millis: i64,
) -> i64 {
    let started = state::now_millis() - age_millis;
    let requested = started - state::SANDBOX_SETTLE_MILLIS;
    let state_handle = engine.state::<CodeBuildState>();
    state_handle.lock().insert_sandbox(
        id.to_string(),
        StoredSandbox {
            sandbox: Sandbox {
                id: Some(id.to_string()),
                arn: Some(state::sandbox_arn(REGION, id)),
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
    started
}

/// The `ssmSession` a successful call reported, asserted to carry every member.
fn reported_session(case: &str, status: u16, body: &Value) -> Value {
    assert_eq!(
        status, 200,
        "case {case:?}: StartSandboxConnection expected 200, got {status} with {body}"
    );
    let session = body["ssmSession"].clone();
    assert!(
        session.is_object(),
        "case {case:?}: expected an ssmSession, got {body}"
    );
    session
}

/// RULE: every `StartSandboxConnection` against a live sandbox starts its OWN Session
/// Manager session into it — a session id no other connection carries, an opaque token
/// of its own, and the data-channel URL of the region the request was made in — and
/// persists that session, leaving the sandbox itself untouched.
///
/// The table spans how a sandbox is addressed ("A sandboxId or sandboxArn") and the
/// region the call is made in, with each row called twice so that what one call reports
/// is compared against the other's.
#[tokio::test]
async fn every_connection_to_a_live_sandbox_starts_its_own_persisted_session() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(Arc::clone(&engine));
    create_project(&router, "connect-proj", json!({})).await;

    // (case, address the sandbox by its ARN rather than its id, the region called in)
    let cases: Vec<(&str, bool, &str)> = vec![
        ("by sandbox id, in us-west-2", false, "us-west-2"),
        ("by sandbox ARN, in us-west-2", true, "us-west-2"),
        ("by sandbox id, in eu-central-1", false, "eu-central-1"),
        ("by sandbox ARN, in ap-southeast-2", true, "ap-southeast-2"),
    ];

    // One sandbox per row, all started before the single wait for the provisioning span
    // the clock owes them.
    let mut sandboxes = Vec::new();
    for _ in &cases {
        sandboxes.push(start_sandbox(&router, "connect-proj").await);
    }
    tokio::time::sleep(PAST_THE_SETTLE_SPAN).await;

    let mut session_ids: BTreeSet<String> = BTreeSet::new();
    let mut tokens: BTreeSet<String> = BTreeSet::new();

    for ((case, by_arn, region), sandbox) in cases.iter().zip(sandboxes.iter()) {
        let id = text(sandbox, "id");
        let arn = text(sandbox, "arn");
        let addressed = if *by_arn { arn.clone() } else { id.clone() };

        let before = read_sandbox(&router, &id).await;
        assert_eq!(
            text(&before, "status"),
            "RUNNING",
            "case {case:?}: a sandbox past its provisioning span must be live before it \
             is connected to, got {before}"
        );

        for round in 1..=2 {
            let case = format!("{case}, call {round}");
            let (status, body) = start_connection(&router, region, &addressed).await;
            let session = reported_session(&case, status, &body);

            let session_id = text(&session, "sessionId");
            let token = text(&session, "tokenValue");
            let stream_url = text(&session, "streamUrl");

            assert!(
                !session_id.is_empty(),
                "case {case:?}: a connection reports the id of the session it started, \
                 got {session}"
            );
            assert!(
                session_ids.insert(session_id.clone()),
                "case {case:?}: session id {session_id:?} was already handed out by \
                 another connection"
            );
            assert!(
                token.chars().count() >= SHORTEST_OPAQUE_TOKEN,
                "case {case:?}: expected an opaque token of at least \
                 {SHORTEST_OPAQUE_TOKEN} characters, got {} in {session}",
                token.chars().count()
            );
            assert!(
                token
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '/' || c == '='),
                "case {case:?}: expected an encoded token, got {token:?}"
            );
            assert!(
                tokens.insert(token.clone()),
                "case {case:?}: the token was already handed out by another connection"
            );
            // Nothing about the caller or the request is readable out of the token.
            for leaked in [id.as_str(), "connect-proj", ACCESS_KEY_ID] {
                assert!(
                    !token.contains(leaked),
                    "case {case:?}: the token must carry nothing of the request, found \
                     {leaked:?} in it"
                );
            }
            assert_eq!(
                stream_url,
                format!(
                    "wss://ssmmessages.{region}.amazonaws.com/v1/data-channel/\
                     {session_id}?role=publish_subscribe"
                ),
                "case {case:?}: expected the data-channel URL of this session in \
                 {region}, got {stream_url:?}"
            );

            // Persisted, not fabricated per call: the store holds exactly the session
            // the caller was handed, against the sandbox's own id even when the request
            // named its ARN.
            let held = stored_connection(&engine, &session_id)
                .unwrap_or_else(|| panic!("case {case:?}: the session must be persisted"));
            assert_eq!(
                held.sandbox_id, id,
                "case {case:?}: the connection must be held against the sandbox id, got \
                 {:?}",
                held.sandbox_id
            );
            assert_eq!(
                (
                    held.ssm_session.session_id.clone().unwrap_or_default(),
                    held.ssm_session.token_value.clone().unwrap_or_default(),
                    held.ssm_session.stream_url.clone().unwrap_or_default(),
                ),
                (session_id.clone(), token.clone(), stream_url.clone()),
                "case {case:?}: the stored session must be the one the caller was handed"
            );
            assert!(
                held.expires_millis > held.started_millis,
                "case {case:?}: a started connection's token is in force for a span, got \
                 {held:?}"
            );
        }

        // Two calls, two sessions of this sandbox, both retained.
        assert_eq!(
            connection_ids_of(&engine, &id).len(),
            2,
            "case {case:?}: both connections of this sandbox must be retained, got {:?}",
            connection_ids_of(&engine, &id)
        );

        // Connecting changes nothing about the sandbox itself.
        let after = read_sandbox(&router, &id).await;
        assert_eq!(
            after, before,
            "case {case:?}: starting a connection must not change the sandbox, got {after}"
        );
    }

    assert_eq!(
        connection_count(&engine),
        2 * cases.len(),
        "every call must have persisted its own session"
    );
}

/// RULE: a connection's token is in force only while the sandbox session it connects to
/// is — it stops being in force at the end of the wait `Sandbox$timeoutInMinutes`
/// documents ("How long, in minutes, from 5 to 2160 (36 hours), for CodeBuild to wait
/// before timing out this sandbox if it does not get marked as completed", served on the
/// emulator's compressed sandbox clock), measured from the instant that session went
/// live.
///
/// The table spans that wait: its published bounds, a value between them, and the
/// project default a sandbox carries when the member was never set.
#[tokio::test]
async fn a_connection_token_is_in_force_only_while_its_sandbox_session_is() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(Arc::clone(&engine));
    create_project(&router, "ttl-proj", json!({})).await;

    // (case, the sandbox's timeoutInMinutes)
    let cases: Vec<(&str, Option<i32>)> = vec![
        ("the documented minimum", Some(5)),
        ("a quarter of an hour", Some(15)),
        ("a value between the bounds", Some(480)),
        ("the documented maximum", Some(2160)),
        ("no timeout member, the project default", None),
    ];

    for (index, (case, timeout_minutes)) in cases.iter().enumerate() {
        let id = format!("ttl-proj:case-{index:02}");
        // Seeded a second into its session, which every wait outlives.
        let started = seed_live_sandbox(&engine, "ttl-proj", &id, *timeout_minutes, 1_000);

        let (status, body) = start_connection(&router, REGION, &id).await;
        let session = reported_session(case, status, &body);
        let session_id = text(&session, "sessionId");

        let held = stored_connection(&engine, &session_id)
            .unwrap_or_else(|| panic!("case {case:?}: the session must be persisted"));
        let span = live_span_millis(*timeout_minutes);
        let expected_expiry = started + span;
        assert_eq!(
            held.expires_millis, expected_expiry,
            "case {case:?}: the token must stop being in force when the session does, \
             {span} ms after it went live, got {held:?}"
        );

        // In force right up to that instant, and never past it.
        for (moment, offset, in_force) in [
            ("as it was started", 0, true),
            ("a second before the session ends", -1_000, true),
            ("a millisecond before the session ends", -1, true),
            ("exactly when the session ends", 0, false),
            ("a second after the session ends", 1_000, false),
        ] {
            let at = if in_force && offset == 0 {
                held.started_millis
            } else {
                expected_expiry + offset
            };
            let live = live_connection_ids_at(&engine, &id, at);
            assert_eq!(
                live.contains(&session_id),
                in_force,
                "case {case:?} {moment}: expected in force={in_force}, got {live:?}"
            );
        }

        // Past the wait the clock has reaped the sandbox, so there is no session left to
        // connect to at all.
        let reaped = format!("ttl-proj:reaped-{index:02}");
        seed_live_sandbox(&engine, "ttl-proj", &reaped, *timeout_minutes, span + 1_000);
        let (status, body) = start_connection(&router, REGION, &reaped).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a sandbox past its own wait cannot be connected to, got \
             {status} with {body}"
        );
        assert!(
            connection_ids_of(&engine, &reaped).is_empty(),
            "case {case:?}: a refused connection must persist nothing"
        );
    }
}

/// RULE: a connection is a session INTO a sandbox's live session, so a sandbox whose
/// session is not in its run phase has nothing to connect to: the call is refused with
/// `InvalidInputException` (HTTP 400) naming the phase that session is sitting in, and
/// nothing is persisted.
#[tokio::test]
async fn a_sandbox_outside_its_run_phase_cannot_be_connected_to() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(Arc::clone(&engine));
    create_project(&router, "phase-proj", json!({})).await;

    // Stopped while still queued, and stopped after its session went live: both ended.
    let stopped_from_queue = text(&start_sandbox(&router, "phase-proj").await, "id");
    let (status, body) = call(
        &router,
        "StopSandbox",
        json!({"id": stopped_from_queue.clone()}),
    )
    .await;
    assert_eq!(
        status, 200,
        "StopSandbox: expected 200, got {status} with {body}"
    );
    let stopped_while_live = text(&start_sandbox(&router, "phase-proj").await, "id");
    tokio::time::sleep(PAST_THE_SETTLE_SPAN).await;
    let (status, body) = call(
        &router,
        "StopSandbox",
        json!({"id": stopped_while_live.clone()}),
    )
    .await;
    assert_eq!(
        status, 200,
        "StopSandbox: expected 200, got {status} with {body}"
    );

    // Reaped by the clock on its own, with nobody having called.
    let reaped = "phase-proj:reaped".to_string();
    seed_live_sandbox(
        &engine,
        "phase-proj",
        &reaped,
        Some(5),
        live_span_millis(Some(5)) + 1_000,
    );

    // Still in the queue: its container is not provisioned yet. Started last, so the
    // provisioning span the clock owes it has not elapsed when it is asked for below.
    let queued = text(&start_sandbox(&router, "phase-proj").await, "id");

    // (case, the sandbox asked for)
    let cases = [
        ("still in the queue", queued),
        ("stopped out of the queue", stopped_from_queue),
        ("stopped while live", stopped_while_live),
        ("reaped by the clock on its own", reaped),
    ];

    for (case, id) in &cases {
        // The phase the message must name is the one the sandbox's own session reports.
        let record = read_sandbox(&router, id).await;
        let phase = text(&record["currentSession"], "currentPhase");
        assert!(
            !phase.is_empty(),
            "case {case:?}: the sandbox's session must report a phase, got {record}"
        );

        let (status, body) = start_connection(&router, REGION, id).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {body}"
        );
        assert!(
            error_type(&body).contains("InvalidInputException"),
            "case {case:?}: expected InvalidInputException, got {body}"
        );
        assert_eq!(
            text(&body, "message"),
            format!("Sandbox found with invalid build phase to start connection: {phase}"),
            "case {case:?}: the refusal must name the phase the session is in, got {body}"
        );
        assert!(
            connection_ids_of(&engine, id).is_empty(),
            "case {case:?}: a refused connection must persist nothing"
        );
    }

    assert_eq!(
        connection_count(&engine),
        0,
        "no connection may be persisted for a sandbox that is not in its run phase"
    );
}

/// RULE: a WELL-FORMED id — or ARN — no sandbox of the account answers to is refused by
/// THIS operation as bad input: `InvalidInputException` (HTTP 400) carrying CodeBuild's
/// fixed unresolvable-id sentence and naming nothing of the request. A value that is not a
/// sandbox identifier at all is the other class and carries the other sentence — see
/// `tests/all/identifier_shape_refusals.rs`, which drives it across every operation that
/// addresses a sandbox.
#[tokio::test]
async fn an_unresolvable_sandbox_is_refused_as_invalid_input() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(Arc::clone(&engine));
    create_project(&router, "missing-proj", json!({})).await;
    // One live sandbox of the account, so the refusals below are about the id asked for
    // and not about an empty store.
    let live = text(&start_sandbox(&router, "missing-proj").await, "id");
    tokio::time::sleep(PAST_THE_SETTLE_SPAN).await;

    // (case, the id asked for)
    let cases = [
        (
            "an id never minted",
            "missing-proj:00000000-0000-8000-8000-000000000000".to_string(),
        ),
        ("an id of another project", "other-proj:1".to_string()),
        (
            "an ARN of a sandbox never minted",
            state::sandbox_arn(REGION, "missing-proj:never-minted"),
        ),
        (
            "a live sandbox's id with a suffix",
            format!("{live}-not-this-one"),
        ),
    ];

    for (case, id) in &cases {
        let (status, body) = start_connection(&router, REGION, id).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {body}"
        );
        assert!(
            error_type(&body).contains("InvalidInputException"),
            "case {case:?}: expected InvalidInputException, got {body}"
        );
        assert_eq!(
            text(&body, "message"),
            state::UNRESOLVABLE_SANDBOX_ID_MESSAGE,
            "case {case:?}: expected CodeBuild's unresolvable-sandbox-id sentence, got {body}"
        );
    }

    assert_eq!(
        connection_count(&engine),
        0,
        "a refused connection must persist nothing"
    );

    // The sandbox that IS there still connects, so the refusals above were about the id.
    let (status, body) = start_connection(&router, REGION, &live).await;
    let session = reported_session("the live sandbox of the account", status, &body);
    assert!(
        !text(&session, "sessionId").is_empty(),
        "the live sandbox must still connect, got {body}"
    );
}
