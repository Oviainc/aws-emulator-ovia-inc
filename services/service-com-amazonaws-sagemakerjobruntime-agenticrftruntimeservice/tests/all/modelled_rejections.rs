// SPDX-License-Identifier: CC0-1.0

//! How this service claims a request, and the modelled rejections each of its four
//! operations answers once the request reaches it.
//!
//! Four rules, each asserted over a TABLE whose rows differ along what the rule
//! generalises over:
//!
//! 1. CLAIMING — this service claims a request only when the resolved routing key names
//!    it (the shared `sagemaker` signing name, or its own `job-runtime.sagemaker`
//!    endpoint) AND the request addresses one of the four URI paths its model binds. The
//!    signing name is shared with every other SageMaker-family service, so a claim on
//!    the signing name alone makes every sibling claim every request and the router
//!    answers `500 AmbiguousService` for all of it.
//! 2. JOB RESOLUTION — a sampling call for a `JobArn` this runtime holds no job for is
//!    refused `400 ValidationException`, with a message naming the job ARN that was
//!    looked up.
//! 3. TRAJECTORY RESOLUTION — `CompleteRollout` and `UpdateReward` answer
//!    `404 ResourceNotFoundException` for a `(JobArn, TrajectoryId)` pair the store
//!    holds no trajectory for, with a message naming the trajectory and the account.
//! 4. INPUT VALIDATION BEFORE RESOURCE RESOLUTION — a value outside what the model
//!    declares (a `TrajectoryId` outside `@length` 1..256, a `Status` outside the
//!    `CompletionStatus` enum) or an empty `Rewards` list is refused
//!    `400 ValidationException`, naming the member and the requirement, whether or not
//!    the trajectory it names exists.
//!
//! Every answer goes over the wire through a [`MultiServiceRouter`], so what is pinned
//! is the emulator's HTTP behaviour: the status, the restJson1 `x-amzn-errortype` and
//! `application/json` envelope, and the `Message` member.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_sagemakerjobruntime_agenticrftruntimeservice::state;
use service_com_amazonaws_sagemakerjobruntime_agenticrftruntimeservice::ComAmazonawsSagemakerjobruntimeAgenticrftruntimeserviceState as ServiceState;
use service_com_amazonaws_sagemakerjobruntime_agenticrftruntimeservice::EmulatedService;
use smithy_server_runtime::{MultiServiceRouter, Service};

/// A SigV4 `Authorization` header whose credential scope names `service` — the primary
/// routing signal `resolve_route_key` reads. Request authentication is off on the test
/// router, so the signature itself is never checked.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/us-east-1/{service}/aws4_request, \
         SignedHeaders=host, Signature=\
         0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// The header `JobArn` is bound to.
const JOB_ARN_HEADER: &str = "x-amzn-sagemaker-job-arn";

/// The header `TrajectoryId` is bound to on the sampling operations.
const TRAJECTORY_ID_HEADER: &str = "x-amzn-sagemaker-trajectory-id";

/// The paths this service's model binds, one per operation.
const SAMPLE_PATH: &str = "/sample";
const SAMPLE_STREAM_PATH: &str = "/sample-with-response-stream";
const COMPLETE_ROLLOUT_PATH: &str = "/complete-rollout";
const UPDATE_REWARD_PATH: &str = "/update-reward";

/// A job ARN satisfying the modelled
/// `^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:job/[a-zA-Z0-9_\-]+/[a-zA-Z0-9](-*[a-zA-Z0-9]){0,62}$`
/// pattern, so a row can vary the region, the account and the job name.
fn job_arn(region: &str, account: &str, job: &str, run: &str) -> String {
    format!("arn:aws:sagemaker:{region}:{account}:job/{job}/{run}")
}

/// One HTTP answer, reduced to what these tests assert on.
#[derive(Debug)]
struct Reply {
    status: u16,
    error_type: Option<String>,
    content_type: Option<String>,
    body: Vec<u8>,
}

impl Reply {
    /// The bare error code out of `x-amzn-errortype`, which the recorded front door
    /// sends in a Coral-suffixed form (`ValidationException:http://…`); an SDK reads the
    /// name before the colon.
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

