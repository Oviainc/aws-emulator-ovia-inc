// SPDX-License-Identifier: CC0-1.0

//! EVERY operation of this data plane resolves the `JobArn` it carries, not just the
//! sampling ones.
//!
//! RULE: a data-plane operation that names its parent resource only by an identifier in
//! the request resolves that identifier against the resource the control plane created
//! before it reads or writes any child of it, so a well-formed identifier that resolves to
//! nothing is refused with the operation's own modelled client error and the call changes
//! no state. Here the parent is the job (`JobArn`: `@required`, `@length` 1..256, pattern
//! `^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:job/[a-zA-Z0-9_\-]+/[a-zA-Z0-9](-*[a-zA-Z0-9]){0,62}$`
//! on all four operations — `smithy_query(query="operation",operation="Sample,SampleWithResponseStream,CompleteRollout,UpdateReward")`)
//! and the child is the trajectory: a trajectory exists only inside the job its key names,
//! so once the ARN resolves to no job, `CompleteRollout` and `UpdateReward` find no
//! trajectory of it and answer `404 ResourceNotFoundException` naming the trajectory and
//! the account, exactly as they answer for an id nothing ever sampled, while `Sample`
//! answers `400 ValidationException` naming the ARN. Neither of the two writes anything:
//! a trajectory captured while the job existed is left exactly as it was.
//!
//! The rows span what the rule generalises over rather than one request: WHY the ARN
//! resolves to nothing (never created, created then deleted, re-created under the other
//! job category), whether a trajectory was captured under that ARN first and how far
//! through its lifecycle it got, the partition, Region and account the ARN is addressed
//! through, the trajectory id's length, the number of captured turns, the `Status` asked
//! for and the number of reward values submitted.
//!
//! Every answer goes over the wire through a [`MultiServiceRouter`], and the store is read
//! afterwards the way the NEXT request would read it, so what is pinned is the emulator's
//! HTTP behaviour and that the answer came from persisted state.

use serde_json::json;
use service_com_amazonaws_sagemakerjobruntime_agenticrftruntimeservice::state;
use service_com_amazonaws_sagemakerjobruntime_agenticrftruntimeservice::ComAmazonawsSagemakerjobruntimeAgenticrftruntimeserviceState as ServiceState;
use smithy_server_runtime::MultiServiceRouter;

// The shared harness: the wire-answer reduction and the job-directory setup every suite
// of this crate drives (`tests/all/harness.rs`).
use crate::harness::{
    delete_job, job_arn, mounted_service, register_job as create_job, send, Reply, ACCOUNT,
    COMPLETE_ROLLOUT_PATH, JOB_ARN_HEADER, SAMPLE_PATH, SAMPLE_STREAM_PATH, TRAJECTORY_ID_HEADER,
    UPDATE_REWARD_PATH,
};

/// The `JobCategory` values the SageMaker control plane creates a job under.
///
/// Catalog source: `smithy_query(query="shape",shape="JobCategory",
/// model_path="services/service-com-amazonaws-sagemaker-sagemaker/smithy-model/com-amazonaws-sagemaker-sagemaker.json")`
/// — exactly `AgentRFT` and `AgentRFTEvaluation`.
const JOB_CATEGORIES: [&str; 2] = ["AgentRFT", "AgentRFTEvaluation"];

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
        "ClientToken": "tok-reward",
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

/// How far the trajectory under the row's ARN got before the job stopped resolving.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Captured {
    /// No sampling call was ever served for the pair.
    Nothing,
    /// `turns` turns were captured, leaving the trajectory in progress.
    Turns(usize),
    /// One turn was captured and `CompleteRollout` sealed the trajectory `ready`.
    Sealed,
}

/// What the job directory holds by the time the rollout operations are called.
#[derive(Debug, Clone, Copy)]
enum Directory {
    /// No job was ever created.
    Empty,
    /// The row's job was created and then deleted.
    Deleted,
    /// The row's job was created, deleted, and the same NAME re-created under the other
    /// job category — so the row's ARN still resolves to nothing.
    RecreatedUnderOtherCategory,
}

/// One row of the unresolvable-job table.
struct Row {
    case: &'static str,
    /// The job category and name the row's ARN addresses.
    category: &'static str,
    name: &'static str,
    /// The ARN every call of the row carries.
    arn: String,
    trajectory_id: String,
    captured: Captured,
    directory: Directory,
    /// The `Status` the refused `CompleteRollout` asks for.
    status: &'static str,
    /// The reward values the refused `UpdateReward` submits.
    rewards: Vec<f64>,
}

