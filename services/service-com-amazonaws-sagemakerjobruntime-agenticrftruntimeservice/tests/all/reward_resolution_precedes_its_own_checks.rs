// SPDX-License-Identifier: CC0-1.0

//! `UpdateReward` resolves the job BEFORE any check or write about the trajectory.
//!
//! RULE: an operation that mutates a child resource named by a parent identifier in the
//! request resolves that parent against the resources the control plane created before it
//! inspects or writes the child, so while the parent identifier resolves to nothing the
//! call is refused with the operation's own not-found error — whatever state the child is
//! in and whatever the request would otherwise be answered with — and the call changes
//! nothing; once the parent resolves again, the same request is answered by the child's own
//! state. Here the parent is the job (`JobArn`: `@required`, `@length` 1..256, pattern
//! `^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:job/[a-zA-Z0-9_\-]+/[a-zA-Z0-9](-*[a-zA-Z0-9]){0,62}$`
//! — `smithy_query(query="operation",operation="UpdateReward")`), which this model binds no
//! operation to create, list or delete, so the shared directory the SageMaker control plane
//! writes is the only place an ARN resolves; the child is the trajectory, which lives only
//! inside that job, so an unresolvable ARN is answered `404 ResourceNotFoundException` with
//! the sentence an id nothing ever sampled gets.
//!
//! What this suite pins that the job-resolution suites do not: the resolution comes before
//! EVERY answer this operation owes from the trajectory's own state — the conflict of an
//! unsealed or `failed` trajectory, the conflict of scores already recorded, the per-turn
//! cardinality refusal, and the successful recording itself, including the idempotent
//! replay of an identical submission. Each row drives ONE request twice: once while the ARN
//! resolves to no job (it must be the not-found refusal, with the store untouched) and once
//! after the control plane re-creates the job (it must be the answer the trajectory's state
//! owes it). The one request check that precedes resolution is a submission carrying no
//! score at all, which carries nothing to record whatever the ARN resolves to; it is a row
//! too, with its own expectation, so the ordering is stated rather than assumed.
//!
//! The rows span what the rule generalises over: how far the trajectory got (never sampled,
//! sampled and unsealed, sealed `ready`, sealed `failed`, already rewarded), the number of
//! captured turns, the number and values of the submitted scores relative to those turns,
//! the job category, and the partition, Region and account the ARN is addressed through.
//!
//! Every answer goes over the wire through a [`MultiServiceRouter`], and the store is read
//! afterwards the way the NEXT request would read it, so what is pinned is the emulator's
//! HTTP behaviour and that the answer came from persisted state.

use serde_json::json;
use service_com_amazonaws_sagemakerjobruntime_agenticrftruntimeservice::state;
use service_com_amazonaws_sagemakerjobruntime_agenticrftruntimeservice::ComAmazonawsSagemakerjobruntimeAgenticrftruntimeserviceState as ServiceState;
use smithy_server_runtime::MultiServiceRouter;

// The shared harness: the wire-answer reduction and the job-directory setup every suite of
// this crate drives (`tests/all/harness.rs`).
use crate::harness::{
    delete_job, job_arn, mounted_service, register_job, send, Reply, ACCOUNT,
    COMPLETE_ROLLOUT_PATH, JOB_ARN_HEADER, SAMPLE_PATH, SAMPLE_STREAM_PATH, TRAJECTORY_ID_HEADER,
    UPDATE_REWARD_PATH,
};

/// The `JobCategory` values the SageMaker control plane creates a job under.
///
/// Catalog source: `smithy_query(query="shape",shape="JobCategory",
/// model_path="services/service-com-amazonaws-sagemaker-sagemaker/smithy-model/com-amazonaws-sagemaker-sagemaker.json")`
/// — exactly `AgentRFT` and `AgentRFTEvaluation`.
const JOB_CATEGORIES: [&str; 2] = ["AgentRFT", "AgentRFTEvaluation"];

/// The `ClientToken` the seeding `CompleteRollout` of a row carries.
const SEALING_TOKEN: &str = "tok-complete";

