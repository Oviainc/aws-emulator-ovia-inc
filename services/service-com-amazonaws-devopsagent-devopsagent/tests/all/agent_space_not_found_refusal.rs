// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the refusal this service's control plane answers when
//! the `agentSpaceId` a request carries names no AgentSpace.
//!
//! The rule under test: a control-plane operation that resolves an `agentSpaceId`
//! and holds no AgentSpace under it for the calling account refuses the request
//! with HTTP 404 `ResourceNotFoundException` whose message names BOTH the
//! identifier the request carried and the account the lookup ran in, each
//! double-quoted — one wording for every such operation, computed from the request
//! and the store rather than from which operation was called.
//!
//! The table spans every implemented control-plane operation that takes an
//! `agentSpaceId` crossed with identifiers that differ in length, character class
//! and form across the whole of `AgentSpaceId`'s `^[a-zA-Z0-9-]{1,64}$`, plus the
//! minted identifier of an AgentSpace that existed and was deleted. The served
//! cases are exercised too, so a refusal that ignored the store would fail here.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::agent_space_not_found::{
    assert_not_found, caller_account, expected_message, missing_identifiers,
    AGENT_SPACE_ID_MAX_LEN,
};

/// A SigV4 credential scope naming this service's signing name (`aidevops`), which
/// is how the router resolves which service claims a request. Signature
/// verification is switched off for these tests, so the signature is a placeholder.
const AUTHORIZATION: &str = concat!(
    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-west-2/aidevops/aws4_request, ",
    "SignedHeaders=host, Signature=0123456789abcdef"
);

/// One emulator: a single shared engine behind the production router.
fn new_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_devopsagent_devopsagent::EmulatedService::new(engine),
    );
    router
}

/// Send one request and return its status and parsed body (`Value::Null` for an
/// empty body).
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body
        .as_ref()
        .map(|b| Bytes::from(serde_json::to_vec(b).expect("serialize request body")))
        .unwrap_or_default();
    let mut request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "aidevops.us-west-2.amazonaws.com")
        .header("authorization", AUTHORIZATION);
    if body.is_some() {
        request = request.header("content-type", "application/json");
    }
    let request = request.body(payload).expect("build request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|e| {
            panic!(
                "response body is not JSON ({e}): {}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    (status, parsed)
}

/// One control-plane call that resolves an `agentSpaceId`.
struct ControlPlaneCall {
    /// The modelled operation name, used to name the case in an assertion.
    operation: &'static str,
    method: &'static str,
    /// The modelled URI, with `{agentSpaceId}` where the `@httpLabel` goes.
    uri: &'static str,
    /// The request payload the operation needs, or `None` when it takes none.
    body: Option<Value>,
    /// The status the same call answers for an AgentSpace that DOES exist.
    served_status: u16,
}

/// An IAM role ARN satisfying `RoleArn`'s modelled pattern
/// (`^arn:(aws|aws-cn|aws-us-gov):iam::\d{12}:role/[a-zA-Z0-9+=,.@_/-]+$`).
fn role_arn(name: &str) -> String {
    format!("arn:aws:iam::{}:role/{name}", caller_account())
}

/// Every implemented control-plane operation that resolves an `agentSpaceId`:
/// method, URI and success status from
/// `smithy_query(query="operation", operation="GetAgentSpace,DeleteAgentSpace,EnableOperatorApp,GetOperatorApp")`,
/// each of which carries `endpoint hostPrefix "cp."` and an `agentSpaceId`
/// `@httpLabel` targeting `AgentSpaceId`. The remaining control-plane operations
/// taking one (`UpdateAgentSpace`, `DisableOperatorApp`,
/// `UpdateOperatorAppIdpConfig`) are still unimplemented stubs, and the data-plane
/// backlog operations take the separate `AgentSpaceIdentifier` shape.
fn control_plane_calls() -> Vec<ControlPlaneCall> {
    vec![
        ControlPlaneCall {
            operation: "GetAgentSpace",
            method: "GET",
            uri: "/v1/agentspaces/{agentSpaceId}",
            body: None,
            served_status: 200,
        },
        ControlPlaneCall {
            operation: "GetOperatorApp",
            method: "GET",
            uri: "/v2/agentspaces/{agentSpaceId}/operator",
            body: None,
            served_status: 200,
        },
        ControlPlaneCall {
            operation: "EnableOperatorApp",
            method: "POST",
            uri: "/v1/agentspaces/{agentSpaceId}/operator",
            body: Some(json!({
                "authFlow": "iam",
                "operatorAppRoleArn": role_arn("OperatorAppRole"),
            })),
            served_status: 200,
        },
        ControlPlaneCall {
            operation: "DeleteAgentSpace",
            method: "DELETE",
            uri: "/v1/agentspaces/{agentSpaceId}",
            body: None,
            served_status: 204,
        },
    ]
}

/// Issue `call` against `agent_space_id`.
async fn dispatch(
    router: &MultiServiceRouter,
    operation: &ControlPlaneCall,
    agent_space_id: &str,
) -> (u16, Value) {
    let uri = operation.uri.replace("{agentSpaceId}", agent_space_id);
    call(router, operation.method, &uri, operation.body.clone()).await
}

/// Create an agent space and return its minted id.
async fn create_space(router: &MultiServiceRouter, name: &str) -> String {
    let (status, body) = call(router, "POST", "/v1/agentspaces", Some(json!({"name": name}))).await;
    assert_eq!(status, 201, "case {name:?}: expected 201, got {status}");
    body["agentSpace"]["agentSpaceId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name:?}: no agentSpaceId in {body}"))
        .to_string()
}