fn unresolvable_rows() -> Vec<Row> {
    // The trajectory id at its modelled 256-character maximum (`@length` 1..256).
    let at_max_id = "t".repeat(256);
    vec![
        Row {
            case: "no job was ever created and nothing was captured",
            category: JOB_CATEGORIES[0],
            name: "ghost-run",
            arn: job_arn("aws", "us-east-1", ACCOUNT, JOB_CATEGORIES[0], "ghost-run"),
            trajectory_id: "traj-1".to_string(),
            captured: Captured::Nothing,
            directory: Directory::Empty,
            status: "ready",
            rewards: vec![1.0],
        },
        Row {
            case: "one turn captured, then the job was deleted",
            category: JOB_CATEGORIES[0],
            name: "deleted-run",
            arn: job_arn(
                "aws",
                "us-east-1",
                ACCOUNT,
                JOB_CATEGORIES[0],
                "deleted-run",
            ),
            trajectory_id: "traj-1".to_string(),
            captured: Captured::Turns(1),
            directory: Directory::Deleted,
            status: "ready",
            rewards: vec![0.5],
        },
        Row {
            case: "three turns captured, then the job was deleted, sealing as failed",
            category: JOB_CATEGORIES[1],
            name: "eval-run",
            arn: job_arn(
                "aws",
                "us-east-1",
                ACCOUNT,
                JOB_CATEGORIES[1],
                "eval-run",
            ),
            trajectory_id: "rollout-42".to_string(),
            captured: Captured::Turns(3),
            directory: Directory::Deleted,
            status: "failed",
            rewards: vec![0.1, 0.2, 0.3],
        },
        Row {
            case: "the trajectory was already sealed when the job was deleted",
            category: JOB_CATEGORIES[0],
            name: "sealed-run",
            arn: job_arn("aws", "us-east-1", ACCOUNT, JOB_CATEGORIES[0], "sealed-run"),
            trajectory_id: "traj-sealed".to_string(),
            captured: Captured::Sealed,
            directory: Directory::Deleted,
            status: "ready",
            rewards: vec![1.0],
        },
        Row {
            case: "the job was deleted and the name re-created under the other category",
            category: JOB_CATEGORIES[0],
            name: "moved-run",
            arn: job_arn("aws", "us-east-1", ACCOUNT, JOB_CATEGORIES[0], "moved-run"),
            trajectory_id: "traj-1".to_string(),
            captured: Captured::Turns(1),
            directory: Directory::RecreatedUnderOtherCategory,
            status: "ready",
            rewards: vec![2.5],
        },
        Row {
            case: "addressed in another partition, Region and account, then deleted",
            category: JOB_CATEGORIES[0],
            name: "far-run",
            arn: job_arn(
                "aws-cn",
                "cn-north-1",
                "123456789012",
                JOB_CATEGORIES[0],
                "far-run",
            ),
            trajectory_id: at_max_id.clone(),
            captured: Captured::Turns(2),
            directory: Directory::Deleted,
            status: "failed",
            rewards: vec![1.0, 1.0],
        },
        Row {
            case: "a trajectory id at the modelled 256 maximum, never captured",
            category: JOB_CATEGORIES[0],
            name: "absent-run",
            arn: job_arn("aws", "us-west-2", ACCOUNT, JOB_CATEGORIES[0], "absent-run"),
            trajectory_id: at_max_id,
            captured: Captured::Nothing,
            directory: Directory::Empty,
            status: "ready",
            rewards: vec![1.0, 2.0],
        },
    ]
}

/// A snapshot of the stored trajectory, as the NEXT request would read it.
#[derive(Debug, PartialEq)]
struct Stored {
    status: &'static str,
    turns: usize,
    completion_status: Option<&'static str>,
    rewards: Option<usize>,
    sealing_token: Option<String>,
    reward_token: Option<String>,
}

/// Read the stored trajectory the way the next request would, or `None` when the pair
/// names nothing.
fn stored(
    engine: &engine::InMemoryEngine,
    job_arn: &str,
    trajectory_id: &str,
) -> Option<Stored> {
    let store = engine.state::<ServiceState>();
    let guard = store.lock();
    let trajectory = guard.trajectory(job_arn, trajectory_id)?;
    Some(Stored {
        status: trajectory.status.as_str(),
        turns: trajectory.turns.len(),
        completion_status: trajectory
            .completion_status
            .map(state::CompletionStatus::as_str),
        rewards: trajectory.rewards.as_ref().map(Vec::len),
        sealing_token: trajectory.complete_rollout_token.clone(),
        reward_token: trajectory.update_reward_token.clone(),
    })
}

