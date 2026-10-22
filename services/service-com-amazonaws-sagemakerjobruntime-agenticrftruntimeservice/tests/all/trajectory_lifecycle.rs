// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the AgenticRFT runtime emulator's trajectory
//! lifecycle: `Sample`, `SampleWithResponseStream`, `CompleteRollout` and
//! `UpdateReward`.
//!
//! Every assertion goes over the wire — the suite mounts the service in a
//! [`MultiServiceRouter`] and dispatches real `restJson1` requests — so what it
//! pins is the emulator's HTTP behaviour, never a handler's Rust signature. The
//! service's own store is read afterwards, the way the NEXT request would read
//! it, to show the answer came from persisted state.
//!
//! Each rule is stated once and asserted over a TABLE whose rows differ along
//! what that rule generalises over: the job ARN's partition/region/account, the
//! trajectory id's length (both modelled bounds), the number of turns, which
//! sampling operation served each turn, the form and size of the inference
//! payload, the terminal completion status, and the length and values of the
//! reward list.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_sagemakerjobruntime_agenticrftruntimeservice::state;
use service_com_amazonaws_sagemakerjobruntime_agenticrftruntimeservice::ComAmazonawsSagemakerjobruntimeAgenticrftruntimeserviceState as ServiceState;
use service_com_amazonaws_sagemakerjobruntime_agenticrftruntimeservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A host-style endpoint built from the model's endpoint prefix
/// (`job-runtime.sagemaker`).
const ENDPOINT: &str = "https://job-runtime.sagemaker.us-east-1.amazonaws.com";

/// A SigV4 `Authorization` header whose credential scope names this service's
/// sigv4 signing name (`sagemaker`) — the routing signal `resolve_route_key`
/// reads. Request authentication is off on the test router, so the signature
/// itself is never checked.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20240101/us-east-1/sagemaker/aws4_request, \
     SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000";

/// The header `JobArn` is bound to.
const JOB_ARN_HEADER: &str = "x-amzn-sagemaker-job-arn";

/// The header `TrajectoryId` is bound to on the sampling operations.
const TRAJECTORY_ID_HEADER: &str = "x-amzn-sagemaker-trajectory-id";

/// One HTTP answer, reduced to what these tests assert on.
#[derive(Debug)]
struct Reply {
    status: u16,
    /// `x-amzn-errortype` — how restJson1 names the error shape.
    error_type: Option<String>,
    content_type: Option<String>,
    body: Vec<u8>,
}

impl Reply {
    /// The bare error code out of an `x-amzn-errortype` value, which may carry a
    /// namespace or a shape-id URL alongside the name.
    fn error_code(&self) -> String {
        let raw = self.error_type.clone().unwrap_or_default();
        raw.split(':')
            .next()
            .unwrap_or_default()
            .rsplit('#')
            .next()
            .unwrap_or_default()
            .to_string()
    }

    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

/// A router carrying exactly this service over one shared engine — the same
/// assembly `core/router` performs, minus the other services. The engine comes
/// back too, so a test can read this service's store the way the next request
/// would.
fn mounted_service() -> (Arc<engine::InMemoryEngine>, MultiServiceRouter) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router
        .set_request_authentication(false)
        .register_service(EmulatedService::new(Arc::clone(&engine)));
    (engine, router)
}

/// Record `job_arn` as a job this runtime serves rollouts for.
///
/// A sampling call is refused for a `JobArn` the store does not hold, and this service
/// models no operation that creates a job (the store seeds a fixture pair for that
/// reason — see `state::seed_job_arns`), so a test that samples a job of its own
/// registers it first, the way the control plane's job creation would.
fn register_job(engine: &engine::InMemoryEngine, job_arn: &str) {
    engine.state::<ServiceState>().lock().register_job(job_arn);
}

