// SPDX-License-Identifier: CC0-1.0

//! A trajectory belongs to the JOB a call resolves to, not to the ARN string the call
//! spelled that job with.
//!
//! RULE: this data plane resolves a `JobArn` against the shared job directory, which
//! identifies a job by the `<category>/<name>` resource path of its ARN alone — the
//! partition, the Region and the account an ARN was addressed through identify no job,
//! because an emulator serves one account in one Region. So the per-job state this
//! service owns is keyed by that identity too: two ARNs that resolve to one job address
//! one set of trajectories, while another job name or another job category is a
//! different job and therefore a different trajectory under the same id.
//!
//! The three things that rule decides, one test each:
//!
//! * `TrajectoryId` "group[s] turns into a single rollout", so turns served through any
//!   spelling of one job land in ONE trajectory and keep counting up;
//! * `CompleteRollout` means "no further turns will be appended to the trajectory", so
//!   its seal refuses a later turn whichever spelling carries it;
//! * `CompleteRollout` and `UpdateReward` resolve the trajectory the sampling calls
//!   built, whichever spelling they carry.
//!
//! Rows differ along WHICH ARN component the later call changes — the Region, the
//! account, the partition, all three at once, the empty Region the pattern admits — and
//! along the two components that DO name another job, the job name and the job category.
//! Every answer goes over the wire through a [`MultiServiceRouter`], and the store is
//! read afterwards the way the NEXT request would read it.

use serde_json::{json, Value};
use service_com_amazonaws_sagemakerjobruntime_agenticrftruntimeservice::state;
use service_com_amazonaws_sagemakerjobruntime_agenticrftruntimeservice::ComAmazonawsSagemakerjobruntimeAgenticrftruntimeserviceState as ServiceState;
use smithy_server_runtime::MultiServiceRouter;

// The shared harness: the wire-answer reduction and the job-directory setup every suite
// of this crate drives (`tests/all/harness.rs`).
use crate::harness::{
    create_job_at_arn, job_arn, mounted_service, send, Reply, COMPLETE_ROLLOUT_PATH,
    JOB_ARN_HEADER, SAMPLE_PATH, SAMPLE_STREAM_PATH, TRAJECTORY_ID_HEADER, UPDATE_REWARD_PATH,
};

