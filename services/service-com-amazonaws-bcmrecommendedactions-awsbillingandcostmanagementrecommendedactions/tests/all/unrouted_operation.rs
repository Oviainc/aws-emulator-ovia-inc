// SPDX-License-Identifier: CC0-1.0

//! The unrouted-operation default the `crate::service` module docs state, on the
//! wire.
//!
//! One rule, over every shape a routing key can take: a request this service
//! claims that names an operation the model does not declare is answered 501
//! `NotImplemented` in the media type the model's protocol trait declares,
//! whatever media type the client sent, and the answer names the operation it
//! could not route. A request that names a modeled operation is served, so the
//! status keys on the operation NAME and on nothing else.
//!
//! The suite exists because `service.rs` documents that status in prose, in the
//! bullet for `override_routing_error` and in that seam's own body: a reader
//! deciding whether this service needs its own routing envelope trusts the prose,
//! so the prose needs something that fails when it drifts.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_bcmrecommendedactions_awsbillingandcostmanagementrecommendedactions as bcm;

/// The one operation the model declares
/// (`smithy_query(query="list_operations")`), as the awsJson target header names
/// it.
const MODELED_OPERATION: &str = "ListRecommendedActions";

/// The service prefix a real client sends ahead of the operation name.
const SERVICE_PREFIX: &str = "AWSBillingAndCostManagementRecommendedActions";

/// A SigV4 `Authorization` header whose credential scope names this service's
/// signing name — the routing signal the router reads, so a request reaches this
/// service even when its target names nothing. The signature is not checked:
/// request authentication is off for these tests.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20261028/us-east-1/bcm-recommended-actions/aws4_request, SignedHeaders=host;x-amz-target, Signature=0000000000000000000000000000000000000000000000000000000000000000";

/// The media type a routing refusal announces: the one the model's protocol trait
/// declares, which is independent of what the client sent (see the
/// `crate::service` module docs).
const ROUTING_MEDIA_TYPE: &str = "application/x-amz-json-1.0";

/// The status a request naming no modeled operation is answered with, as the
/// `crate::service` module docs state it.
const UNROUTED_STATUS: u16 = 501;

/// The wire error code that status carries.
const UNROUTED_CODE: &str = "NotImplemented";

/// One emulator: a shared engine, and a router with this service mounted on it.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    /// A fresh emulator, holding the inventory the service computed for this
    /// account.
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(bcm::EmulatedService::new(engine));
        Self { router }
    }

    /// Dispatch `body` at `target`, as a client announcing `request_content_type`
    /// (empty for a client that announces none), reporting the response's status,
    /// announced media type and payload.
    async fn dispatch(
        &self,
        target: &str,
        request_content_type: &str,
        body: &Value,
    ) -> (u16, String, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", "bcm-recommended-actions.us-east-1.amazonaws.com")
            .header("content-type", request_content_type)
            .header("x-amz-target", target)
            .header("authorization", AUTHORIZATION)
            .body(bytes::Bytes::from(
                serde_json::to_vec(body).expect("serialize the request body"),
            ))
            .expect("build the request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let payload = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or_else(|error| {
                panic!(
                    "response body is not JSON ({error}): {}",
                    String::from_utf8_lossy(response.body())
                )
            })
        };
        (status, content_type, payload)
    }
}

/// The wire error code of a failure response (`__type`, bare or namespaced).
fn error_code(case: &str, body: &Value) -> String {
    let raw = body
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: error body carries no __type: {body}"));
    raw.rsplit('#').next().unwrap_or(raw).to_string()
}

/// One unrouted-operation case: the target header a client sent, the media type it
/// announced, and the operation name the answer must name — the substring of the
/// target after its first `.`, which is the routing key an awsJson request carries.
struct UnroutedCase {
    case: &'static str,
    target: String,
    request_content_type: &'static str,
    operation: String,
}

impl UnroutedCase {
    /// A case from a target of `prefix.operation`.
    fn new(
        case: &'static str,
        prefix: &str,
        operation: &str,
        request_content_type: &'static str,
    ) -> Self {
        Self {
            case,
            target: format!("{prefix}.{operation}"),
            request_content_type,
            operation: operation.to_string(),
        }
    }
}

