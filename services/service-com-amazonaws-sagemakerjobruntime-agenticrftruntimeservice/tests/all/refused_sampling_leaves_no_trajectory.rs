// SPDX-License-Identifier: CC0-1.0

//! A sampling call that is REFUSED leaves nothing behind for the next caller.
//!
//! RULE: a request a service refuses writes no state, so a resource that call would have
//! created is still absent to every later caller, which answers for its absence with its
//! own modelled not-found error. Here that is `Sample` and `SampleWithResponseStream`:
//! whichever validation stage refuses the call — the `JobArn` `@pattern`
//! (`^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:job/[a-zA-Z0-9_\-]+/[a-zA-Z0-9](-*[a-zA-Z0-9]){0,62}$`),
//! the resolution of that ARN against the job the SageMaker control plane created, the
//! inference payload, or the emulator's retained-size bound — no turn is captured, so the
//! `(JobArn, TrajectoryId)` pair names no trajectory and the `CompleteRollout` and
//! `UpdateReward` that follow it answer `404 ResourceNotFoundException` naming the
//! trajectory and the account, never `200` and never a conflict with state a refused call
//! would have written.
//!
//! Member bounds, the operations' URIs and their modelled errors come from the model:
//! `smithy_query(query="operation",operation="Sample,SampleWithResponseStream,CompleteRollout,UpdateReward")`.
//!
//! The rows span what the rule generalises over rather than one request: WHICH stage
//! refuses the sampling call (ARN syntax, job resolution, the payload, the retained-size
//! bound), WHY the job resolves to nothing (never created, created then deleted, held
//! under the other category), and which of the two sampling operations serves the call.
//! Every answer goes over the wire through a [`MultiServiceRouter`], and the store is read
//! afterwards the way the NEXT request would read it.

use serde_json::json;
use service_com_amazonaws_sagemakerjobruntime_agenticrftruntimeservice::state;
use service_com_amazonaws_sagemakerjobruntime_agenticrftruntimeservice::ComAmazonawsSagemakerjobruntimeAgenticrftruntimeserviceState as ServiceState;
use smithy_server_runtime::MultiServiceRouter;

// The shared harness: the wire-answer reduction and the job-directory setup every suite
// of this crate drives (`tests/all/harness.rs`). A row of this suite addresses a job by
// the ARN the control plane minted for it, so the harness's `minted_job_arn` is this
// suite's `job_arn`.
use crate::harness::{
    create_job, delete_job, minted_job_arn as job_arn, mounted_service, send, Reply, ACCOUNT,
    COMPLETE_ROLLOUT_PATH, JOB_ARN_HEADER, SAMPLE_PATH, SAMPLE_STREAM_PATH, TRAJECTORY_ID_HEADER,
    UPDATE_REWARD_PATH,
};

