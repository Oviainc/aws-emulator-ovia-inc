// SPDX-License-Identifier: CC0-1.0

//! Cross-service routing for the SigV4 signing name `sagemaker`, shared by NINE emulated
//! services.
//!
//! The router resolves a request's routing key from the SigV4 credential scope first, so
//! every SageMaker-family service sees the same key `sagemaker`: a service that claims on
//! the signing name ALONE claims every sibling's traffic too, the router finds nine
//! claimants and answers `500 AmbiguousService` — with an awsJson1_1 envelope and an
//! error type no AWS model declares — for ALL of it. AWS separates them by endpoint host
//! (`api.sagemaker`, `runtime.sagemaker`, `a2i-runtime.sagemaker`, `edge.sagemaker`,
//! `featurestore-runtime.sagemaker`, `metrics.sagemaker`,
//! `training-session-runtime.sagemaker`, `job-runtime.sagemaker`), which an emulator
//! cannot use: a client configured with one local endpoint sends the same `Host` for all
//! of them.
//!
//! Each one therefore also requires the request to address its OWN modeled URI path
//! space — the root path for the awsJson1_1 control plane, whose operations all arrive as
//! `POST /`. This suite pins that: the claim table (who claims what) and the answers the
//! AgenticRFT job runtime's four operations give through the whole router.

use std::sync::Arc;

use bytes::Bytes;
use smithy_server_runtime::{MultiServiceRouter, Service};

/// A SigV4 `Authorization` header whose credential scope names `signing_name`. The
/// signature is never checked: the router here runs with request authentication off.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/us-east-1/{signing_name}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// Every emulated service whose SigV4 signing name is `sagemaker`, over one engine, as
/// `core/router/src/lib.rs` mounts them.
fn sagemaker_family(engine: &Arc<engine::InMemoryEngine>) -> Vec<Box<dyn Service>> {
    vec![
        Box::new(service_com_amazonaws_sagemaker_sagemaker::EmulatedService::new(
            Arc::clone(engine),
        )),
        Box::new(
            service_com_amazonaws_sagemakerruntime_amazonsagemakerruntime::EmulatedService::new(
                Arc::clone(engine),
            ),
        ),
        Box::new(
            service_com_amazonaws_sagemakerruntimehttp2_amazonsagemakerruntimehttp2::EmulatedService::new(
                Arc::clone(engine),
            ),
        ),
        Box::new(
            service_com_amazonaws_sagemakera2iruntime_amazonsagemakera2iruntime::EmulatedService::new(
                Arc::clone(engine),
            ),
        ),
        Box::new(
            service_com_amazonaws_sagemakeredge_amazonsagemakeredge::EmulatedService::new(
                Arc::clone(engine),
            ),
        ),
        Box::new(
            service_com_amazonaws_sagemakerfeaturestoreruntime_amazonsagemakerfeaturestoreruntime::EmulatedService::new(
                Arc::clone(engine),
            ),
        ),
        Box::new(
            service_com_amazonaws_sagemakermetrics_sagemakermetricsservice::EmulatedService::new(
                Arc::clone(engine),
            ),
        ),
        Box::new(
            service_com_amazonaws_sagemakertrainingsessionruntime_legorouterservice::EmulatedService::new(
                Arc::clone(engine),
            ),
        ),
        Box::new(
            service_com_amazonaws_sagemakerjobruntime_agenticrftruntimeservice::EmulatedService::new(
                Arc::clone(engine),
            ),
        ),
    ]
}