#[tokio::test]
async fn every_control_plane_lookup_refuses_a_missing_agent_space_with_one_wording() {
    let router = new_router();
    // A stored AgentSpace, so the store is not empty and a refusal has to be
    // decided from what it holds rather than from the store being empty.
    let present = create_space(&router, "a-space-that-exists").await;

    for operation in control_plane_calls() {
        for agent_space_id in missing_identifiers() {
            assert_ne!(
                agent_space_id, present,
                "the table must not name the stored AgentSpace"
            );
            let case = format!("{} {agent_space_id:?}", operation.operation);
            let (status, body) = dispatch(&router, &operation, &agent_space_id).await;
            assert_not_found(&case, &agent_space_id, status, &body);
        }
    }
}

#[tokio::test]
async fn the_refusal_names_the_account_the_lookup_ran_in() {
    let router = new_router();
    let (status, body) = call(&router, "GET", "/v1/agentspaces/no-such-space", None).await;
    assert_eq!(status, 404, "expected 404, got {status} ({body})");

    let message = body["message"].as_str().unwrap_or("<no message>").to_string();
    let account = caller_account();
    assert!(
        message.contains(&format!("in accountId \"{account}\"")),
        "expected the refusal to name the caller's account scope, got {message:?}"
    );
    assert!(
        account.len() == 12 && account.bytes().all(|b| b.is_ascii_digit()),
        "the account scope must be a 12-digit AWS account id, got {account:?}"
    );
}

#[tokio::test]
async fn an_agent_space_that_exists_is_served_and_refused_once_it_is_deleted() {
    for operation in control_plane_calls() {
        // A fresh emulator per row: the row's own call can mutate or delete the
        // AgentSpace it is served for.
        let router = new_router();
        let agent_space_id = create_space(&router, "round-trip").await;

        let (status, body) = dispatch(&router, &operation, &agent_space_id).await;
        assert_eq!(
            status, operation.served_status,
            "case {:?}: an AgentSpace that exists must be served, got {status} ({body})",
            operation.operation
        );

        // Remove it (a no-op for the row that already deleted it) and read the same
        // call back: the identifier is the one this service minted, so the refusal
        // carries a value no literal in this test supplied.
        let (deleted, body) =
            call(&router, "DELETE", &format!("/v1/agentspaces/{agent_space_id}"), None).await;
        assert!(
            deleted == 204 || deleted == 404,
            "case {:?}: expected the space to be gone after a delete, got {deleted} ({body})",
            operation.operation
        );

        let case = format!("{} after deletion", operation.operation);
        let (status, body) = dispatch(&router, &operation, &agent_space_id).await;
        assert_not_found(&case, &agent_space_id, status, &body);
    }
}

#[tokio::test]
async fn an_identifier_past_the_pattern_bound_is_not_a_not_found_refusal() {
    let router = new_router();
    // One character past `AgentSpaceId`'s maximum length: the request never reaches
    // a lookup, so it is a validation refusal and not the not-found wording.
    let too_long = "z".repeat(AGENT_SPACE_ID_MAX_LEN + 1);

    for operation in control_plane_calls() {
        let (status, body) = dispatch(&router, &operation, &too_long).await;
        assert_eq!(
            status, 400,
            "case {:?}: expected 400 for an agentSpaceId past the pattern bound, got {status} ({body})",
            operation.operation
        );
        assert_ne!(
            body["message"].as_str().unwrap_or_default(),
            expected_message(&too_long),
            "case {:?}: a value the pattern rejects must not be answered as a lookup miss, got {body}",
            operation.operation
        );
    }
}
