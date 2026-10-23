// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for `InvokeEndpoint`, SageMaker's real-time inference data
//! plane.
//!
//! Every case goes over real HTTP through the same `MultiServiceRouter` the production
//! router assembles, with BOTH SageMaker services mounted on ONE shared engine — the
//! control plane (`awsJson1_1`, `POST /` with `X-Amz-Target: SageMaker.<Op>`) that owns
//! the endpoint, and this `restJson1` data plane that invokes it. So what the tests
//! exercise is what an SDK client gets.
//!
//! The assertions are RULES over TABLES of inputs, never one recorded call:
//!
//! * an invocation resolves the endpoint it names in the control plane's own store, and
//!   a name no endpoint record carries is refused (`ValidationError`, HTTP 400);
//! * an endpoint serves inference only once it is `InService`;
//! * `TargetVariant` must name a production variant the endpoint hosts;
//! * `EnableExplanations` applies only to an endpoint carrying an `ExplainerConfig`;
//! * a `SessionId` other than `NEW_SESSION` names a stateful session, and none exists
//!   until a model container creates one;
//! * every otherwise-valid invocation is answered with the modelled `ModelError`
//!   (HTTP 424), because no model inference container runs in this emulator — never with
//!   a synthesised inference body — and it is logged in this service's own state.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_sagemaker_sagemaker::ops::types::ENDPOINT_STATUS_VALUES;
use service_com_amazonaws_sagemaker_sagemaker::state::ComAmazonawsSagemakerSagemakerState as ControlPlaneState;
use service_com_amazonaws_sagemakerruntime_amazonsagemakerruntime::state::{
    EndpointInvocation, InvocationOutcome, MAX_RECORDED_INVOCATIONS,
};
use service_com_amazonaws_sagemakerruntime_amazonsagemakerruntime::ComAmazonawsSagemakerruntimeAmazonsagemakerruntimeState as RuntimeState;
use smithy_server_runtime::MultiServiceRouter;

/// The canonical account id every emulator ARN and message carries.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// Long enough to pass the control plane's `ENDPOINT_IN_SERVICE_AFTER` (300 ms), after
/// which the clock owes the endpoint its `InService` status.
const PAST_IN_SERVICE_DUE: Duration = Duration::from_millis(600);

/// A SigV4 `Authorization` header scoped to the signing name both SageMaker services
/// share (`sagemaker`). Request authentication is off in these tests, so the signature
/// is not checked; the scope is what the ARN region is read from.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEMULATOR000000000/20260101/us-east-1/sagemaker/aws4_request, \
     SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000";

/// One HTTP exchange with the data plane.
struct Call {
    status: u16,
    headers: http::HeaderMap,
    body: bytes::Bytes,
}

