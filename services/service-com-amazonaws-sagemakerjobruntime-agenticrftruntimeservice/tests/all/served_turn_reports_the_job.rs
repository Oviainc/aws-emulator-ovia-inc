// SPDX-License-Identifier: CC0-1.0

//! The completion envelope a served turn carries reports the JOB the call resolved to, not
//! the ARN string that call addressed it through.
//!
//! RULE: every per-job value an inference response envelope carries — the turn's `id` and
//! the `model` that served it — is derived from the job a `JobArn` resolves to, as the
//! shared job directory identifies one (the `<category>/<name>` resource path of the ARN),
//! because the partition, the Region and the account an ARN was addressed through identify
//! no job. So all the turns of ONE trajectory, and the bodies retained for them, agree
//! about the job that served them however each of their calls spelled that job's ARN,
//! while another job name or another job category is another job and reports it.
//!
//! Rows differ along WHICH ARN component the later call changes — the Region, the account,
//! the partition, all three at once, the empty Region the pattern admits — and along the
//! two components that DO name another job, the job name and the job category. Each row's
//! expectation is written out as the `<category>/<name>` pair the directory keys that job
//! by, so it does not come from the code under test. Both sampling operations are driven,
//! because they serve the one envelope in two framings, and the retained bodies are read
//! afterwards the way the NEXT request would read them.

use serde_json::{json, Value};
use service_com_amazonaws_sagemakerjobruntime_agenticrftruntimeservice::ComAmazonawsSagemakerjobruntimeAgenticrftruntimeserviceState as ServiceState;
use smithy_server_runtime::MultiServiceRouter;

// The shared harness: the wire-answer reduction, the completion-document reduction and the
// job-directory setup every suite of this crate drives (`tests/all/harness.rs`).
use crate::harness::{
    completion_document, create_job_at_arn, job_arn, mounted_service, send, served_completion,
    Reply, ACCOUNT, JOB_ARN_HEADER, REGION, SAMPLE_PATH, SAMPLE_STREAM_PATH, TRAJECTORY_ID_HEADER,
};

/// The `JobCategory` values the SageMaker control plane creates a job under.
///
/// Catalog source: `smithy_query(query="shape",shape="JobCategory",
/// model_path="services/service-com-amazonaws-sagemaker-sagemaker/smithy-model/com-amazonaws-sagemaker-sagemaker.json")`
/// — exactly `AgentRFT` and `AgentRFTEvaluation`.
const JOB_CATEGORIES: [&str; 2] = ["AgentRFT", "AgentRFTEvaluation"];

/// One way of addressing a job: the ARN a call carries, and the job that ARN resolves to as
/// the shared directory identifies one.
struct Spelling {
    /// The `JobArn` value a request carries.
    arn: String,
    /// The `<category>/<name>` pair the directory keys that job by — written out here,
    /// never computed by the emulator, so the expectation is independent of it.
    job: String,
}

/// One ARN spelling of the job `category` / `name`, addressed through `partition` /
/// `region` / `account`.
fn spelling(partition: &str, region: &str, account: &str, category: &str, name: &str) -> Spelling {
    Spelling {
        arn: job_arn(partition, region, account, category, name),
        job: format!("{category}/{name}"),
    }
}

/// The spelling the control plane minted: the one account and Region this emulator serves.
fn minted(category: &str, name: &str) -> Spelling {
    spelling("aws", REGION, ACCOUNT, category, name)
}

/// One row: the ARN a rollout's first turn carries and the ARN its later turn carries.
struct Row {
    case: &'static str,
    first: Spelling,
    later: Spelling,
}