/// One `Sample` or `SampleWithResponseStream` call.
async fn sample(
    router: &MultiServiceRouter,
    path: &str,
    job_arn: &str,
    trajectory_id: &str,
    payload: &[u8],
) -> Reply {
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

/// One `CompleteRollout` call.
async fn complete_rollout(
    router: &MultiServiceRouter,
    job_arn: &str,
    trajectory_id: &str,
    status: &str,
) -> Reply {
    let body = json!({
        "TrajectoryId": trajectory_id,
        "Status": status,
        "ClientToken": "tok-complete",
    });
    send(
        router,
        COMPLETE_ROLLOUT_PATH,
        &[(JOB_ARN_HEADER, job_arn), ("content-type", "application/json")],
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
) -> Reply {
    let body = json!({
        "TrajectoryId": trajectory_id,
        "Rewards": rewards,
        "ClientToken": "tok-reward",
    });
    send(
        router,
        UPDATE_REWARD_PATH,
        &[(JOB_ARN_HEADER, job_arn), ("content-type", "application/json")],
        serde_json::to_vec(&body).expect("a serializable body"),
    )
    .await
}

/// An OpenAI-compatible chat-completions request body.
fn chat_payload(text: &str) -> Vec<u8> {
    serde_json::to_vec(&json!({ "messages": [{ "role": "user", "content": text }] }))
        .expect("a serializable payload")
}

/// What the job directory holds before the refused call.
#[derive(Debug, Clone, Copy)]
enum Directory {
    /// No job was ever created.
    Empty,
    /// `category` / `name` was created.
    Created(&'static str, &'static str),
    /// `category` / `name` was created and then deleted.
    Deleted(&'static str, &'static str),
}

/// How far a later call carrying the SAME `JobArn` gets, which decides which refusal it
/// answers with: a `JobArn` the `@pattern` declines is refused at the ARN stage by every
/// operation, while a well-formed one reaches trajectory resolution and finds nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    Malformed,
    WellFormed,
}

/// RULE: whichever stage refuses a sampling call, the call captures no turn, so the
/// `CompleteRollout` and `UpdateReward` that follow it on the same pair are refused too —
/// `404 ResourceNotFoundException` naming the trajectory and the account whenever the ARN
/// itself is well formed.
#[tokio::test]
async fn a_refused_sampling_call_leaves_no_trajectory_for_the_rollout_operations() {
    // One body past the per-turn retained bound, which the emulator refuses rather than
    // capture (`state::MAX_TURN_BODY_BYTES`).
    let oversized = vec![b'x'; state::MAX_TURN_BODY_BYTES + 1];
    let chat = chat_payload("go");

    // Rows differ along the stage that refuses the sampling call and, for the resolution
    // stage, along why the ARN resolves to nothing.
    let rows: Vec<(&str, Directory, String, &[u8], Stage)> = vec![
        (
            "a job that was never created",
            Directory::Empty,
            job_arn("AgentRFT", "ghost-run"),
            &chat,
            Stage::WellFormed,
        ),
        (
            "a job created and then deleted",
            Directory::Deleted("AgentRFT", "finished-run"),
            job_arn("AgentRFT", "finished-run"),
            &chat,
            Stage::WellFormed,
        ),
        (
            "a name held under the other job category",
            Directory::Created("AgentRFTEvaluation", "rollout-1"),
            job_arn("AgentRFT", "rollout-1"),
            &chat,
            Stage::WellFormed,
        ),
        (
            "a JobArn that is not an ARN at all",
            Directory::Empty,
            "not-a-valid-job-arn".to_string(),
            &chat,
            Stage::Malformed,
        ),
        (
            "an ARN-shaped JobArn with too short an account id",
            Directory::Empty,
            "arn:aws:sagemaker:us-east-1:12345:job/AgentRFT/rollout-1".to_string(),
            &chat,
            Stage::Malformed,
        ),
        (
            "an empty inference payload for a job that exists",
            Directory::Created("AgentRFT", "empty-run"),
            job_arn("AgentRFT", "empty-run"),
            b"",
            Stage::WellFormed,
        ),
        (
            "a payload past the per-turn retained bound",
            Directory::Created("AgentRFT", "big-run"),
            job_arn("AgentRFT", "big-run"),
            &oversized,
            Stage::WellFormed,
        ),
    ];

    for (case, directory, arn, payload, stage) in rows {
        for path in [SAMPLE_PATH, SAMPLE_STREAM_PATH] {
            let case = format!("{case} on {path}");
            let (engine, router) = mounted_service();
            match directory {
                Directory::Empty => {}
                Directory::Created(category, name) => create_job(&engine, category, name),
                Directory::Deleted(category, name) => {
                    create_job(&engine, category, name);
                    delete_job(&engine, category, name);
                }
            }

            let refused = sample(&router, path, &arn, "traj-1", payload).await;
            assert_eq!(
                (refused.status, refused.error_code().as_str()),
                (400, "ValidationException"),
                "case {case:?}: expected the sampling call to be refused 400 \
                 ValidationException, got {} {:?} / {}",
                refused.status,
                refused.error_type,
                refused.text()
            );

            // Nothing was captured: the pair names no trajectory in the store the next
            // request reads.
            let held = engine.state::<ServiceState>().lock().trajectories.len();
            assert_eq!(
                held, 0,
                "case {case:?}: expected a refused sampling call to capture no turn, got \
                 {held} trajectory/ies"
            );

            // And nothing was captured as far as the NEXT caller can tell either.
            for (operation, reply) in [
                (
                    "CompleteRollout",
                    complete_rollout(&router, &arn, "traj-1", "ready").await,
                ),
                (
                    "UpdateReward",
                    update_reward(&router, &arn, "traj-1", &[1.0]).await,
                ),
            ] {
                let expected = match stage {
                    Stage::WellFormed => (404, "ResourceNotFoundException"),
                    Stage::Malformed => (400, "ValidationException"),
                };
                assert_eq!(
                    (reply.status, reply.error_code().as_str()),
                    expected,
                    "case {case:?}/{operation}: expected {expected:?} after a refused \
                     sampling call, got {} {:?} / {}",
                    reply.status,
                    reply.error_type,
                    reply.text()
                );
                assert_eq!(
                    reply.content_type.as_deref(),
                    Some("application/json"),
                    "case {case:?}/{operation}: expected a restJson1 error document, got {:?}",
                    reply.content_type
                );
                let expected_message = match stage {
                    Stage::WellFormed => {
                        format!("Trajectory traj-1 not found for account {ACCOUNT}")
                    }
                    Stage::Malformed => format!("Input arn {arn} is malformed"),
                };
                assert_eq!(
                    reply.message(),
                    expected_message,
                    "case {case:?}/{operation}: expected the message {expected_message:?}, got \
                     {:?}",
                    reply.message()
                );
            }

            let held = engine.state::<ServiceState>().lock().trajectories.len();
            assert_eq!(
                held, 0,
                "case {case:?}: expected the refused rollout calls to create nothing either, \
                 got {held} trajectory/ies"
            );
        }
    }
}

/// RULE (the served half): the same chain SUCCEEDS for a job the control plane created, so
/// the refusals above are about the sampling call writing nothing and not about a chain
/// that never works.
#[tokio::test]
async fn the_same_chain_succeeds_once_the_job_exists() {
    // Rows differ along which sampling operation captures the turn.
    for (case, path) in [
        ("captured by Sample", SAMPLE_PATH),
        ("captured by SampleWithResponseStream", SAMPLE_STREAM_PATH),
    ] {
        let (engine, router) = mounted_service();
        create_job(&engine, "AgentRFT", "served-run");
        let arn = job_arn("AgentRFT", "served-run");

        for (operation, reply) in [
            (
                "Sample",
                sample(&router, path, &arn, "traj-1", &chat_payload("go")).await,
            ),
            (
                "CompleteRollout",
                complete_rollout(&router, &arn, "traj-1", "ready").await,
            ),
            (
                "UpdateReward",
                update_reward(&router, &arn, "traj-1", &[0.5]).await,
            ),
        ] {
            assert_eq!(
                reply.status,
                200,
                "case {case:?}/{operation}: expected 200 for a job the control plane created, \
                 got {} {:?} / {}",
                reply.status,
                reply.error_type,
                reply.text()
            );
        }

        // The chain's answers came from persisted state: one captured turn, the rewards
        // recorded, and the trajectory in reward-received status.
        let store = engine.state::<ServiceState>();
        let guard = store.lock();
        let trajectory = guard
            .trajectory(&arn, "traj-1")
            .unwrap_or_else(|| panic!("case {case:?}: expected the trajectory to be held"));
        assert_eq!(
            (
                trajectory.turns.len(),
                trajectory.status,
                trajectory.rewards.clone()
            ),
            (
                1,
                state::TrajectoryStatus::RewardReceived,
                Some(vec![0.5])
            ),
            "case {case:?}: expected one captured turn, reward-received status and the \
             recorded rewards, got {:?} turn(s), {:?}, {:?}",
            trajectory.turns.len(),
            trajectory.status,
            trajectory.rewards
        );
    }
}

/// RULE (distinct stages, distinct sentences): the ARN-syntax refusal and the
/// job-resolution refusal are different stages, so a caller can tell a mis-spelled ARN
/// from an ARN naming no job, on both sampling operations.
#[tokio::test]
async fn the_syntax_and_resolution_refusals_carry_distinct_messages() {
    let malformed = "not-a-valid-job-arn";
    let unresolvable = job_arn("AgentRFT", "ghost-run");

    for path in [SAMPLE_PATH, SAMPLE_STREAM_PATH] {
        let (_engine, router) = mounted_service();
        let syntax = sample(&router, path, malformed, "traj-1", &chat_payload("go")).await;
        let resolution = sample(&router, path, &unresolvable, "traj-1", &chat_payload("go")).await;

        assert_eq!(
            syntax.message(),
            format!("Input arn {malformed} is malformed"),
            "case \"the ARN-syntax stage on {path}\": got {:?}",
            syntax.message()
        );
        assert_eq!(
            resolution.message(),
            format!("Could not find job with jobArn {unresolvable}"),
            "case \"the job-resolution stage on {path}\": got {:?}",
            resolution.message()
        );
        assert_ne!(
            syntax.message(),
            resolution.message(),
            "case \"the two stages on {path}\": expected distinct sentences, got {:?} twice",
            syntax.message()
        );
    }
}