impl Call {
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }

    /// The wire error code: the `x-amzn-errortype` header restJson1 sets, or the body's
    /// `__type` when no header carried it.
    fn error_code(&self) -> String {
        if let Some(value) = self.headers.get("x-amzn-errortype") {
            return value.to_str().unwrap_or_default().to_string();
        }
        self.json()
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    fn message(&self) -> String {
        let rendered = self.json();
        rendered
            .get("message")
            .or_else(|| rendered.get("Message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }
}

/// One emulator: the shared engine with the SageMaker control plane and this data plane
/// mounted on the production router.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_sagemaker_sagemaker::EmulatedService::new(Arc::clone(&engine)),
        );
        router.register_service(
            service_com_amazonaws_sagemakerruntime_amazonsagemakerruntime::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        Self { router, engine }
    }

    /// A control-plane call (`awsJson1_1`), which must succeed.
    async fn control(&self, target: &str, body: &Value) -> Value {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("SageMaker.{target}"))
            .header("authorization", AUTHORIZATION)
            .body(bytes::Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
        assert_eq!(
            status, 200,
            "case {target} {body}: expected HTTP 200 from the control plane, got {status} with {parsed}"
        );
        parsed
    }

    /// `POST /endpoints/{endpoint}/invocations` with `headers` and the raw `body`.
    async fn invoke(&self, endpoint: &str, headers: &[(&str, &str)], body: &[u8]) -> Call {
        let mut request = http::Request::builder()
            .method("POST")
            .uri(format!("/endpoints/{endpoint}/invocations"))
            .header("authorization", AUTHORIZATION);
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        let request = request
            .body(bytes::Bytes::copy_from_slice(body))
            .expect("a well-formed test request");
        let response = self.router.dispatch(request).await;
        let (parts, body) = response.into_parts();
        Call {
            status: parts.status.as_u16(),
            headers: parts.headers,
            body,
        }
    }

    /// The data plane's own invocation log, as the next request would read it.
    fn invocation_log(&self) -> Vec<EndpointInvocation> {
        self.engine
            .state::<RuntimeState>()
            .lock()
            .invocations()
            .cloned()
            .collect()
    }

    /// The most recently logged invocation.
    fn last_logged(&self) -> Option<EndpointInvocation> {
        self.engine
            .state::<RuntimeState>()
            .lock()
            .last_invocation()
            .cloned()
    }

    /// The logged invocations of one endpoint, oldest first.
    fn invocations_of(&self, endpoint: &str) -> Vec<EndpointInvocation> {
        self.engine
            .state::<RuntimeState>()
            .lock()
            .invocations_of(endpoint)
            .cloned()
            .collect()
    }

    /// Create `model` -> `config` (with `extra` merged into the configuration body) ->
    /// `endpoint`, and wait until the clock owes the endpoint its `InService` status.
    async fn deploy(&self, endpoint: &str, variants: &[&str], extra: Value) {
        self.deploy_without_waiting(endpoint, variants, extra).await;
        tokio::time::sleep(PAST_IN_SERVICE_DUE).await;
        let described = self
            .control("DescribeEndpoint", &json!({ "EndpointName": endpoint }))
            .await;
        assert_eq!(
            described["EndpointStatus"].as_str(),
            Some("InService"),
            "case {endpoint}: the endpoint must be InService before it is invoked, got {described}"
        );
    }

    /// As [`Emulator::deploy`], but returning as soon as `CreateEndpoint` has answered —
    /// so the endpoint is still in the transient `Creating` status.
    async fn deploy_without_waiting(&self, endpoint: &str, variants: &[&str], extra: Value) {
        let model = format!("{endpoint}-model");
        let config = format!("{endpoint}-config");
        self.control(
            "CreateModel",
            &json!({
                "ModelName": model,
                "PrimaryContainer": { "Image": "123456789012.dkr.ecr.us-east-1.amazonaws.com/demo:1" },
            }),
        )
        .await;
        let mut config_body = json!({
            "EndpointConfigName": config,
            "ProductionVariants": variants
                .iter()
                .map(|variant| json!({
                    "VariantName": variant,
                    "ModelName": model,
                    "InitialInstanceCount": 1,
                    "InstanceType": "ml.m5.large",
                }))
                .collect::<Vec<Value>>(),
        });
        for (member, value) in extra.as_object().cloned().unwrap_or_default() {
            config_body[member] = value;
        }
        self.control("CreateEndpointConfig", &config_body).await;
        self.control(
            "CreateEndpoint",
            &json!({ "EndpointName": endpoint, "EndpointConfigName": config }),
        )
        .await;
    }
}

/// A `DataCaptureConfig` capturing both the request and the response to `uri`, as
/// `CreateEndpointConfig$DataCaptureConfig` models it.
fn data_capture(uri: &str) -> Value {
    json!({
        "EnableCapture": true,
        "InitialSamplingPercentage": 100,
        "DestinationS3Uri": uri,
        "CaptureOptions": [{ "CaptureMode": "Input" }, { "CaptureMode": "Output" }],
    })
}

/// An `ExplainerConfig` carrying the SHAP baseline `ClarifyExplainerConfig` requires.
fn explainer_config() -> Value {
    json!({
        "ClarifyExplainerConfig": {
            "ShapConfig": {
                "ShapBaselineConfig": { "MimeType": "text/csv", "ShapBaseline": "0,0,0" },
            },
        },
    })
}

/// The content-type header pair, so a row can vary the request's MIME type.
fn content_type(value: &str) -> (&str, &str) {
    ("content-type", value)
}

/// RULE: an invocation resolves the endpoint it names in the SageMaker control plane's
/// own endpoint store; a name no endpoint record carries is refused with the modelled
/// `ValidationError` (HTTP 400), and the refusal is logged as an unresolved endpoint.
#[tokio::test]
async fn an_endpoint_the_control_plane_does_not_hold_is_refused() {
    // Rows differ along WHY the name resolves to nothing — never created, created as a
    // different resource kind of the same name, created then deleted — and along the
    // name's shape, including the 63-character modelled maximum.
    let emulator = Emulator::new();
    emulator.deploy("live", &["AllTraffic"], json!({})).await;

    // A model and an endpoint CONFIG named `ghost`: neither is an endpoint.
    emulator
        .control(
            "CreateModel",
            &json!({ "ModelName": "ghost", "PrimaryContainer": { "Image": "demo:1" } }),
        )
        .await;
    emulator
        .control(
            "CreateEndpointConfig",
            &json!({
                "EndpointConfigName": "ghost",
                "ProductionVariants": [{
                    "VariantName": "AllTraffic",
                    "ModelName": "ghost",
                    "InitialInstanceCount": 1,
                    "InstanceType": "ml.m5.large",
                }],
            }),
        )
        .await;

    // An endpoint that existed and was deleted resolves to nothing again.
    emulator.deploy("gone", &["AllTraffic"], json!({})).await;
    emulator
        .control("DeleteEndpoint", &json!({ "EndpointName": "gone" }))
        .await;

    let missing = [
        "nosuchendpoint",
        "ghost",
        "gone",
        "a",
        "ep-with-hyphens-1",
        // `EndpointName` is `len 0..63` (smithy_query operation=InvokeEndpoint).
        "a234567890123456789012345678901234567890123456789012345678901b",
    ];
    for name in missing {
        let call = emulator
            .invoke(name, &[content_type("text/csv")], b"1,2,3")
            .await;
        assert_eq!(
            call.status,
            400,
            "case {name}: expected HTTP 400 for an endpoint the control plane does not hold, got {} / {}",
            call.status,
            String::from_utf8_lossy(&call.body)
        );
        assert_eq!(
            call.error_code(),
            "ValidationError",
            "case {name}: expected ValidationError, got {:?}",
            call.error_code()
        );
        assert!(
            call.message().contains(name) && call.message().contains(ACCOUNT),
            "case {name}: the refusal must name the endpoint and the account, got {:?}",
            call.message()
        );
    }

    let logged = emulator.invocation_log();
    assert_eq!(
        logged.len(),
        missing.len(),
        "case unresolved endpoints: every invocation is logged, got {logged:?}"
    );
    for (name, entry) in missing.iter().zip(&logged) {
        assert_eq!(
            (entry.endpoint_name.as_str(), entry.outcome),
            (*name, InvocationOutcome::EndpointNotFound),
            "case {name}: expected a logged EndpointNotFound, got {entry:?}"
        );
    }

    // The one endpoint that DOES resolve is not refused as missing.
    let resolved = emulator
        .invoke("live", &[content_type("text/csv")], b"1,2,3")
        .await;
    assert_eq!(
        resolved.status,
        424,
        "case live: a resolved endpoint reaches the inference step, got {} / {}",
        resolved.status,
        String::from_utf8_lossy(&resolved.body)
    );
    for (name, expected) in [
        ("live", InvocationOutcome::NoInferenceContainer),
        ("nosuchendpoint", InvocationOutcome::EndpointNotFound),
        ("gone", InvocationOutcome::EndpointNotFound),
    ] {
        let per_endpoint = emulator.invocations_of(name);
        assert_eq!(
            per_endpoint
                .iter()
                .map(|entry| entry.outcome)
                .collect::<Vec<_>>(),
            vec![expected],
            "case {name}: the log is queryable per endpoint, got {per_endpoint:?}"
        );
    }
}

/// RULE: the invocation log is bounded, so a client invoking an endpoint in a loop
/// cannot grow this service's state without end: at the cap the oldest entry is dropped
/// and the most recent [`MAX_RECORDED_INVOCATIONS`] calls are what the log holds.
#[tokio::test]
async fn the_invocation_log_keeps_the_most_recent_calls_within_its_bound() {
    let emulator = Emulator::new();
    // Rows: one call below the cap, the call AT the cap, and five past it — the
    // transition the bound is about. Each call carries its own `InferenceId`, so which
    // calls survived is unambiguous.
    let overshoot = 5;
    for sequence in 1..=(MAX_RECORDED_INVOCATIONS + overshoot) {
        let inference_id = format!("call-{sequence}");
        let call = emulator
            .invoke(
                "nosuchendpoint",
                &[
                    content_type("text/csv"),
                    ("x-amzn-sagemaker-inference-id", &inference_id),
                ],
                b"1,2,3",
            )
            .await;
        assert_eq!(
            call.status, 400,
            "case {inference_id}: expected HTTP 400, got {}",
            call.status
        );
        let logged = emulator.invocation_log();
        assert_eq!(
            logged.len(),
            sequence.min(MAX_RECORDED_INVOCATIONS),
            "case {inference_id}: the log holds at most {MAX_RECORDED_INVOCATIONS} entries"
        );
        assert_eq!(
            logged.last().and_then(|entry| entry.inference_id.clone()),
            Some(inference_id.clone()),
            "case {inference_id}: the newest call is the last entry"
        );
        // Once past the cap, the oldest surviving entry is the call that many back.
        let oldest_survivor = sequence.saturating_sub(MAX_RECORDED_INVOCATIONS) + 1;
        assert_eq!(
            logged.first().and_then(|entry| entry.inference_id.clone()),
            Some(format!("call-{oldest_survivor}")),
            "case {inference_id}: the oldest entry is dropped at the cap"
        );
    }
}

/// RULE: an endpoint serves inference only once its deployment has landed; while it is
/// still in the transient `Creating` status an invocation is refused with the modelled
/// `ValidationError`, and the refusal names the status it is in.
#[tokio::test]
async fn an_endpoint_that_is_not_in_service_yet_is_refused() {
    // Rows: the same invocation at the two moments the clock defines — before the
    // endpoint's `InService` instant is due, and after it.
    for (case, variants) in [
        ("one variant", vec!["AllTraffic"]),
        ("two variants", vec!["Blue", "Green"]),
    ] {
        let emulator = Emulator::new();
        emulator
            .deploy_without_waiting("fresh", &variants, json!({}))
            .await;

        let creating = emulator
            .invoke("fresh", &[content_type("text/csv")], b"1,2,3")
            .await;
        assert_eq!(
            creating.status,
            400,
            "case {case}: an endpoint still Creating is refused, got {} / {}",
            creating.status,
            String::from_utf8_lossy(&creating.body)
        );
        assert_eq!(
            creating.error_code(),
            "ValidationError",
            "case {case}: expected ValidationError, got {:?}",
            creating.error_code()
        );
        assert!(
            creating.message().contains("Creating"),
            "case {case}: the refusal must name the endpoint's status, got {:?}",
            creating.message()
        );
        assert_eq!(
            emulator.last_logged().map(|entry| entry.outcome),
            Some(InvocationOutcome::EndpointNotInService),
            "case {case}: expected a logged EndpointNotInService, got {:?}",
            emulator.last_logged()
        );

        // Once the transition the clock owed is due, the same request gets past
        // resolution and reaches the inference step.
        tokio::time::sleep(PAST_IN_SERVICE_DUE).await;
        let in_service = emulator
            .invoke("fresh", &[content_type("text/csv")], b"1,2,3")
            .await;
        assert_eq!(
            in_service.status,
            424,
            "case {case}: an InService endpoint reaches the inference step, got {} / {}",
            in_service.status,
            String::from_utf8_lossy(&in_service.body)
        );
    }
}

/// RULE: an endpoint serves inference while a deployment is behind it — through an
/// update as well as in `InService` — and an invocation is refused only for the statuses
/// that have nothing to route to.
#[tokio::test]
async fn only_a_status_with_no_deployment_behind_it_refuses_an_invocation() {
    // One row per member of the modelled `EndpointStatus` enum, with the status the real
    // service keeps serving in (the previous or the new deployment is up) answering the
    // inference step and the rest refused. The catalog's source is
    // `ENDPOINT_STATUS_VALUES` in the SageMaker crate's `ops/types/shared_types.rs`, and
    // the assertion below pins these rows to it so a model change cannot leave a member
    // untested.
    const ROWS: &[(&str, u16)] = &[
        ("Creating", 400),
        ("Deleting", 400),
        ("Failed", 400),
        ("InService", 424),
        ("OutOfService", 400),
        ("RollingBack", 424),
        ("SystemUpdating", 424),
        ("UpdateRollbackFailed", 400),
        ("Updating", 424),
    ];
    let covered: Vec<&str> = ROWS.iter().map(|(status, _)| *status).collect();
    assert_eq!(
        covered,
        ENDPOINT_STATUS_VALUES.to_vec(),
        "case status catalog: every modelled EndpointStatus must have a row, got {covered:?}"
    );

    for (status, expected) in ROWS {
        let emulator = Emulator::new();
        emulator.deploy("ep", &["AllTraffic"], json!({})).await;

        // The control plane's own record is what the data plane reads, so the status under
        // test is set there. `Creating` also gets its due instant pushed into the future,
        // so the transition the clock owes is not yet due and the status stands.
        {
            let store = emulator.engine.state::<ControlPlaneState>();
            let mut store = store.lock();
            let endpoint = store
                .endpoints
                .get_mut("ep")
                .expect("the deployed endpoint is in the control plane's store");
            endpoint.status = (*status).to_string();
            endpoint.in_service_at = SystemTime::now() + Duration::from_secs(3600);
        }

        let call = emulator
            .invoke("ep", &[content_type("text/csv")], b"1,2,3")
            .await;
        assert_eq!(
            call.status,
            *expected,
            "case status={status}: expected HTTP {expected}, got {} / {}",
            call.status,
            String::from_utf8_lossy(&call.body)
        );
        let logged = emulator
            .last_logged()
            .map(|entry| entry.outcome)
            .expect("the invocation is logged");
        assert_eq!(
            logged,
            if *expected == 400 {
                InvocationOutcome::EndpointNotInService
            } else {
                InvocationOutcome::NoInferenceContainer
            },
            "case status={status}: unexpected logged outcome {logged:?}"
        );
        if *expected == 400 {
            assert!(
                call.message().contains(status),
                "case status={status}: the refusal must name the status, got {:?}",
                call.message()
            );
        }
    }
}

/// RULE: `TargetVariant` names the production variant to send the request to, so it must
/// name one the endpoint hosts; any other name is refused with the modelled
/// `ValidationError`, and a hosted one gets past targeting.
#[tokio::test]
async fn target_variant_must_name_a_variant_the_endpoint_hosts() {
    // Rows differ along the variant COUNT the endpoint hosts and along which name is
    // targeted: each hosted name, a name hosted by a DIFFERENT endpoint, a case-shifted
    // spelling of a hosted name, and a name nothing hosts.
    let hosted_rows: [&[&str]; 3] = [
        &["AllTraffic"],
        &["Blue", "Green"],
        &["v1", "v2", "variant3"],
    ];
    for hosted in hosted_rows {
        let emulator = Emulator::new();
        emulator.deploy("ep", hosted, json!({})).await;
        emulator.deploy("other", &["Elsewhere"], json!({})).await;

        for target in hosted {
            let call = emulator
                .invoke(
                    "ep",
                    &[
                        content_type("text/csv"),
                        ("x-amzn-sagemaker-target-variant", target),
                    ],
                    b"1,2,3",
                )
                .await;
            assert_eq!(
                call.status,
                424,
                "case hosted={hosted:?} target={target}: a hosted variant is accepted, got {} / {}",
                call.status,
                String::from_utf8_lossy(&call.body)
            );
            assert_eq!(
                emulator.last_logged().map(|entry| (
                    entry.target_variant.clone(),
                    entry.outcome
                )),
                Some((
                    Some((*target).to_string()),
                    InvocationOutcome::NoInferenceContainer
                )),
                "case hosted={hosted:?} target={target}: the log records the targeted variant"
            );
        }

        for target in ["Elsewhere", "alltraffic", "nosuchvariant"] {
            if hosted.contains(&target) {
                continue;
            }
            let call = emulator
                .invoke(
                    "ep",
                    &[
                        content_type("text/csv"),
                        ("x-amzn-sagemaker-target-variant", target),
                    ],
                    b"1,2,3",
                )
                .await;
            assert_eq!(
                call.status,
                400,
                "case hosted={hosted:?} target={target}: an unhosted variant is refused, got {} / {}",
                call.status,
                String::from_utf8_lossy(&call.body)
            );
            assert_eq!(
                call.error_code(),
                "ValidationError",
                "case hosted={hosted:?} target={target}: expected ValidationError, got {:?}",
                call.error_code()
            );
            assert!(
                call.message().contains(target),
                "case hosted={hosted:?} target={target}: the refusal must name the variant, got {:?}",
                call.message()
            );
            assert_eq!(
                emulator.last_logged().map(|entry| entry.outcome),
                Some(InvocationOutcome::VariantNotFound),
                "case hosted={hosted:?} target={target}: expected a logged VariantNotFound"
            );
        }
    }
}

/// RULE: `EnableExplanations` overrides a parameter of the endpoint's
/// `ClarifyExplainerConfig`, so it applies only to an endpoint whose configuration
/// carries an `ExplainerConfig`; on one that carries none it is refused with the modelled
/// `ValidationError`.
#[tokio::test]
async fn enable_explanations_applies_only_to_an_endpoint_with_an_explainer_config() {
    let emulator = Emulator::new();
    emulator.deploy("plain", &["AllTraffic"], json!({})).await;
    emulator
        .deploy(
            "clarified",
            &["AllTraffic"],
            json!({ "ExplainerConfig": explainer_config() }),
        )
        .await;

    // Rows differ along the JMESPath expression's shape, which is what the member
    // generalises over (`len 1..64`, pattern `.*`).
    let expressions = ["`true`", "`false`", "predictions[0].probability > `0.5`"];
    for expression in expressions {
        let refused = emulator
            .invoke(
                "plain",
                &[
                    content_type("text/csv"),
                    ("x-amzn-sagemaker-enable-explanations", expression),
                ],
                b"1,2,3",
            )
            .await;
        assert_eq!(
            refused.status,
            400,
            "case {expression:?} on an endpoint with no ExplainerConfig: expected HTTP 400, got {} / {}",
            refused.status,
            String::from_utf8_lossy(&refused.body)
        );
        assert_eq!(
            refused.error_code(),
            "ValidationError",
            "case {expression:?}: expected ValidationError, got {:?}",
            refused.error_code()
        );
        assert_eq!(
            emulator.last_logged().map(|entry| entry.outcome),
            Some(InvocationOutcome::ExplanationsNotConfigured),
            "case {expression:?}: expected a logged ExplanationsNotConfigured"
        );

        let accepted = emulator
            .invoke(
                "clarified",
                &[
                    content_type("text/csv"),
                    ("x-amzn-sagemaker-enable-explanations", expression),
                ],
                b"1,2,3",
            )
            .await;
        assert_eq!(
            accepted.status,
            424,
            "case {expression:?} on an endpoint WITH an ExplainerConfig: it reaches the \
             inference step, got {} / {}",
            accepted.status,
            String::from_utf8_lossy(&accepted.body)
        );
    }
}

/// RULE: a stateful session is created by the MODEL, so `SessionId` resolves against the
/// sessions that exist: `NEW_SESSION` asks for one to be created and reaches the
/// inference step, while any other ID names a session that does not exist and is refused
/// with the modelled `ValidationError`.
#[tokio::test]
async fn a_session_id_that_names_no_session_is_refused() {
    let emulator = Emulator::new();
    emulator.deploy("ep", &["AllTraffic"], json!({})).await;

    // Rows span the ID shapes the member's pattern accepts
    // (`^(NEW_SESSION)$|^[a-zA-Z0-9](-*[a-zA-Z0-9])*$`, `len 0..256`), including its
    // upper bound.
    let unknown_sessions = [
        "abc",
        "session-1",
        "NEWSESSION",
        &"a".repeat(256),
    ];
    for session in unknown_sessions {
        let call = emulator
            .invoke(
                "ep",
                &[
                    content_type("text/csv"),
                    ("x-amzn-sagemaker-session-id", session),
                ],
                b"1,2,3",
            )
            .await;
        assert_eq!(
            call.status,
            400,
            "case session={session}: an unknown session is refused, got {} / {}",
            call.status,
            String::from_utf8_lossy(&call.body)
        );
        assert_eq!(
            call.error_code(),
            "ValidationError",
            "case session={session}: expected ValidationError, got {:?}",
            call.error_code()
        );
        assert_eq!(
            emulator.last_logged().map(|entry| entry.outcome),
            Some(InvocationOutcome::SessionNotFound),
            "case session={session}: expected a logged SessionNotFound"
        );
    }

    // `NEW_SESSION` is the one value that is not a session ID: it asks the model to
    // create one, which is the inference step.
    let new_session = emulator
        .invoke(
            "ep",
            &[
                content_type("text/csv"),
                ("x-amzn-sagemaker-session-id", "NEW_SESSION"),
            ],
            b"1,2,3",
        )
        .await;
    assert_eq!(
        new_session.status,
        424,
        "case session=NEW_SESSION: it reaches the inference step, got {} / {}",
        new_session.status,
        String::from_utf8_lossy(&new_session.body)
    );
    assert!(
        new_session
            .headers
            .get("x-amzn-sagemaker-new-session-id")
            .is_none(),
        "case session=NEW_SESSION: no session is minted when no model created one, got {:?}",
        new_session.headers
    );
}

/// RULE: no model inference container runs in this emulator, so an otherwise-valid
/// invocation of an `InService` endpoint is answered with the modelled `ModelError`
/// (HTTP 424) for every request shape — never with a synthesised inference body, and
/// never with a captured-data record for a request nothing inferred — and each call is
/// logged with the request's size and routing members.
#[tokio::test]
async fn an_invocation_no_container_can_serve_answers_the_modelled_model_error() {
    // Rows differ along what the rule generalises over: payload size and content, the
    // request and response MIME types, whether the caller supplied an inference ID or
    // custom attributes, and whether the endpoint captures data to S3.
    struct Row {
        case: &'static str,
        capture: bool,
        body: Vec<u8>,
        content_type: &'static str,
        accept: &'static str,
        inference_id: Option<&'static str>,
        custom_attributes: Option<&'static str>,
    }
    let rows = vec![
        Row {
            case: "empty body, no optional header",
            capture: false,
            body: Vec::new(),
            content_type: "text/csv",
            accept: "",
            inference_id: None,
            custom_attributes: None,
        },
        Row {
            case: "csv row",
            capture: false,
            body: b"1,2,3".to_vec(),
            content_type: "text/csv",
            accept: "text/csv",
            inference_id: Some("inference-1"),
            custom_attributes: None,
        },
        Row {
            case: "json payload with custom attributes",
            capture: false,
            body: br#"{"instances":[[1,2,3],[4,5,6]]}"#.to_vec(),
            content_type: "application/json",
            accept: "application/json",
            inference_id: Some("inference-2"),
            custom_attributes: Some("tenant=blue"),
        },
        Row {
            case: "binary payload",
            capture: false,
            body: vec![0u8, 1, 2, 255, 128],
            content_type: "application/octet-stream",
            accept: "*/*",
            inference_id: None,
            custom_attributes: None,
        },
        Row {
            case: "large payload",
            capture: false,
            body: vec![b'x'; 256 * 1024],
            content_type: "application/x-npy",
            accept: "",
            inference_id: None,
            custom_attributes: None,
        },
        Row {
            case: "data capture enabled",
            capture: true,
            body: b"7,8,9".to_vec(),
            content_type: "text/csv",
            accept: "text/csv",
            inference_id: Some("captured-1"),
            custom_attributes: Some("trace=1"),
        },
    ];

    for row in &rows {
        let emulator = Emulator::new();
        let extra = if row.capture {
            json!({ "DataCaptureConfig": data_capture("s3://capture-bucket/prefix") })
        } else {
            json!({})
        };
        emulator.deploy("ep", &["AllTraffic"], extra).await;

        let mut headers = vec![content_type(row.content_type)];
        if !row.accept.is_empty() {
            headers.push(("accept", row.accept));
        }
        if let Some(inference_id) = row.inference_id {
            headers.push(("x-amzn-sagemaker-inference-id", inference_id));
        }
        if let Some(attributes) = row.custom_attributes {
            headers.push(("x-amzn-sagemaker-custom-attributes", attributes));
        }

        let call = emulator.invoke("ep", &headers, &row.body).await;
        assert_eq!(
            call.status,
            424,
            "case {}: expected HTTP 424, got {} / {}",
            row.case,
            call.status,
            String::from_utf8_lossy(&call.body)
        );
        assert_eq!(
            call.error_code(),
            "ModelError",
            "case {}: expected ModelError, got {:?}",
            row.case,
            call.error_code()
        );
        // Nothing answered, so the response carries no inference and no container status
        // code, message or log stream fabricated for one.
        let rendered = call.json();
        for absent in [
            "OriginalStatusCode",
            "OriginalMessage",
            "LogStreamArn",
            "Body",
        ] {
            assert!(
                rendered.get(absent).is_none(),
                "case {}: the refusal must not report {absent}, got {rendered}",
                row.case
            );
        }
        assert!(
            call.headers.get("x-amzn-invoked-production-variant").is_none(),
            "case {}: no variant served the request, got {:?}",
            row.case,
            call.headers
        );

        // The call is logged with the request's SIZE (the body itself is @sensitive) and
        // its routing members.
        let logged = emulator.invocation_log();
        assert_eq!(
            logged.len(),
            1,
            "case {}: exactly one invocation is logged, got {logged:?}",
            row.case
        );
        let entry = &logged[0];
        assert_eq!(
            (
                entry.endpoint_name.as_str(),
                entry.outcome,
                entry.request_bytes,
                entry.content_type.as_deref(),
                entry.inference_id.as_deref(),
                entry.target_variant.as_deref(),
            ),
            (
                "ep",
                InvocationOutcome::NoInferenceContainer,
                row.body.len(),
                Some(row.content_type),
                row.inference_id,
                None,
            ),
            "case {}: unexpected log entry {entry:?}",
            row.case
        );

        // The endpoint's own record is untouched by the invocation: a data-capture
        // destination gets nothing when nothing was inferred, and the status does not
        // move because something was invoked.
        let described = emulator
            .control("DescribeEndpoint", &json!({ "EndpointName": "ep" }))
            .await;
        assert_eq!(
            described["EndpointStatus"].as_str(),
            Some("InService"),
            "case {}: the endpoint keeps its status, got {described}",
            row.case
        );
        assert_eq!(
            described["DataCaptureConfig"]["EnableCapture"].as_bool(),
            row.capture.then_some(true),
            "case {}: the capture configuration is unchanged, got {described}",
            row.case
        );
    }
}
