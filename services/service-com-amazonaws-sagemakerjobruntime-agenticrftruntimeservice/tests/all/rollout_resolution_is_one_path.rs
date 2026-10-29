// SPDX-License-Identifier: CC0-1.0

//! `CompleteRollout` and `UpdateReward` are ONE resolution of the `(JobArn, TrajectoryId)`
//! pair.
//!
//! RULE: both rollout-lifecycle operations resolve the pair they carry through the same
//! path, in the same order — the `JobArn` against the jobs the SageMaker control plane
//! created, then the pair against this service's own trajectory map — so for one store and
//! one pair they answer the same `404 ResourceNotFoundException` with the same sentence
//! naming the trajectory and the account the call is served for, write nothing when it does
//! not resolve, and are handed the one trajectory stored under that exact pair when it
//! does. What stays each operation's own is the modelled error the shared sentence is
//! reported through and what it then does to the trajectory: the two model the same errors
//! (`smithy_query(query="operation",operation="CompleteRollout,UpdateReward")` — both bind
//! `ResourceNotFoundException` 404, `ConflictException` 409 and `ValidationException` 400)
//! and both carry `JobArn` (`@required`) and `TrajectoryId` (`@required`, `@length` 1..256).
//!
//! The rows span what the rule generalises over: WHY the pair fails to resolve (no job was
//! ever created, the job resolves but the id was never sampled, a sibling id of the same
//! job was sampled instead, the job was deleted under a captured trajectory, the name was
//! re-created under the other job category), where it resolves (the minted ARN, an ARN in
//! another partition, Region and account, an id at the modelled 256-character maximum, a
//! job deleted and re-created at the same ARN), the number of captured turns, and — in the
//! keying suite — which pair of several in one store the call names.
//!
//! Every answer goes over the wire through a [`MultiServiceRouter`], and the store is read
//! afterwards the way the NEXT request would read it.

use serde_json::json;
use service_com_amazonaws_sagemakerjobruntime_agenticrftruntimeservice::state;
use service_com_amazonaws_sagemakerjobruntime_agenticrftruntimeservice::ComAmazonawsSagemakerjobruntimeAgenticrftruntimeserviceState as ServiceState;
use smithy_server_runtime::MultiServiceRouter;

// The shared harness: the wire-answer reduction and the job-directory setup every suite
// of this crate drives (`tests/all/harness.rs`).
use crate::harness::{
    create_job_at_arn, delete_job, job_arn, minted_job_arn, mounted_service, register_job, send,
    Reply, ACCOUNT, COMPLETE_ROLLOUT_PATH, JOB_ARN_HEADER, SAMPLE_PATH, SAMPLE_STREAM_PATH,
    TRAJECTORY_ID_HEADER, UPDATE_REWARD_PATH,
};

/// The `JobCategory` values the SageMaker control plane creates a job under.
///
/// Catalog source: `smithy_query(query="shape",shape="JobCategory",
/// model_path="services/service-com-amazonaws-sagemaker-sagemaker/smithy-model/com-amazonaws-sagemaker-sagemaker.json")`
/// — exactly `AgentRFT` and `AgentRFTEvaluation`.
const JOB_CATEGORIES: [&str; 2] = ["AgentRFT", "AgentRFTEvaluation"];

/// One sampling call, which is what captures a turn and creates the trajectory.
async fn sample(router: &MultiServiceRouter, path: &str, arn: &str, trajectory_id: &str) -> Reply {
    send(
        router,
        path,
        &[
            (JOB_ARN_HEADER, arn),
            (TRAJECTORY_ID_HEADER, trajectory_id),
            ("content-type", "application/json"),
        ],
        serde_json::to_vec(&json!({ "messages": [{ "role": "user", "content": "go" }] }))
            .expect("a serializable payload"),
    )
    .await
}

/// One `CompleteRollout` call.
async fn complete_rollout(
    router: &MultiServiceRouter,
    arn: &str,
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
        &[(JOB_ARN_HEADER, arn), ("content-type", "application/json")],
        serde_json::to_vec(&body).expect("a serializable body"),
    )
    .await
}

/// One `UpdateReward` call.
async fn update_reward(
    router: &MultiServiceRouter,
    arn: &str,
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
        &[(JOB_ARN_HEADER, arn), ("content-type", "application/json")],
        serde_json::to_vec(&body).expect("a serializable body"),
    )
    .await
}

/// A snapshot of what the store holds under one pair, as the NEXT request would read it.
#[derive(Debug, PartialEq)]
struct Stored {
    status: &'static str,
    turns: usize,
    completion_status: Option<&'static str>,
    rewards: Option<Vec<f64>>,
    sealing_token: Option<String>,
    reward_token: Option<String>,
}