/// The `ClientToken` every `UpdateReward` of a row carries.
const REWARD_TOKEN: &str = "tok-reward";

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
        "ClientToken": SEALING_TOKEN,
    });
    send(
        router,
        COMPLETE_ROLLOUT_PATH,
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
) -> Reply {
    let body = json!({
        "TrajectoryId": trajectory_id,
        "Rewards": rewards,
        "ClientToken": REWARD_TOKEN,
    });
    send(
        router,
        UPDATE_REWARD_PATH,
        &[
            (JOB_ARN_HEADER, job_arn),
            ("content-type", "application/json"),
        ],
        serde_json::to_vec(&body).expect("a serializable body"),
    )
    .await
}

/// An OpenAI-compatible chat-completions request body.
fn chat_payload(text: &str) -> Vec<u8> {
    serde_json::to_vec(&json!({ "messages": [{ "role": "user", "content": text }] }))
        .expect("a serializable payload")
}

/// What one answer is asserted to be: the status, the modelled error code behind
/// `x-amzn-errortype` (empty on a success), and the `Message` sentence when the answer
/// carries one.
#[derive(Debug, Clone)]
struct Expect {
    status: u16,
    code: &'static str,
    message: Option<String>,
}

impl Expect {
    /// A successful `UpdateReward`, whose output shape carries no member.
    fn served() -> Self {
        Self {
            status: 200,
            code: "",
            message: None,
        }
    }

    /// A refusal, with the sentence it must carry.
    fn refused(status: u16, code: &'static str, message: String) -> Self {
        Self {
            status,
            code,
            message: Some(message),
        }
    }
}

/// Assert one answer against what the row expects of it.
fn assert_answer(case: &str, phase: &str, expected: &Expect, reply: &Reply) {
    assert_eq!(
        (reply.status, reply.error_code().as_str()),
        (expected.status, expected.code),
        "case {case:?} ({phase}): expected {} {:?}, got {} {:?} / {}",
        expected.status,
        expected.code,
        reply.status,
        reply.error_type,
        reply.text()
    );
    assert_eq!(
        reply.content_type.as_deref(),
        Some("application/json"),
        "case {case:?} ({phase}): expected a restJson1 document, got {:?}",
        reply.content_type
    );
    if let Some(message) = &expected.message {
        assert_eq!(
            &reply.message(),
            message,
            "case {case:?} ({phase}): expected the sentence {message:?}, got {:?}",
            reply.message()
        );
    }
}

/// A snapshot of the stored trajectory, as the NEXT request would read it: the reward
/// scores are compared by VALUE, so a refused submission that silently overwrote them with
/// a list of the same length would still be caught.
#[derive(Debug, Clone, PartialEq)]
struct Snapshot {
    status: &'static str,
    turns: usize,
    completion_status: Option<&'static str>,
    rewards: Option<Vec<f64>>,
    reward_token: Option<String>,
    sealing_token: Option<String>,
}

/// Read the stored trajectory the way the next request would, or `None` when the pair names
/// nothing.
fn snapshot(
    engine: &engine::InMemoryEngine,
    job_arn: &str,
    trajectory_id: &str,
) -> Option<Snapshot> {
    let store = engine.state::<ServiceState>();
    let guard = store.lock();
    let trajectory = guard.trajectory(job_arn, trajectory_id)?;
    Some(Snapshot {
        status: trajectory.status.as_str(),
        turns: trajectory.turns.len(),
        completion_status: trajectory
            .completion_status
            .map(state::CompletionStatus::as_str),
        rewards: trajectory.rewards.clone(),
        reward_token: trajectory.update_reward_token.clone(),
        sealing_token: trajectory.complete_rollout_token.clone(),
    })
}