    /// The `Message` member of a restJson1 error document.
    ///
    /// Read under the name the MODEL declares (`Message`, on every error shape of this
    /// service) and under no other: the framework's own pre-dispatch refusal envelope
    /// writes a lowercase `message` instead, so accepting both would hide a refusal that
    /// never reached this service's modelled `ValidationException`.
    fn message(&self) -> String {
        serde_json::from_slice::<Value>(&self.body)
            .ok()
            .and_then(|body| {
                body.get("Message")
                    .and_then(Value::as_str)
                    .map(ToString::to_string)
            })
            .unwrap_or_default()
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

/// A router carrying exactly this service over one shared engine.
fn mounted_service() -> (Arc<engine::InMemoryEngine>, MultiServiceRouter) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router
        .set_request_authentication(false)
        .register_service(EmulatedService::new(Arc::clone(&engine)));
    (engine, router)
}

/// Record `job_arn` as a job this runtime serves rollouts for — what the control plane's
/// job creation does, which this service models no operation for.
fn register_job(engine: &engine::InMemoryEngine, job_arn: &str) {
    engine.state::<ServiceState>().lock().register_job(job_arn);
}

async fn send(
    router: &MultiServiceRouter,
    path: &str,
    headers: &[(&str, &str)],
    body: Vec<u8>,
) -> Reply {
    let mut request = http::Request::builder()
        .method("POST")
        .uri(format!("https://job-runtime.sagemaker.us-east-1.amazonaws.com{path}"))
        .header("authorization", authorization("sagemaker"))
        .header("host", "job-runtime.sagemaker.us-east-1.amazonaws.com");
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let request = request
        .body(bytes::Bytes::from(body))
        .expect("a well-formed test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(ToString::to_string);
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .map(ToString::to_string);
    Reply {
        status,
        error_type,
        content_type,
        body: response.into_body().to_vec(),
    }
}

async fn sample(
    router: &MultiServiceRouter,
    path: &str,
    job: &str,
    trajectory_id: &str,
    payload: &[u8],
) -> Reply {
    send(
        router,
        path,
        &[
            (JOB_ARN_HEADER, job),
            (TRAJECTORY_ID_HEADER, trajectory_id),
            ("content-type", "application/json"),
        ],
        payload.to_vec(),
    )
    .await
}

async fn complete_rollout(
    router: &MultiServiceRouter,
    job: &str,
    trajectory_id: &str,
    status: Option<&str>,
) -> Reply {
    let mut body = json!({ "TrajectoryId": trajectory_id, "ClientToken": "tok" });
    if let Some(status) = status {
        body["Status"] = Value::String(status.to_string());
    }
    send(
        router,
        COMPLETE_ROLLOUT_PATH,
        &[(JOB_ARN_HEADER, job), ("content-type", "application/json")],
        serde_json::to_vec(&body).expect("a serializable body"),
    )
    .await
}

async fn update_reward(
    router: &MultiServiceRouter,
    job: &str,
    trajectory_id: &str,
    rewards: &[f64],
) -> Reply {
    let body = json!({
        "TrajectoryId": trajectory_id,
        "Rewards": rewards,
        "ClientToken": "tok",
    });
    send(
        router,
        UPDATE_REWARD_PATH,
        &[(JOB_ARN_HEADER, job), ("content-type", "application/json")],
        serde_json::to_vec(&body).expect("a serializable body"),
    )
    .await
}

/// An OpenAI-compatible chat-completions request body.
fn chat_payload(text: &str) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "messages": [{ "role": "user", "content": text }],
    }))
    .expect("a serializable payload")
}

// ---------------------------------------------------------------------------
// 1. Claiming
// ---------------------------------------------------------------------------