/// Read the stored trajectory the way the next request would, or `None` when the pair names
/// nothing.
fn stored(engine: &engine::InMemoryEngine, arn: &str, trajectory_id: &str) -> Option<Stored> {
    let store = engine.state::<ServiceState>();
    let guard = store.lock();
    let trajectory = guard.trajectory(arn, trajectory_id)?;
    Some(Stored {
        status: trajectory.status.as_str(),
        turns: trajectory.turns.len(),
        completion_status: trajectory
            .completion_status
            .map(state::CompletionStatus::as_str),
        rewards: trajectory.rewards.clone(),
        sealing_token: trajectory.complete_rollout_token.clone(),
        reward_token: trajectory.update_reward_token.clone(),
    })
}

/// What the control plane's job directory holds by the time the rollout calls are made.
#[derive(Debug, Clone, Copy)]
enum Directory {
    /// The job was never created.
    NeverCreated,
    /// The job was created and is still there.
    Created,
    /// The job was created and then deleted.
    Deleted,
    /// The job was created, deleted, and re-created at the SAME ARN.
    RecreatedAtSameArn,
    /// The job was created, deleted, and the same NAME re-created under the other job
    /// category, so the row's ARN still resolves to nothing.
    RecreatedUnderOtherCategory,
}

/// What the shared resolution owes a row's pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expected {
    /// The pair resolves to no trajectory, so both operations answer the shared 404.
    NotFound,
    /// The pair resolves, so each operation gets past the shared resolution and answers on
    /// its own rules — never a 404.
    Resolves,
}

/// One row of the resolution table.
struct Row {
    case: &'static str,
    category: &'static str,
    name: &'static str,
    /// The ARN every call of the row carries.
    arn: String,
    /// The id the calls under test carry.
    trajectory_id: String,
    /// The id a sampling call captured turns under while the job resolved, which a row
    /// deliberately sets to another id to pin that the PAIR is the key.
    sampled_id: Option<String>,
    /// How many turns were captured under `sampled_id`.
    turns: usize,
    directory: Directory,
    expected: Expected,
}

fn rows() -> Vec<Row> {
    // The trajectory id at its modelled 256-character maximum (`@length` 1..256).
    let at_max_id = "t".repeat(256);
    vec![
        Row {
            case: "no job was ever created and nothing was sampled",
            category: JOB_CATEGORIES[0],
            name: "ghost-run",
            arn: minted_job_arn(JOB_CATEGORIES[0], "ghost-run"),
            trajectory_id: "traj-1".to_string(),
            sampled_id: None,
            turns: 0,
            directory: Directory::NeverCreated,
            expected: Expected::NotFound,
        },
        Row {
            case: "the job resolves but the id was never sampled",
            category: JOB_CATEGORIES[0],
            name: "empty-run",
            arn: minted_job_arn(JOB_CATEGORIES[0], "empty-run"),
            trajectory_id: "traj-1".to_string(),
            sampled_id: None,
            turns: 0,
            directory: Directory::Created,
            expected: Expected::NotFound,
        },
        Row {
            case: "a sibling id of the same job was sampled instead",
            category: JOB_CATEGORIES[1],
            name: "sibling-run",
            arn: minted_job_arn(JOB_CATEGORIES[1], "sibling-run"),
            trajectory_id: "traj-asked".to_string(),
            sampled_id: Some("traj-other".to_string()),
            turns: 2,
            directory: Directory::Created,
            expected: Expected::NotFound,
        },
        Row {
            case: "three turns captured, then the job was deleted",
            category: JOB_CATEGORIES[0],
            name: "deleted-run",
            arn: minted_job_arn(JOB_CATEGORIES[0], "deleted-run"),
            trajectory_id: "traj-1".to_string(),
            sampled_id: Some("traj-1".to_string()),
            turns: 3,
            directory: Directory::Deleted,
            expected: Expected::NotFound,
        },
        Row {
            case: "the name was re-created under the other job category",
            category: JOB_CATEGORIES[0],
            name: "moved-run",
            arn: minted_job_arn(JOB_CATEGORIES[0], "moved-run"),
            trajectory_id: "traj-1".to_string(),
            sampled_id: Some("traj-1".to_string()),
            turns: 1,
            directory: Directory::RecreatedUnderOtherCategory,
            expected: Expected::NotFound,
        },
        Row {
            case: "an ARN in another partition, Region and account, then deleted",
            category: JOB_CATEGORIES[0],
            name: "far-gone-run",
            arn: job_arn(
                "aws-cn",
                "cn-north-1",
                "123456789012",
                JOB_CATEGORIES[0],
                "far-gone-run",
            ),
            trajectory_id: at_max_id.clone(),
            sampled_id: Some(at_max_id.clone()),
            turns: 1,
            directory: Directory::Deleted,
            expected: Expected::NotFound,
        },
        Row {
            case: "the minted ARN with one captured turn",
            category: JOB_CATEGORIES[0],
            name: "served-run",
            arn: minted_job_arn(JOB_CATEGORIES[0], "served-run"),
            trajectory_id: "traj-1".to_string(),
            sampled_id: Some("traj-1".to_string()),
            turns: 1,
            directory: Directory::Created,
            expected: Expected::Resolves,
        },
        Row {
            case: "an ARN in another partition, Region and account, id at the 256 maximum",
            category: JOB_CATEGORIES[1],
            name: "served-far",
            arn: job_arn(
                "aws-us-gov",
                "us-gov-west-1",
                "210987654321",
                JOB_CATEGORIES[1],
                "served-far",
            ),
            trajectory_id: at_max_id.clone(),
            sampled_id: Some(at_max_id),
            turns: 2,
            directory: Directory::Created,
            expected: Expected::Resolves,
        },
        Row {
            case: "the job was deleted and re-created at the same ARN",
            category: JOB_CATEGORIES[0],
            name: "back-run",
            arn: minted_job_arn(JOB_CATEGORIES[0], "back-run"),
            trajectory_id: "t".to_string(),
            sampled_id: Some("t".to_string()),
            turns: 4,
            directory: Directory::RecreatedAtSameArn,
            expected: Expected::Resolves,
        },
    ]
}

