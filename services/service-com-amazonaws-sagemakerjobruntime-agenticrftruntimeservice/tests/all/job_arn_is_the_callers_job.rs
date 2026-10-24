// SPDX-License-Identifier: CC0-1.0

//! The job a sampling call names is the job the caller is running under.
//!
//! RULE: `Sample` and `SampleWithResponseStream` serve the job their `JobArn` names and
//! capture the turn on that job's `(JobArn, TrajectoryId)` trajectory for EVERY value
//! the member's own modelled constraints admit, and for no other: `JobArn` carries
//! `@length` 1..256 plus
//! `^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:job/[a-zA-Z0-9_\-]+/[a-zA-Z0-9](-*[a-zA-Z0-9]){0,62}$`
//! (`smithy_query(query="shape",shape="JobArn")`), which admit any partition, region,
//! account and job/run name. This service's model binds no operation that creates,
//! lists or deletes a job — the job is created in the SageMaker control plane and this
//! is its data plane — so a freshly mounted runtime holds no job of its own and nothing
//! has to be registered before the first call: a caller sampling against the ARN its
//! own job execution was handed is served, whatever that ARN is. A value the
//! constraints do NOT admit is still refused `400 ValidationException`, and nothing is
//! written for it.
//!
//! The rows span what the rule generalises over, not one request: the partition
//! (`aws`, `aws-cn`, `aws-us-gov`, `aws-iso-b`), the region (including the EMPTY region
//! the pattern's `*` admits), the account, the job-name segment's character classes
//! (letters, digits, `_`, `-`), the run segment's length (one character and its
//! 63-character maximum), the whole ARN's length (at the modelled 256 maximum and one
//! past it), and which of the two sampling operations serves the call.
//!
//! Every answer goes over the wire through a [`MultiServiceRouter`], and the store is
//! read afterwards the way the NEXT request would read it, so what is pinned is the
//! emulator's HTTP behaviour and that the answer came from persisted state.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_sagemakerjobruntime_agenticrftruntimeservice::state;
use service_com_amazonaws_sagemakerjobruntime_agenticrftruntimeservice::ComAmazonawsSagemakerjobruntimeAgenticrftruntimeserviceState as ServiceState;
use service_com_amazonaws_sagemakerjobruntime_agenticrftruntimeservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// This service's own endpoint host, built from the model's endpoint prefix.
const HOST: &str = "job-runtime.sagemaker.us-east-1.amazonaws.com";

/// A SigV4 `Authorization` header whose credential scope names this service's signing
/// name (`sagemaker`). Request authentication is off on the test router, so the
/// signature itself is never checked.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20240101/us-east-1/sagemaker/aws4_request, SignedHeaders=host, \
     Signature=0000000000000000000000000000000000000000000000000000000000000000";

/// The headers `JobArn` and `TrajectoryId` are bound to.
const JOB_ARN_HEADER: &str = "x-amzn-sagemaker-job-arn";
const TRAJECTORY_ID_HEADER: &str = "x-amzn-sagemaker-trajectory-id";

/// The two sampling paths this model binds.
const SAMPLE_PATH: &str = "/sample";
const SAMPLE_STREAM_PATH: &str = "/sample-with-response-stream";

/// `JobArn`'s modelled `@length` bounds
/// (`smithy_query(query="shape",shape="JobArn")`).
const JOB_ARN_MAX: usize = 256;

/// One HTTP answer, reduced to what these tests assert on.
#[derive(Debug)]
struct Reply {
    status: u16,
    error_type: Option<String>,
    content_type: Option<String>,
    body: Vec<u8>,
}

impl Reply {
    /// The bare error code out of `x-amzn-errortype`, which may carry a namespace or a
    /// Coral shape-id suffix alongside the name.
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

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

/// A router carrying exactly this service over one shared engine. The engine comes back
/// too, so a test can read this service's store the way the next request would.
fn mounted_service() -> (Arc<engine::InMemoryEngine>, MultiServiceRouter) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router
        .set_request_authentication(false)
        .register_service(EmulatedService::new(Arc::clone(&engine)));
    (engine, router)
}

