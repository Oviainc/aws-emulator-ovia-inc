// SPDX-License-Identifier: CC0-1.0

//! How a tagging write's empty answer is framed.
//!
//! RULE under test: a tagging write answers the recorded 204 with NO body, and states the
//! length that answer has (`content-length: 0`) — which is what the recorded AWS
//! responses carry for an empty-bodied success, and what the in-process front door (the
//! inter-service callback path and the SDK connector, both of which serve `dispatch`'s
//! response unchanged) delivers.
//!
//! What a real socket delivers is the other half of the same rule, and is pinned in
//! `wire_probe.rs`: the HTTP/1.1 writer this emulator is served by removes
//! `content-length` from a `204`, which is below anything this crate decides.

use std::sync::Arc;

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_ivsrealtime_amazoninteractivevideoservicerealtime::EmulatedService;

/// A SigV4 `Authorization` header is how a request names the service and the region it is
/// addressed to, so every call carries one; the signature itself is never checked here.
pub const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDTESTIVS/20260101/us-east-1/ivs/aws4_request, \
     SignedHeaders=host, Signature=00";

/// An ARN as a client puts it in the non-greedy `{resourceArn}` path label: its `:` and
/// `/` percent-encoded, so the whole ARN is one path segment.
pub fn label(arn: &str) -> String {
    arn.replace(':', "%3A").replace('/', "%2F")
}

/// The router this suite serves, and a stage in it to tag.
pub async fn router_with_a_stage() -> (MultiServiceRouter, String) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(EmulatedService::new(engine));

    let create = http::Request::builder()
        .method("POST")
        .uri("/CreateStage")
        .header("content-type", "application/json")
        .header("authorization", AUTHORIZATION)
        .body(bytes::Bytes::from(json!({ "name": "framing" }).to_string()))
        .expect("the test request is well formed");
    let response = router.dispatch(create).await;
    assert_eq!(
        response.status().as_u16(),
        200,
        "the fixture stage must be created, got {} {:?}",
        response.status().as_u16(),
        String::from_utf8_lossy(response.body())
    );
    let body: serde_json::Value =
        serde_json::from_slice(response.body()).expect("CreateStage answers JSON");
    let arn = body["stage"]["arn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(!arn.is_empty(), "CreateStage must mint an ARN, got {body}");
    (router, arn)
}

/// One tagging request through the in-process front door: the status, whether the body is
/// empty, and the length the answer states.
async fn dispatch(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<String>,
) -> (u16, bool, Option<String>) {
    let request = http::Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json")
        .header("authorization", AUTHORIZATION)
        .body(body.map(bytes::Bytes::from).unwrap_or_default())
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let content_length = response
        .headers()
        .get("content-length")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    (status, response.body().is_empty(), content_length)
}

/// RULE: both tagging writes answer 204 with no body, stating `content-length: 0`.
#[tokio::test]
async fn a_tagging_write_answers_an_empty_204_that_states_its_length() {
    let (router, arn) = router_with_a_stage().await;
    let path = format!("/tags/{}", label(&arn));

    // Rows: the two operations that answer with no content, each in the shape a client
    // sends it — a body for the write, a query for the removal.
    for (case, method, uri, body) in [
        (
            "TagResource",
            "POST",
            path.clone(),
            Some(json!({ "tags": { "owner": "behavioral" } }).to_string()),
        ),
        (
            "UntagResource",
            "DELETE",
            format!("{path}?tagKeys=owner"),
            None,
        ),
    ] {
        let (status, body_empty, content_length) = dispatch(&router, method, &uri, body).await;
        assert_eq!(
            (status, body_empty, content_length.as_deref()),
            (204, true, Some("0")),
            "case {case}: expected an empty 204 stating content-length 0, got {status} \
             body_empty={body_empty} content-length={content_length:?}"
        );
    }
}