/// One `Sample` or `SampleWithResponseStream` call.
async fn sample(
    router: &MultiServiceRouter,
    job_arn: &str,
    trajectory_id: &str,
    text: &str,
    streamed: bool,
) -> Reply {
    let path = if streamed {
        SAMPLE_STREAM_PATH
    } else {
        SAMPLE_PATH
    };
    send(
        router,
        path,
        &[
            (JOB_ARN_HEADER, job_arn),
            (TRAJECTORY_ID_HEADER, trajectory_id),
            ("content-type", "application/json"),
        ],
        serde_json::to_vec(&json!({ "messages": [{ "role": "user", "content": text }] }))
            .expect("a serializable payload"),
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
    send(
        router,
        COMPLETE_ROLLOUT_PATH,
        &[
            (JOB_ARN_HEADER, job_arn),
            ("content-type", "application/json"),
        ],
        serde_json::to_vec(&json!({
            "TrajectoryId": trajectory_id,
            "Status": status,
            "ClientToken": "seal-token",
        }))
        .expect("a serializable body"),
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
    send(
        router,
        UPDATE_REWARD_PATH,
        &[
            (JOB_ARN_HEADER, job_arn),
            ("content-type", "application/json"),
        ],
        serde_json::to_vec(&json!({
            "TrajectoryId": trajectory_id,
            "Rewards": rewards,
            "ClientToken": "reward-token",
        }))
        .expect("a serializable body"),
    )
    .await
}

/// How many turns the trajectory under `job_arn` / `trajectory_id` holds, read the way
/// the next request would read it.
fn captured_turns(engine: &engine::InMemoryEngine, job_arn: &str, trajectory_id: &str) -> usize {
    let store = engine.state::<ServiceState>();
    let guard = store.lock();
    guard
        .trajectory(job_arn, trajectory_id)
        .map_or(0, |trajectory| trajectory.turns.len())
}

/// One row: the ARN a rollout's first calls carry, the ARN its LATER calls carry, and
/// whether the two name the same job.
struct Row {
    case: &'static str,
    /// The ARN the rollout's first calls carry.
    first: String,
    /// The ARN the later calls carry.
    later: String,
    /// Whether `later` resolves to the same job as `first`.
    same_job: bool,
}

/// The `JobCategory` values the SageMaker control plane creates a job under.
///
/// Catalog source: `smithy_query(query="shape",shape="JobCategory",
/// model_path="services/service-com-amazonaws-sagemaker-sagemaker/smithy-model/com-amazonaws-sagemaker-sagemaker.json")`
/// — exactly `AgentRFT` and `AgentRFTEvaluation`.
const JOB_CATEGORIES: [&str; 2] = ["AgentRFT", "AgentRFTEvaluation"];

/// The one account and Region this emulator serves, as the control plane mints an ARN in
/// them.
fn minted(category: &str, name: &str) -> String {
    job_arn("aws", "us-east-1", "000000000000", category, name)
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the very spelling the control plane minted",
            first: minted(JOB_CATEGORIES[0], "rollout-1"),
            later: minted(JOB_CATEGORIES[0], "rollout-1"),
            same_job: true,
        },
        Row {
            case: "the same job addressed in another Region",
            first: minted(JOB_CATEGORIES[0], "rollout-2"),
            later: job_arn(
                "aws",
                "eu-west-1",
                "000000000000",
                JOB_CATEGORIES[0],
                "rollout-2",
            ),
            same_job: true,
        },
        Row {
            case: "the same job addressed through another account",
            first: minted(JOB_CATEGORIES[1], "eval-1"),
            later: job_arn(
                "aws",
                "us-east-1",
                "123456789012",
                JOB_CATEGORIES[1],
                "eval-1",
            ),
            same_job: true,
        },
        Row {
            case: "the same job addressed in another partition",
            first: minted(JOB_CATEGORIES[0], "rollout-3"),
            later: job_arn(
                "aws-cn",
                "cn-north-1",
                "000000000000",
                JOB_CATEGORIES[0],
                "rollout-3",
            ),
            same_job: true,
        },
        Row {
            case: "the same job addressed through another partition, Region and account",
            first: job_arn(
                "aws-cn",
                "cn-north-1",
                "123456789012",
                JOB_CATEGORIES[1],
                "eval-2",
            ),
            later: job_arn(
                "aws-us-gov",
                "us-gov-west-1",
                "210987654321",
                JOB_CATEGORIES[1],
                "eval-2",
            ),
            same_job: true,
        },
        Row {
            // `JobArn`'s pattern admits an empty Region (`[a-z0-9\-]*`), which is still
            // no part of the job's identity.
            case: "the same job addressed with the empty Region the pattern admits",
            first: minted(JOB_CATEGORIES[0], "rollout-4"),
            later: job_arn("aws", "", "000000000000", JOB_CATEGORIES[0], "rollout-4"),
            same_job: true,
        },
        Row {
            case: "another job name, in the Region the first was addressed in",
            first: minted(JOB_CATEGORIES[0], "rollout-5"),
            later: minted(JOB_CATEGORIES[0], "rollout-6"),
            same_job: false,
        },
        Row {
            case: "the same job name under another job category",
            first: minted(JOB_CATEGORIES[0], "rollout-7"),
            later: minted(JOB_CATEGORIES[1], "rollout-7"),
            same_job: false,
        },
        Row {
            case: "another job name, addressed in another Region",
            first: minted(JOB_CATEGORIES[0], "rollout-8"),
            later: job_arn(
                "aws",
                "eu-west-1",
                "000000000000",
                JOB_CATEGORIES[0],
                "rollout-9",
            ),
            same_job: false,
        },
    ]
}

