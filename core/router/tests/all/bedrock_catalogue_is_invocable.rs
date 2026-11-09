// SPDX-License-Identifier: CC0-1.0

//! Every foundation model the Amazon Bedrock control plane PUBLISHES is one the Bedrock
//! Runtime data plane resolves.
//!
//! The rule under test: **the two planes of Amazon Bedrock carry the same base models**.
//! The control plane publishes the catalogue (`ListFoundationModels`, `GetFoundationModel`)
//! and the data plane resolves a `modelId` against its own inference inventory, so a client
//! that finds a model in the listing and invokes it must not be told its identifier is
//! invalid. Two crates have to agree for that to hold, so the suite lives here: a
//! crate-local test can only ever see one side of it, and nothing else pins the two lists
//! against each other.
//!
//! The check is driven from the catalogue itself rather than from a list written here, so a
//! model added to (or renamed in) either plane is covered the day it lands: every summary
//! the listing answers with is sent to the data plane, and the one answer that is a defect —
//! the data plane reporting the identifier as one it cannot interpret — fails this suite.
//! A model the Converse API does not serve is a DIFFERENT refusal (the model resolved; that
//! surface does not serve it), and so is a model whose modality this emulator computes no
//! inference for: both are admitted here, since what is under test is whether the
//! identifier RESOLVES.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request here is signed for.
const REGION: &str = "us-east-1";

/// The signing name the whole Bedrock family shares; the route decides which member serves
/// a request.
const BEDROCK: &str = "bedrock";

/// The data plane's own message for a `modelId` it cannot interpret as naming a model it
/// offers — the one answer a catalogued model must never get.
const INVALID_MODEL_IDENTIFIER: &str = "The provided model identifier is invalid.";

/// The emulator with the two Bedrock planes on ONE engine, as `core/router/src/lib.rs`
/// mounts them.
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_bedrockruntime_amazonbedrockfrontendservice::EmulatedService::new(
            engine,
        ),
    );
    router
}

/// A SigV4 `Authorization` header whose credential scope names the Bedrock signing name,
/// which is what routes the request. The signature is never checked.
fn authorization() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/{BEDROCK}/\
         aws4_request, SignedHeaders=host, Signature=0000000000000000000000000000000000000\
         000000000000000000000000"
    )
}

/// One call to either plane, answered with the status and the document.
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    host: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body
        .map(|value| Bytes::from(value.to_string()))
        .unwrap_or_default();
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", host)
        .header("content-type", "application/json")
        .header("authorization", authorization())
        .body(payload)
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// Every model the control plane publishes, as `(modelId, lifecycle status)`.
async fn catalogued_models(router: &MultiServiceRouter) -> Vec<(String, String)> {
    let (status, listed) = call(
        router,
        "GET",
        "/foundation-models",
        &format!("bedrock.{REGION}.amazonaws.com"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "expected ListFoundationModels to answer the catalogue, got {status} with {listed}"
    );
    listed["modelSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("expected modelSummaries in {listed}"))
        .iter()
        .map(|summary| {
            (
                summary["modelId"].as_str().unwrap_or_default().to_string(),
                summary["modelLifecycle"]["status"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            )
        })
        .collect()
}

#[tokio::test]
async fn every_published_model_resolves_in_the_inference_plane() {
    let router = build_router();
    let published = catalogued_models(&router).await;
    assert!(
        published.len() > 50,
        "expected the published catalogue to carry the models of every provider, got {} \
         entries",
        published.len()
    );

    for (model_id, lifecycle) in &published {
        let case = format!("{model_id} ({lifecycle})");
        let encoded = model_id.replace(':', "%3A");
        let (status, answered) = call(
            &router,
            "POST",
            &format!("/model/{encoded}/converse"),
            &format!("bedrock-runtime.{REGION}.amazonaws.com"),
            Some(json!({"messages": [{"role": "user", "content": [{"text": "hi"}]}]})),
        )
        .await;
        let message = answered["message"].as_str().unwrap_or_default();
        assert_ne!(
            message, INVALID_MODEL_IDENTIFIER,
            "case {case:?}: a model the control plane publishes must not be reported as an \
             identifier the inference plane cannot interpret (got {status} with {answered})"
        );
        // And the answer is one of the two a resolved model gets: the turn, or a refusal
        // that is ABOUT the model rather than about the identifier.
        assert!(
            status == 200 || (400..500).contains(&status),
            "case {case:?}: expected the turn or a modelled client refusal, got {status} \
             with {answered}"
        );
    }
}
