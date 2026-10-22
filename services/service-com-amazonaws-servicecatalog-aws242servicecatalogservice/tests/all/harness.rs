// SPDX-License-Identifier: CC0-1.0

//! The awsJson1_1 wire harness every suite in this crate drives the service through.
//!
//! One emulator over one shared engine, one `POST /` request per call with the
//! `X-Amz-Target` and SigV4 credential scope a real Service Catalog client sends, and the
//! small readers a suite needs to assert on a response body. It lives in its own module
//! so a suite does not carry a second copy of the transport.

use std::collections::BTreeMap;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The `X-Amz-Target` prefix: the service shape's name,
/// `com.amazonaws.servicecatalog#AWS242ServiceCatalogService`.
pub(crate) const TARGET_PREFIX: &str = "AWS242ServiceCatalogService";

/// The region the harness signs for unless a case picks another.
pub(crate) const DEFAULT_REGION: &str = "us-east-1";

/// The canonical account id every emulated ARN carries
/// (`engine::EMULATOR_ACCOUNT_ID`).
pub(crate) const ACCOUNT_ID: &str = "000000000000";

/// One emulator: a single shared engine behind the router, exactly as
/// `core/router` assembles it.
pub(crate) fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // The harness signs nothing, so authentication is off for this router only.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_servicecatalog_aws242servicecatalogservice::EmulatedService::new(
            engine,
        ),
    );
    router
}

/// Send one awsJson1_1 request signed for `region` and return its status and body.
pub(crate) async fn call_in(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = ::http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTHARNESS/20260101/{region}/servicecatalog/\
                 aws4_request, SignedHeaders=host;x-amz-target, Signature=0"
            ),
        )
        .body(Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|e| {
            panic!(
                "{operation}: response body is not JSON ({e}): {}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    (status, parsed)
}

/// Send one request in [`DEFAULT_REGION`].
pub(crate) async fn call(
    router: &MultiServiceRouter,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    call_in(router, DEFAULT_REGION, operation, body).await
}

/// The wire error code of a failure body: the last segment of awsJson's `__type`.
pub(crate) fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .map(|t| t.split('#').next_back().unwrap_or(t).to_string())
        .unwrap_or_else(|| format!("<no __type in {body}>"))
}

/// A `Tags` list from a response, as a key -> value map.
pub(crate) fn tag_map(case: &str, tags: &Value) -> BTreeMap<String, String> {
    let list = tags
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a Tags list, got {tags}"));
    list.iter()
        .map(|tag| {
            let key = tag["Key"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a tag has no Key: {tag}"));
            let value = tag["Value"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a tag has no Value: {tag}"));
            (key.to_string(), value.to_string())
        })
        .collect()
}

/// `pairs` as the modelled `Tags`/`AddTags` list.
pub(crate) fn tag_list(pairs: &[(&str, &str)]) -> Value {
    Value::Array(
        pairs
            .iter()
            .map(|(key, value)| json!({"Key": key, "Value": value}))
            .collect(),
    )
}

/// AWS mints a Service Catalog id as a `<prefix>-` followed by 12 lowercase base-36
/// characters, which the modelled `Id` pattern `^[a-zA-Z0-9_\-]*$` admits.
pub(crate) fn is_catalog_id(prefix: &str, id: &str) -> bool {
    id.strip_prefix(&format!("{prefix}-")).is_some_and(|rest| {
        rest.len() == 12
            && rest
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
    })
}