/// Put the emulator in the row's state: create the job (unless the row never had one),
/// capture its turns, then let the control plane do whatever the row says it did.
async fn apply_setup(engine: &engine::InMemoryEngine, router: &MultiServiceRouter, row: &Row) {
    if !matches!(row.directory, Directory::NeverCreated) {
        register_job(engine, row.category, row.name, &row.arn);
    }
    if let Some(sampled_id) = &row.sampled_id {
        for turn in 0..row.turns {
            // Alternate the two sampling operations, so a row's turns are not all served
            // by the same one.
            let path = if turn % 2 == 0 {
                SAMPLE_PATH
            } else {
                SAMPLE_STREAM_PATH
            };
            let served = sample(router, path, &row.arn, sampled_id).await;
            assert_eq!(
                served.status,
                200,
                "case {:?}: expected the seeding sampling call to answer 200, got {} / {}",
                row.case,
                served.status,
                served.text()
            );
        }
    }
    match row.directory {
        Directory::NeverCreated | Directory::Created => {}
        Directory::Deleted => delete_job(engine, row.category, row.name),
        Directory::RecreatedAtSameArn => {
            delete_job(engine, row.category, row.name);
            register_job(engine, row.category, row.name, &row.arn);
        }
        Directory::RecreatedUnderOtherCategory => {
            delete_job(engine, row.category, row.name);
            let other = JOB_CATEGORIES
                .iter()
                .find(|category| **category != row.category)
                .expect("the model declares two job categories");
            register_job(
                engine,
                other,
                row.name,
                &minted_job_arn(other, row.name),
            );
        }
    }
}