impl Row {
    /// Whether the two spellings resolve to one job.
    fn same_job(&self) -> bool {
        self.first.job == self.later.job
    }
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the very spelling the control plane minted",
            first: minted(JOB_CATEGORIES[0], "rollout-1"),
            later: minted(JOB_CATEGORIES[0], "rollout-1"),
        },
        Row {
            case: "the same job addressed in another Region",
            first: minted(JOB_CATEGORIES[0], "rollout-2"),
            later: spelling("aws", "eu-west-1", ACCOUNT, JOB_CATEGORIES[0], "rollout-2"),
        },
        Row {
            case: "the same job addressed through another account",
            first: minted(JOB_CATEGORIES[1], "eval-1"),
            later: spelling("aws", REGION, "123456789012", JOB_CATEGORIES[1], "eval-1"),
        },
        Row {
            case: "the same job addressed in another partition",
            first: minted(JOB_CATEGORIES[0], "rollout-3"),
            later: spelling(
                "aws-cn",
                "cn-north-1",
                ACCOUNT,
                JOB_CATEGORIES[0],
                "rollout-3",
            ),
        },
        Row {
            case: "the same job addressed through another partition, Region and account",
            first: spelling(
                "aws-cn",
                "cn-north-1",
                "123456789012",
                JOB_CATEGORIES[1],
                "eval-2",
            ),
            later: spelling(
                "aws-us-gov",
                "us-gov-west-1",
                "210987654321",
                JOB_CATEGORIES[1],
                "eval-2",
            ),
        },
        Row {
            // `JobArn`'s pattern admits an empty Region (`[a-z0-9\-]*`), which is still no
            // part of the job's identity.
            case: "the same job addressed with the empty Region the pattern admits",
            first: minted(JOB_CATEGORIES[0], "rollout-4"),
            later: spelling("aws", "", ACCOUNT, JOB_CATEGORIES[0], "rollout-4"),
        },
        Row {
            case: "another job name, in the Region the first was addressed in",
            first: minted(JOB_CATEGORIES[0], "rollout-5"),
            later: minted(JOB_CATEGORIES[0], "rollout-6"),
        },
        Row {
            case: "the same job name under another job category",
            first: minted(JOB_CATEGORIES[0], "rollout-7"),
            later: minted(JOB_CATEGORIES[1], "rollout-7"),
        },
        Row {
            case: "another job name, addressed in another Region",
            first: minted(JOB_CATEGORIES[0], "rollout-8"),
            later: spelling("aws", "eu-west-1", ACCOUNT, JOB_CATEGORIES[0], "rollout-9"),
        },
    ]
}

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