async fn sample(
    router: &MultiServiceRouter,
    path: &str,
    job_arn: &str,
    trajectory_id: &str,
    payload: &[u8],
) -> Reply {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://{HOST}{path}"))
        .header("authorization", AUTHORIZATION)
        .header("host", HOST)
        .header(JOB_ARN_HEADER, job_arn)
        .header(TRAJECTORY_ID_HEADER, trajectory_id)
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(payload.to_vec()))
        .expect("a well-formed test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let header = |name: &str| {
        response
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(ToString::to_string)
    };
    let error_type = header("x-amzn-errortype");
    let content_type = header("content-type");
    Reply {
        status,
        error_type,
        content_type,
        body: response.into_body().to_vec(),
    }
}

/// An OpenAI-compatible chat-completions request body.
fn chat_payload(text: &str) -> Vec<u8> {
    serde_json::to_vec(&json!({ "messages": [{ "role": "user", "content": text }] }))
        .expect("a serializable payload")
}

/// A job ARN assembled from the parts the modelled pattern lets vary.
fn job_arn(partition: &str, region: &str, account: &str, job: &str, run: &str) -> String {
    format!("arn:{partition}:sagemaker:{region}:{account}:job/{job}/{run}")
}

/// A job ARN of exactly `total` characters, padded in the job-name segment — which the
/// pattern leaves unbounded (`[a-zA-Z0-9_\-]+`), so the only thing a long ARN can fail
/// is the `@length` maximum.
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