/// Targets that name no modeled operation. The rows differ along the operation
/// name's relation to the modeled one (unrelated, a case variant, a prefix of it, a
/// suffixed form, a name carrying a dot of its own, an empty name), along the
/// service prefix sent ahead of it, and along the media type the client announced
/// — none of which the rule depends on.
fn unrouted_cases() -> Vec<UnroutedCase> {
    vec![
        UnroutedCase::new(
            "an unrelated operation name",
            SERVICE_PREFIX,
            "NoSuchOperation",
            "application/x-amz-json-1.0",
        ),
        UnroutedCase::new(
            "the modeled name in another case",
            SERVICE_PREFIX,
            "listRecommendedActions",
            "application/x-amz-json-1.0",
        ),
        UnroutedCase::new(
            "the modeled name with a version suffix",
            SERVICE_PREFIX,
            "ListRecommendedActionsV2",
            "application/x-amz-json-1.1",
        ),
        UnroutedCase::new(
            "a prefix of the modeled name",
            SERVICE_PREFIX,
            "ListRecommended",
            "application/x-amz-json-1.1",
        ),
        UnroutedCase::new(
            "a sibling verb on the same noun",
            SERVICE_PREFIX,
            "GetRecommendedAction",
            "",
        ),
        UnroutedCase::new(
            "an unknown operation under another service's prefix",
            "AWSSomeOtherService",
            "DescribeThings",
            "application/json",
        ),
        UnroutedCase::new(
            "an operation name carrying a dot of its own",
            SERVICE_PREFIX,
            "Nested.Operation",
            "application/x-amz-json-1.0",
        ),
        UnroutedCase::new("an empty operation name", SERVICE_PREFIX, "", ""),
    ]
}

#[tokio::test]
async fn a_target_naming_no_modeled_operation_is_answered_with_the_documented_status() {
    let emulator = Emulator::new();

    for UnroutedCase {
        case,
        target,
        request_content_type,
        operation,
    } in unrouted_cases()
    {
        let (status, media_type, payload) = emulator
            .dispatch(&target, request_content_type, &json!({}))
            .await;
        assert_eq!(
            status, UNROUTED_STATUS,
            "case {case:?}: expected HTTP {UNROUTED_STATUS} for target {target:?}, got {status} \
             with {payload}"
        );
        assert_eq!(
            media_type, ROUTING_MEDIA_TYPE,
            "case {case:?}: expected media type {ROUTING_MEDIA_TYPE:?} whatever the client \
             announced ({request_content_type:?}), got {media_type:?} with {payload}"
        );
        let code = error_code(case, &payload);
        assert_eq!(
            code, UNROUTED_CODE,
            "case {case:?}: expected error code {UNROUTED_CODE:?}, got {code:?} in {payload}"
        );
        let message = payload
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: the answer states no message: {payload}"));
        assert!(
            message.contains(&operation),
            "case {case:?}: expected the answer to name the operation it could not route \
             ({operation:?}), got {message:?}"
        );
        assert!(
            payload.get("recommendedActions").is_none(),
            "case {case:?}: an unrouted request must report no page, got {payload}"
        );
    }
}

#[tokio::test]
async fn a_target_naming_the_modeled_operation_is_still_served() {
    let emulator = Emulator::new();

    // The other side of the rule: the status above keys on the operation NAME, so a
    // target that names the modeled operation is served whatever prefix precedes it
    // and whatever media type the client announced.
    let cases: Vec<(&str, String, &str)> = vec![
        (
            "the target a real client sends",
            format!("{SERVICE_PREFIX}.{MODELED_OPERATION}"),
            "application/x-amz-json-1.0",
        ),
        (
            "the modeled operation, awsJson1_1 client",
            format!("{SERVICE_PREFIX}.{MODELED_OPERATION}"),
            "application/x-amz-json-1.1",
        ),
        (
            "the modeled operation, no media type announced",
            format!("{SERVICE_PREFIX}.{MODELED_OPERATION}"),
            "",
        ),
        (
            "the modeled operation under another prefix",
            format!("AWSSomeOtherService.{MODELED_OPERATION}"),
            "application/x-amz-json-1.0",
        ),
    ];

    for (case, target, request_content_type) in cases {
        let (status, _, payload) = emulator
            .dispatch(&target, request_content_type, &json!({}))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 for target {target:?}, got {status} with {payload}"
        );
        assert!(
            payload.get("recommendedActions").is_some(),
            "case {case:?}: expected a reported page, got {payload}"
        );
    }
}