/// The whole family mounted in a router, over the engine the caller keeps a handle on.
fn build_router(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_sagemaker_sagemaker::EmulatedService::new(
        Arc::clone(engine),
    ));
    router.register_service(
        service_com_amazonaws_sagemakerruntime_amazonsagemakerruntime::EmulatedService::new(
            Arc::clone(engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_sagemakerruntimehttp2_amazonsagemakerruntimehttp2::EmulatedService::new(
            Arc::clone(engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_sagemakera2iruntime_amazonsagemakera2iruntime::EmulatedService::new(
            Arc::clone(engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_sagemakeredge_amazonsagemakeredge::EmulatedService::new(Arc::clone(
            engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_sagemakerfeaturestoreruntime_amazonsagemakerfeaturestoreruntime::EmulatedService::new(
            Arc::clone(engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_sagemakermetrics_sagemakermetricsservice::EmulatedService::new(
            Arc::clone(engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_sagemakertrainingsessionruntime_legorouterservice::EmulatedService::new(
            Arc::clone(engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_sagemakerjobruntime_agenticrftruntimeservice::EmulatedService::new(
            Arc::clone(engine),
        ),
    );
    router
}

/// The Smithy shape ids of the family members, as `identity().shape_id()` reports them.
const CONTROL_PLANE: &str = "com.amazonaws.sagemaker#SageMaker";
const ENDPOINT_RUNTIME: &str = "com.amazonaws.sagemakerruntime#AmazonSageMakerRuntime";
const ENDPOINT_RUNTIME_HTTP2: &str =
    "com.amazonaws.sagemakerruntimehttp2#AmazonSageMakerRuntimeHttp2";
const A2I_RUNTIME: &str = "com.amazonaws.sagemakera2iruntime#AmazonSageMakerA2IRuntime";
const EDGE: &str = "com.amazonaws.sagemakeredge#AmazonSageMakerEdge";
const FEATURESTORE_RUNTIME: &str =
    "com.amazonaws.sagemakerfeaturestoreruntime#AmazonSageMakerFeatureStoreRuntime";
const METRICS: &str = "com.amazonaws.sagemakermetrics#SageMakerMetricsService";
const TRAINING_SESSION_RUNTIME: &str =
    "com.amazonaws.sagemakertrainingsessionruntime#LegoRouterService";
const JOB_RUNTIME: &str = "com.amazonaws.sagemakerjobruntime#AgenticRFTRuntimeService";

/// Which mounted services claim a `sagemaker`-signed request for `method path`, with
/// `target` as its `X-Amz-Target` when it carries one (how an awsJson1_1 request names
/// the service and operation it is for).
fn claimants(
    family: &[Box<dyn Service>],
    method: &str,
    path: &str,
    target: Option<&str>,
) -> Vec<String> {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("https://emulator.local{path}"))
        .header("host", "emulator.local")
        .header("authorization", authorization("sagemaker"));
    if let Some(target) = target {
        builder = builder.header("x-amz-target", target);
    }
    let request = builder.body(Bytes::new()).expect("build the request");
    let (parts, _body) = request.into_parts();
    family
        .iter()
        .filter(|service| service.can_handle(&parts))
        .map(|service| service.definition().identity().shape_id().to_string())
        .collect()
}

/// RULE: a request signed under the shared `sagemaker` signing name is claimed by the one
/// family member whose modelled wire surface it addresses — the control plane for a root
/// `POST` whose `X-Amz-Target` names IT, each REST data-plane service for its own URI
/// path space.
#[test]
fn each_service_sharing_the_sagemaker_signing_name_claims_only_its_own_path_space() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let family = sagemaker_family(&engine);

    /// One row: a request under the shared signing name, and every service that may
    /// claim it.
    struct Row {
        case: &'static str,
        method: &'static str,
        path: &'static str,
        /// The `X-Amz-Target`, for a row addressed in the awsJson1_1 RPC form.
        target: Option<&'static str>,
        claimants: Vec<&'static str>,
    }

    // Rows differ along the wire surface they address: one row per family member's own
    // surface (the control plane's RPC form, each REST member's paths), the root path
    // without an `X-Amz-Target` and with one naming another service, and a path no
    // member binds.
    let rows = [
        Row {
            case: "control plane, awsJson1_1 root path targeted at it",
            method: "POST",
            path: "/",
            target: Some("SageMaker.ListModels"),
            claimants: vec![CONTROL_PLANE],
        },
        Row {
            case: "root path with no X-Amz-Target names no service",
            method: "POST",
            path: "/",
            target: None,
            claimants: vec![],
        },
        Row {
            case: "root path targeted at a service that is not this family's",
            method: "POST",
            path: "/",
            target: Some("DynamoDB_20120810.GetItem"),
            claimants: vec![],
        },
        Row {
            case: "a REST member's path with the control plane's X-Amz-Target \
                   contradicts itself",
            method: "POST",
            path: "/sample",
            target: Some("SageMaker.ListModels"),
            // The target names the control plane, which claims only its own protocol's
            // path; the path is the job runtime's, which claims it whatever the header
            // says, so exactly one service still serves it.
            claimants: vec![JOB_RUNTIME],
        },
        Row {
            case: "endpoint runtime invocations (a collision this change does not resolve)",
            method: "POST",
            path: "/endpoints/my-endpoint/invocations",
            target: None,
            // The two endpoint runtimes publish the SAME `/endpoints/…` space (AWS
            // separates them by port, not by path), so both still claim it and the
            // router still answers their traffic `500 AmbiguousService`. That pair's own
            // collision PRE-DATES this change and is NOT resolved here — it is pinned as
            // a known-unresolved claim, not as correct behaviour. What matters for this
            // suite is that neither of them claims any other member's paths.
            claimants: vec![ENDPOINT_RUNTIME, ENDPOINT_RUNTIME_HTTP2],
        },
        Row {
            case: "a2i human loops",
            method: "GET",
            path: "/human-loops",
            target: None,
            claimants: vec![A2I_RUNTIME],
        },
        Row {
            case: "edge device registration",
            method: "POST",
            path: "/GetDeviceRegistration",
            target: None,
            claimants: vec![EDGE],
        },
        Row {
            case: "featurestore record",
            method: "GET",
            path: "/FeatureGroup/fg-1",
            target: None,
            claimants: vec![FEATURESTORE_RUNTIME],
        },
        Row {
            case: "metrics ingestion",
            method: "PUT",
            path: "/BatchPutMetrics",
            target: None,
            claimants: vec![METRICS],
        },
        Row {
            case: "training session sampling",
            method: "POST",
            path: "/training-sessions/session-1/sample",
            target: None,
            claimants: vec![TRAINING_SESSION_RUNTIME],
        },
        Row {
            case: "training session operations under /v1",
            method: "POST",
            path: "/v1/training-sessions/session-1/operations",
            target: None,
            claimants: vec![TRAINING_SESSION_RUNTIME],
        },
        Row {
            case: "job runtime sample",
            method: "POST",
            path: "/sample",
            target: None,
            claimants: vec![JOB_RUNTIME],
        },
        Row {
            case: "job runtime streaming sample",
            method: "POST",
            path: "/sample-with-response-stream",
            target: None,
            claimants: vec![JOB_RUNTIME],
        },
        Row {
            case: "job runtime complete rollout",
            method: "POST",
            path: "/complete-rollout",
            target: None,
            claimants: vec![JOB_RUNTIME],
        },
        Row {
            case: "job runtime update reward",
            method: "POST",
            path: "/update-reward",
            target: None,
            claimants: vec![JOB_RUNTIME],
        },
        Row {
            case: "a path no SageMaker service binds",
            method: "POST",
            path: "/not-a-sagemaker-path",
            target: None,
            claimants: vec![],
        },
    ];

    for row in rows {
        let actual = claimants(&family, row.method, row.path, row.target);
        assert_eq!(
            actual, row.claimants,
            "case {:?}: expected {:?} to claim {} {} (target {:?}), got {actual:?}",
            row.case, row.claimants, row.method, row.path, row.target
        );
    }
}

/// One answer from the whole router, reduced to what the rows assert on.
struct Answer {
    status: u16,
    error_type: Option<String>,
    content_type: Option<String>,
    body: String,
}

impl Answer {
    /// The bare error code out of `x-amzn-errortype`, which restJson1 names the error
    /// shape in; the router's own refusals set it too, so `AmbiguousService` and
    /// `UnknownService` are visible here.
    fn error_code(&self) -> String {
        self.error_type
            .clone()
            .unwrap_or_default()
            .split(':')
            .next()
            .unwrap_or_default()
            .rsplit('#')
            .next()
            .unwrap_or_default()
            .to_string()
    }

    fn message(&self) -> String {
        serde_json::from_str::<serde_json::Value>(&self.body)
            .ok()
            .and_then(|body| {
                body.get("Message")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string)
            })
            .unwrap_or_default()
    }
}

async fn dispatch(
    router: &MultiServiceRouter,
    path: &str,
    headers: &[(&str, &str)],
    body: &str,
) -> Answer {
    let mut builder = http::Request::builder()
        .method("POST")
        .uri(format!("https://emulator.local{path}"))
        .header("host", "emulator.local")
        .header("content-type", "application/json")
        .header("authorization", authorization("sagemaker"));
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let request = builder
        .body(Bytes::from(body.to_string()))
        .expect("build the request");
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
    Answer {
        status,
        error_type,
        content_type,
        body: String::from_utf8_lossy(response.body()).to_string(),
    }
}

/// RULE: with the whole family mounted, each of the AgenticRFT job runtime's four
/// operations answers its OWN modeled restJson1 rejection — a client-fault 4xx as
/// `application/json` with the error shape in `x-amzn-errortype` and a `Message` member
/// — never the router's 500 `AmbiguousService` or 404 `UnknownService`.
#[tokio::test]
async fn the_job_runtime_answers_its_own_modelled_rejections_through_the_whole_router() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = build_router(&engine);

    let ghost_job = "arn:aws:sagemaker:us-west-2:000000000000:job/AgentRFT/wire-sjr-ghost";
    let long_trajectory_id = "t".repeat(280);
    let job_arn_header = "x-amzn-sagemaker-job-arn";
    let trajectory_id_header = "x-amzn-sagemaker-trajectory-id";
    let seeded_job = service_com_amazonaws_sagemakerjobruntime_agenticrftruntimeservice::state::seed_job_arns()
        .first()
        .cloned()
        .expect("the job runtime seeds at least one job");

    // One row: a request to one of the job runtime's paths, and the answer the one
    // service that owns that path must give.
    struct Row {
        case: &'static str,
        path: &'static str,
        headers: Vec<(&'static str, String)>,
        body: &'static str,
        status: u16,
        error_code: &'static str,
        /// A fragment the `Message` member must state.
        fragment: String,
    }

    let rows = [
        Row {
            case: "Sample for a job the runtime does not hold",
            path: "/sample",
            headers: vec![
                (job_arn_header, ghost_job.to_string()),
                (trajectory_id_header, "traj-1".to_string()),
            ],
            body: r#"{"messages":[{"role":"user","content":"go"}]}"#,
            status: 400,
            error_code: "ValidationException",
            fragment: format!("Could not find job with jobArn {ghost_job}"),
        },
        Row {
            case: "SampleWithResponseStream with a TrajectoryId past the modelled maximum",
            path: "/sample-with-response-stream",
            headers: vec![
                (job_arn_header, ghost_job.to_string()),
                (trajectory_id_header, long_trajectory_id.clone()),
            ],
            body: r#"{"messages":[{"role":"user","content":"go"}]}"#,
            status: 400,
            error_code: "ValidationException",
            fragment: "at 'trajectoryId' failed to satisfy constraint: Member must have length \
                       less than or equal to 256"
                .to_string(),
        },
        Row {
            case: "CompleteRollout against a trajectory that does not exist",
            path: "/complete-rollout",
            headers: vec![(job_arn_header, seeded_job.clone())],
            body: r#"{"TrajectoryId":"wire-sjr-traj-778a2a85","Status":"ready"}"#,
            status: 404,
            error_code: "ResourceNotFoundException",
            fragment: "Trajectory wire-sjr-traj-778a2a85 not found for account 000000000000"
                .to_string(),
        },
        Row {
            case: "UpdateReward against a trajectory that does not exist",
            path: "/update-reward",
            headers: vec![(job_arn_header, seeded_job.clone())],
            body: r#"{"TrajectoryId":"wire-sjr-traj-8bca0ce6","Rewards":[1.0]}"#,
            status: 404,
            error_code: "ResourceNotFoundException",
            fragment: "Trajectory wire-sjr-traj-8bca0ce6 not found for account 000000000000"
                .to_string(),
        },
    ];

    for row in rows {
        let Row {
            case,
            path,
            headers,
            body,
            status: expected_status,
            error_code: expected_code,
            fragment,
        } = row;
        let headers: Vec<(&str, &str)> = headers
            .iter()
            .map(|(name, value)| (*name, value.as_str()))
            .collect();
        let answer = dispatch(&router, path, &headers, body).await;
        assert_eq!(
            (answer.status, answer.error_code().as_str()),
            (expected_status, expected_code),
            "case {case:?}: expected HTTP {expected_status} {expected_code} from the one service \
             that owns {path}, got {} {:?} / {}",
            answer.status,
            answer.error_type,
            answer.body
        );
        assert_eq!(
            answer.content_type.as_deref(),
            Some("application/json"),
            "case {case:?}: expected a restJson1 error document, got {:?} / {}",
            answer.content_type,
            answer.body
        );
        assert!(
            answer.message().contains(&fragment),
            "case {case:?}: expected the Message to state {fragment:?}, got {:?}",
            answer.message()
        );
    }
}
