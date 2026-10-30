// SPDX-License-Identifier: CC0-1.0

//! Router-level test: SageMaker's `GetSearchSuggestions` answers a stored trial
//! component's own property names while the SageMaker METRICS endpoint is mounted beside
//! the control plane.
//!
//! The two services share the SigV4 signing name `sagemaker` and are separated only by
//! their modelled URI path spaces, so this suite belongs here rather than in one
//! service's crate: it mounts both on ONE shared engine and drives the whole path over
//! the wire — report a metric on the metrics endpoint, create the component on the
//! control plane, then ask the control plane for suggestions.
//!
//! The RULE it pins: the property names `GetSearchSuggestions` answers for a resource type
//! are the INDEXED ones — for every key the account's stored resources of that type carry
//! in a keyed member of that type's `SearchRecord` entity, the dotted PATH that key sits at
//! with the containing member LOWERCASED (`parameters.<key>`, `tags.<key>`) — never the
//! bare key, never a FIXED member of the entity (that is schema, not an indexed property),
//! and never a name no stored resource of that type carries.
//!
//! KNOWN GAP, documented on the handler: a trial component's `metrics.<name>` are not
//! among those names, because the metric series `BatchPutMetrics` ingests live in the
//! metrics service's own state and the control plane stores none. This suite therefore
//! asserts nothing about metric names; it does drive `BatchPutMetrics` so that the
//! control plane's answer is pinned with that store populated beside it.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The SageMaker control plane and the SageMaker Metrics endpoint, over ONE engine, as
/// `core/router/src/lib.rs` mounts them. They share the SigV4 signing name `sagemaker`
/// and are separated by their modelled URI path spaces, so both can be mounted at once.
fn build_router(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    // These tests exercise handler behaviour, not SigV4.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_sagemaker_sagemaker::EmulatedService::new(Arc::clone(engine)),
    );
    router.register_service(
        service_com_amazonaws_sagemakermetrics_sagemakermetricsservice::EmulatedService::new(
            Arc::clone(engine),
        ),
    );
    router
}

/// A SigV4 `Authorization` header whose credential scope names the signing name both
/// services publish. The signature is never checked here.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/\
     us-east-1/sagemaker/aws4_request, SignedHeaders=host, Signature=0000";