/// RULE (the refused half): once the `JobArn` resolves to no job, `CompleteRollout` and
/// `UpdateReward` answer `404 ResourceNotFoundException` naming the trajectory and the
/// account — the same answer `Sample`'s refusal leaves them with — and whatever the store
/// holds under the pair is left untouched.
#[tokio::test]
async fn the_rollout_operations_refuse_a_job_arn_that_resolves_to_nothing() {
    for row in unresolvable_rows() {
        let case = row.case;
        let (engine, router) = mounted_service();
        // A row whose directory is empty never had the job created at all; every other row
        // needs it while its turns are captured.
        if !matches!(row.directory, Directory::Empty) {
            create_job(&engine, row.category, row.name, &row.arn);
        }

        // Capture what the row says existed while the job still resolved.
        match row.captured {
            Captured::Nothing => {}
            Captured::Turns(turns) => {
                for turn in 0..turns {
                    // Alternate the two sampling operations, so a row's turns are not all
                    // served by the same one.
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
                        "case {case:?}: expected the seeding sampling call to answer 200, got \
                         {} / {}",
                        served.status,
                        served.text()
                    );
                }
            }
            Captured::Sealed => {
                let served = sample(
                    &router,
                    SAMPLE_PATH,
                    &row.arn,
                    &row.trajectory_id,
                    &chat_payload("go"),
                )
                .await;
                assert_eq!(
                    served.status,
                    200,
                    "case {case:?}: expected the seeding Sample to answer 200, got {} / {}",
                    served.status,
                    served.text()
                );
                let sealed = complete_rollout(&router, &row.arn, &row.trajectory_id, "ready").await;
                assert_eq!(
                    sealed.status,
                    200,
                    "case {case:?}: expected the seeding CompleteRollout to answer 200, got {} / {}",
                    sealed.status,
                    sealed.text()
                );
            }
        }

        // Now the control plane takes the job away.
        match row.directory {
            Directory::Empty => {}
            Directory::Deleted => delete_job(&engine, row.category, row.name),
            Directory::RecreatedUnderOtherCategory => {
                delete_job(&engine, row.category, row.name);
                let other = JOB_CATEGORIES
                    .iter()
                    .find(|category| **category != row.category)
                    .expect("the model declares two job categories");
                let other_arn = job_arn("aws", "us-east-1", ACCOUNT, other, row.name);
                create_job(&engine, other, row.name, &other_arn);
            }
        }
        let before = stored(&engine, &row.arn, &row.trajectory_id);

        // The sampling path already refuses this ARN, naming it.
        let refused_sample = sample(
            &router,
            SAMPLE_PATH,
            &row.arn,
            &row.trajectory_id,
            &chat_payload("go"),
        )
        .await;
        assert_eq!(
            (refused_sample.status, refused_sample.error_code().as_str()),
            (400, "ValidationException"),
            "case {case:?}/Sample: expected 400 ValidationException for an ARN that resolves to \
             no job, got {} {:?} / {}",
            refused_sample.status,
            refused_sample.error_type,
            refused_sample.text()
        );
        assert_eq!(
            refused_sample.message(),
            format!("Could not find job with jobArn {}", row.arn),
            "case {case:?}/Sample: expected the job-not-found sentence naming the ARN, got {:?}",
            refused_sample.message()
        );

        // And so do the two rollout operations, which until now disagreed with it.
        for (operation, reply) in [
            (
                "CompleteRollout",
                complete_rollout(&router, &row.arn, &row.trajectory_id, row.status).await,
            ),
            (
                "UpdateReward",
                update_reward(&router, &row.arn, &row.trajectory_id, &row.rewards).await,
            ),
        ] {
            assert_eq!(
                (reply.status, reply.error_code().as_str()),
                (404, "ResourceNotFoundException"),
                "case {case:?}/{operation}: expected 404 ResourceNotFoundException for an ARN \
                 that resolves to no job, got {} {:?} / {}",
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
            assert_eq!(
                reply.message(),
                format!(
                    "Trajectory {} not found for account {ACCOUNT}",
                    row.trajectory_id
                ),
                "case {case:?}/{operation}: expected the trajectory-not-found sentence, got {:?}",
                reply.message()
            );
        }

        // Nothing the refused calls carried reached the store: no trajectory was created,
        // and one captured while the job existed is byte-for-byte as it was.
        let after = stored(&engine, &row.arn, &row.trajectory_id);
        assert_eq!(
            after, before,
            "case {case:?}: expected the refused calls to leave the stored trajectory \
             unchanged, got {after:?} after {before:?}"
        );
        if row.captured == Captured::Nothing {
            assert!(
                after.is_none(),
                "case {case:?}: expected the refused calls to create no trajectory, got {after:?}"
            );
        }
    }
}

