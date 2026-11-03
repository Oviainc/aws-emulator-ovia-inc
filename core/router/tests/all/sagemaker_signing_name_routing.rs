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
//!
//! The two endpoint runtimes are the one pair whose path spaces are NOT disjoint: both
//! bind `/endpoints/{EndpointName}/…` and both publish the endpointPrefix
//! `runtime.sagemaker`, so the first path segment cannot separate them either. The
//! segment their modelled URIs differ at does — `smithy_query(query="digest")` on the two
//! models gives HTTP2 only `…/invocations-bidirectional-stream`, against
//! `…/invocations`, `…/async-invocations` and `…/invocations-response-stream` — and
//! `service_common::addresses_runtime_bidirectional_stream` states that split once for
//! both sides, so every path in the space they share has exactly one claimant too. The
//! claim table below has a row for each side of it.
//!
//! The last test is the metrics data plane the SageMaker Metrics checks exercise, driven
//! with the whole family mounted: what `BatchPutMetrics` stores is what `BatchGetMetrics`
//! aggregates, so a routing fix that reached the wrong member could not produce it.

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
/// path space, and, where two members publish the same path space, the one whose own
/// model binds the deeper segment the request addresses.
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
    // surface (the control plane's RPC form, each REST member's paths), both sides of the
    // `/endpoints/…` space the two endpoint runtimes share, the root path without an
    // `X-Amz-Target` and with one naming another service, and a path no member binds.
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
            case: "endpoint runtime invocations",
            method: "POST",
            path: "/endpoints/my-endpoint/invocations",
            target: None,
            // The two endpoint runtimes publish the SAME `/endpoints/…` space and the
            // same endpointPrefix, so the first path segment cannot separate them; the
            // third can, because that is where their modelled URIs differ. This one is
            // the SageMaker Runtime endpoint's.
            claimants: vec![ENDPOINT_RUNTIME],
        },
        Row {
            case: "endpoint runtime async invocations",
            method: "POST",
            path: "/endpoints/my-endpoint/async-invocations",
            target: None,
            claimants: vec![ENDPOINT_RUNTIME],
        },
        Row {
            case: "endpoint runtime response stream",
            method: "POST",
            path: "/endpoints/my-endpoint/invocations-response-stream",
            target: None,
            claimants: vec![ENDPOINT_RUNTIME],
        },
        Row {
            case: "the HTTP2 endpoint runtime's bidirectional stream",
            method: "POST",
            path: "/endpoints/my-endpoint/invocations-bidirectional-stream",
            target: None,
            // The one invocation the HTTP2 model binds, and the only one of the shared
            // space it claims.
            claimants: vec![ENDPOINT_RUNTIME_HTTP2],
        },
        Row {
            case: "an invocation neither endpoint-runtime model binds",
            method: "POST",
            path: "/endpoints/my-endpoint/no-such-invocation",
            target: None,
            // Stated as a complement on the SageMaker Runtime side, so a path inside the
            // shared space that names no modelled invocation still has exactly one
            // claimant — which answers in its own protocol envelope — instead of none,
            // which the router would answer `404 UnknownService`.
            claimants: vec![ENDPOINT_RUNTIME],
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
            case: "metrics retrieval",
            method: "POST",
            path: "/BatchGetMetrics",
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
    dispatch_with_method(router, "POST", path, headers, body).await
}