/// One row: a trajectory brought to some point of its lifecycle, and ONE `UpdateReward`
/// request driven against it while the job does not resolve and again once it does.
struct Row {
    case: &'static str,
    category: &'static str,
    job_name: &'static str,
    arn: String,
    trajectory_id: String,
    /// How many turns are captured before the probe (0 = the pair is never sampled, so no
    /// trajectory exists at all).
    turns: usize,
    /// The terminal `Status` a seeding `CompleteRollout` seals with, or `None` for a
    /// trajectory left in progress.
    seal: Option<&'static str>,
    /// Scores recorded by a seeding `UpdateReward` before the probe.
    prior_rewards: Option<Vec<f64>>,
    /// The scores the probe submits, twice.
    rewards: Vec<f64>,
    /// What the probe is answered with once the job resolves again.
    resolved: Expect,
    /// What the probe is answered with while the ARN resolves to no job. Every row but the
    /// scoreless submission is the shared not-found refusal.
    unresolved: Expect,
}

/// The sentence this service reports a `(JobArn, TrajectoryId)` pair it holds no trajectory
/// for with, which is also what a pair under an unresolvable job gets.
fn not_found_sentence(trajectory_id: &str) -> String {
    format!("Trajectory {trajectory_id} not found for account {ACCOUNT}")
}

/// The sentence a submission carrying no score at all is refused with.
const EMPTY_REWARDS_SENTENCE: &str = "Rewards list cannot be empty";

/// The per-turn cardinality sentence, naming the trajectory and both counts.
fn count_sentence(trajectory_id: &str, turns: usize, rewards: usize) -> String {
    format!(
        "Rewards list must contain one reward value per turn in the trajectory; trajectory \
         {trajectory_id} has {turns} turn(s) but {rewards} reward value(s) were provided"
    )
}