/// RULE (the served half): the same calls SUCCEED while the job resolves, and succeed
/// again once the control plane re-creates it — so the refusals above are about the job
/// resolving and not about a trajectory the emulator has poisoned.
#[tokio::test]
async fn the_rollout_operations_serve_the_same_pair_while_the_job_resolves() {
    // Rows differ along the job's category, the partition, Region and account the ARN is
    // addressed through, the trajectory id's length, the turns captured and the terminal
    // status asked for.
    let long_id = "r".repeat(256);
    let rows: Vec<(&str, &str, &str, String, String, usize, &str)> = vec![
        (
            "the ARN the control plane minted, sealed ready",
            JOB_CATEGORIES[0],
            "served-run",
            job_arn("aws", "us-east-1", ACCOUNT, JOB_CATEGORIES[0], "served-run"),
            "traj-1".to_string(),
            1,
            "ready",
        ),
        (
            "the evaluation category, two turns, sealed failed",
            JOB_CATEGORIES[1],
            "served-eval",
            job_arn(
                "aws",
                "us-east-1",
                ACCOUNT,
                JOB_CATEGORIES[1],
                "served-eval",
            ),
            "traj-2".to_string(),
            2,
            "failed",
        ),
        (
            "addressed in another partition, Region and account, id at the 256 maximum",
            JOB_CATEGORIES[0],
            "served-far",
            job_arn(
                "aws-cn",
                "cn-north-1",
                "123456789012",
                JOB_CATEGORIES[0],
                "served-far",
            ),
            long_id,
            3,
            "ready",
        ),
    ];

    for (case, category, name, arn, trajectory_id, turns, status) in rows {
        let (engine, router) = mounted_service();
        create_job(&engine, category, name, &arn);
        for _ in 0..turns {
            let served = sample(
                &router,
                SAMPLE_PATH,
                &arn,
                &trajectory_id,
                &chat_payload("go"),
            )
            .await;
            assert_eq!(
                served.status,
                200,
                "case {case:?}: expected a sampling call for a created job to answer 200, got {} \
                 / {}",
                served.status,
                served.text()
            );
        }

        // While the job resolves, the rollout is sealed and the sealed status is what the
        // store holds.
        let sealed = complete_rollout(&router, &arn, &trajectory_id, status).await;
        assert_eq!(
            sealed.status,
            200,
            "case {case:?}: expected CompleteRollout to answer 200 for a created job, got {} / {}",
            sealed.status,
            sealed.text()
        );
        let after_sealing = stored(&engine, &arn, &trajectory_id);
        assert_eq!(
            after_sealing,
            Some(Stored {
                status: "complete",
                turns,
                completion_status: Some(status),
                rewards: None,
                sealing_token: Some("tok-complete".to_string()),
                reward_token: None,
            }),
            "case {case:?}: expected the sealed trajectory in state, got {after_sealing:?}"
        );

        // The control plane takes the job away: the SAME pair is now unresolvable.
        delete_job(&engine, category, name);
        let rewards: Vec<f64> = vec![1.0; turns];
        let refused = update_reward(&router, &arn, &trajectory_id, &rewards).await;
        assert_eq!(
            (refused.status, refused.error_code().as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: expected UpdateReward to be refused 404 once the job was deleted, \
             got {} {:?} / {}",
            refused.status,
            refused.error_type,
            refused.text()
        );

        // And once the control plane re-creates the job, the pair resolves again and the
        // trajectory is still the one that was captured — only `ready` accepts rewards.
        create_job(&engine, category, name, &arn);
        let rewarded = update_reward(&router, &arn, &trajectory_id, &rewards).await;
        let expected_status = if status == "ready" { 200 } else { 409 };
        assert_eq!(
            rewarded.status,
            expected_status,
            "case {case:?}: expected UpdateReward to answer {expected_status} for a trajectory \
             sealed {status:?} once the job resolves again, got {} {:?} / {}",
            rewarded.status,
            rewarded.error_type,
            rewarded.text()
        );
        let final_state = stored(&engine, &arn, &trajectory_id);
        let expected_rewards = if status == "ready" { Some(turns) } else { None };
        assert_eq!(
            final_state,
            Some(Stored {
                status: if status == "ready" {
                    "reward-received"
                } else {
                    "complete"
                },
                turns,
                completion_status: Some(status),
                rewards: expected_rewards,
                sealing_token: Some("tok-complete".to_string()),
                reward_token: if status == "ready" {
                    Some("tok-reward".to_string())
                } else {
                    None
                },
            }),
            "case {case:?}: expected the rewarded trajectory in state, got {final_state:?}"
        );
    }
}