async fn send(
    router: &MultiServiceRouter,
    path: &str,
    headers: &[(&str, &str)],
    body: Vec<u8>,
) -> Reply {
    let mut request = http::Request::builder()
        .method("POST")
        .uri(format!("{ENDPOINT}{path}"))
        .header("authorization", AUTHORIZATION)
        .header("host", "job-runtime.sagemaker.us-east-1.amazonaws.com");
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let request = request
        .body(bytes::Bytes::from(body))
        .expect("a well-formed test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(ToString::to_string);
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .map(ToString::to_string);
    Reply {
        status,
        error_type,
        content_type,
        body: response.into_body().to_vec(),
    }
}

/// One `Sample` or `SampleWithResponseStream` call.
async fn sample(
    router: &MultiServiceRouter,
    job_arn: &str,
    trajectory_id: &str,
    payload: &[u8],
    streamed: bool,
) -> Reply {
    let path = if streamed {
        "/sample-with-response-stream"
    } else {
        "/sample"
    };
    send(
        router,
        path,
        &[
            (JOB_ARN_HEADER, job_arn),
            (TRAJECTORY_ID_HEADER, trajectory_id),
            ("content-type", "application/json"),
        ],
        payload.to_vec(),
    )
    .await
}

/// One `CompleteRollout` call; `status` is omitted from the body when `None`.
async fn complete_rollout(
    router: &MultiServiceRouter,
    job_arn: &str,
    trajectory_id: &str,
    status: Option<&str>,
    client_token: &str,
) -> Reply {
    let mut body = json!({ "TrajectoryId": trajectory_id, "ClientToken": client_token });
    if let Some(status) = status {
        body["Status"] = Value::String(status.to_string());
    }
    send(
        router,
        "/complete-rollout",
        &[
            (JOB_ARN_HEADER, job_arn),
            ("content-type", "application/json"),
        ],
        serde_json::to_vec(&body).expect("a serializable body"),
    )
    .await
}

/// One `UpdateReward` call.
async fn update_reward(
    router: &MultiServiceRouter,
    job_arn: &str,
    trajectory_id: &str,
    rewards: &[f64],
    client_token: &str,
) -> Reply {
    let body = json!({
        "TrajectoryId": trajectory_id,
        "Rewards": rewards,
        "ClientToken": client_token,
    });
    send(
        router,
        "/update-reward",
        &[
            (JOB_ARN_HEADER, job_arn),
            ("content-type", "application/json"),
        ],
        serde_json::to_vec(&body).expect("a serializable body"),
    )
    .await
}

// ---------------------------------------------------------------------------
// The trajectory table
// ---------------------------------------------------------------------------

/// A job ARN that satisfies the modelled
/// `^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:job/[a-zA-Z0-9_\-]+/[a-zA-Z0-9](-*[a-zA-Z0-9]){0,62}$`
/// pattern, assembled so a row can vary the partition, the region, the account
/// and the job name independently.
fn job_arn(partition: &str, region: &str, account: &str, job: &str, run: &str) -> String {
    format!("arn:{partition}:sagemaker:{region}:{account}:job/{job}/{run}")
}

/// One row of the trajectory table: a whole rollout, so a row differs from its
/// neighbours along the ARN's partition/region/account, the trajectory id's
/// length, the number of turns, which operation serves each turn, and the form
/// and size of each inference payload.
struct TrajectoryCase {
    case: &'static str,
    job_arn: String,
    trajectory_id: String,
    /// One `(payload, streamed)` pair per turn, in the order they are served.
    turns: Vec<(Vec<u8>, bool)>,
}

/// An OpenAI-compatible chat-completions request body.
fn chat_payload(text: &str) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "model": "rft-policy",
        "messages": [{ "role": "user", "content": text }],
        "max_tokens": 64,
    }))
    .expect("a serializable payload")
}

fn trajectory_cases() -> Vec<TrajectoryCase> {
    vec![
        TrajectoryCase {
            case: "one Sample turn, chat-completions payload",
            job_arn: job_arn("aws", "us-east-1", "123456789012", "rft-job", "traj-run"),
            trajectory_id: "traj-1".to_string(),
            turns: vec![(chat_payload("Summarize the rollout."), false)],
        },
        TrajectoryCase {
            case: "three turns mixing Sample and SampleWithResponseStream",
            job_arn: job_arn(
                "aws-us-gov",
                "us-gov-west-1",
                "210987654321",
                "agentic_rft-2",
                "run-7",
            ),
            trajectory_id: "trajectory-with-a-longer-id-0042".to_string(),
            turns: vec![
                (
                    serde_json::to_vec(&json!({ "prompt": "step one" })).expect("payload"),
                    false,
                ),
                (b"this payload is not JSON at all".to_vec(), true),
                (chat_payload(&"token ".repeat(512)), false),
            ],
        },
        TrajectoryCase {
            case: "two streamed turns, trajectory id at the modelled maximum of 256",
            job_arn: job_arn("aws-cn", "cn-north-1", "000000000001", "j", "r"),
            trajectory_id: "t".repeat(256),
            turns: vec![
                (chat_payload("first"), true),
                (chat_payload("second"), true),
            ],
        },
        TrajectoryCase {
            case: "one streamed turn, trajectory id at the modelled minimum of 1",
            job_arn: job_arn(
                "aws",
                "ap-southeast-2",
                "999999999999",
                "JobName-With_Mixed-Case",
                "a",
            ),
            trajectory_id: "x".to_string(),
            turns: vec![(b"{}".to_vec(), true)],
        },
    ]
}