fn rows() -> Vec<Row> {
    // The trajectory id at its modelled 256-character maximum (`@length` 1..256).
    let at_max_id = "t".repeat(256);
    vec![
        Row {
            case: "a first submission on a trajectory sealed ready",
            category: JOB_CATEGORIES[0],
            job_name: "served-run",
            arn: job_arn("aws", "us-east-1", ACCOUNT, JOB_CATEGORIES[0], "served-run"),
            trajectory_id: "traj-1".to_string(),
            turns: 1,
            seal: Some("ready"),
            prior_rewards: None,
            rewards: vec![0.5],
            resolved: Expect::served(),
            unresolved: Expect::refused(
                404,
                "ResourceNotFoundException",
                not_found_sentence("traj-1"),
            ),
        },
        Row {
            case: "an idempotent replay of the scores already recorded",
            category: JOB_CATEGORIES[1],
            job_name: "eval-run",
            arn: job_arn(
                "aws",
                "us-east-1",
                ACCOUNT,
                JOB_CATEGORIES[1],
                "eval-run",
            ),
            trajectory_id: at_max_id.clone(),
            turns: 3,
            seal: Some("ready"),
            prior_rewards: Some(vec![0.1, -0.2, 7.5]),
            rewards: vec![0.1, -0.2, 7.5],
            resolved: Expect::served(),
            unresolved: Expect::refused(
                404,
                "ResourceNotFoundException",
                not_found_sentence(&at_max_id),
            ),
        },
        Row {
            case: "scores that would replace the ones already recorded",
            category: JOB_CATEGORIES[0],
            job_name: "rewarded-run",
            arn: job_arn(
                "aws-cn",
                "cn-north-1",
                "123456789012",
                JOB_CATEGORIES[0],
                "rewarded-run",
            ),
            trajectory_id: "traj-replace".to_string(),
            turns: 2,
            seal: Some("ready"),
            prior_rewards: Some(vec![1.0, 1.0]),
            rewards: vec![1.0, 2.0],
            resolved: Expect::refused(
                409,
                "ConflictException",
                "Trajectory traj-replace is already in reward-received status with 2 reward \
                 value(s); they cannot be replaced with 2 value(s)."
                    .to_string(),
            ),
            unresolved: Expect::refused(
                404,
                "ResourceNotFoundException",
                not_found_sentence("traj-replace"),
            ),
        },
        Row {
            case: "a submission on a trajectory CompleteRollout has not sealed",
            category: JOB_CATEGORIES[0],
            job_name: "open-run",
            arn: job_arn("aws", "us-west-2", ACCOUNT, JOB_CATEGORIES[0], "open-run"),
            trajectory_id: "traj-open".to_string(),
            turns: 2,
            seal: None,
            prior_rewards: None,
            rewards: vec![1.0, 2.0],
            resolved: Expect::refused(
                409,
                "ConflictException",
                "Trajectory traj-open is in in-progress status; call CompleteRollout before \
                 UpdateReward."
                    .to_string(),
            ),
            unresolved: Expect::refused(
                404,
                "ResourceNotFoundException",
                not_found_sentence("traj-open"),
            ),
        },
        Row {
            case: "a submission on a trajectory sealed failed",
            category: JOB_CATEGORIES[1],
            job_name: "failed-run",
            arn: job_arn(
                "aws",
                "eu-west-1",
                "123456789012",
                JOB_CATEGORIES[1],
                "failed-run",
            ),
            trajectory_id: "traj-failed".to_string(),
            turns: 1,
            seal: Some("failed"),
            prior_rewards: None,
            rewards: vec![3.25],
            resolved: Expect::refused(
                409,
                "ConflictException",
                "Trajectory traj-failed was completed with Status failed, which is terminal and \
                 accepts no rewards."
                    .to_string(),
            ),
            unresolved: Expect::refused(
                404,
                "ResourceNotFoundException",
                not_found_sentence("traj-failed"),
            ),
        },
        Row {
            case: "fewer scores than the trajectory captured turns",
            category: JOB_CATEGORIES[0],
            job_name: "short-run",
            arn: job_arn("aws", "us-east-1", ACCOUNT, JOB_CATEGORIES[0], "short-run"),
            trajectory_id: "traj-short".to_string(),
            turns: 3,
            seal: Some("ready"),
            prior_rewards: None,
            rewards: vec![1.0, 2.0],
            resolved: Expect::refused(
                400,
                "ValidationException",
                count_sentence("traj-short", 3, 2),
            ),
            unresolved: Expect::refused(
                404,
                "ResourceNotFoundException",
                not_found_sentence("traj-short"),
            ),
        },
        Row {
            case: "more scores than the trajectory captured turns",
            category: JOB_CATEGORIES[0],
            job_name: "long-run",
            arn: job_arn("aws", "us-east-1", ACCOUNT, JOB_CATEGORIES[0], "long-run"),
            trajectory_id: "t".to_string(),
            turns: 1,
            seal: Some("ready"),
            prior_rewards: None,
            rewards: vec![1.0, 2.0, 3.0],
            resolved: Expect::refused(400, "ValidationException", count_sentence("t", 1, 3)),
            unresolved: Expect::refused(404, "ResourceNotFoundException", not_found_sentence("t")),
        },
        Row {
            case: "a pair no sampling call ever captured",
            category: JOB_CATEGORIES[0],
            job_name: "empty-run",
            arn: job_arn("aws", "us-east-1", ACCOUNT, JOB_CATEGORIES[0], "empty-run"),
            trajectory_id: "traj-absent".to_string(),
            turns: 0,
            seal: None,
            prior_rewards: None,
            rewards: vec![1.0],
            resolved: Expect::refused(
                404,
                "ResourceNotFoundException",
                not_found_sentence("traj-absent"),
            ),
            unresolved: Expect::refused(
                404,
                "ResourceNotFoundException",
                not_found_sentence("traj-absent"),
            ),
        },
        Row {
            // The ONE check this operation owns that precedes the job resolution: a
            // submission with no score in it carries nothing to record, so it is refused as
            // invalid whether or not the ARN resolves — the same ordering an empty list
            // already gets against a trajectory that does not exist.
            case: "a submission carrying no score at all",
            category: JOB_CATEGORIES[0],
            job_name: "scoreless-run",
            arn: job_arn(
                "aws",
                "us-east-1",
                ACCOUNT,
                JOB_CATEGORIES[0],
                "scoreless-run",
            ),
            trajectory_id: "traj-scoreless".to_string(),
            turns: 1,
            seal: Some("ready"),
            prior_rewards: None,
            rewards: vec![],
            resolved: Expect::refused(
                400,
                "ValidationException",
                EMPTY_REWARDS_SENTENCE.to_string(),
            ),
            unresolved: Expect::refused(
                400,
                "ValidationException",
                EMPTY_REWARDS_SENTENCE.to_string(),
            ),
        },
    ]
}

