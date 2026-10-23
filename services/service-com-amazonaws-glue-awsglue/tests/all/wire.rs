// SPDX-License-Identifier: CC0-1.0

//! The wire harness every suite in this crate drives the emulator through.
//!
//! One copy, shared: a suite makes its calls the way a client does — `POST /` with
//! `X-Amz-Target: AWSGlue.<Operation>` and an `awsJson1_1` body — through the same
//! `MultiServiceRouter::dispatch` the production front door uses, so what a suite
//! asserts is HTTP behaviour and not a direct call into a handler.

use std::sync::Arc;

use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

/// This emulator's account id, which is also the default `CatalogId`.
pub const ACCOUNT_ID: &str = "000000000000";

/// The access key id every request here is signed with. The router routes on the
/// SigV4 credential scope's service field (`glue`), so this one header is what gets a
/// request to this service. It is a credential id, NOT a principal: the catalog never
/// renders it into an ARN (see [`CALLER_PRINCIPAL`]).
pub const AKID: &str = "AKIDGLUETEST";

/// The principal ARN every request here speaks for: the emulator's single long-term
/// identity, the one `sts:GetCallerIdentity` reports (`arn:aws:iam::<account>:user/
/// emulator`). This is what the catalog records in `Table$CreatedBy`.
pub const CALLER_PRINCIPAL: &str = "arn:aws:iam::000000000000:user/emulator";

/// The `__type` discriminator the Data Catalog writes onto a nested
/// `PrincipalPermissions` it returns: the structure's own Smithy shape name.
///
/// Source: the `__type` member of `com.amazonaws.glue#PrincipalPermissions` in this
/// crate's Smithy model (`smithy_query shape=PrincipalPermissions`), whose documentation
/// states the value is the structure's own shape name. Declared once here so every suite
/// that asserts the tag reads the same spelling.
pub const PRINCIPAL_PERMISSIONS_TAG: &str = "PrincipalPermissions";

/// The same discriminator on a nested `ErrorDetail`. Source: the `__type` member of
/// `com.amazonaws.glue#ErrorDetail` (`smithy_query shape=ErrorDetail`).
pub const ERROR_DETAIL_TAG: &str = "ErrorDetail";

/// A SigV4 `Authorization` header scoped to `glue`: the routing signal the router
/// reads first. Authentication is off on the test router, so the signature is not
/// verified — only the scope is read.
fn authorization() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential={AKID}/20260101/us-east-1/glue/aws4_request, \
         SignedHeaders=host;x-amz-target, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// An emulator with only this service mounted, on one shared engine, with request
/// authentication off (the test sends no real signature).
pub fn emulator() -> MultiServiceRouter {
    emulator_on(Arc::new(engine::InMemoryEngine::new()))
}

/// The same, on an engine the caller holds — so a test can seed the store directly
/// instead of driving thousands of requests to reach a capacity ceiling.
pub fn emulator_on(engine: Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_glue_awsglue::EmulatedService::new(
        engine,
    ));
    router
}

/// Make one awsJson1_1 call and return its HTTP status and parsed body.
pub async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = smithy_server_runtime::http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("AWSGlue.{operation}"))
        .header("authorization", authorization())
        .body(bytes::Bytes::from(body.to_string()))
        .expect("build the request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// Make one call that must succeed, and return its body.
pub async fn ok(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, parsed) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "{operation} must answer 200, got {status} with body {parsed}"
    );
    parsed
}

/// The awsJson error code of a failure body (`__type`), which may carry a namespace.
pub fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .to_string()
}

/// The names in a `GetTables` `TableList`, in the order the service returned them.
///
/// One copy, shared: every suite that pages over tables reads the list the same way.
pub fn table_names(page: &Value) -> Vec<String> {
    page["TableList"]
        .as_array()
        .expect("TableList is a list")
        .iter()
        .map(|table| {
            table["Name"]
                .as_str()
                .expect("Table.Name is a string")
                .to_string()
        })
        .collect()
}

/// Assert that every member of `sent` reappears, unchanged, on `got`. Recurses into
/// nested structures AND into the elements of a nested list, so a member the server
/// fills with its own default (an absent `Compressed`, say) or ADDS to a nested
/// structure (the `__type` tag the catalog writes onto a nested `PrincipalPermissions`)
/// is not read as a mismatch: the rule under test is that what the client sent comes
/// back, not that the server adds nothing.
pub fn assert_echoed(case: &str, sent: &Value, got: &Value) {
    match sent {
        Value::Object(members) => {
            for (member, value) in members {
                let Some(actual) = got.get(member) else {
                    if value.is_object() || value.is_array() {
                        panic!("case {case}: member {member} is absent from the response {got}");
                    }
                    panic!(
                        "case {case}: member {member} must round-trip; expected {value}, got \
                         nothing in {got}"
                    );
                };
                assert_echoed(case, value, actual);
            }
        }
        Value::Array(items) => {
            let actual = got.as_array().unwrap_or_else(|| {
                panic!("case {case}: expected a list to round-trip as a list, got {got}")
            });
            assert_eq!(
                items.len(),
                actual.len(),
                "case {case}: a list must round-trip element for element; expected {sent}, got \
                 {got}"
            );
            for (element, actual) in items.iter().zip(actual.iter()) {
                assert_echoed(case, element, actual);
            }
        }
        _ => assert_eq!(
            got, sent,
            "case {case}: a scalar must round-trip; expected {sent}, got {got}"
        ),
    }
}
