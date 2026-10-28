// SPDX-License-Identifier: CC0-1.0

//! The job a sampling call is routed to must be a job that EXISTS.
//!
//! RULE: a data-plane operation that routes a request to a resource named only by an
//! identifier in the request resolves that identifier against the resource the control
//! plane created, and refuses a well-formed identifier that resolves to nothing with its
//! own modelled client error rather than serving the call. Here that is `Sample` and
//! `SampleWithResponseStream` resolving `JobArn` (`@required`, `@length` 1..256, pattern
//! `^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:job/[a-zA-Z0-9_\-]+/[a-zA-Z0-9](-*[a-zA-Z0-9]){0,62}$`
//! — `smithy_query(query="operation",operation="Sample")`) against the jobs the SageMaker
//! control plane minted: this model binds no operation that creates, lists or deletes a
//! job, so the shared job directory the control plane writes is the only place an ARN can
//! be resolved, and an ARN no create ever minted is refused `400 ValidationException`
//! with a `Message` naming the ARN the request carried, with nothing written.
//!
//! The rows span what the rule generalises over rather than one request: the job's
//! category, the partition, region and account the ARN was addressed through (an emulator
//! serving one account resolves a job however it is addressed), the job-name characters,
//! which of the two sampling operations serves the call, and — for the refusal — WHY the
//! ARN resolves to nothing: no job was ever created, another job was, the name is held
//! under another category, or the job was created and then deleted.
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
    create_job, delete_job, job_arn, mounted_service, send, Reply, JOB_ARN_HEADER, SAMPLE_PATH,
    SAMPLE_STREAM_PATH, TRAJECTORY_ID_HEADER,
};

/// The `JobCategory` values the SageMaker control plane creates a job under.
///
/// Catalog source: `smithy_query(query="shape",shape="JobCategory",
/// model_path="services/service-com-amazonaws-sagemaker-sagemaker/smithy-model/com-amazonaws-sagemaker-sagemaker.json")`
/// — exactly `AgentRFT` and `AgentRFTEvaluation`.
const JOB_CATEGORIES: [&str; 2] = ["AgentRFT", "AgentRFTEvaluation"];

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

/// An OpenAI-compatible chat-completions request body.
fn chat_payload(text: &str) -> Vec<u8> {
    serde_json::to_vec(&json!({ "messages": [{ "role": "user", "content": text }] }))
        .expect("a serializable payload")
}

/// RULE (the refused half): a well-formed `JobArn` that resolves to no created job is
/// refused `400 ValidationException` naming the ARN the request carried, on both sampling
/// operations, and nothing is written.
#[tokio::test]
async fn a_sampling_call_for_a_job_that_was_never_created_is_refused() {
    // Rows differ along WHY the ARN resolves to nothing: the directory is empty, it holds
    // another job of the same category, it holds the same name under the other category,
    // or it held this very job and the job was deleted.
    enum Setup {
        /// Nothing was ever created.
        Empty,
        /// `category`/`name` was created, and the request names something else.
        Created(&'static str, &'static str),
        /// `category`/`name` was created and then deleted.
        Deleted(&'static str, &'static str),
    }

    let rows: [(&str, Setup, String); 5] = [
        (
            "no job was ever created",
            Setup::Empty,
            job_arn("aws", "us-east-1", "000000000000", "AgentRFT", "ghost-run"),
        ),
        (
            "another job of the same category exists",
            Setup::Created("AgentRFT", "rollout-1"),
            job_arn("aws", "us-east-1", "000000000000", "AgentRFT", "rollout-2"),
        ),
        (
            "the name is held under the other category",
            Setup::Created("AgentRFTEvaluation", "rollout-1"),
            job_arn("aws", "us-east-1", "000000000000", "AgentRFT", "rollout-1"),
        ),
        (
            "the job was created and then deleted",
            Setup::Deleted("AgentRFT", "finished-run"),
            job_arn("aws", "us-east-1", "000000000000", "AgentRFT", "finished-run"),
        ),
        (
            "a job in another region and account that was never created",
            Setup::Empty,
            job_arn("aws", "eu-west-1", "123456789012", "AgentRFT", "ghost-run-2"),
        ),
    ];

    for (case, setup, arn) in rows {
        for path in [SAMPLE_PATH, SAMPLE_STREAM_PATH] {
            let (engine, router) = mounted_service();
            match setup {
                Setup::Empty => {}
                Setup::Created(category, name) => create_job(&engine, category, name),
                Setup::Deleted(category, name) => {
                    create_job(&engine, category, name);
                    delete_job(&engine, category, name);
                }
            }

            let reply = sample(&router, path, &arn, "traj-1", &chat_payload("go")).await;
            assert_eq!(
                (reply.status, reply.error_code().as_str()),
                (400, "ValidationException"),
                "case {case:?} on {path}: expected 400 ValidationException for an ARN that \
                 resolves to no job, got {} {:?} / {}",
                reply.status,
                reply.error_type,
                reply.text()
            );
            assert_eq!(
                reply.content_type.as_deref(),
                Some("application/json"),
                "case {case:?} on {path}: expected a restJson1 error document, got {:?}",
                reply.content_type
            );
            assert!(
                reply.message().contains(&arn),
                "case {case:?} on {path}: expected the refusal to name the requested jobArn \
                 {arn:?}, got {:?}",
                reply.message()
            );
            let held = engine.state::<ServiceState>().lock().trajectories.len();
            assert_eq!(
                held, 0,
                "case {case:?} on {path}: expected a refused call to capture no turn, got \
                 {held} trajectory/ies"
            );
        }
    }

    // The requirement is the status, the error code and that the `Message` names the
    // request's own JobArn; this one row also pins the whole sentence the recording shows,
    // so the rows above cannot pass on a message that merely happens to quote the ARN.
    let (_engine, router) = mounted_service();
    let arn = job_arn("aws", "us-east-1", "000000000000", "AgentRFT", "ghost-run");
    let reply = sample(&router, SAMPLE_PATH, &arn, "traj-1", &chat_payload("go")).await;
    assert_eq!(
        reply.message(),
        format!("Could not find job with jobArn {arn}"),
        "case \"the whole refusal sentence\": got {:?}",
        reply.message()
    );
}