/// RULE: this service claims a request only when the resolved routing key names it (the
/// shared `sagemaker` signing name, or its own endpoint) AND the request addresses one
/// of the URI paths its model binds.
#[tokio::test]
async fn the_service_claims_its_own_path_space_under_the_shared_signing_name() {
    let service = EmulatedService::new(Arc::new(engine::InMemoryEngine::new()));

    // Rows differ along the routing-key SOURCE (a SigV4 credential scope, or an
    // endpoint host for an unsigned request) and along the path: the four paths this
    // model binds, a path another service sharing the signing name owns, the root path
    // the awsJson1_1 control plane answers on, and a path no SageMaker service binds.
    let rows: [(&str, Option<&str>, &str, &str, bool); 11] = [
        ("sigv4 sagemaker, /sample", Some("sagemaker"), "job-runtime.sagemaker.us-east-1.amazonaws.com", SAMPLE_PATH, true),
        ("sigv4 sagemaker, /sample-with-response-stream", Some("sagemaker"), "job-runtime.sagemaker.us-east-1.amazonaws.com", SAMPLE_STREAM_PATH, true),
        ("sigv4 sagemaker, /complete-rollout", Some("sagemaker"), "job-runtime.sagemaker.us-east-1.amazonaws.com", COMPLETE_ROLLOUT_PATH, true),
        ("sigv4 sagemaker, /update-reward", Some("sagemaker"), "job-runtime.sagemaker.us-east-1.amazonaws.com", UPDATE_REWARD_PATH, true),
        ("unsigned, addressed at this endpoint host", None, "job-runtime.sagemaker.us-east-1.amazonaws.com", SAMPLE_PATH, true),
        ("sigv4 sagemaker, the runtime sibling's path", Some("sagemaker"), "runtime.sagemaker.us-east-1.amazonaws.com", "/endpoints/my-endpoint/invocations", false),
        ("sigv4 sagemaker, the A2I sibling's path", Some("sagemaker"), "a2i-runtime.sagemaker.us-east-1.amazonaws.com", "/human-loops", false),
        ("sigv4 sagemaker, the FeatureStore sibling's path", Some("sagemaker"), "featurestore-runtime.sagemaker.us-east-1.amazonaws.com", "/FeatureGroup/fg-1", false),
        ("sigv4 sagemaker, the control plane's root path", Some("sagemaker"), "api.sagemaker.us-east-1.amazonaws.com", "/", false),
        ("sigv4 sagemaker, a path no SageMaker service binds", Some("sagemaker"), "api.sagemaker.us-east-1.amazonaws.com", "/not-a-modelled-path", false),
        ("sigv4 another signing name, this service's path", Some("bedrock"), "bedrock.us-east-1.amazonaws.com", SAMPLE_PATH, false),
    ];

    for (case, signing_name, host, path, expected) in rows {
        let mut request = http::Request::builder()
            .method("POST")
            .uri(format!("https://{host}{path}"))
            .header("host", host);
        if let Some(signing_name) = signing_name {
            request = request.header("authorization", authorization(signing_name));
        }
        let (parts, _body) = request
            .body(bytes::Bytes::new())
            .expect("a well-formed test request")
            .into_parts();
        let actual = service.can_handle(&parts);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected can_handle={expected} for {path}, got {actual}"
        );
    }
}

// ---------------------------------------------------------------------------
// 2. Job resolution
// ---------------------------------------------------------------------------

