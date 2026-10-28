// SPDX-License-Identifier: CC0-1.0

//! `JobArn` is read in TWO stages, and the two refusals are distinct.
//!
//! RULE: an identifier member is first checked against the constraints its model
//! declares, and only a value that satisfies them is resolved against the resource it
//! names — so a value outside the constraints is refused as MALFORMED (it denotes nothing
//! at all), while a well-formed value that resolves to no resource is refused as
//! unresolvable. Here that is `JobArn` on `Sample` and `SampleWithResponseStream`:
//! `@length` 1..256 plus
//! `^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:job/[a-zA-Z0-9_\-]+/[a-zA-Z0-9](-*[a-zA-Z0-9]){0,62}$`
//! (`smithy_query(query="shape",shape="JobArn")`) decide the first stage, and the jobs the
//! SageMaker control plane created decide the second. Both refusals are
//! `400 ValidationException` and neither writes anything, but they state different facts,
//! so a client can tell a typo'd ARN from a job that is gone.
//!
//! This suite pins the FIRST stage and the boundary between the two. The second stage —
//! which ARNs resolve to a created job and which do not — is `job_resolution.rs`: this
//! file used to assert that every pattern-valid ARN is SERVED, which the recorded service
//! does not do.
//!
//! The rows span what each stage generalises over: for the first, WHICH constraint the
//! value misses (the `@length` maximum and minimum, and the pattern in its partition,
//! service namespace, account, resource type and missing name segment); for the boundary,
//! ARNs that satisfy the data plane's own pattern but can name no job the control plane
//! could mint (its `JobArn` puts the `JobCategory`, `[a-zA-Z0-9]+`, where the data plane
//! admits `[a-zA-Z0-9_\-]+`).
//!
//! Every answer goes over the wire through a [`MultiServiceRouter`], and the store is read
//! afterwards the way the NEXT request would read it, so what is pinned is the emulator's
//! HTTP behaviour.

use serde_json::json;
use service_com_amazonaws_sagemakerjobruntime_agenticrftruntimeservice::state;
use service_com_amazonaws_sagemakerjobruntime_agenticrftruntimeservice::ComAmazonawsSagemakerjobruntimeAgenticrftruntimeserviceState as ServiceState;
use smithy_server_runtime::MultiServiceRouter;

// The shared harness: the wire-answer reduction and the job-directory setup every suite
// of this crate drives (`tests/all/harness.rs`).
use crate::harness::{
    create_job, job_arn, mounted_service, send, Reply, JOB_ARN_HEADER, SAMPLE_PATH,
    SAMPLE_STREAM_PATH, TRAJECTORY_ID_HEADER,
};

/// `JobArn`'s modelled `@length` bounds
/// (`smithy_query(query="shape",shape="JobArn")`).
const JOB_ARN_MAX: usize = 256;

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

/// A job ARN of exactly `total` characters, padded in the job-category segment — which
/// the data plane's pattern leaves unbounded (`[a-zA-Z0-9_\-]+`), so the only thing a long
/// ARN can fail is the `@length` maximum.
fn job_arn_of_length(total: usize) -> String {
    let fixed = job_arn("aws", "us-east-1", "000000000000", "", "r");
    let padding = total
        .checked_sub(fixed.len())
        .expect("a target length longer than the ARN's fixed parts");
    let arn = job_arn(
        "aws",
        "us-east-1",
        "000000000000",
        &"j".repeat(padding),
        "r",
    );
    assert_eq!(
        arn.len(),
        total,
        "the padded ARN must be exactly {total} characters, got {}",
        arn.len()
    );
    arn
}