/// RULE (the served half): once the control plane has created the job, every `JobArn` the
/// modelled constraints admit for it is served and captured on its own trajectory — the
/// refusals above are about the job existing, not about the ARN's spelling.
#[tokio::test]
async fn a_sampling_call_for_a_created_job_is_served_however_the_arn_addresses_it() {
    // The run segment at its 63-character maximum: `[a-zA-Z0-9](-*[a-zA-Z0-9]){0,62}`.
    let long_name = format!("r{}", "a".repeat(62));

    // Rows differ along the job's category and name characters and along the partition,
    // region and account the ARN was addressed through: an emulator serves one account in
    // one Region, so a job it created resolves however a caller addresses it.
    let rows: Vec<(&str, &str, String, String)> = vec![
        (
            "the ARN the control plane minted",
            JOB_CATEGORIES[0],
            "rollout-1".to_string(),
            job_arn("aws", "us-east-1", "000000000000", "AgentRFT", "rollout-1"),
        ),
        (
            "the evaluation category",
            JOB_CATEGORIES[1],
            "eval-1".to_string(),
            job_arn(
                "aws",
                "us-east-1",
                "000000000000",
                "AgentRFTEvaluation",
                "eval-1",
            ),
        ),
        (
            "addressed in another region",
            JOB_CATEGORIES[0],
            "rollout-2".to_string(),
            job_arn("aws", "us-west-2", "000000000000", "AgentRFT", "rollout-2"),
        ),
        (
            "addressed through another account",
            JOB_CATEGORIES[0],
            "rollout-3".to_string(),
            job_arn("aws", "us-east-1", "123456789012", "AgentRFT", "rollout-3"),
        ),
        (
            "addressed in another partition",
            JOB_CATEGORIES[0],
            "rollout-4".to_string(),
            job_arn("aws-cn", "cn-north-1", "123456789012", "AgentRFT", "rollout-4"),
        ),
        (
            "a name at its 63-character maximum",
            JOB_CATEGORIES[0],
            long_name.clone(),
            job_arn("aws", "us-east-1", "000000000000", "AgentRFT", &long_name),
        ),
        (
            "a name of one character",
            JOB_CATEGORIES[0],
            "r".to_string(),
            job_arn("aws", "us-east-1", "000000000000", "AgentRFT", "r"),
        ),
    ];

    for (case, category, name, arn) in rows {
        for (path, streamed, expected_content_type) in [
            (SAMPLE_PATH, false, state::JSON_CONTENT_TYPE),
            (SAMPLE_STREAM_PATH, true, state::EVENT_STREAM_CONTENT_TYPE),
        ] {
            let (engine, router) = mounted_service();
            create_job(&engine, category, &name);

            let served = sample(&router, path, &arn, "traj-1", &chat_payload("go")).await;
            assert_eq!(
                served.status,
                200,
                "case {case:?} on {path}: expected 200 for a job the control plane created, \
                 got {} {:?} / {}",
                served.status,
                served.error_type,
                served.text()
            );
            assert_eq!(
                served.content_type.as_deref(),
                Some(expected_content_type),
                "case {case:?} on {path}: expected Content-Type {expected_content_type:?}, got \
                 {:?}",
                served.content_type
            );

            // The turn the call served is the one the store holds, under THIS ARN.
            let store = engine.state::<ServiceState>();
            let guard = store.lock();
            let trajectory = guard.trajectory(&arn, "traj-1").unwrap_or_else(|| {
                panic!("case {case:?} on {path}: expected a trajectory under the ARN the call carried")
            });
            assert_eq!(
                (trajectory.turns.len(), trajectory.turns[0].streamed),
                (1, streamed),
                "case {case:?} on {path}: expected one captured turn with streamed={streamed}, \
                 got {:?}",
                trajectory.turns
            );
            assert_eq!(
                trajectory.turns[0].response_body, served.body,
                "case {case:?} on {path}: expected the served body to be the recorded one, got \
                 {} served vs {} recorded byte(s)",
                served.body.len(),
                trajectory.turns[0].response_body.len()
            );
        }
    }
}