/// [`dispatch`] for a row whose method the model fixes to something other than `POST` —
/// `BatchPutMetrics` is a `PUT`.
async fn dispatch_with_method(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &str,
) -> Answer {
    let mut builder = http::Request::builder()
        .method(method)
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

    let malformed_job = "a-job-of-my-own";
    let long_trajectory_id = "t".repeat(280);
    let job_arn_header = "x-amzn-sagemaker-job-arn";
    let trajectory_id_header = "x-amzn-sagemaker-trajectory-id";
    // The job the data plane's rows address: a job ARN the modelled `JobArn` constraints
    // admit, naming a job no `CreateJob` ever minted — which is what the rows below
    // require, since each asserts a REFUSAL.
    let job = "arn:aws:sagemaker:us-west-2:000000000000:job/AgentRFT/wire-sjr-rollout-1";

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
            case: "Sample with a JobArn that misses the modelled pattern",
            path: "/sample",
            headers: vec![
                (job_arn_header, malformed_job.to_string()),
                (trajectory_id_header, "traj-1".to_string()),
            ],
            body: r#"{"messages":[{"role":"user","content":"go"}]}"#,
            status: 400,
            error_code: "ValidationException",
            fragment: format!("Input arn {malformed_job} is malformed"),
        },
        Row {
            case: "SampleWithResponseStream with a TrajectoryId past the modelled maximum",
            path: "/sample-with-response-stream",
            headers: vec![
                (job_arn_header, job.to_string()),
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
            headers: vec![(job_arn_header, job.to_string())],
            body: r#"{"TrajectoryId":"wire-sjr-traj-778a2a85","Status":"ready"}"#,
            status: 404,
            error_code: "ResourceNotFoundException",
            fragment: "Trajectory wire-sjr-traj-778a2a85 not found for account 000000000000"
                .to_string(),
        },
        Row {
            case: "UpdateReward against a trajectory that does not exist",
            path: "/update-reward",
            headers: vec![(job_arn_header, job.to_string())],
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

/// RULE: a data-plane operation that routes a request to a resource the CONTROL PLANE
/// creates resolves it against what the control plane recorded — so with the whole family
/// mounted on one engine, a job `SageMaker.CreateJob` minted is sampled end to end, and
/// an equally well-formed ARN no create minted is refused by the job runtime's own
/// modelled `ValidationException` naming that ARN.
///
/// This is the cross-service half of the rule: the two services are different crates
/// reading one shared job directory, so neither side alone can produce this answer.
#[tokio::test]
async fn a_job_the_control_plane_created_is_sampled_through_the_whole_router() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = build_router(&engine);

    let job_arn_header = "x-amzn-sagemaker-job-arn";
    let trajectory_id_header = "x-amzn-sagemaker-trajectory-id";
    let payload = r#"{"messages":[{"role":"user","content":"go"}]}"#;

    // Rows differ along the job's category and name, and along which sampling operation
    // serves the rollout; each row is created through the control plane and then sampled,
    // and a sibling ARN that was never created is refused.
    let rows: [(&str, &str, &str, &str); 3] = [
        (
            "a fine-tuning job, non-streaming",
            "AgentRFT",
            "router-rollout-1",
            "/sample",
        ),
        (
            "an evaluation job, streaming",
            "AgentRFTEvaluation",
            "router-eval-1",
            "/sample-with-response-stream",
        ),
        (
            "a fine-tuning job, streaming",
            "AgentRFT",
            "router-rollout-2",
            "/sample-with-response-stream",
        ),
    ];

    for (case, category, name, path) in rows {
        let created = control_plane_call(
            &router,
            "CreateJob",
            &serde_json::json!({
                "JobName": name,
                "JobCategory": category,
                "JobConfigSchemaVersion": "1.0.0",
                // A document conforming to the schema the service publishes, under a role
                // of another account — which this emulator's IAM is not asked about — so
                // this routing case turns on the ROUTING and on nothing the create admits.
                "JobConfigDocument": serde_json::json!({
                    "AgentConfig": {},
                    "InputDataConfig": {},
                    "ModelPackageConfig": { "BaseModel": "demo-model" },
                    "OutputDataConfig": {},
                    "TrainingConfig": {},
                })
                .to_string(),
                "RoleArn": "arn:aws:iam::123456789012:role/service-role/AmazonSageMaker-ExecutionRole",
            })
            .to_string(),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected HTTP 200 from SageMaker.CreateJob, got {} / {}",
            created.status, created.body
        );
        let parsed: serde_json::Value =
            serde_json::from_str(&created.body).unwrap_or(serde_json::Value::Null);
        let minted = parsed["JobArn"].as_str().unwrap_or_default().to_string();
        assert_eq!(
            minted,
            format!("arn:aws:sagemaker:us-east-1:000000000000:job/{category}/{name}"),
            "case {case:?}: expected the minted JobArn, got {}",
            created.body
        );

        let served = dispatch(
            &router,
            path,
            &[(job_arn_header, &minted), (trajectory_id_header, "traj-1")],
            payload,
        )
        .await;
        assert_eq!(
            (served.status, served.error_code().as_str()),
            (200, ""),
            "case {case:?}: expected the job runtime to serve the created job, got {} {:?} / {}",
            served.status,
            served.error_type,
            served.body
        );

        // A sibling ARN of the same form that no create minted: the same request, refused.
        let ghost = format!("arn:aws:sagemaker:us-east-1:000000000000:job/{category}/{name}-ghost");
        let refused = dispatch(
            &router,
            path,
            &[(job_arn_header, &ghost), (trajectory_id_header, "traj-1")],
            payload,
        )
        .await;
        assert_eq!(
            (refused.status, refused.error_code().as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException for a job no create minted, got {} \
             {:?} / {}",
            refused.status,
            refused.error_type,
            refused.body
        );
        assert!(
            refused.message().contains(&ghost),
            "case {case:?}: expected the refusal to name the requested jobArn, got {:?}",
            refused.message()
        );
    }
}

/// One awsJson1_1 control-plane call (`POST /` with `X-Amz-Target: SageMaker.<Op>`), which
/// is how every operation of the SageMaker control plane arrives.
async fn control_plane_call(router: &MultiServiceRouter, target: &str, body: &str) -> Answer {
    let request = http::Request::builder()
        .method("POST")
        .uri("https://emulator.local/")
        .header("host", "emulator.local")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("SageMaker.{target}"))
        .header("authorization", authorization("sagemaker"))
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

/// RULE: with the whole family mounted, the metrics endpoint's own data plane works end
/// to end — what `BatchPutMetrics` stores is what `BatchGetMetrics` aggregates — so the
/// claim resolution above reaches SageMaker Metrics and nothing else does.
///
/// The aggregates are computed from the three stored points, not echoed: the three
/// timestamps share one `OneMinute` bucket, so each `MetricStatistic` the model declares
/// is a different function of the same series, and the x value is that bucket's start.
#[tokio::test]
async fn the_metrics_endpoint_round_trips_its_own_data_plane_through_the_whole_router() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = build_router(&engine);

    let trial_component = "routing-roundtrip-tc";
    let resource_arn = format!(
        "arn:aws:sagemaker:us-east-1:000000000000:experiment-trial-component/{trial_component}"
    );
    let put_body = serde_json::json!({
        "TrialComponentName": trial_component,
        "MetricData": [
            {"MetricName": "loss", "Timestamp": 1_700_000_000, "Step": 0, "Value": 1.0},
            {"MetricName": "loss", "Timestamp": 1_700_000_001, "Step": 1, "Value": 0.5},
            {"MetricName": "loss", "Timestamp": 1_700_000_002, "Step": 2, "Value": 0.25},
        ],
    })
    .to_string();

    let answer = dispatch_with_method(&router, "PUT", "/BatchPutMetrics", &[], &put_body).await;
    assert_eq!(
        (answer.status, answer.error_code().as_str()),
        (200, ""),
        "case \"BatchPutMetrics\": expected HTTP 200 from SageMaker Metrics, got {} {:?} / {}",
        answer.status,
        answer.error_type,
        answer.body
    );
    assert!(
        !answer.body.contains("Errors"),
        "case \"BatchPutMetrics\": a wholly accepted batch reports no Errors, got {}",
        answer.body
    );

    // `floor(1_700_000_000 / 60) * 60`: the start of the minute all three points fall in.
    const MINUTE_BUCKET: i64 = 1_699_999_980;
    // Five of the six `MetricStatistic` members, each a different function of the same
    // three values (`smithy_query(query="shape", shape="MetricStatistic")`).
    let expected: [(&str, f64); 5] = [
        ("Last", 0.25),
        ("Min", 0.25),
        ("Max", 1.0),
        ("Count", 3.0),
        ("Avg", (1.0 + 0.5 + 0.25) / 3.0),
    ];

    for (stat, want) in expected {
        let get_body = serde_json::json!({"MetricQueries": [{
            "MetricName": "loss",
            "ResourceArn": resource_arn,
            "MetricStat": stat,
            "Period": "OneMinute",
            "XAxisType": "Timestamp",
        }]})
        .to_string();
        let answer = dispatch(&router, "/BatchGetMetrics", &[], &get_body).await;
        assert_eq!(
            (answer.status, answer.error_code().as_str()),
            (200, ""),
            "case {stat:?}: expected HTTP 200 from SageMaker Metrics, got {} {:?} / {}",
            answer.status,
            answer.error_type,
            answer.body
        );
        let parsed: serde_json::Value =
            serde_json::from_str(&answer.body).unwrap_or(serde_json::Value::Null);
        let result = &parsed["MetricQueryResults"][0];
        assert_eq!(
            result["Status"].as_str(),
            Some("Complete"),
            "case {stat:?}: expected a Complete query result, got {result}"
        );
        let values = result["MetricValues"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let actual = values.first().and_then(serde_json::Value::as_f64);
        assert!(
            values.len() == 1 && actual.is_some_and(|value| (value - want).abs() < 1e-9),
            "case {stat:?}: expected MetricValues [{want}] for the points written, got {result}"
        );
        assert_eq!(
            result["XAxisValues"],
            serde_json::json!([MINUTE_BUCKET]),
            "case {stat:?}: expected the one minute bucket [{MINUTE_BUCKET}] the three points \
             share, got {result}"
        );
    }
}