/// RULE: `UpdateReward` resolves the `JobArn` against the jobs the control plane created
/// before it inspects or writes the trajectory, so while the ARN resolves to nothing every
/// one of its answers is replaced by the not-found refusal and the store is untouched; once
/// the job resolves again the same request gets the answer the trajectory's own state owes
/// it.
#[tokio::test]
async fn update_reward_resolves_the_job_before_every_answer_it_owes() {
    for row in rows() {
        let case = row.case;
        let (engine, router) = mounted_service();
        register_job(&engine, row.category, row.job_name, &row.arn);

        // Bring the trajectory to the point of its lifecycle the row describes, alternating
        // the two sampling operations so a row's turns are not all served by one of them.
        for turn in 0..row.turns {
            let path = if turn % 2 == 0 {
                SAMPLE_PATH
            } else {
                SAMPLE_STREAM_PATH
            };
            let served = sample(
                &router,
                path,
                &row.arn,
                &row.trajectory_id,
                &chat_payload("go"),
            )
            .await;
            assert_eq!(
                served.status,
                200,
                "case {case:?} (seeding turn {turn}): expected 200, got {} / {}",
                served.status,
                served.text()
            );
        }
        if let Some(status) = row.seal {
            let sealed = complete_rollout(&router, &row.arn, &row.trajectory_id, status).await;
            assert_eq!(
                sealed.status,
                200,
                "case {case:?} (seeding CompleteRollout): expected 200, got {} / {}",
                sealed.status,
                sealed.text()
            );
        }
        if let Some(prior) = &row.prior_rewards {
            let rewarded = update_reward(&router, &row.arn, &row.trajectory_id, prior).await;
            assert_eq!(
                rewarded.status,
                200,
                "case {case:?} (seeding UpdateReward): expected 200, got {} / {}",
                rewarded.status,
                rewarded.text()
            );
        }
        let before = snapshot(&engine, &row.arn, &row.trajectory_id);

        // The control plane takes the job away: the ARN is well formed and resolves to
        // nothing.
        delete_job(&engine, row.category, row.job_name);
        let refused = update_reward(&router, &row.arn, &row.trajectory_id, &row.rewards).await;
        assert_answer(case, "job does not resolve", &row.unresolved, &refused);
        let after_refusal = snapshot(&engine, &row.arn, &row.trajectory_id);
        assert_eq!(
            after_refusal, before,
            "case {case:?}: expected the refused submission to leave the store untouched, got \
             {after_refusal:?} after {before:?}"
        );

        // The control plane re-creates it: the SAME request now reaches the trajectory and
        // is answered by its state.
        register_job(&engine, row.category, row.job_name, &row.arn);
        let answered = update_reward(&router, &row.arn, &row.trajectory_id, &row.rewards).await;
        assert_answer(case, "job resolves", &row.resolved, &answered);

        // And the store holds what that answer implies: the submitted scores when it was
        // served, and exactly what it held before when it was refused.
        let expected = if row.resolved.status == 200 {
            Some(Snapshot {
                status: "reward-received",
                turns: row.turns,
                completion_status: row.seal,
                rewards: Some(row.rewards.clone()),
                reward_token: Some(REWARD_TOKEN.to_string()),
                sealing_token: Some(SEALING_TOKEN.to_string()),
            })
        } else {
            before.clone()
        };
        let after = snapshot(&engine, &row.arn, &row.trajectory_id);
        assert_eq!(
            after, expected,
            "case {case:?}: expected the store to hold {expected:?} once the job resolved \
             again, got {after:?}"
        );
    }
}