/// RULE (stage one): a `JobArn` outside the modelled constraints is refused
/// `400 ValidationException` as MALFORMED — before any job is resolved, so the refusal
/// does not depend on which jobs exist — and nothing is written.
#[tokio::test]
async fn a_job_arn_outside_the_modelled_constraints_is_refused_as_malformed() {
    // Rows differ along WHICH constraint the value misses: the `@length` maximum (one
    // character past it), the `@length` minimum (the empty value), and the pattern — in
    // the partition, the service namespace, the account's digit count, the resource
    // type and the missing run segment.
    let one_past_max = job_arn_of_length(JOB_ARN_MAX + 1);
    let rows: Vec<(&str, String)> = vec![
        ("one character past the 256 maximum", one_past_max),
        ("the empty value, below the 1 minimum", String::new()),
        ("not an ARN at all", "a-job-of-my-own".to_string()),
        (
            "another service's namespace",
            job_arn("aws", "us-east-1", "123456789012", "AgentRFT", "run-1")
                .replace(":sagemaker:", ":bedrock:"),
        ),
        (
            "an account that is not twelve digits",
            job_arn("aws", "us-east-1", "12345", "AgentRFT", "run-1"),
        ),
        (
            "a resource that is not a job",
            "arn:aws:sagemaker:us-east-1:123456789012:endpoint/AgentRFT/run-1".to_string(),
        ),
        (
            "no run segment",
            "arn:aws:sagemaker:us-east-1:123456789012:job/AgentRFT".to_string(),
        ),
    ];

    for (case, arn) in rows {
        for path in [SAMPLE_PATH, SAMPLE_STREAM_PATH] {
            let (engine, router) = mounted_service();
            // A job DOES exist, under the ARN a correct request would carry, so a row is
            // refused because of its own value and not because the account holds no job.
            create_job(&engine, "AgentRFT", "run-1");

            let reply = sample(&router, path, &arn, "traj-1", &chat_payload("go")).await;
            assert_eq!(
                (reply.status, reply.error_code().as_str()),
                (400, "ValidationException"),
                "case {case:?} on {path}: expected 400 ValidationException, got {} {:?} / {}",
                reply.status,
                reply.error_type,
                reply.text()
            );
            assert!(
                !reply.message().is_empty()
                    && reply.message() != state::job_not_found_message(&arn),
                "case {case:?} on {path}: expected the malformed-value refusal rather than the \
                 job-not-found one, got {:?}",
                reply.message()
            );
            assert_eq!(
                reply.content_type.as_deref(),
                Some("application/json"),
                "case {case:?} on {path}: expected a restJson1 error document, got {:?}",
                reply.content_type
            );
            let held = engine.state::<ServiceState>().lock().trajectories.len();
            assert_eq!(
                held, 0,
                "case {case:?} on {path}: expected a refused call to write nothing, got {held} \
                 trajectory/ies"
            );
        }
    }
}

/// RULE (the boundary between the stages): an ARN this data plane's own pattern admits but
/// whose job segments can name no job the control plane could mint passes stage one and is
/// refused by stage two — as unresolvable, naming the ARN the request carried.
#[tokio::test]
async fn a_well_formed_arn_that_can_name_no_created_job_is_refused_as_unresolvable() {
    // The control plane's `JobArn` carries the `JobCategory` (`[a-zA-Z0-9]+`) where this
    // data plane's pattern admits `[a-zA-Z0-9_\-]+`
    // (smithy_query(query="shape",shape="JobArn") on each service's own model), so a
    // category segment carrying `_` or `-` is well formed here and belongs to no job.
    let rows: Vec<(&str, String)> = vec![
        (
            "a category segment carrying an underscore",
            job_arn("aws", "eu-central-1", "123456789012", "rft_job_2", "run-1"),
        ),
        (
            "a category segment carrying a hyphen",
            job_arn("aws", "us-west-2", "123456789012", "agentic-rft", "run-1"),
        ),
        (
            "the empty region the data plane's pattern admits",
            job_arn("aws", "", "123456789012", "AgentRFT", "never-created"),
        ),
        (
            "an ARN at the modelled 256-character maximum",
            job_arn_of_length(JOB_ARN_MAX),
        ),
    ];

    for (case, arn) in rows {
        for path in [SAMPLE_PATH, SAMPLE_STREAM_PATH] {
            let (engine, router) = mounted_service();
            create_job(&engine, "AgentRFT", "run-1");

            let reply = sample(&router, path, &arn, "traj-1", &chat_payload("go")).await;
            assert_eq!(
                (reply.status, reply.error_code().as_str()),
                (400, "ValidationException"),
                "case {case:?} on {path}: expected 400 ValidationException, got {} {:?} / {}",
                reply.status,
                reply.error_type,
                reply.text()
            );
            assert!(
                reply.message().contains(&arn),
                "case {case:?} on {path}: expected the unresolvable-job refusal to name the \
                 requested ARN {arn:?}, got {:?}",
                reply.message()
            );
            assert_eq!(
                reply.content_type.as_deref(),
                Some("application/json"),
                "case {case:?} on {path}: expected a restJson1 error document, got {:?}",
                reply.content_type
            );
            let held = engine.state::<ServiceState>().lock().trajectories.len();
            assert_eq!(
                held, 0,
                "case {case:?} on {path}: expected a refused call to write nothing, got {held} \
                 trajectory/ies"
            );
        }
    }
}
