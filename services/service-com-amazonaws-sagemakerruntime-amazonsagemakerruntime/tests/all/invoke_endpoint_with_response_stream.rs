// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for `InvokeEndpointWithResponseStream`, the streaming
//! real-time inference route.
//!
//! The operation is HAND-AUTHORED (the scaffold generator declines a `@streaming`
//! event-stream output) and is routed through `Service::override_operation`, so these
//! cases also pin that routing: they go over real HTTP through the production
//! `MultiServiceRouter` with both SageMaker services on one shared engine (see
//! [`crate::harness`]).
//!
//! The assertions are RULES over TABLES of inputs:
//!
//! * the route the model publishes is served, and a path it does not publish is not;
//! * the endpoint is resolved in the control plane's store, and the same refusals
//!   `InvokeEndpoint` gives apply here (unknown endpoint, no deployment behind it, a
//!   `TargetVariant` the endpoint does not host, a `SessionId` naming no session);
//! * an otherwise valid invocation is answered with the modelled `ModelError` (HTTP 424),
//!   because no model inference container runs in this emulator — never with an
//!   event-stream body carrying parts nothing computed — and is logged in this service's
//!   own state.

use service_com_amazonaws_sagemakerruntime_amazonsagemakerruntime::state::InvocationOutcome;

use crate::harness::{post, response_stream, Emulator, OutcomeCase, RequestCase, ACCOUNT};

/// RULE: an invocation that resolves to a serving endpoint is answered with the modelled
/// `ModelError` (HTTP 424) — the stream carries no `PayloadPart`, because nothing
/// computed one — and the call is logged as having reached the inference step.
#[tokio::test]
async fn a_resolved_streamed_invocation_answers_the_modelled_model_error() {
    let emulator = Emulator::new();
    emulator
        .deploy("streamer", &["AllTraffic", "Shadow"], serde_json::json!({}))
        .await;

    // Rows differ along the request members the operation binds: payload shape, content
    // type, a hosted target variant, custom attributes and an inference id.
    let cases: [RequestCase; 4] = [
        ("a bare payload", vec![], b"1,2,3"),
        (
            "a json payload",
            vec![("content-type", "application/json")],
            b"{\"instances\":[1,2,3]}",
        ),
        (
            "a hosted target variant",
            vec![("x-amzn-sagemaker-target-variant", "Shadow")],
            b"1,2,3",
        ),
        (
            "an inference id and custom attributes",
            vec![
                ("x-amzn-sagemaker-inference-id", "stream-1"),
                ("x-amzn-sagemaker-custom-attributes", "trace=abc"),
            ],
            b"",
        ),
    ];
    for (case, headers, body) in cases {
        let call = post(&emulator, &response_stream("streamer"), &headers, body).await;
        assert_eq!(
            call.status,
            424,
            "case {case}: expected the modelled ModelError (HTTP 424), got {} / {}",
            call.status,
            String::from_utf8_lossy(&call.body)
        );
        assert_eq!(
            call.error_code(),
            "ModelError",
            "case {case}: expected ModelError, got {:?}",
            call.error_code()
        );
        assert!(
            call.message().contains("streamer"),
            "case {case}: the answer must name the endpoint that returned no inference, got {:?}",
            call.message()
        );
        let content_type = call
            .headers
            .get("content-type")
            .map(|value| value.to_str().unwrap_or_default().to_string());
        assert_ne!(
            content_type.as_deref(),
            Some("application/vnd.amazon.eventstream"),
            "case {case}: no event stream may be framed for an inference that never ran"
        );
        let logged = emulator
            .last_logged()
            .unwrap_or_else(|| panic!("case {case}: the invocation must be logged"));
        assert_eq!(
            logged.outcome,
            InvocationOutcome::NoInferenceContainer,
            "case {case}: expected the NoInferenceContainer outcome, got {}",
            logged.outcome.label()
        );
        assert_eq!(
            logged.request_bytes,
            body.len(),
            "case {case}: the log keeps the request body's size, got {}",
            logged.request_bytes
        );
    }
}