/// RULE (served half): every `JobArn` the modelled constraints admit names the job the
/// call is served for, on both sampling operations, with nothing registered first.
#[tokio::test]
async fn every_modelled_job_arn_is_sampled_and_captured_on_its_own_trajectory() {
    // The run segment at its 63-character maximum: `[a-zA-Z0-9](-*[a-zA-Z0-9]){0,62}`.
    let long_run = format!("r{}", "a".repeat(62));
    let at_max_length = job_arn_of_length(JOB_ARN_MAX);

    // Rows differ along partition, region (including the empty one the pattern admits),
    // account, job-name characters, run length and whole-ARN length.
    let rows: Vec<(&str, String)> = vec![
        (
            "the caller's own job in another region",
            job_arn("aws", "us-west-2", "123456789012", "agentic-rft", "run-1"),
        ),
        (
            "the emulator's own account and region",
            job_arn("aws", "us-east-1", "000000000000", "AgentRFT", "rollout-1"),
        ),
        (
            "the China partition",
            job_arn("aws-cn", "cn-north-1", "123456789012", "AgentRFT", "run-1"),
        ),
        (
            "the GovCloud partition",
            job_arn("aws-us-gov", "us-gov-west-1", "123456789012", "AgentRFT", "run-1"),
        ),
        (
            "an ISO partition",
            job_arn("aws-iso-b", "us-isob-east-1", "123456789012", "AgentRFT", "run-1"),
        ),
        (
            "the empty region the pattern admits",
            job_arn("aws", "", "123456789012", "AgentRFT", "run-1"),
        ),
        (
            "a job name carrying underscores and digits",
            job_arn("aws", "eu-central-1", "123456789012", "rft_job_2", "run-1"),
        ),
        (
            "a run name of one character",
            job_arn("aws", "ap-southeast-2", "123456789012", "AgentRFT", "r"),
        ),
        (
            "a run name at its 63-character maximum",
            job_arn("aws", "us-east-1", "123456789012", "AgentRFT", &long_run),
        ),
        (
            "an ARN at the modelled 256-character maximum",
            at_max_length,
        ),
    ];

    for (case, arn) in rows {
        for (path, streamed, expected_content_type) in [
            (SAMPLE_PATH, false, state::JSON_CONTENT_TYPE),
            (
                SAMPLE_STREAM_PATH,
                true,
                state::EVENT_STREAM_CONTENT_TYPE,
            ),
        ] {
            let (engine, router) = mounted_service();

            // Nothing is registered first, and the runtime holds no job or trajectory
            // of its own: whatever this call is served comes from the ARN it carries.
            {
                let store = engine.state::<ServiceState>();
                let guard = store.lock();
                assert!(
                    guard.trajectories.is_empty(),
                    "case {case:?} on {path}: expected a freshly mounted runtime to hold no \
                     trajectory, got {}",
                    guard.trajectories.len()
                );
            }

            let first = sample(&router, path, &arn, "traj-1", &chat_payload("go")).await;
            assert_eq!(
                first.status,
                200,
                "case {case:?} on {path}: expected 200 for a job the caller names, got {} {:?} \
                 / {}",
                first.status,
                first.error_type,
                first.text()
            );
            assert_eq!(
                first.content_type.as_deref(),
                Some(expected_content_type),
                "case {case:?} on {path}: expected Content-Type {expected_content_type:?}, got \
                 {:?}",
                first.content_type
            );
            assert!(
                !first.body.is_empty(),
                "case {case:?} on {path}: expected a payload, got an empty body"
            );
            if streamed {
                let text = first.text();
                assert!(
                    text.starts_with("data: ") && text.ends_with("data: [DONE]\n\n"),
                    "case {case:?} on {path}: expected a server-sent-event stream terminated by \
                     [DONE], got {text:?}"
                );
            }

            // The turn the call served is the one the store holds, under THIS ARN.
            {
                let store = engine.state::<ServiceState>();
                let guard = store.lock();
                let trajectory = guard.trajectory(&arn, "traj-1").unwrap_or_else(|| {
                    panic!(
                        "case {case:?} on {path}: expected a trajectory under the ARN the call \
                         carried"
                    )
                });
                assert_eq!(
                    trajectory.turns.len(),
                    1,
                    "case {case:?} on {path}: expected one captured turn, got {}",
                    trajectory.turns.len()
                );
                let turn = &trajectory.turns[0];
                assert_eq!(
                    turn.streamed, streamed,
                    "case {case:?} on {path}: expected streamed={streamed}, got {}",
                    turn.streamed
                );
                assert_eq!(
                    turn.response_body, first.body,
                    "case {case:?} on {path}: expected the served body to be the recorded one, \
                     got {} served vs {} recorded byte(s)",
                    first.body.len(),
                    turn.response_body.len()
                );
                assert_eq!(
                    trajectory.job_arn, arn,
                    "case {case:?} on {path}: expected the trajectory to record the ARN the call \
                     carried, got {:?}",
                    trajectory.job_arn
                );
            }

            // The answer is derived from the ARN the request carried: the turn's
            // completion id is the one that ARN hashes to.
            let expected_id = state::completion_id(&arn, "traj-1", 0);
            assert!(
                first.text().contains(&expected_id),
                "case {case:?} on {path}: expected the completion id {expected_id:?} derived from \
                 the request's own job ARN, got {:?}",
                first.text()
            );

            // A second call appends to the SAME trajectory, so the first call is what
            // created it rather than a fixture.
            let second = sample(&router, path, &arn, "traj-1", &chat_payload("again")).await;
            assert_eq!(
                second.status,
                200,
                "case {case:?} on {path}: expected 200 for the second turn, got {} / {}",
                second.status,
                second.text()
            );
            let store = engine.state::<ServiceState>();
            let guard = store.lock();
            let turns = guard
                .trajectory(&arn, "traj-1")
                .map_or(0, |trajectory| trajectory.turns.len());
            assert_eq!(
                turns, 2,
                "case {case:?} on {path}: expected two captured turns, got {turns}"
            );
            let second_id = state::completion_id(&arn, "traj-1", 1);
            assert!(
                second.text().contains(&second_id),
                "case {case:?} on {path}: expected the second turn's completion id \
                 {second_id:?}, got {:?}",
                second.text()
            );
        }
    }
}

/// RULE (refused half): a `JobArn` the modelled constraints do NOT admit is refused
/// `400 ValidationException` and nothing is written — so the rows above are about what
/// the model admits, not about accepting any header at all.
#[tokio::test]
async fn a_job_arn_outside_the_modelled_constraints_is_still_refused() {
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
            let reply = sample(&router, path, &arn, "traj-1", &chat_payload("go")).await;
            assert_eq!(
                (reply.status, reply.error_code().as_str()),
                (400, "ValidationException"),
                "case {case:?} on {path}: expected 400 ValidationException, got {} {:?} / {}",
                reply.status,
                reply.error_type,
                reply.text()
            );
            let document: Value = serde_json::from_slice(&reply.body).unwrap_or(Value::Null);
            assert!(
                document.get("Message").and_then(Value::as_str).is_some(),
                "case {case:?} on {path}: expected a restJson1 error document with a Message, \
                 got {:?}",
                reply.text()
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