/// RULE: for one store and one pair, `CompleteRollout` and `UpdateReward` answer the same
/// `404 ResourceNotFoundException` with the same sentence when the pair resolves to no
/// trajectory, and neither writes anything; where it resolves, neither answers 404 and each
/// goes on to its own rules.
#[tokio::test]
async fn both_rollout_paths_resolve_one_pair_the_same_way() {
    for row in rows() {
        let case = row.case;
        // The rewards a refused submission carries: one per captured turn, so where the pair
        // resolves the row is not refused for its cardinality, and never empty, which
        // `UpdateReward` refuses on its own before any resolution.
        let rewards: Vec<f64> = vec![1.0; row.turns.max(1)];
        let mut answers: Vec<(&str, Reply)> = Vec::new();
        for operation in ["CompleteRollout", "UpdateReward"] {
            // Each operation is driven on its OWN engine, so what it answers is its own and
            // not a leftover of the other's write.
            let (engine, router) = mounted_service();
            apply_setup(&engine, &router, &row).await;
            let before = stored(&engine, &row.arn, &row.trajectory_id);

            let reply = if operation == "CompleteRollout" {
                complete_rollout(&router, &row.arn, &row.trajectory_id, "ready").await
            } else {
                update_reward(&router, &row.arn, &row.trajectory_id, &rewards).await
            };

            match row.expected {
                Expected::NotFound => {
                    assert_eq!(
                        (reply.status, reply.error_code().as_str()),
                        (404, "ResourceNotFoundException"),
                        "case {case:?}/{operation}: expected 404 ResourceNotFoundException for a \
                         pair that resolves to no trajectory, got {} {:?} / {}",
                        reply.status,
                        reply.error_type,
                        reply.text()
                    );
                    assert_eq!(
                        reply.message(),
                        format!(
                            "Trajectory {} not found for account {ACCOUNT}",
                            row.trajectory_id
                        ),
                        "case {case:?}/{operation}: expected the shared trajectory-not-found \
                         sentence, got {:?}",
                        reply.message()
                    );
                    let after = stored(&engine, &row.arn, &row.trajectory_id);
                    assert_eq!(
                        after, before,
                        "case {case:?}/{operation}: expected a refused call to write nothing, got \
                         {after:?} after {before:?}"
                    );
                }
                Expected::Resolves => {
                    assert_ne!(
                        reply.status, 404,
                        "case {case:?}/{operation}: expected a resolving pair not to be refused \
                         404, got {} {:?} / {}",
                        reply.status,
                        reply.error_type,
                        reply.text()
                    );
                    assert_ne!(
                        reply.error_code().as_str(),
                        "ResourceNotFoundException",
                        "case {case:?}/{operation}: expected a resolving pair not to be answered \
                         with the not-found error, got {:?} / {}",
                        reply.error_type,
                        reply.text()
                    );
                    // Past the shared resolution, each operation answers on its OWN rules:
                    // `CompleteRollout` seals the in-progress trajectory, while
                    // `UpdateReward` refuses one that was never sealed.
                    let (expected_status, expected_message) = if operation == "CompleteRollout" {
                        (200, String::new())
                    } else {
                        (
                            409,
                            format!(
                                "Trajectory {} is in in-progress status; call CompleteRollout \
                                 before UpdateReward.",
                                row.trajectory_id
                            ),
                        )
                    };
                    assert_eq!(
                        reply.status,
                        expected_status,
                        "case {case:?}/{operation}: expected {expected_status} once the pair \
                         resolves, got {} {:?} / {}",
                        reply.status,
                        reply.error_type,
                        reply.text()
                    );
                    assert_eq!(
                        reply.message(),
                        expected_message,
                        "case {case:?}/{operation}: expected the operation's own sentence \
                         {expected_message:?}, got {:?}",
                        reply.message()
                    );
                }
            }
            answers.push((operation, reply));
        }

        // Whatever the row's state, the two operations agree on whether the pair resolved
        // and on the sentence the shared resolution refuses it with.
        let (first_name, first) = &answers[0];
        let (second_name, second) = &answers[1];
        let refused = row.expected == Expected::NotFound;
        assert_eq!(
            (first.status == 404, second.status == 404),
            (refused, refused),
            "case {case:?}: expected {first_name} and {second_name} to agree on whether the pair \
             resolves, got {} and {}",
            first.status,
            second.status
        );
        if refused {
            assert_eq!(
                (first.error_code(), first.message()),
                (second.error_code(), second.message()),
                "case {case:?}: expected {first_name} and {second_name} to refuse the pair \
                 identically, got {:?}/{:?} and {:?}/{:?}",
                first.error_code(),
                first.message(),
                second.error_code(),
                second.message()
            );
        }
    }
}

/// One pair in a store that holds several, and how many turns it captured.
struct Pair {
    arn: String,
    trajectory_id: &'static str,
    turns: usize,
}

