// SPDX-License-Identifier: CC0-1.0

//! The crate's README and `src/state.rs` state ONE job-resolution rule.
//!
//! RULE: the `JobArn` a data-plane call carries is resolved against the jobs the SageMaker
//! control plane created — the shared job directory is the only place it can be resolved,
//! because this model binds no operation that creates, lists or deletes a job — so an ARN
//! the modelled constraints admit (`@length` 1..256 and the ARN `@pattern`, which allow any
//! partition, region, account and job name) but that no create ever minted is refused by
//! EVERY operation with that operation's own modelled client error, and a job the control
//! plane did create is served however a caller addresses it. That is the rule the crate's
//! README "Job resolution" section states and the rule `src/state.rs` implements through the
//! one resolution both operation families reach (`state::job_exists`).
//!
//! The rule is written down twice — once as prose, once as code — so this suite asserts BOTH
//! statements of it: the README section is read back here and must state the resolution it
//! documents rather than deny it, and the behaviour it describes is then driven over the wire
//! through an operation of each family (`Sample` and `SampleWithResponseStream`, which the
//! prose copy covered, and `CompleteRollout` and `UpdateReward`, which it omitted). A drift
//! in either copy fails this suite.
//!
//! The rows span what the rule generalises over rather than one request: the job's category,
//! the partition, Region and account the ARN was addressed through, the job-name length, and
//! the operation the call is made on.
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
    create_job_at_arn, job_arn, mounted_service, send, Reply, COMPLETE_ROLLOUT_PATH,
    JOB_ARN_HEADER, SAMPLE_PATH, SAMPLE_STREAM_PATH, TRAJECTORY_ID_HEADER, UPDATE_REWARD_PATH,
};

/// The crate's own README, the prose statement of the rule this suite pins.
const README: &str = include_str!("../../README.md");

/// The `JobCategory` values the SageMaker control plane creates a job under.
///
/// Catalog source: `smithy_query(query="shape",shape="JobCategory",
/// model_path="services/service-com-amazonaws-sagemaker-sagemaker/smithy-model/com-amazonaws-sagemaker-sagemaker.json")`
/// — exactly `AgentRFT` and `AgentRFTEvaluation`.
const JOB_CATEGORIES: [&str; 2] = ["AgentRFT", "AgentRFTEvaluation"];

/// The README section that states the rule, from its heading to the next heading.
fn documented_rule() -> &'static str {
    let heading = "## Job resolution";
    let from = README.find(heading).unwrap_or_else(|| {
        panic!("case \"the section exists\": expected {heading:?} in the crate README")
    });
    let body = &README[from + heading.len()..];
    let to = body.find("\n## ").unwrap_or(body.len());
    &body[..to]
}

/// Which family an operation belongs to, which is what decides the modelled error its own
/// refusal of an unresolved job is reported through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Family {
    /// `Sample` / `SampleWithResponseStream`: the turn can be routed to no model session.
    Sampling,
    /// `CompleteRollout` / `UpdateReward`: the pair names no trajectory of the job.
    Rollout,
}