// ---------------------------------------------------------------------------
// Sampling
// ---------------------------------------------------------------------------

/// RULE: every sampling call captures one turn on the `(JobArn, TrajectoryId)`
/// trajectory — creating it on the first call — and serves exactly the response
/// body it recorded for that turn, in the framing its operation uses.
#[tokio::test]
async fn every_sampling_call_captures_a_turn_and_serves_what_it_recorded() {
    for case in trajectory_cases() {
        let (engine, router) = mounted_service();
        let TrajectoryCase {
            case,
            job_arn,
            trajectory_id,
            turns,
        } = case;

        let mut served: Vec<Reply> = Vec::new();
        register_job(&engine, &job_arn);
        for (index, (payload, streamed)) in turns.iter().enumerate() {
            let reply = sample(&router, &job_arn, &trajectory_id, payload, *streamed).await;
            assert_eq!(
                reply.status,
                200,
                "case {case:?} turn {index}: expected 200, got {} / {}",
                reply.status,
                reply.text()
            );
            let expected_content_type = if *streamed {
                state::EVENT_STREAM_CONTENT_TYPE
            } else {
                state::JSON_CONTENT_TYPE
            };
            assert_eq!(
                reply.content_type.as_deref(),
                Some(expected_content_type),
                "case {case:?} turn {index}: expected Content-Type {expected_content_type:?}, got \
                 {:?}",
                reply.content_type
            );
            assert!(
                !reply.body.is_empty(),
                "case {case:?} turn {index}: expected a response payload, got an empty body"
            );
            served.push(reply);
        }

        // The store, read the way the next request would read it.
        let store = engine.state::<ServiceState>();
        let guard = store.lock();
        let trajectory = guard
            .trajectory(&job_arn, &trajectory_id)
            .unwrap_or_else(|| panic!("case {case:?}: expected the trajectory to exist in state"));
        assert_eq!(
            trajectory.turns.len(),
            turns.len(),
            "case {case:?}: expected {} captured turn(s), got {}",
            turns.len(),
            trajectory.turns.len()
        );
        assert_eq!(
            trajectory.status,
            state::TrajectoryStatus::InProgress,
            "case {case:?}: expected an un-sealed trajectory, got {:?}",
            trajectory.status
        );
        assert!(
            trajectory.completion_status.is_none() && trajectory.rewards.is_none(),
            "case {case:?}: expected no completion status and no rewards before CompleteRollout, \
             got {:?} / {:?}",
            trajectory.completion_status,
            trajectory.rewards
        );

        for (index, ((payload, streamed), reply)) in turns.iter().zip(&served).enumerate() {
            let turn = &trajectory.turns[index];
            assert_eq!(
                turn.index, index,
                "case {case:?} turn {index}: expected index {index}, got {}",
                turn.index
            );
            assert_eq!(
                &turn.request_body, payload,
                "case {case:?} turn {index}: expected the prompt to be captured verbatim, got {} \
                 byte(s)",
                turn.request_body.len()
            );
            assert_eq!(
                turn.request_content_type.as_deref(),
                Some("application/json"),
                "case {case:?} turn {index}: expected the request Content-Type to be captured, got \
                 {:?}",
                turn.request_content_type
            );
            assert_eq!(
                turn.streamed, *streamed,
                "case {case:?} turn {index}: expected streamed={streamed}, got {}",
                turn.streamed
            );
            assert_eq!(
                turn.response_body, reply.body,
                "case {case:?} turn {index}: expected the served body to be the recorded one, got \
                 {} served vs {} recorded byte(s)",
                reply.body.len(),
                turn.response_body.len()
            );

            // The framing each operation serves, and the id the turn reports.
            let expected_id = state::completion_id(&job_arn, &trajectory_id, index);
            if *streamed {
                let text = reply.text();
                assert!(
                    text.starts_with("data: ") && text.ends_with("data: [DONE]\n\n"),
                    "case {case:?} turn {index}: expected server-sent-event framing, got {text:?}"
                );
                let first = text
                    .strip_prefix("data: ")
                    .and_then(|rest| rest.split("\n\n").next())
                    .unwrap_or_default()
                    .to_string();
                let chunk: Value = serde_json::from_str(&first).unwrap_or(Value::Null);
                assert_eq!(
                    chunk["object"], "chat.completion.chunk",
                    "case {case:?} turn {index}: expected a chat.completion.chunk, got {:?}",
                    chunk["object"]
                );
                assert_eq!(
                    chunk["id"], expected_id,
                    "case {case:?} turn {index}: expected id {expected_id:?}, got {:?}",
                    chunk["id"]
                );
            } else {
                let completion = reply.json();
                assert_eq!(
                    completion["object"], "chat.completion",
                    "case {case:?} turn {index}: expected a chat.completion, got {:?}",
                    completion["object"]
                );
                assert_eq!(
                    completion["id"], expected_id,
                    "case {case:?} turn {index}: expected id {expected_id:?}, got {:?}",
                    completion["id"]
                );
                let content = completion["choices"][0]["message"]["content"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                assert_eq!(
                    content,
                    state::emulated_turn_text(
                        index + 1,
                        state::count_text_tokens(&state::prompt_text_of_body(payload))
                    ),
                    "case {case:?} turn {index}: expected the emulator's own turn notice, got \
                     {content:?}"
                );
            }
        }

        // Every turn of a trajectory reports its own id.
        let mut ids: Vec<String> = (0..turns.len())
            .map(|index| state::completion_id(&job_arn, &trajectory_id, index))
            .collect();
        let distinct = {
            ids.sort();
            ids.dedup();
            ids.len()
        };
        assert_eq!(
            distinct,
            turns.len(),
            "case {case:?}: expected {} distinct completion id(s), got {distinct}",
            turns.len()
        );
    }
}

/// RULE: a trajectory is keyed by the `(JobArn, TrajectoryId)` PAIR, so the same
/// trajectory id under a different job is a different trajectory.
#[tokio::test]
async fn the_trajectory_key_is_the_job_and_trajectory_pair() {
    let (engine, router) = mounted_service();
    let first = job_arn("aws", "us-east-1", "123456789012", "rft-job", "run-a");
    let second = job_arn("aws", "us-east-1", "123456789012", "rft-job", "run-b");
    let third = job_arn("aws", "eu-west-1", "123456789012", "rft-job", "run-a");
    let rows = [(&first, 2usize), (&second, 1), (&third, 3)];

    for (arn, _) in rows {
        register_job(&engine, arn);
    }

    for (arn, turns) in rows {
        for _ in 0..turns {
            let reply = sample(&router, arn, "shared-id", &chat_payload("hello"), false).await;
            assert_eq!(
                reply.status,
                200,
                "case {arn:?}: expected 200, got {} / {}",
                reply.status,
                reply.text()
            );
        }
    }

    let store = engine.state::<ServiceState>();
    let guard = store.lock();
    assert_eq!(
        guard.trajectories.len(),
        rows.len(),
        "case \"distinct jobs\": expected {} trajectories, got {}",
        rows.len(),
        guard.trajectories.len()
    );
    for (arn, turns) in rows {
        let actual = guard
            .trajectory(arn, "shared-id")
            .map_or(0, |trajectory| trajectory.turns.len());
        assert_eq!(
            actual, turns,
            "case {arn:?}: expected {turns} captured turn(s), got {actual}"
        );
    }
}

// ---------------------------------------------------------------------------
// CompleteRollout
// ---------------------------------------------------------------------------

/// RULE: `CompleteRollout` seals the trajectory with its terminal status; a
/// repeat asking for the status it already holds succeeds again, a request that
/// would change it conflicts, and no further turn can be appended once sealed.
#[tokio::test]
async fn complete_rollout_seals_the_trajectory_and_refuses_a_different_status() {
    // Rows: the status the first call asks for (including the omitted form,
    // which the model leaves optional), and the status a later call tries to
    // move it to. The catalogue of values comes from the model's own
    // `CompletionStatus` enum.
    let rows: [(&str, Option<&str>, state::CompletionStatus, &str); 3] = [
        (
            "sealed ready",
            Some("ready"),
            state::CompletionStatus::Ready,
            "failed",
        ),
        (
            "sealed failed",
            Some("failed"),
            state::CompletionStatus::Failed,
            "ready",
        ),
        (
            "Status omitted",
            None,
            state::CompletionStatus::Ready,
            "failed",
        ),
    ];

    for (case, requested, expected, other) in rows {
        let (engine, router) = mounted_service();
        let arn = job_arn("aws", "us-east-1", "123456789012", "rft-job", "run-1");
        register_job(&engine, &arn);
        assert_eq!(
            sample(&router, &arn, "traj-1", &chat_payload("go"), false)
                .await
                .status,
            200,
            "case {case:?}: expected the first Sample to answer 200"
        );

        let sealed = complete_rollout(&router, &arn, "traj-1", requested, "tok-1").await;
        assert_eq!(
            sealed.status,
            200,
            "case {case:?}: expected CompleteRollout to answer 200, got {} / {}",
            sealed.status,
            sealed.text()
        );

        {
            let store = engine.state::<ServiceState>();
            let guard = store.lock();
            let trajectory = guard
                .trajectory(&arn, "traj-1")
                .unwrap_or_else(|| panic!("case {case:?}: expected the trajectory in state"));
            assert_eq!(
                trajectory.completion_status,
                Some(expected),
                "case {case:?}: expected completion status {expected:?}, got {:?}",
                trajectory.completion_status
            );
            assert_eq!(
                trajectory.status,
                state::TrajectoryStatus::Complete,
                "case {case:?}: expected a sealed trajectory, got {:?}",
                trajectory.status
            );
            assert_eq!(
                trajectory.complete_rollout_token.as_deref(),
                Some("tok-1"),
                "case {case:?}: expected the sealing ClientToken to be recorded, got {:?}",
                trajectory.complete_rollout_token
            );
        }

        // @idempotent: the same request again, and the same request with a fresh
        // token, both succeed and change nothing.
        for (replay_case, token) in [("same token", "tok-1"), ("fresh token", "tok-2")] {
            let replay = complete_rollout(&router, &arn, "traj-1", requested, token).await;
            assert_eq!(
                replay.status,
                200,
                "case {case:?}/{replay_case:?}: expected an idempotent replay to answer 200, got \
                 {} / {}",
                replay.status,
                replay.text()
            );
        }

        // A request that would CHANGE the sealed status conflicts.
        let conflicting = complete_rollout(&router, &arn, "traj-1", Some(other), "tok-1").await;
        assert_eq!(
            conflicting.status,
            409,
            "case {case:?}: expected 409 when changing the sealed status to {other:?}, got {} / {}",
            conflicting.status,
            conflicting.text()
        );
        assert_eq!(
            conflicting.error_code(),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {:?}",
            conflicting.error_type
        );

        // And no further turn is appended, by either sampling operation.
        for streamed in [false, true] {
            let refused = sample(&router, &arn, "traj-1", &chat_payload("more"), streamed).await;
            assert_eq!(
                refused.status,
                400,
                "case {case:?}/streamed={streamed}: expected a sealed trajectory to refuse another \
                 turn with 400, got {} / {}",
                refused.status,
                refused.text()
            );
            assert_eq!(
                refused.error_code(),
                "ValidationException",
                "case {case:?}/streamed={streamed}: expected ValidationException, got {:?}",
                refused.error_type
            );
        }
        let store = engine.state::<ServiceState>();
        let guard = store.lock();
        let turns = guard
            .trajectory(&arn, "traj-1")
            .map_or(0, |trajectory| trajectory.turns.len());
        assert_eq!(
            turns, 1,
            "case {case:?}: expected the sealed trajectory to still hold 1 turn, got {turns}"
        );
    }
}

// ---------------------------------------------------------------------------
// UpdateReward
// ---------------------------------------------------------------------------

/// RULE: `UpdateReward` records the reward list on a trajectory sealed `ready`
/// and moves it to reward-received; a replay of the same list succeeds, a
/// different list conflicts, and a trajectory that is not eligible — not sealed,
/// or sealed `failed` — conflicts.
#[tokio::test]
async fn update_reward_records_scores_only_on_a_ready_trajectory() {
    // Rows differ along the length and the values of the reward list: one score,
    // several mixed-sign fractional scores, a list whose scores are all zero, and
    // a long list. (The EMPTY list is refused, not recorded — see
    // `an_empty_reward_list_is_refused`.)
    let rows: [(&str, Vec<f64>, Vec<f64>); 4] = [
        ("single reward", vec![1.0], vec![0.0]),
        (
            "mixed-sign fractional rewards",
            vec![0.5, -0.25, 3.0],
            vec![0.5, -0.25],
        ),
        ("all-zero rewards", vec![0.0, 0.0], vec![0.0]),
        (
            "long reward list",
            (0..64).map(|index| f64::from(index) / 8.0).collect(),
            (0..64).map(|index| f64::from(index) / 4.0).collect(),
        ),
    ];

    for (case, rewards, other) in rows {
        let (engine, router) = mounted_service();
        let arn = job_arn("aws", "us-west-2", "123456789012", "rft-job", "run-1");
        register_job(&engine, &arn);

        // Not eligible before CompleteRollout.
        assert_eq!(
            sample(&router, &arn, "traj-1", &chat_payload("go"), false)
                .await
                .status,
            200,
            "case {case:?}: expected the first Sample to answer 200"
        );
        let early = update_reward(&router, &arn, "traj-1", &rewards, "tok-r").await;
        assert_eq!(
            early.status,
            409,
            "case {case:?}: expected 409 for a reward on an un-sealed trajectory, got {} / {}",
            early.status,
            early.text()
        );
        assert_eq!(
            early.error_code(),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {:?}",
            early.error_type
        );

        // Sealed `ready`: the reward is recorded and the trajectory transitions.
        assert_eq!(
            complete_rollout(&router, &arn, "traj-1", Some("ready"), "tok-c")
                .await
                .status,
            200,
            "case {case:?}: expected CompleteRollout to answer 200"
        );
        let accepted = update_reward(&router, &arn, "traj-1", &rewards, "tok-r").await;
        assert_eq!(
            accepted.status,
            200,
            "case {case:?}: expected UpdateReward to answer 200, got {} / {}",
            accepted.status,
            accepted.text()
        );
        {
            let store = engine.state::<ServiceState>();
            let guard = store.lock();
            let trajectory = guard
                .trajectory(&arn, "traj-1")
                .unwrap_or_else(|| panic!("case {case:?}: expected the trajectory in state"));
            assert_eq!(
                trajectory.rewards.as_deref(),
                Some(rewards.as_slice()),
                "case {case:?}: expected the rewards to be persisted, got {:?}",
                trajectory.rewards
            );
            assert_eq!(
                trajectory.status,
                state::TrajectoryStatus::RewardReceived,
                "case {case:?}: expected reward-received status, got {:?}",
                trajectory.status
            );
            assert_eq!(
                trajectory.update_reward_token.as_deref(),
                Some("tok-r"),
                "case {case:?}: expected the UpdateReward ClientToken to be recorded, got {:?}",
                trajectory.update_reward_token
            );
        }

        // @idempotent on the same list, conflicting on a different one.
        let replay = update_reward(&router, &arn, "traj-1", &rewards, "tok-r").await;
        assert_eq!(
            replay.status,
            200,
            "case {case:?}: expected an idempotent reward replay to answer 200, got {} / {}",
            replay.status,
            replay.text()
        );
        let changed = update_reward(&router, &arn, "traj-1", &other, "tok-r2").await;
        assert_eq!(
            changed.status,
            409,
            "case {case:?}: expected 409 when replacing the recorded rewards, got {} / {}",
            changed.status,
            changed.text()
        );
        assert_eq!(
            changed.error_code(),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {:?}",
            changed.error_type
        );
        {
            let store = engine.state::<ServiceState>();
            let guard = store.lock();
            let persisted = guard
                .trajectory(&arn, "traj-1")
                .and_then(|trajectory| trajectory.rewards.clone());
            assert_eq!(
                persisted.as_deref(),
                Some(rewards.as_slice()),
                "case {case:?}: expected the refused call to leave the rewards alone, got {persisted:?}"
            );
        }

        // Sealed `failed` is terminal: it accepts no reward at all.
        assert_eq!(
            sample(&router, &arn, "traj-2", &chat_payload("go"), true)
                .await
                .status,
            200,
            "case {case:?}: expected the first Sample on traj-2 to answer 200"
        );
        assert_eq!(
            complete_rollout(&router, &arn, "traj-2", Some("failed"), "tok-c2")
                .await
                .status,
            200,
            "case {case:?}: expected CompleteRollout(failed) to answer 200"
        );
        let on_failed = update_reward(&router, &arn, "traj-2", &rewards, "tok-r3").await;
        assert_eq!(
            on_failed.status,
            409,
            "case {case:?}: expected 409 for a reward on a failed trajectory, got {} / {}",
            on_failed.status,
            on_failed.text()
        );
        assert_eq!(
            on_failed.error_code(),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {:?}",
            on_failed.error_type
        );
    }
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// RULE: a `CompleteRollout` or `UpdateReward` naming a `(JobArn,
/// TrajectoryId)` pair this emulator holds no trajectory for is not found.
#[tokio::test]
async fn an_unknown_trajectory_is_not_found() {
    let known = job_arn("aws", "us-east-1", "123456789012", "rft-job", "run-1");
    let other_job = job_arn("aws", "us-east-1", "123456789012", "rft-job", "run-2");
    let other_region = job_arn("aws", "eu-central-1", "123456789012", "rft-job", "run-1");

    // Rows: the pair that is looked up, against a store holding exactly
    // (known, "traj-1").
    let rows: [(&str, &String, &str); 4] = [
        ("never-sampled trajectory id", &known, "never-sampled"),
        ("trajectory id that differs by case", &known, "TRAJ-1"),
        ("known trajectory id, other job run", &other_job, "traj-1"),
        ("known trajectory id, other region", &other_region, "traj-1"),
    ];

    for (case, arn, trajectory_id) in rows {
        let (engine, router) = mounted_service();
        register_job(&engine, &known);
        assert_eq!(
            sample(&router, &known, "traj-1", &chat_payload("go"), false)
                .await
                .status,
            200,
            "case {case:?}: expected the seeding Sample to answer 200"
        );

        let completed = complete_rollout(&router, arn, trajectory_id, Some("ready"), "tok").await;
        assert_eq!(
            completed.status,
            404,
            "case {case:?}: expected CompleteRollout to answer 404, got {} / {}",
            completed.status,
            completed.text()
        );
        assert_eq!(
            completed.error_code(),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {:?}",
            completed.error_type
        );

        let rewarded = update_reward(&router, arn, trajectory_id, &[1.0], "tok").await;
        assert_eq!(
            rewarded.status,
            404,
            "case {case:?}: expected UpdateReward to answer 404, got {} / {}",
            rewarded.status,
            rewarded.text()
        );
        assert_eq!(
            rewarded.error_code(),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {:?}",
            rewarded.error_type
        );
    }
}

/// RULE: a request that violates a modelled constraint on `JobArn` or
/// `TrajectoryId`, carries no inference payload, or names a `Status` outside the
/// modelled `CompletionStatus` enum, is refused as invalid and writes nothing.
#[tokio::test]
async fn a_malformed_request_is_refused_as_invalid() {
    let valid = job_arn("aws", "us-east-1", "123456789012", "rft-job", "run-1");

    // Rows: each one breaks a DIFFERENT modelled constraint — the ARN pattern in
    // four places, both ends of the trajectory id's 1..256 length range, the
    // required payload, and the enum catalogue.
    let long_trajectory_id = "t".repeat(257);
    let long_arn = job_arn("aws", "us-east-1", "123456789012", "rft-job", &"a".repeat(63));
    let sampling_rows: Vec<(&str, String, String, Vec<u8>)> = vec![
        (
            "JobArn is not an ARN",
            "not-an-arn".to_string(),
            "traj-1".to_string(),
            chat_payload("go"),
        ),
        (
            "JobArn names another service",
            "arn:aws:bedrock:us-east-1:123456789012:job/rft-job/run-1".to_string(),
            "traj-1".to_string(),
            chat_payload("go"),
        ),
        (
            "JobArn account is not 12 digits",
            "arn:aws:sagemaker:us-east-1:12345:job/rft-job/run-1".to_string(),
            "traj-1".to_string(),
            chat_payload("go"),
        ),
        (
            "JobArn has no job run segment",
            "arn:aws:sagemaker:us-east-1:123456789012:job/rft-job".to_string(),
            "traj-1".to_string(),
            chat_payload("go"),
        ),
        (
            "TrajectoryId one past the 256-character maximum",
            valid.clone(),
            long_trajectory_id.clone(),
            chat_payload("go"),
        ),
        (
            "TrajectoryId below the 1-character minimum",
            valid.clone(),
            String::new(),
            chat_payload("go"),
        ),
        (
            "no inference payload",
            valid.clone(),
            "traj-1".to_string(),
            Vec::new(),
        ),
    ];

    for (case, arn, trajectory_id, payload) in sampling_rows {
        for streamed in [false, true] {
            let (engine, router) = mounted_service();
            register_job(&engine, &valid);
            let reply = sample(&router, &arn, &trajectory_id, &payload, streamed).await;
            assert_eq!(
                reply.status,
                400,
                "case {case:?}/streamed={streamed}: expected 400, got {} / {}",
                reply.status,
                reply.text()
            );
            assert_eq!(
                reply.error_code(),
                "ValidationException",
                "case {case:?}/streamed={streamed}: expected ValidationException, got {:?}",
                reply.error_type
            );
            let store = engine.state::<ServiceState>();
            let held = store.lock().trajectories.len();
            assert_eq!(
                held, 0,
                "case {case:?}/streamed={streamed}: expected a refused request to write nothing, \
                 got {held} trajectory/ies"
            );
        }
    }

    // The ARN rows above are refused on the pattern; this one satisfies it at the
    // job-run segment's 63-character maximum and is accepted, which is what makes
    // the refusals above about the pattern and not about length.
    let (engine, router) = mounted_service();
    register_job(&engine, &long_arn);
    let accepted = sample(&router, &long_arn, "traj-1", &chat_payload("go"), false).await;
    assert_eq!(
        accepted.status,
        200,
        "case \"job run segment at its 63-character maximum\": expected 200, got {} / {}",
        accepted.status,
        accepted.text()
    );

    // A `Status` outside the modelled enum, on a trajectory that does exist.
    let (engine, router) = mounted_service();
    register_job(&engine, &valid);
    assert_eq!(
        sample(&router, &valid, "traj-1", &chat_payload("go"), false)
            .await
            .status,
        200,
        "case \"unknown Status\": expected the seeding Sample to answer 200"
    );
    let bad_status = complete_rollout(&router, &valid, "traj-1", Some("Ready"), "tok").await;
    assert_eq!(
        bad_status.status,
        400,
        "case \"unknown Status\": expected 400, got {} / {}",
        bad_status.status,
        bad_status.text()
    );
    assert_eq!(
        bad_status.error_code(),
        "ValidationException",
        "case \"unknown Status\": expected ValidationException, got {:?}",
        bad_status.error_type
    );
    let store = engine.state::<ServiceState>();
    let guard = store.lock();
    let sealed = guard
        .trajectory(&valid, "traj-1")
        .and_then(|trajectory| trajectory.completion_status);
    assert!(
        sealed.is_none(),
        "case \"unknown Status\": expected a refused CompleteRollout to seal nothing, got {sealed:?}"
    );
}

/// RULE: the emulator's per-trajectory turn bound is reported as the modelled
/// `ServiceQuotaExceededException`, and the turn is not captured.
#[tokio::test]
async fn the_turn_bound_is_reported_as_a_service_quota() {
    let arn = job_arn("aws", "us-east-1", "123456789012", "rft-job", "run-1");
    for streamed in [false, true] {
        let (engine, router) = mounted_service();
        register_job(&engine, &arn);

        // Seed a trajectory that already holds the bound, so the test does not
        // have to drive a thousand requests to reach it.
        {
            let store = engine.state::<ServiceState>();
            let mut guard = store.lock();
            let now = std::time::SystemTime::now();
            let mut trajectory = state::Trajectory::new(&arn, "traj-full", now);
            trajectory.turns = (0..state::MAX_TURNS_PER_TRAJECTORY)
                .map(|index| state::Turn {
                    request_body: b"{}".to_vec(),
                    request_content_type: Some("application/json".to_string()),
                    response_body: b"{}".to_vec(),
                    response_content_type: Some(state::JSON_CONTENT_TYPE.to_string()),
                    streamed: false,
                    index,
                    at: now,
                })
                .collect();
            guard
                .trajectories
                .insert((arn.clone(), "traj-full".to_string()), trajectory);
        }

        let reply = sample(&router, &arn, "traj-full", &chat_payload("one more"), streamed).await;
        assert_eq!(
            reply.status,
            402,
            "case streamed={streamed}: expected 402 at the turn bound, got {} / {}",
            reply.status,
            reply.text()
        );
        assert_eq!(
            reply.error_code(),
            "ServiceQuotaExceededException",
            "case streamed={streamed}: expected ServiceQuotaExceededException, got {:?}",
            reply.error_type
        );
        let store = engine.state::<ServiceState>();
        let guard = store.lock();
        let turns = guard
            .trajectory(&arn, "traj-full")
            .map_or(0, |trajectory| trajectory.turns.len());
        assert_eq!(
            turns,
            state::MAX_TURNS_PER_TRAJECTORY,
            "case streamed={streamed}: expected the refused turn not to be captured, got {turns}"
        );
    }
}