/// RULE: the trajectory the shared resolution hands an operation is the one stored under the
/// EXACT pair the call carries, so a seal by `CompleteRollout` and a reward by
/// `UpdateReward` land on that pair and leave every other pair of the store as it was.
#[tokio::test]
async fn the_shared_resolution_keys_each_rollout_call_on_its_own_pair() {
    // Rows differ along what else the store holds — the same id under another job, another
    // id under the same job, several of both — and along which of them the calls name.
    let rows: Vec<(&str, Vec<Pair>, usize)> = vec![
        (
            "the same trajectory id under two different jobs",
            vec![
                Pair {
                    arn: minted_job_arn(JOB_CATEGORIES[0], "keyed-run-a"),
                    trajectory_id: "shared-id",
                    turns: 1,
                },
                Pair {
                    arn: minted_job_arn(JOB_CATEGORIES[0], "keyed-run-b"),
                    trajectory_id: "shared-id",
                    turns: 2,
                },
            ],
            0,
        ),
        (
            "two trajectory ids under one job, the second one named",
            vec![
                Pair {
                    arn: minted_job_arn(JOB_CATEGORIES[1], "keyed-eval"),
                    trajectory_id: "traj-1",
                    turns: 3,
                },
                Pair {
                    arn: minted_job_arn(JOB_CATEGORIES[1], "keyed-eval"),
                    trajectory_id: "traj-2",
                    turns: 1,
                },
            ],
            1,
        ),
        (
            "three pairs across two jobs and two partitions, the middle one named",
            vec![
                Pair {
                    arn: minted_job_arn(JOB_CATEGORIES[0], "keyed-run-c"),
                    trajectory_id: "traj-1",
                    turns: 2,
                },
                Pair {
                    arn: job_arn(
                        "aws-cn",
                        "cn-north-1",
                        "123456789012",
                        JOB_CATEGORIES[0],
                        "keyed-run-d",
                    ),
                    trajectory_id: "traj-1",
                    turns: 1,
                },
                Pair {
                    arn: minted_job_arn(JOB_CATEGORIES[0], "keyed-run-c"),
                    trajectory_id: "traj-2",
                    turns: 4,
                },
            ],
            1,
        ),
    ];

    for (case, pairs, target) in rows {
        let (engine, router) = mounted_service();
        for pair in &pairs {
            // The job each pair is addressed through, created at the ARN the row states.
            create_job_at_arn(&engine, &pair.arn);
            for _ in 0..pair.turns {
                let served = sample(&router, SAMPLE_PATH, &pair.arn, pair.trajectory_id).await;
                assert_eq!(
                    served.status,
                    200,
                    "case {case:?}: expected the seeding sampling call to answer 200, got {} / {}",
                    served.status,
                    served.text()
                );
            }
        }

        let named = &pairs[target];
        let untouched: Vec<Option<Stored>> = pairs
            .iter()
            .enumerate()
            .map(|(index, pair)| {
                if index == target {
                    None
                } else {
                    stored(&engine, &pair.arn, pair.trajectory_id)
                }
            })
            .collect();

        // `CompleteRollout` seals the named pair, and the store holds the seal under that
        // pair when the next request reads it.
        let sealed = complete_rollout(&router, &named.arn, named.trajectory_id, "ready").await;
        assert_eq!(
            sealed.status,
            200,
            "case {case:?}: expected CompleteRollout to seal the named pair, got {} / {}",
            sealed.status,
            sealed.text()
        );
        let after_sealing = stored(&engine, &named.arn, named.trajectory_id);
        assert_eq!(
            after_sealing,
            Some(Stored {
                status: "complete",
                turns: named.turns,
                completion_status: Some("ready"),
                rewards: None,
                sealing_token: Some("tok-complete".to_string()),
                reward_token: None,
            }),
            "case {case:?}: expected the sealed trajectory under the named pair, got \
             {after_sealing:?}"
        );

        // `UpdateReward`, reaching the SAME trajectory through the same resolution, records
        // the scores on it.
        let rewards: Vec<f64> = (0..named.turns)
            .map(|index| 0.5 + f64::from(u32::try_from(index).expect("a small turn count")))
            .collect();
        let rewarded = update_reward(&router, &named.arn, named.trajectory_id, &rewards).await;
        assert_eq!(
            rewarded.status,
            200,
            "case {case:?}: expected UpdateReward to record the scores on the named pair, got {} \
             / {}",
            rewarded.status,
            rewarded.text()
        );
        let after_reward = stored(&engine, &named.arn, named.trajectory_id);
        assert_eq!(
            after_reward,
            Some(Stored {
                status: "reward-received",
                turns: named.turns,
                completion_status: Some("ready"),
                rewards: Some(rewards),
                sealing_token: Some("tok-complete".to_string()),
                reward_token: Some("tok-reward".to_string()),
            }),
            "case {case:?}: expected the rewarded trajectory under the named pair, got \
             {after_reward:?}"
        );

        // Every other pair of the store is exactly as it was before either call.
        for (index, pair) in pairs.iter().enumerate() {
            if index == target {
                continue;
            }
            let now = stored(&engine, &pair.arn, pair.trajectory_id);
            assert_eq!(
                now, untouched[index],
                "case {case:?}: expected the pair ({}, {}) to be left as it was, got {now:?} \
                 after {:?}",
                pair.arn, pair.trajectory_id, untouched[index]
            );
        }
    }
}