/// RULE: a sampling call whose `JobArn` names no job this runtime holds is refused
/// `400 ValidationException`, in a restJson1 error document whose `Message` names the
/// job ARN that was looked up; a job the store does hold is sampled.
#[tokio::test]
async fn a_sampling_call_for_an_unknown_job_is_refused_as_invalid() {
    // Rows differ along the ARN's region, account and job/run names — every one is
    // well-formed and none of them is in the store.
    let unknown = [
        (
            "ghost job in us-west-2",
            job_arn("us-west-2", "000000000000", "AgentRFT", "probe-ghost-cdf5a33f"),
        ),
        (
            "ghost job in another region",
            job_arn("eu-central-1", "000000000000", "AgentRFT", "another-ghost"),
        ),
        (
            "ghost job under another account",
            job_arn("us-east-1", "123456789012", "rft_job-2", "run-1"),
        ),
        (
            "ghost job whose run name is one character",
            job_arn("us-east-1", "000000000000", "AgentRFT", "r"),
        ),
    ];

    for (case, arn) in unknown {
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
            assert_eq!(
                reply.content_type.as_deref(),
                Some("application/json"),
                "case {case:?} on {path}: expected a restJson1 error document, got {:?}",
                reply.content_type
            );
            assert_eq!(
                reply.message(),
                format!("Could not find job with jobArn {arn}"),
                "case {case:?} on {path}: expected the job-not-found message, got {:?}",
                reply.message()
            );
            let held = engine.state::<ServiceState>().lock().trajectories.len();
            assert_eq!(
                held, 0,
                "case {case:?} on {path}: expected a refused call to write nothing, got {held} \
                 trajectory/ies"
            );
        }
    }

    // The same calls against jobs the store DOES hold: the two it is seeded with (for a
    // caller that creates no job of its own) and one registered here.
    let registered = job_arn("us-east-1", "000000000000", "AgentRFT", "registered-job");
    let mut known = state::seed_job_arns();
    known.push(registered.clone());
    for arn in known {
        for path in [SAMPLE_PATH, SAMPLE_STREAM_PATH] {
            let (engine, router) = mounted_service();
            register_job(&engine, &registered);
            let reply = sample(&router, path, &arn, "traj-1", &chat_payload("go")).await;
            assert_eq!(
                reply.status,
                200,
                "case {arn:?} on {path}: expected a known job to be sampled, got {} / {}",
                reply.status,
                reply.text()
            );
            // The turn came from, and is readable in, the store the next call reads.
            let store = engine.state::<ServiceState>();
            let guard = store.lock();
            let turns = guard
                .trajectory(&arn, "traj-1")
                .map_or(0, |trajectory| trajectory.turns.len());
            assert_eq!(
                turns, 1,
                "case {arn:?} on {path}: expected one captured turn in state, got {turns}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// 3. Trajectory resolution
// ---------------------------------------------------------------------------

/// RULE: `CompleteRollout` and `UpdateReward` answer `404 ResourceNotFoundException` for
/// a `(JobArn, TrajectoryId)` pair the store holds no trajectory for, with a `Message`
/// naming the trajectory and the account the call is served for.
#[tokio::test]
async fn an_unknown_trajectory_is_reported_as_not_found() {
    // Rows differ along WHY the pair is unknown: nothing was ever sampled, the id
    // differs in case, the id differs in length, the job differs — and along the job
    // ARN, which is a job the runtime does not hold in the last row (a trajectory
    // lookup, unlike a sampling call, does not resolve the job first).
    let seeded_job = state::seed_job_arns()
        .first()
        .cloned()
        .expect("the store seeds at least one job");
    let second_job = state::seed_job_arns()
        .get(1)
        .cloned()
        .expect("the store seeds two jobs");
    let rows: [(&str, String, String); 5] = [
        ("never sampled", seeded_job.clone(), "never-sampled".to_string()),
        (
            "trajectory id differing by case",
            seeded_job.clone(),
            "TRAJ-1".to_string(),
        ),
        (
            "trajectory id at the modelled 256 maximum",
            seeded_job.clone(),
            "t".repeat(256),
        ),
        (
            "another job of the same runtime",
            second_job,
            "traj-1".to_string(),
        ),
        (
            "a job the runtime does not hold",
            job_arn("us-west-2", "000000000000", "AgentRFT", "wire-sjr-traj-778a2a85"),
            "traj-1".to_string(),
        ),
    ];

    for (case, arn, trajectory_id) in rows {
        let (engine, router) = mounted_service();
        // One trajectory exists, under the first seeded job and the id `traj-1`, so a
        // row that is not found is not found because of its own pair.
        let seeded = sample(&router, SAMPLE_PATH, &seeded_job, "traj-1", &chat_payload("go")).await;
        assert_eq!(
            seeded.status,
            200,
            "case {case:?}: expected the seeding Sample to answer 200, got {} / {}",
            seeded.status,
            seeded.text()
        );

        for (operation, reply) in [
            (
                "CompleteRollout",
                complete_rollout(&router, &arn, &trajectory_id, Some("ready")).await,
            ),
            (
                "UpdateReward",
                update_reward(&router, &arn, &trajectory_id, &[1.0]).await,
            ),
        ] {
            assert_eq!(
                (reply.status, reply.error_code().as_str()),
                (404, "ResourceNotFoundException"),
                "case {case:?}/{operation}: expected 404 ResourceNotFoundException, got {} {:?} \
                 / {}",
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
                format!("Trajectory {trajectory_id} not found for account 000000000000"),
                "case {case:?}/{operation}: expected the trajectory-not-found message, got {:?}",
                reply.message()
            );
        }

        let store = engine.state::<ServiceState>();
        let held = store.lock().trajectories.len();
        assert_eq!(
            held, 1,
            "case {case:?}: expected the refused lookups to create nothing, got {held} \
             trajectory/ies"
        );
    }
}

// ---------------------------------------------------------------------------
// 3b. The retained-payload bound
// ---------------------------------------------------------------------------

/// RULE: a captured turn retains the caller's payload for the life of the process, so
/// the payload's SIZE is bounded: a `Body` over `state::MAX_TURN_BODY_BYTES` is refused
/// with nothing written, and what IS captured is counted against the store's
/// retained-byte total.
#[tokio::test]
async fn a_payload_too_large_to_retain_is_refused_and_captured_bytes_are_counted() {
    let seeded_job = state::seed_job_arns()
        .first()
        .cloned()
        .expect("the store seeds at least one job");

    // Rows differ along the size relative to the bound: just over it, far over it, and
    // AT it (which must be accepted, so the refusals above are about the bound and not
    // about size in general).
    let rows: [(&str, usize, bool); 3] = [
        ("one byte over the per-turn bound", state::MAX_TURN_BODY_BYTES + 1, false),
        ("far over the per-turn bound", state::MAX_TURN_BODY_BYTES * 4, false),
        ("exactly at the per-turn bound", state::MAX_TURN_BODY_BYTES, true),
    ];

    for (case, size, accepted) in rows {
        for path in [SAMPLE_PATH, SAMPLE_STREAM_PATH] {
            let (engine, router) = mounted_service();
            let payload = vec![b'x'; size];
            let reply = sample(&router, path, &seeded_job, "traj-1", &payload).await;
            let store = engine.state::<ServiceState>();
            let guard = store.lock();
            if accepted {
                assert_eq!(
                    reply.status,
                    200,
                    "case {case:?} on {path}: expected a payload at the bound to be captured, \
                     got {} / {}",
                    reply.status,
                    reply.text()
                );
                let retained = guard.retained_body_bytes();
                assert!(
                    retained >= size,
                    "case {case:?} on {path}: expected the captured payload to be counted \
                     (>= {size} bytes), got {retained}"
                );
            } else {
                assert_eq!(
                    (reply.status, reply.error_code().as_str()),
                    (400, "ValidationException"),
                    "case {case:?} on {path}: expected 400 ValidationException, got {} {:?}",
                    reply.status,
                    reply.error_type
                );
                assert!(
                    reply.message().contains(&size.to_string())
                        && reply
                            .message()
                            .contains(&state::MAX_TURN_BODY_BYTES.to_string()),
                    "case {case:?} on {path}: expected the message to state the size and the \
                     bound, got {:?}",
                    reply.message()
                );
                assert_eq!(
                    guard.trajectories.len(),
                    0,
                    "case {case:?} on {path}: expected a refused call to write nothing"
                );
                assert_eq!(
                    guard.retained_body_bytes(),
                    0,
                    "case {case:?} on {path}: expected a refused call to retain nothing, got {}",
                    guard.retained_body_bytes()
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 5. A value that is not an ARN
// ---------------------------------------------------------------------------

/// RULE: a `JobArn` that fails the modelled ARN `@pattern` is refused
/// `400 ValidationException` and reported as a malformed ARN naming the value the
/// request sent — on every operation, each of which binds that member.
#[tokio::test]
async fn a_job_arn_that_is_not_an_arn_is_reported_as_malformed() {
    // Rows differ along WHERE the value misses the modelled pattern
    // `^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:job/[a-zA-Z0-9_\-]+/[a-zA-Z0-9](-*[a-zA-Z0-9]){0,62}$`
    // (smithy_query(query="operation",operation="Sample")): not an ARN at all, another
    // service's namespace, an account that is not twelve digits, a missing job-run
    // segment, and a resource that is not a job.
    let malformed = [
        ("not an ARN at all", "not-a-valid-job-arn"),
        (
            "another service's namespace",
            "arn:aws:bedrock:us-east-1:000000000000:job/AgentRFT/run-1",
        ),
        (
            "an account that is not twelve digits",
            "arn:aws:sagemaker:us-east-1:12345:job/AgentRFT/run-1",
        ),
        (
            "no job-run segment",
            "arn:aws:sagemaker:us-east-1:000000000000:job/AgentRFT",
        ),
        (
            "a resource that is not a job",
            "arn:aws:sagemaker:us-east-1:000000000000:endpoint/AgentRFT/run-1",
        ),
    ];

    for (case, arn) in malformed {
        // Every operation binds `JobArn`, so the rule holds on all four paths.
        for (operation, reply) in [
            (
                "Sample",
                {
                    let (_engine, router) = mounted_service();
                    sample(&router, SAMPLE_PATH, arn, "traj-1", &chat_payload("go")).await
                },
            ),
            (
                "SampleWithResponseStream",
                {
                    let (_engine, router) = mounted_service();
                    sample(&router, SAMPLE_STREAM_PATH, arn, "traj-1", &chat_payload("go")).await
                },
            ),
            (
                "CompleteRollout",
                {
                    let (_engine, router) = mounted_service();
                    complete_rollout(&router, arn, "traj-1", Some("ready")).await
                },
            ),
            (
                "UpdateReward",
                {
                    let (_engine, router) = mounted_service();
                    update_reward(&router, arn, "traj-1", &[1.0]).await
                },
            ),
        ] {
            assert_eq!(
                (reply.status, reply.error_code().as_str()),
                (400, "ValidationException"),
                "case {case:?}/{operation}: expected 400 ValidationException, got {} {:?} / {}",
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
                format!("Input arn {arn} is malformed"),
                "case {case:?}/{operation}: expected the malformed-ARN sentence, got {:?}",
                reply.message()
            );
        }
    }
}

// ---------------------------------------------------------------------------
// 4. Input validation, before any resource is resolved
// ---------------------------------------------------------------------------

/// RULE: a value outside what the model declares, or an empty `Rewards` list, is refused
/// `400 ValidationException` naming the member and the requirement — before any job or
/// trajectory is resolved, so the refusal does not depend on the resource existing.
#[tokio::test]
async fn a_value_outside_the_model_is_refused_as_invalid() {
    // A trajectory id over the modelled maximum, in two sizes, and one under the
    // minimum. Source of the bound: smithy_query(query="shape",shape="TrajectoryId") —
    // string, @length min 1 max 256.
    let over_max_257 = "t".repeat(257);
    let over_max_280 = "t".repeat(280);
    let at_max = "t".repeat(256);
    let unknown_job = job_arn("us-west-2", "000000000000", "AgentRFT", "ghost-run");

    // Each row names the fragments the refusal's `Message` must state: the member path
    // and the requirement it failed.
    for (case, path, trajectory_id, expected) in [
        (
            "TrajectoryId one past the 256 maximum",
            SAMPLE_PATH,
            over_max_257.clone(),
            vec![
                "at 'trajectoryId'",
                "Member must have length less than or equal to 256",
            ],
        ),
        (
            "TrajectoryId far past the 256 maximum, streaming",
            SAMPLE_STREAM_PATH,
            over_max_280.clone(),
            vec![
                "at 'trajectoryId'",
                "Member must have length less than or equal to 256",
            ],
        ),
        (
            "TrajectoryId below the 1 minimum",
            SAMPLE_STREAM_PATH,
            String::new(),
            vec!["trajectoryId"],
        ),
    ] {
        let (engine, router) = mounted_service();
        // The job is one the runtime does not hold: a modelled-constraint violation is
        // refused before the job is resolved, so the message must still be about the
        // constraint.
        let reply = sample(&router, path, &unknown_job, &trajectory_id, &chat_payload("go")).await;
        assert_eq!(
            (reply.status, reply.error_code().as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {} {:?} / {}",
            reply.status,
            reply.error_type,
            reply.text()
        );
        assert_eq!(
            reply.content_type.as_deref(),
            Some("application/json"),
            "case {case:?}: expected a restJson1 error document, got {:?}",
            reply.content_type
        );
        for fragment in expected {
            assert!(
                reply.message().contains(fragment),
                "case {case:?}: expected the message to state {fragment:?}, got {:?}",
                reply.message()
            );
        }
        let held = engine.state::<ServiceState>().lock().trajectories.len();
        assert_eq!(
            held, 0,
            "case {case:?}: expected a refused call to write nothing, got {held} trajectory/ies"
        );
    }

    // A trajectory id AT the maximum is accepted, which is what makes the rows above
    // about the bound and not about length in general.
    let (engine, router) = mounted_service();
    let seeded_job = state::seed_job_arns()
        .first()
        .cloned()
        .expect("the store seeds at least one job");
    let accepted = sample(&router, SAMPLE_PATH, &seeded_job, &at_max, &chat_payload("go")).await;
    assert_eq!(
        accepted.status,
        200,
        "case \"TrajectoryId at the 256 maximum\": expected 200, got {} / {}",
        accepted.status,
        accepted.text()
    );
    let held = engine.state::<ServiceState>().lock().trajectories.len();
    assert_eq!(
        held, 1,
        "case \"TrajectoryId at the 256 maximum\": expected the turn to be captured, got {held} \
         trajectory/ies"
    );

    // `Status` outside the modelled `CompletionStatus` enum. Source of the value set:
    // smithy_query(query="shape",shape="CompletionStatus") — members READY (`ready`) and
    // FAILED (`failed`). Rows differ along HOW the value misses the set: a value of its
    // own, the right word in the wrong case, and an empty string.
    for (case, value) in [
        ("a status of its own", "bogus-status"),
        ("the right word, capitalised", "Ready"),
        ("the right word, upper case", "FAILED"),
        ("an empty status", ""),
    ] {
        let (engine, router) = mounted_service();
        // A trajectory that does NOT exist: the enum check comes first.
        let reply = complete_rollout(&router, &seeded_job, "no-such-trajectory", Some(value)).await;
        assert_eq!(
            (reply.status, reply.error_code().as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {} {:?} / {}",
            reply.status,
            reply.error_type,
            reply.text()
        );
        assert_eq!(
            reply.message(),
            format!(
                "1 validation error detected: Value '{value}' at 'status' failed to satisfy \
                 constraint: Member must satisfy enum value set: [ready, failed]"
            ),
            "case {case:?}: expected the enum-value-set clause, got {:?}",
            reply.message()
        );
        let sealed = engine
            .state::<ServiceState>()
            .lock()
            .trajectory(&seeded_job, "no-such-trajectory")
            .is_some();
        assert!(
            !sealed,
            "case {case:?}: expected a refused CompleteRollout to create nothing"
        );
    }

    // Every value the enum DOES declare is accepted on a trajectory that exists.
    for value in ["ready", "failed"] {
        let (engine, router) = mounted_service();
        assert_eq!(
            sample(&router, SAMPLE_PATH, &seeded_job, "traj-1", &chat_payload("go"))
                .await
                .status,
            200,
            "case {value:?}: expected the seeding Sample to answer 200"
        );
        let sealed = complete_rollout(&router, &seeded_job, "traj-1", Some(value)).await;
        assert_eq!(
            sealed.status,
            200,
            "case {value:?}: expected a modelled Status to seal the trajectory, got {} / {}",
            sealed.status,
            sealed.text()
        );
        let store = engine.state::<ServiceState>();
        let guard = store.lock();
        let recorded = guard
            .trajectory(&seeded_job, "traj-1")
            .and_then(|trajectory| trajectory.completion_status)
            .map(state::CompletionStatus::as_str);
        assert_eq!(
            recorded,
            Some(value),
            "case {value:?}: expected the sealed status in state, got {recorded:?}"
        );
    }

    // An empty `Rewards` list. `Rewards` is @required targeting `DoubleList`, which
    // declares no @length minimum, so the empty list satisfies every modelled
    // constraint and the service refuses it itself.
    for (case, trajectory_id) in [
        ("empty Rewards, trajectory does not exist", "no-such-trajectory"),
        ("empty Rewards, trajectory exists", "traj-1"),
    ] {
        let (engine, router) = mounted_service();
        assert_eq!(
            sample(&router, SAMPLE_PATH, &seeded_job, "traj-1", &chat_payload("go"))
                .await
                .status,
            200,
            "case {case:?}: expected the seeding Sample to answer 200"
        );
        let reply = update_reward(&router, &seeded_job, trajectory_id, &[]).await;
        assert_eq!(
            (reply.status, reply.error_code().as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {} {:?} / {}",
            reply.status,
            reply.error_type,
            reply.text()
        );
        assert_eq!(
            reply.message(),
            "Rewards list cannot be empty",
            "case {case:?}: expected the empty-rewards sentence, got {:?}",
            reply.message()
        );
        let store = engine.state::<ServiceState>();
        let guard = store.lock();
        let rewards = guard
            .trajectory(&seeded_job, "traj-1")
            .and_then(|trajectory| trajectory.rewards.clone());
        assert!(
            rewards.is_none(),
            "case {case:?}: expected a refused UpdateReward to record nothing, got {rewards:?}"
        );
    }
}