/// RULE: this route resolves the endpoint in the control plane's own store and applies
/// the same rules the real-time route does, so every row below is refused with the
/// modelled `ValidationError` (HTTP 400) and logged with the outcome that refused it.
#[tokio::test]
async fn the_streamed_route_refuses_what_the_real_time_route_refuses() {
    let emulator = Emulator::new();
    emulator.deploy("live", &["AllTraffic"], serde_json::json!({})).await;
    emulator
        .deploy_without_waiting("provisioning", &["AllTraffic"], serde_json::json!({}))
        .await;

    let cases: [OutcomeCase; 4] = [
        (
            "no such endpoint",
            "nosuchendpoint",
            vec![],
            InvocationOutcome::EndpointNotFound,
        ),
        (
            "still provisioning",
            "provisioning",
            vec![],
            InvocationOutcome::EndpointNotInService,
        ),
        (
            "a variant the endpoint does not host",
            "live",
            vec![("x-amzn-sagemaker-target-variant", "Nope")],
            InvocationOutcome::VariantNotFound,
        ),
        (
            "a session id naming no session",
            "live",
            vec![("x-amzn-sagemaker-session-id", "sess1")],
            InvocationOutcome::SessionNotFound,
        ),
    ];
    for (case, endpoint, headers, outcome) in cases {
        let call = post(&emulator, &response_stream(endpoint), &headers, b"1,2,3").await;
        assert_eq!(
            call.status,
            400,
            "case {case}: expected HTTP 400, got {} / {}",
            call.status,
            String::from_utf8_lossy(&call.body)
        );
        assert_eq!(
            call.error_code(),
            "ValidationError",
            "case {case}: expected ValidationError, got {:?}",
            call.error_code()
        );
        let logged = emulator
            .last_logged()
            .unwrap_or_else(|| panic!("case {case}: the refusal must be logged"));
        assert_eq!(
            logged.outcome,
            outcome,
            "case {case}: expected the {} outcome, got {}",
            outcome.label(),
            logged.outcome.label()
        );
    }

    // An unknown endpoint is refused naming the endpoint and the account, as the other
    // two routes do.
    let call = post(&emulator, &response_stream("ghost"), &[], b"").await;
    assert!(
        call.message().contains("ghost") && call.message().contains(ACCOUNT),
        "case the refusal wording: it must name the endpoint and the account, got {:?}",
        call.message()
    );
}

/// RULE: only the URI the model publishes for this operation is served by it; a path the
/// model does not publish is left to the framework's own routing, never answered as an
/// invocation.
#[tokio::test]
async fn only_the_modelled_route_is_served() {
    let emulator = Emulator::new();
    emulator.deploy("live", &["AllTraffic"], serde_json::json!({})).await;

    // Rows differ along HOW the path departs from the modelled one.
    let cases = [
        ("a trailing segment", "/endpoints/live/invocations-response-stream/extra"),
        ("an empty endpoint label", "/endpoints//invocations-response-stream"),
        ("a misspelled last segment", "/endpoints/live/invocations-response-streams"),
        ("the segments reversed", "/invocations-response-stream/endpoints/live"),
    ];
    for (case, path) in cases {
        let before = emulator.invocation_log().len();
        let call = post(&emulator, path, &[], b"1,2,3").await;
        assert_ne!(
            call.status, 424,
            "case {case}: a path the model does not publish must not be served as an invocation"
        );
        assert_eq!(
            emulator.invocation_log().len(),
            before,
            "case {case}: a request this operation does not claim must log no invocation"
        );
    }

    // The published route IS served, so the rows above fail for the path, not because
    // nothing serves this operation at all.
    let served = post(&emulator, &response_stream("live"), &[], b"1,2,3").await;
    assert_eq!(
        served.status, 424,
        "case the published route: it must reach the operation, got {}",
        served.status
    );
}