/// The `id` the served completion reports, out of either framing.
fn served_completion_id(reply: &Reply) -> String {
    let document: Value = if reply.content_type.as_deref() == Some(state::EVENT_STREAM_CONTENT_TYPE)
    {
        let text = reply.text();
        let first = text
            .strip_prefix("data: ")
            .and_then(|rest| rest.split("\n\n").next())
            .unwrap_or_default()
            .to_string();
        serde_json::from_str(&first).unwrap_or(Value::Null)
    } else {
        reply.json()
    };
    document
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// RULE: `TrajectoryId` groups a rollout's turns into one trajectory of the JOB the call
/// resolves to, so turns served through any spelling of that job's ARN land in the one
/// trajectory and keep counting up — while a trajectory id of ANOTHER job is a
/// trajectory of its own.
#[tokio::test]
async fn the_turns_of_one_rollout_group_into_one_trajectory_however_a_call_addresses_the_job() {
    for Row {
        case,
        first,
        later,
        same_job,
    } in rows()
    {
        let (engine, router) = mounted_service();
        // Both ARNs name a created job, so nothing here is refused for a job that does
        // not exist: what a row varies is whether they name the SAME one.
        create_job_at_arn(&engine, &first);
        create_job_at_arn(&engine, &later);

        // Two turns through the first spelling, one of each sampling operation, then one
        // through the later spelling.
        for streamed in [false, true] {
            let served = sample(&router, &first, "shared-id", "go", streamed).await;
            assert_eq!(
                served.status,
                200,
                "case {case:?} (streamed={streamed}): expected the seeding turn to answer 200, \
                 got {} / {}",
                served.status,
                served.text()
            );
        }
        let served = sample(&router, &later, "shared-id", "go again", false).await;
        assert_eq!(
            served.status,
            200,
            "case {case:?}: expected the turn through the later spelling to answer 200, got {} / \
             {}",
            served.status,
            served.text()
        );

        let expected_trajectories = if same_job { 1 } else { 2 };
        let held = engine.state::<ServiceState>().lock().trajectories.len();
        assert_eq!(
            held, expected_trajectories,
            "case {case:?}: expected {expected_trajectories} trajectory/ies for one trajectory \
             id across {first:?} and {later:?}, got {held}"
        );

        let (expected_first, expected_later) = if same_job { (3, 3) } else { (2, 1) };
        let under_first = captured_turns(&engine, &first, "shared-id");
        let under_later = captured_turns(&engine, &later, "shared-id");
        assert_eq!(
            (under_first, under_later),
            (expected_first, expected_later),
            "case {case:?}: expected {expected_first} turn(s) under {first:?} and \
             {expected_later} under {later:?}, got {under_first} and {under_later}"
        );

        // The grouped turns keep counting up rather than restarting, and the turn the
        // later spelling served reports the id of its position in THAT trajectory.
        let store = engine.state::<ServiceState>();
        let guard = store.lock();
        let trajectory = guard
            .trajectory(&later, "shared-id")
            .unwrap_or_else(|| panic!("case {case:?}: expected a trajectory for {later:?}"));
        let indices: Vec<usize> = trajectory.turns.iter().map(|turn| turn.index).collect();
        let expected_indices: Vec<usize> = (0..expected_later).collect();
        assert_eq!(
            indices, expected_indices,
            "case {case:?}: expected the trajectory's turn indices to be {expected_indices:?}, \
             got {indices:?}"
        );
        let reported = served_completion_id(&served);
        // For a same-job row the expected id is computed from the OTHER spelling, which
        // is the point: the id names the turn of the job's trajectory.
        let expected_id = state::completion_id(
            if same_job { &first } else { &later },
            "shared-id",
            expected_later - 1,
        );
        assert_eq!(
            reported, expected_id,
            "case {case:?}: expected the served turn to report the id of turn \
             {} of the job's own trajectory ({expected_id:?}), got {reported:?}",
            expected_later - 1
        );
    }
}

/// RULE: the seal `CompleteRollout` writes belongs to the trajectory of the JOB, so once
/// it is sealed no further turn is appended whichever spelling of that job's ARN carries
/// the call — a sampling call for ANOTHER job is untouched by it.
#[tokio::test]
async fn a_sealed_rollout_appends_no_further_turn_however_a_call_addresses_the_job() {
    for Row {
        case,
        first,
        later,
        same_job,
    } in rows()
    {
        let (engine, router) = mounted_service();
        create_job_at_arn(&engine, &first);
        create_job_at_arn(&engine, &later);

        assert_eq!(
            sample(&router, &first, "traj-1", "go", false).await.status,
            200,
            "case {case:?}: expected the first turn to answer 200"
        );

        // Sealing through the later spelling: it seals the one trajectory when the
        // spelling names the same job, and finds no trajectory when it names another.
        let sealed = complete_rollout(&router, &later, "traj-1", "ready").await;
        if same_job {
            assert_eq!(
                sealed.status,
                200,
                "case {case:?}: expected CompleteRollout through the later spelling to seal the \
                 job's trajectory, got {} / {}",
                sealed.status,
                sealed.text()
            );
        } else {
            assert_eq!(
                (sealed.status, sealed.error_code(), sealed.message()),
                (
                    404,
                    "ResourceNotFoundException".to_string(),
                    state::trajectory_not_found_message("traj-1")
                ),
                "case {case:?}: expected CompleteRollout for another job to report no such \
                 trajectory, got {} {:?} / {}",
                sealed.status,
                sealed.error_code(),
                sealed.text()
            );
        }

        // A further turn, through the ORIGINAL spelling: refused by the seal when the
        // seal reached this job's trajectory.
        let again = sample(&router, &first, "traj-1", "once more", false).await;
        if same_job {
            let expected = state::AppendRefusal::Sealed {
                status: state::TrajectoryStatus::Complete,
                completion_status: Some(state::CompletionStatus::Ready),
            }
            .message("traj-1");
            assert_eq!(
                (again.status, again.error_code(), again.message()),
                (400, "ValidationException".to_string(), expected.clone()),
                "case {case:?}: expected the sealed refusal {expected:?}, got {} {:?} / {}",
                again.status,
                again.error_code(),
                again.text()
            );
        } else {
            assert_eq!(
                again.status,
                200,
                "case {case:?}: expected a sampling call for an unsealed trajectory to answer \
                 200, got {} / {}",
                again.status,
                again.text()
            );
        }

        let under_first = captured_turns(&engine, &first, "traj-1");
        let expected_turns = if same_job { 1 } else { 2 };
        assert_eq!(
            under_first, expected_turns,
            "case {case:?}: expected {expected_turns} captured turn(s) under {first:?}, got \
             {under_first}"
        );
    }
}

/// RULE: `CompleteRollout` and `UpdateReward` resolve the trajectory of the JOB their
/// `JobArn` names, so a rollout sampled through one spelling is sealed and rewarded
/// through any other spelling of that job — and reported absent only when the ARN names
/// another job.
#[tokio::test]
async fn the_rollout_lifecycle_resolves_the_jobs_trajectory_however_it_addresses_the_job() {
    for Row {
        case,
        first,
        later,
        same_job,
    } in rows()
    {
        let (engine, router) = mounted_service();
        create_job_at_arn(&engine, &first);
        create_job_at_arn(&engine, &later);

        for text in ["turn one", "turn two"] {
            assert_eq!(
                sample(&router, &first, "traj-1", text, false).await.status,
                200,
                "case {case:?}: expected the seeding turn to answer 200"
            );
        }
        assert_eq!(
            complete_rollout(&router, &first, "traj-1", "ready")
                .await
                .status,
            200,
            "case {case:?}: expected CompleteRollout to seal the trajectory"
        );

        // "Provide one reward value per turn in the trajectory": two turns, two scores.
        let rewarded = update_reward(&router, &later, "traj-1", &[0.25, 1.0]).await;
        if same_job {
            assert_eq!(
                rewarded.status,
                200,
                "case {case:?}: expected UpdateReward through the later spelling to reward the \
                 job's trajectory, got {} / {}",
                rewarded.status,
                rewarded.text()
            );
        } else {
            assert_eq!(
                (rewarded.status, rewarded.error_code(), rewarded.message()),
                (
                    404,
                    "ResourceNotFoundException".to_string(),
                    state::trajectory_not_found_message("traj-1")
                ),
                "case {case:?}: expected UpdateReward for another job to report no such \
                 trajectory, got {} {:?} / {}",
                rewarded.status,
                rewarded.error_code(),
                rewarded.text()
            );
        }

        // What the next request reads: the scores are on the job's one trajectory when
        // the spelling named that job, and nothing was recorded when it named another.
        let store = engine.state::<ServiceState>();
        let guard = store.lock();
        let trajectory = guard
            .trajectory(&first, "traj-1")
            .unwrap_or_else(|| panic!("case {case:?}: expected the sampled trajectory in state"));
        let expected_rewards = if same_job {
            Some(vec![0.25, 1.0])
        } else {
            None
        };
        let expected_status = if same_job {
            "reward-received"
        } else {
            "complete"
        };
        assert_eq!(
            (trajectory.rewards.clone(), trajectory.status.as_str()),
            (expected_rewards.clone(), expected_status),
            "case {case:?}: expected rewards {expected_rewards:?} in {expected_status} status, \
             got {:?} in {}",
            trajectory.rewards,
            trajectory.status.as_str()
        );
    }
}

/// RULE: a turn's completion id names the turn of the job's trajectory, so it is the same
/// id however the call addressed the job and a different one for another job.
#[test]
fn a_turns_completion_id_is_the_jobs_not_the_arn_spellings() {
    for Row {
        case,
        first,
        later,
        same_job,
    } in rows()
    {
        for index in [0usize, 7] {
            let left = state::completion_id(&first, "traj-1", index);
            let right = state::completion_id(&later, "traj-1", index);
            assert_eq!(
                left == right,
                same_job,
                "case {case:?} (turn {index}): expected same id={same_job} for {first:?} and \
                 {later:?}, got {left:?} and {right:?}"
            );
        }
    }
}