/// One sampling call on `path`.
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
async fn complete_rollout(router: &MultiServiceRouter, arn: &str, trajectory_id: &str) -> Reply {
    let body = json!({
        "TrajectoryId": trajectory_id,
        "Status": "ready",
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

/// One `UpdateReward` call, carrying one reward value per captured turn.
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

/// RULE (the prose copy): the README section states the resolution the code performs — the
/// shared directory the control plane writes, the one predicate this crate resolves through,
/// and the refusal each operation family answers an unresolved ARN with — and nowhere denies
/// that a job is resolved at all.
#[test]
fn the_readme_states_the_resolution_the_code_performs() {
    let section = documented_rule();

    // What the implemented rule is made of, each phrase being a part of `src/state.rs` the
    // prose must not drop: the directory an ARN is resolved against, the predicate the
    // resolution runs through, and the refusal of each operation family.
    let required: [(&str, &str); 6] = [
        ("the shared directory an ARN resolves against", "JobDirectory"),
        ("the one predicate the resolution runs through", "job_exists"),
        ("the sampling refusal's status and code", "400 ValidationException"),
        ("the sampling refusal's sentence", "job_not_found_message"),
        (
            "the rollout refusal's status and code",
            "404 ResourceNotFoundException",
        ),
        (
            "the rollout pair's resolution",
            "unresolved_job_trajectory_message",
        ),
    ];
    for (case, phrase) in required {
        assert!(
            section.contains(phrase),
            "case {case:?}: expected the README \"Job resolution\" section to state {phrase:?}, \
             got {section:?}"
        );
    }

    // The statement the section must NOT make: that this runtime resolves an ARN against
    // nothing, which is the drift this suite exists to catch.
    for (case, phrase) in [
        ("no registry at all", "no registry"),
        ("nothing to resolve against", "registry of jobs"),
    ] {
        assert!(
            !section.contains(phrase),
            "case {case:?}: expected the README \"Job resolution\" section not to deny the \
             resolution with {phrase:?}, got {section:?}"
        );
    }
}

/// RULE (the code copy, driven through an operation of each family): an ARN that resolves to
/// no created job is refused by every operation with its family's own modelled client error
/// and nothing is written, and once the control plane has created the job the whole rollout
/// lifecycle is served over that same ARN.
#[tokio::test]
async fn every_operation_resolves_the_job_the_readme_documents() {
    // The job-name segment at its 63-character maximum: `[a-zA-Z0-9](-*[a-zA-Z0-9]){0,62}`.
    let long_name = format!("r{}", "a".repeat(62));

    // Rows differ along the job's category and name and along the partition, Region and
    // account the ARN is addressed through: an emulator serving one account resolves a job
    // the control plane created however a caller addresses it.
    let rows: Vec<(&str, String)> = vec![
        (
            "the ARN the control plane minted",
            job_arn(
                "aws",
                "us-east-1",
                "000000000000",
                JOB_CATEGORIES[0],
                "documented-run",
            ),
        ),
        (
            "the evaluation category, addressed in another Region",
            job_arn(
                "aws",
                "eu-west-1",
                "000000000000",
                JOB_CATEGORIES[1],
                "documented-eval",
            ),
        ),
        (
            "another partition and account, name at its 63-character maximum",
            job_arn(
                "aws-cn",
                "cn-north-1",
                "123456789012",
                JOB_CATEGORIES[0],
                &long_name,
            ),
        ),
        (
            "a name of one character",
            job_arn("aws-us-gov", "us-gov-west-1", "210987654321", JOB_CATEGORIES[1], "r"),
        ),
    ];

    for (case, arn) in rows {
        // The refused half: no job was created, so every operation answers its family's
        // refusal and the store stays empty.
        for (operation, family) in [
            ("Sample", Family::Sampling),
            ("SampleWithResponseStream", Family::Sampling),
            ("CompleteRollout", Family::Rollout),
            ("UpdateReward", Family::Rollout),
        ] {
            let (engine, router) = mounted_service();
            let reply = match operation {
                "Sample" => sample(&router, SAMPLE_PATH, &arn, "traj-1").await,
                "SampleWithResponseStream" => {
                    sample(&router, SAMPLE_STREAM_PATH, &arn, "traj-1").await
                }
                "CompleteRollout" => complete_rollout(&router, &arn, "traj-1").await,
                _ => update_reward(&router, &arn, "traj-1", &[1.0]).await,
            };
            let (expected_status, expected_code, expected_message) = if family == Family::Sampling {
                (
                    400,
                    "ValidationException",
                    state::job_not_found_message(&arn),
                )
            } else {
                (
                    404,
                    "ResourceNotFoundException",
                    state::trajectory_not_found_message("traj-1"),
                )
            };
            assert_eq!(
                (reply.status, reply.error_code().as_str()),
                (expected_status, expected_code),
                "case {case:?}/{operation}: expected {expected_status} {expected_code} for an ARN \
                 that resolves to no created job, got {} {:?} / {}",
                reply.status,
                reply.error_type,
                reply.text()
            );
            assert_eq!(
                reply.message(),
                expected_message,
                "case {case:?}/{operation}: expected the refusal sentence {expected_message:?}, \
                 got {:?}",
                reply.message()
            );
            let held = engine.state::<ServiceState>().lock().trajectories.len();
            assert_eq!(
                held, 0,
                "case {case:?}/{operation}: expected a refused call to write nothing, got {held} \
                 trajectory/ies"
            );
        }

        // The served half: the control plane creates the job, and the same four operations
        // run the rollout through on that ARN.
        let (engine, router) = mounted_service();
        create_job_at_arn(&engine, &arn);
        for (operation, path) in [
            ("Sample", SAMPLE_PATH),
            ("SampleWithResponseStream", SAMPLE_STREAM_PATH),
        ] {
            let served = sample(&router, path, &arn, "traj-1").await;
            assert_eq!(
                served.status,
                200,
                "case {case:?}/{operation}: expected 200 for a job the control plane created, got \
                 {} {:?} / {}",
                served.status,
                served.error_type,
                served.text()
            );
        }
        let sealed = complete_rollout(&router, &arn, "traj-1").await;
        assert_eq!(
            sealed.status,
            200,
            "case {case:?}/CompleteRollout: expected 200 once the job resolves, got {} {:?} / {}",
            sealed.status,
            sealed.error_type,
            sealed.text()
        );
        // One reward per captured turn, which is what the two sampling calls above captured.
        let rewarded = update_reward(&router, &arn, "traj-1", &[0.5, 1.5]).await;
        assert_eq!(
            rewarded.status,
            200,
            "case {case:?}/UpdateReward: expected 200 once the job resolves, got {} {:?} / {}",
            rewarded.status,
            rewarded.error_type,
            rewarded.text()
        );

        // Read back through the store the way the next request would: the two turns landed on
        // one trajectory of the resolved job, and the lifecycle ran to its end.
        let store = engine.state::<ServiceState>();
        let guard = store.lock();
        let trajectory = guard.trajectory(&arn, "traj-1").unwrap_or_else(|| {
            panic!("case {case:?}: expected a trajectory under the ARN every call carried")
        });
        assert_eq!(
            (
                trajectory.turns.len(),
                trajectory.status.as_str(),
                trajectory.rewards.clone()
            ),
            (2, "reward-received", Some(vec![0.5, 1.5])),
            "case {case:?}: expected two captured turns on a rewarded trajectory, got {:?}",
            trajectory
        );
    }
}