/// Dispatch one request and return its status and parsed body.
async fn dispatch(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    target: Option<&str>,
    body: &Value,
) -> (u16, Value) {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("https://emulator.local{path}"))
        .header("host", "emulator.local")
        .header("content-type", "application/x-amz-json-1.1")
        .header("authorization", AUTHORIZATION);
    if let Some(target) = target {
        builder = builder.header("x-amz-target", format!("SageMaker.{target}"));
    }
    let request = builder
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// The property names `GetSearchSuggestions` answers for a trial component and `hint`.
async fn suggestions(router: &MultiServiceRouter, hint: &str) -> Vec<String> {
    let (status, answered) = dispatch(
        router,
        "POST",
        "/",
        Some("GetSearchSuggestions"),
        &json!({
            "Resource": "ExperimentTrialComponent",
            "SuggestionQuery": { "PropertyNameQuery": { "PropertyNameHint": hint } },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {hint:?}: expected HTTP 200 from GetSearchSuggestions, got {status} \
         with {answered}"
    );
    answered
        .get("PropertyNameSuggestions")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {hint:?}: expected a suggestion list, got {answered}"))
        .iter()
        .map(|suggestion| {
            suggestion
                .get("PropertyName")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {hint:?}: a suggestion with no PropertyName"))
                .to_string()
        })
        .collect()
}

/// RULE: through the assembled router, the suggestions for a resource type are the dotted
/// paths of the keys its stored resources carry in a keyed member of its searchable entity,
/// the facet segment lowercased — never the bare key, never a fixed member of the entity,
/// and never a name no stored resource of that type carries.
#[tokio::test]
async fn trial_component_property_names_are_answered_through_the_router() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = build_router(&engine);

    // Nothing is stored yet, so NOTHING can be answered: a suggestion is an indexed
    // property of a resource the account holds, and the entity's own fixed members are
    // schema rather than indexed properties.
    let before = suggestions(&router, "Parameters").await;
    assert!(
        before.is_empty(),
        "case empty-store: an account holding no trial component has no indexed property \
         to suggest, got {before:?}"
    );

    // `BatchPutMetrics.TrialComponentName` is `^[a-z0-9](-*[a-z0-9]){0,119}$`, so the
    // component is created under a name that pattern admits too.
    let component = "suggestions-router-tc";
    let (status, created) = dispatch(
        &router,
        "POST",
        "/",
        Some("CreateTrialComponent"),
        &json!({
            "TrialComponentName": component,
            "Parameters": {
                "learning_rate": { "NumberValue": 0.01 },
                "optimiser": { "StringValue": "adamw" },
            },
            "InputArtifacts": { "dataset": { "Value": "s3://bucket/train.csv" } },
            "Tags": [{ "Key": "Project", "Value": "forecasting" }],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case create: expected HTTP 200 from CreateTrialComponent, got {status} \
         with {created}"
    );

    // The metrics endpoint, the OTHER service on this engine, accepts a report for that
    // component: the two services coexist under one signing name, separated by their URI
    // path spaces.
    let (status, reported) = dispatch(
        &router,
        "PUT",
        "/BatchPutMetrics",
        None,
        &json!({
            "TrialComponentName": component,
            "MetricData": [
                { "MetricName": "loss", "Timestamp": 1_700_000_000, "Step": 0, "Value": 1.0 },
            ],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case put: expected HTTP 200 from BatchPutMetrics, got {status} with {reported}"
    );

    // Rows: the hint, the names it must answer, and the names it must not. A stored key is
    // answered as the PATH it sits at, the form `Filter`'s `Name` accepts — never bare —
    // and a hint that begins no property name answers nothing. Rows differ along the keyed
    // member the key sits in, the hint's shape (a key, a dotted prefix, a whole container)
    // and its case.
    let cases: Vec<(&str, &str, &[&str], &[&str])> = vec![
        (
            "parameter-key",
            "learning",
            &["parameters.learning_rate"],
            &["learning_rate"],
        ),
        (
            "parameter-path-prefix",
            "parameters.opt",
            &["parameters.optimiser"],
            &["parameters.learning_rate", "optimiser"],
        ),
        (
            "tag-key-case-folded",
            "PROJ",
            &["tags.Project"],
            &["Project"],
        ),
        (
            "input-artifact-key",
            "datas",
            &["inputartifacts.dataset"],
            &["dataset"],
        ),
        (
            "facet-hint-answers-every-key-under-it-but-not-the-member",
            "Parameters",
            &["parameters.learning_rate", "parameters.optimiser"],
            &["Parameters", "tags.Project"],
        ),
        // A fixed member of the entity is schema, not an indexed property, so the hint
        // that begins it answers nothing.
        ("fixed-member", "TrialComponentA", &[], &["TrialComponentArn"]),
        // A hint is text that BEGINS a property's name, so text from the middle of one
        // begins nothing.
        ("substring-not-a-prefix", "rate", &[], &["parameters.learning_rate"]),
        ("unmatched", "zzzz-no-such-name", &[], &[]),
    ];

    for (case, hint, present, absent) in &cases {
        let answered = suggestions(&router, hint).await;
        for name in *present {
            assert!(
                answered.iter().any(|suggested| suggested == name),
                "case {case}: expected {name} among the suggestions for {hint:?}, \
                 got {answered:?}"
            );
        }
        for name in *absent {
            assert!(
                !answered.iter().any(|suggested| suggested == name),
                "case {case}: {name} must not be suggested for {hint:?}, got {answered:?}"
            );
        }
        if present.is_empty() {
            assert!(
                answered.is_empty(),
                "case {case}: {hint:?} begins no property name, got {answered:?}"
            );
        }
    }
}