/// The `model` a completion document reports, or the empty string when it reports none.
fn reported_model(document: &Value) -> String {
    document
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The `model` every body this store RETAINED for the trajectory under `job_arn` /
/// `trajectory_id` reports, in turn order — read the way the next request would read it.
fn retained_models(
    engine: &engine::InMemoryEngine,
    job_arn: &str,
    trajectory_id: &str,
) -> Vec<String> {
    let store = engine.state::<ServiceState>();
    let guard = store.lock();
    guard
        .trajectory(job_arn, trajectory_id)
        .map(|trajectory| {
            trajectory
                .turns
                .iter()
                .map(|turn| reported_model(&completion_document(&turn.response_body)))
                .collect()
        })
        .unwrap_or_default()
}

/// RULE: the `model` a served turn reports is the JOB the call resolved to, so it is the
/// same value however that call spelled the job's ARN — and another job reports itself.
#[tokio::test]
async fn the_served_completion_reports_the_job_whichever_spelling_addressed_it() {
    for row in rows() {
        let case = row.case;
        let (engine, router) = mounted_service();
        // Both ARNs name a created job, so nothing here is refused for a job that does not
        // exist: what a row varies is whether they name the SAME one.
        create_job_at_arn(&engine, &row.first.arn);
        create_job_at_arn(&engine, &row.later.arn);

        // The two framings of the one envelope: `Sample` serves it as JSON, and
        // `SampleWithResponseStream` as the first event of its stream.
        let served_first = sample(&router, &row.first.arn, "shared-id", "go", false).await;
        assert_eq!(
            served_first.status,
            200,
            "case {case:?}: expected the turn through {:?} to answer 200, got {} / {}",
            row.first.arn,
            served_first.status,
            served_first.text()
        );
        let served_later = sample(&router, &row.later.arn, "shared-id", "go again", true).await;
        assert_eq!(
            served_later.status,
            200,
            "case {case:?}: expected the turn through {:?} to answer 200, got {} / {}",
            row.later.arn,
            served_later.status,
            served_later.text()
        );

        let model_first = reported_model(&served_completion(&served_first));
        let model_later = reported_model(&served_completion(&served_later));
        assert_eq!(
            model_first, row.first.job,
            "case {case:?}: expected the turn served through {:?} to report model {:?}, got \
             {model_first:?}",
            row.first.arn, row.first.job
        );
        assert_eq!(
            model_later, row.later.job,
            "case {case:?}: expected the turn served through {:?} to report model {:?}, got \
             {model_later:?}",
            row.later.arn, row.later.job
        );
        // Which is the point: agreement follows the JOB, not the spelling.
        let agree = model_first == model_later;
        assert_eq!(
            agree,
            row.same_job(),
            "case {case:?}: expected the two turns to agree about the model={} for {:?} and \
             {:?}, got {model_first:?} and {model_later:?}",
            row.same_job(),
            row.first.arn,
            row.later.arn
        );

        // The served body is the retained one, so the store holds the same agreement: for
        // one job both turns are on its trajectory and both bodies name it.
        let retained = retained_models(&engine, &row.first.arn, "shared-id");
        let expected: Vec<String> = if row.same_job() {
            vec![row.first.job.clone(), row.first.job.clone()]
        } else {
            vec![row.first.job.clone()]
        };
        assert_eq!(
            retained, expected,
            "case {case:?}: expected the retained bodies under {:?} to report {expected:?}, got \
             {retained:?}",
            row.first.arn
        );
        let retained_later = retained_models(&engine, &row.later.arn, "shared-id");
        let expected_later: Vec<String> = if row.same_job() {
            expected
        } else {
            vec![row.later.job.clone()]
        };
        assert_eq!(
            retained_later, expected_later,
            "case {case:?}: expected the retained bodies under {:?} to report \
             {expected_later:?}, got {retained_later:?}",
            row.later.arn
        );
    }
}

/// RULE, over a whole rollout: every turn of ONE trajectory reports ONE model — the job the
/// calls resolve to — however many different spellings of that job's ARN served its turns,
/// and never the ARN string one of those calls carried.
#[tokio::test]
async fn every_turn_of_one_rollout_reports_one_model_across_every_spelling() {
    // One job, addressed through each component the ARN pattern lets a call vary.
    let spellings = [
        minted(JOB_CATEGORIES[0], "rollout-10"),
        spelling("aws", "eu-west-1", ACCOUNT, JOB_CATEGORIES[0], "rollout-10"),
        spelling("aws", REGION, "123456789012", JOB_CATEGORIES[0], "rollout-10"),
        spelling(
            "aws-cn",
            "cn-north-1",
            "123456789012",
            JOB_CATEGORIES[0],
            "rollout-10",
        ),
        spelling("aws", "", ACCOUNT, JOB_CATEGORIES[0], "rollout-10"),
    ];
    let job = spellings[0].job.clone();

    let (engine, router) = mounted_service();
    create_job_at_arn(&engine, &spellings[0].arn);

    for (turn, addressed) in spellings.iter().enumerate() {
        // Alternating the sampling operation, so one trajectory mixes both framings.
        let streamed = turn % 2 == 1;
        let served = sample(
            &router,
            &addressed.arn,
            "one-rollout",
            "another step",
            streamed,
        )
        .await;
        assert_eq!(
            served.status,
            200,
            "case turn {turn} through {:?} (streamed={streamed}): expected 200, got {} / {}",
            addressed.arn,
            served.status,
            served.text()
        );
        let reported = reported_model(&served_completion(&served));
        assert_eq!(
            reported, job,
            "case turn {turn} through {:?} (streamed={streamed}): expected model {job:?}, got \
             {reported:?}",
            addressed.arn
        );
        assert_ne!(
            reported, addressed.arn,
            "case turn {turn} through {:?} (streamed={streamed}): expected the model NOT to be \
             the ARN spelling the call carried, got {reported:?}",
            addressed.arn
        );
    }

    // What the next request reads: one trajectory, one turn per call, every retained body
    // naming the one job.
    let retained = retained_models(&engine, &spellings[0].arn, "one-rollout");
    let expected: Vec<String> = spellings.iter().map(|_| job.clone()).collect();
    assert_eq!(
        retained, expected,
        "case the whole rollout: expected {} retained bodies all reporting {job:?}, got \
         {retained:?}",
        spellings.len()
    );
    let held = engine.state::<ServiceState>().lock().trajectories.len();
    assert_eq!(
        held, 1,
        "case the whole rollout: expected the turns to be one trajectory of one job, got {held}"
    );
}
