// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the resource-policy surface (`PutResourcePolicy`,
//! `GetResourcePolicy`, `DeleteResourcePolicy`).
//!
//! Every test drives the service the way the router does: an `http::Request` through
//! [`MultiServiceRouter::dispatch`], so the wire protocol (`restJson1`), the boundary
//! constraint validation and the handlers are all exercised. The store is the one shared
//! engine, so what one request writes is what the next request reads.
//!
//! The rule under test: a resource-based policy is the ONE document attached to the
//! resource its ARN names, so `PutResourcePolicy` stores (and replaces) that document on a
//! resource this service holds and reports it back, `GetResourcePolicy` reports exactly the
//! stored document, `DeleteResourcePolicy` removes it, and an ARN that names no resource of
//! a type the feature covers — or a resource carrying no policy — is the modelled 404.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_bedrockagentcorecontrol_amazonbedrockagentcorecontrol::EmulatedService;

/// The region the tests sign for unless a case names another.
const REGION: &str = "us-east-1";

/// The account id every emulated ARN carries (`engine::mock_account_id`).
const ACCOUNT: &str = "000000000000";

/// The modelled maximum of `PutResourcePolicy$policy` / `GetResourcePolicy$policy`
/// (`ResourcePolicyBody`, `len 1..20480`, from
/// `smithy_query(query="shape", shape="ResourcePolicyBody")`).
const MAX_POLICY_LEN: usize = 20_480;

/// How a policy lookup that resolves no document is worded, for a resource this service
/// HOLDS that carries none.
const POLICY_MISSING: &str = "Resource Policy doesn't exist for requested arn";

/// How every resource-policy operation words a `resourceArn` that names no resource a
/// policy can attach to (`crate::state::resource_arn_not_found`).
const RESOURCE_MISSING: &str = "ResourceArn provided in the request does not exist";

fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::default();
    router.register_service(EmulatedService::new(engine));
    router
}

/// A SigV4 `Authorization` header for this service in `region`. It is what the router
/// routes on, and what the handlers read the ARN region from.
fn authorization(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{region}/bedrock-agentcore/aws4_request, \
         SignedHeaders=host, Signature=abc"
    )
}

async fn send(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    region: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let payload = body.map_or_else(Bytes::new, |value| {
        Bytes::from(serde_json::to_vec(value).expect("the request body serializes"))
    });
    let mut request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", authorization(region))
        .header("host", "bedrock-agentcore-control.amazonaws.com");
    if body.is_some() {
        request = request.header("content-type", "application/json");
    }
    let request = request.body(payload).expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// The ARN form, percent-encoded as a client encodes a non-greedy `@httpLabel` — the
/// same helper `tests/all/registry_lifecycle.rs` uses for this service's other
/// identifier-in-the-path route.
fn encoded_label(value: &str) -> String {
    value.replace(':', "%3A").replace('/', "%2F")
}

/// The `/resourcepolicy/{resourceArn}` path of `arn`
/// (`smithy_query(query="operation", operation="PutResourcePolicy")`).
fn resource_policy_uri(arn: &str) -> String {
    format!("/resourcepolicy/{}", encoded_label(arn))
}

async fn put_policy(
    router: &MultiServiceRouter,
    arn: &str,
    policy: &str,
    region: &str,
) -> (u16, Value) {
    send(
        router,
        "PUT",
        &resource_policy_uri(arn),
        region,
        Some(&json!({"policy": policy})),
    )
    .await
}

async fn get_policy(router: &MultiServiceRouter, arn: &str, region: &str) -> (u16, Value) {
    send(router, "GET", &resource_policy_uri(arn), region, None).await
}

async fn delete_policy(router: &MultiServiceRouter, arn: &str, region: &str) -> (u16, Value) {
    send(router, "DELETE", &resource_policy_uri(arn), region, None).await
}

/// The error code `restJson1` puts in `__type` / `x-amzn-errortype`.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

fn text(body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Create an agent runtime and answer its ARN.
async fn create_runtime(router: &MultiServiceRouter, name: &str) -> String {
    let (status, created) = send(
        router,
        "PUT",
        "/runtimes/",
        REGION,
        Some(&json!({
            "agentRuntimeName": name,
            "agentRuntimeArtifact": {"containerConfiguration": {
                "containerUri": "public.ecr.aws/agentcore/demo:v1"
            }},
            "roleArn": "arn:aws:iam::000000000000:role/AgentRole",
        })),
    )
    .await;
    assert_eq!(
        status, 202,
        "the agent runtime a policy case attaches to is created, got {status} ({created})"
    );
    text(&created, "agentRuntimeArn")
}

/// Create a gateway and answer its ARN.
async fn create_gateway(router: &MultiServiceRouter, name: &str) -> String {
    let (status, created) = send(
        router,
        "POST",
        "/gateways/",
        REGION,
        Some(&json!({
            "name": name,
            "roleArn": "arn:aws:iam::000000000000:role/GatewayRole",
            "authorizerType": "AWS_IAM",
        })),
    )
    .await;
    assert_eq!(
        status, 202,
        "the gateway a policy case attaches to is created, got {status} ({created})"
    );
    text(&created, "gatewayArn")
}

/// The two resource types the feature covers: "This feature is currently available only
/// for AgentCore Runtime and Gateway"
/// (`smithy_query(query="doc", shape="PutResourcePolicy")`).
#[derive(Debug, Clone, Copy)]
enum Covered {
    AgentRuntime,
    Gateway,
}

async fn create_covered(router: &MultiServiceRouter, kind: Covered, name: &str) -> String {
    match kind {
        Covered::AgentRuntime => create_runtime(router, name).await,
        Covered::Gateway => create_gateway(router, name).await,
    }
}

/// A resource policy document granting `action` to the account's root principal, with the
/// statement id `sid`.
fn policy_document(sid: &str, action: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": sid,
            "Effect": "Allow",
            "Principal": {"AWS": format!("arn:aws:iam::{ACCOUNT}:root")},
            "Action": action,
            "Resource": "*",
        }],
    })
    .to_string()
}

/// A policy document of exactly `length` characters, padded in its statement id, for a case
/// that drives the body to a published length bound.
fn policy_of_length(length: usize) -> String {
    let empty = policy_document("", "bedrock-agentcore:InvokeAgentRuntime");
    let padding = length.saturating_sub(empty.len());
    let document = policy_document(
        &"S".repeat(padding),
        "bedrock-agentcore:InvokeAgentRuntime",
    );
    assert_eq!(
        document.len(),
        length,
        "the padded document is {length} characters long, got {}",
        document.len()
    );
    document
}

/// A policy round-trips through the resource its ARN names: the write reports the document
/// it stored, a read reports exactly that document, a second write REPLACES it (one
/// resource carries one policy), the delete removes it, and the read afterwards is the
/// modelled 404.
///
/// The rows differ along what the rule generalises over: the type of resource the policy
/// attaches to (both the feature covers) and the form and size of the document — statement
/// count, embedded whitespace, and the modelled maximum length.
#[tokio::test]
async fn a_resource_policy_round_trips_on_every_covered_resource() {
    let router = router();

    // (case, resource type, the document first written, the document that replaces it)
    let rows: [(&str, Covered, String, String); 4] = [
        (
            "a runtime, one statement",
            Covered::AgentRuntime,
            policy_document("InvokeFromAccount", "bedrock-agentcore:InvokeAgentRuntime"),
            policy_document("InvokeAndRead", "bedrock-agentcore:GetAgentRuntime"),
        ),
        (
            "a runtime, a document at the modelled maximum length",
            Covered::AgentRuntime,
            policy_of_length(MAX_POLICY_LEN),
            policy_document("ShortAgain", "bedrock-agentcore:InvokeAgentRuntime"),
        ),
        (
            "a gateway, a pretty-printed two-statement document",
            Covered::Gateway,
            serde_json::to_string_pretty(&json!({
                "Version": "2012-10-17",
                "Statement": [
                    {
                        "Sid": "First",
                        "Effect": "Allow",
                        "Principal": {"AWS": format!("arn:aws:iam::{ACCOUNT}:root")},
                        "Action": "bedrock-agentcore:InvokeGateway",
                        "Resource": "*",
                    },
                    {
                        "Sid": "Second",
                        "Effect": "Deny",
                        "Principal": "*",
                        "Action": "bedrock-agentcore:DeleteGateway",
                        "Resource": "*",
                    },
                ],
            }))
            .expect("the document serializes"),
            policy_document("OnlyOneLeft", "bedrock-agentcore:InvokeGateway"),
        ),
        (
            "a gateway, a document carrying non-ASCII text",
            Covered::Gateway,
            policy_document("ポリシー", "bedrock-agentcore:InvokeGateway"),
            policy_document("Ascii", "bedrock-agentcore:InvokeGateway"),
        ),
    ];

    for (index, (case, kind, first, replacement)) in rows.into_iter().enumerate() {
        let arn = create_covered(&router, kind, &format!("policyholder{index}")).await;

        let (status, written) = put_policy(&router, &arn, &first, REGION).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected HTTP 201 from PutResourcePolicy, got {status} ({written})"
        );
        assert_eq!(
            text(&written, "policy"),
            first,
            "case {case:?}: PutResourcePolicy reports the document it stored, got {}",
            text(&written, "policy")
        );

        let (status, read) = get_policy(&router, &arn, REGION).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from GetResourcePolicy, got {status} ({read})"
        );
        assert_eq!(
            text(&read, "policy"),
            first,
            "case {case:?}: GetResourcePolicy reports the stored document verbatim, got {}",
            text(&read, "policy")
        );

        // One resource carries ONE policy: the second write replaces the whole document
        // rather than adding a second policy beside it.
        let (status, replaced) = put_policy(&router, &arn, &replacement, REGION).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected HTTP 201 from the replacing PutResourcePolicy, got \
             {status} ({replaced})"
        );
        let (status, read) = get_policy(&router, &arn, REGION).await;
        assert_eq!(
            (status, text(&read, "policy")),
            (200, replacement.clone()),
            "case {case:?}: after the replacement the read reports the new document, got \
             {status} {}",
            text(&read, "policy")
        );

        let (status, removed) = delete_policy(&router, &arn, REGION).await;
        assert_eq!(
            status, 204,
            "case {case:?}: expected HTTP 204 from DeleteResourcePolicy, got {status} ({removed})"
        );

        // The document is gone, so the resource carries no policy any more.
        let (status, answer) = get_policy(&router, &arn, REGION).await;
        assert_eq!(
            (status, error_code(&answer).as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: a read after the delete is the modelled 404, got {status} {} \
             ({answer})",
            error_code(&answer)
        );

        // The resource itself is untouched by the policy's removal, so it takes a new
        // policy right after.
        let (status, rewritten) = put_policy(&router, &arn, &replacement, REGION).await;
        assert_eq!(
            status, 201,
            "case {case:?}: the resource takes a policy again after the delete, got {status} \
             ({rewritten})"
        );
    }
}

/// A policy is held per resource: what one resource carries is not reported for another,
/// and each resource's delete removes only its own document.
#[tokio::test]
async fn each_resource_carries_its_own_policy() {
    let router = router();
    let runtime = create_runtime(&router, "isolated_runtime").await;
    let gateway = create_gateway(&router, "isolatedgateway").await;

    let runtime_policy = policy_document("OnTheRuntime", "bedrock-agentcore:InvokeAgentRuntime");
    let gateway_policy = policy_document("OnTheGateway", "bedrock-agentcore:InvokeGateway");
    for (case, arn, policy) in [
        ("the runtime", runtime.as_str(), runtime_policy.as_str()),
        ("the gateway", gateway.as_str(), gateway_policy.as_str()),
    ] {
        let (status, _) = put_policy(&router, arn, policy, REGION).await;
        assert_eq!(
            status, 201,
            "case {case:?}: the policy of {arn} is written, got {status}"
        );
    }

    for (case, arn, expected) in [
        ("the runtime", runtime.as_str(), runtime_policy.as_str()),
        ("the gateway", gateway.as_str(), gateway_policy.as_str()),
    ] {
        let (status, read) = get_policy(&router, arn, REGION).await;
        assert_eq!(
            (status, text(&read, "policy").as_str()),
            (200, expected),
            "case {case:?}: each resource reports its own document, got {status} {}",
            text(&read, "policy")
        );
    }

    // Deleting one resource's policy leaves the other's in place.
    let (status, _) = delete_policy(&router, &runtime, REGION).await;
    assert_eq!(status, 204, "the runtime's policy is removed, got {status}");
    let (status, read) = get_policy(&router, &gateway, REGION).await;
    assert_eq!(
        (status, text(&read, "policy").as_str()),
        (200, gateway_policy.as_str()),
        "case \"the gateway after the runtime's delete\": the other document is untouched, got \
         {status} {}",
        text(&read, "policy")
    );
}

/// A policy exists only as a property of its resource: once the resource is gone, so is the
/// document it carried, and the ARN names nothing to read.
#[tokio::test]
async fn a_deleted_resource_carries_no_policy() {
    let router = router();
    let arn = create_runtime(&router, "short_lived").await;
    let runtime_id = arn.rsplit('/').next().unwrap_or_default().to_string();
    let policy = policy_document("Doomed", "bedrock-agentcore:InvokeAgentRuntime");
    let (status, _) = put_policy(&router, &arn, &policy, REGION).await;
    assert_eq!(status, 201, "the policy is written, got {status}");

    let (status, deleted) = send(
        &router,
        "DELETE",
        &format!("/runtimes/{runtime_id}/"),
        REGION,
        None,
    )
    .await;
    assert_eq!(
        status, 202,
        "the runtime the policy was attached to is deleted, got {status} ({deleted})"
    );

    for (case, status_and_answer) in [
        (
            "GetResourcePolicy on the deleted resource",
            get_policy(&router, &arn, REGION).await,
        ),
        (
            "DeleteResourcePolicy on the deleted resource",
            delete_policy(&router, &arn, REGION).await,
        ),
        (
            "PutResourcePolicy on the deleted resource",
            put_policy(&router, &arn, &policy, REGION).await,
        ),
    ] {
        let (status, answer) = status_and_answer;
        assert_eq!(
            (status, error_code(&answer).as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: expected HTTP 404 ResourceNotFoundException, got {status} {} \
             ({answer})",
            error_code(&answer)
        );
    }
}

/// What the resource-policy operations refuse, and with which modelled error: an ARN that
/// names no resource this service holds, one of a resource type the feature does not cover,
/// one outside the request's own account or Region, a resource carrying no policy, and a
/// document outside the modelled length range.
#[tokio::test]
async fn the_resource_policy_operations_refuse_what_the_model_states_they_must() {
    let router = router();
    let runtime = create_runtime(&router, "refusal_runtime").await;
    let gateway = create_gateway(&router, "refusalgateway").await;
    let runtime_id = runtime.rsplit('/').next().unwrap_or_default().to_string();
    let gateway_id = gateway.rsplit('/').next().unwrap_or_default().to_string();
    let policy = policy_document("Refused", "bedrock-agentcore:InvokeAgentRuntime");

    // (case, the ARN addressed) — each is a well-formed `BedrockAgentcoreResourceArn`
    // (`len 20..1011`, no pattern), so every refusal below is the handler's and not the
    // boundary's.
    let unresolved: [(&str, String); 6] = [
        (
            "a runtime ARN naming no stored runtime",
            format!("arn:aws:bedrock-agentcore:{REGION}:{ACCOUNT}:runtime/absent_runtime-0123456789"),
        ),
        (
            "a gateway ARN naming no stored gateway",
            format!("arn:aws:bedrock-agentcore:{REGION}:{ACCOUNT}:gateway/absent-gateway-0123456789"),
        ),
        (
            // The feature covers the runtime and the gateway, not an endpoint of a runtime.
            "a runtime-endpoint ARN of a stored runtime",
            format!(
                "arn:aws:bedrock-agentcore:{REGION}:{ACCOUNT}:runtime/{runtime_id}/runtime-endpoint/DEFAULT"
            ),
        ),
        (
            // A memory is a resource of this service the feature does not cover.
            "a memory ARN",
            format!("arn:aws:bedrock-agentcore:{REGION}:{ACCOUNT}:memory/{runtime_id}"),
        ),
        (
            "a stored gateway's id under another account",
            format!("arn:aws:bedrock-agentcore:{REGION}:210987654321:gateway/{gateway_id}"),
        ),
        (
            "a stored gateway's id in another Region than the request",
            format!("arn:aws:bedrock-agentcore:eu-west-2:{ACCOUNT}:gateway/{gateway_id}"),
        ),
    ];

    for (case, arn) in unresolved {
        // Every one of the three asked about a resource that is not there, so each
        // reports the ARN that resolves to nothing rather than a policy: the remedy is
        // to address a resource that exists, whichever operation was called.
        for (operation, expected_message, (status, answer)) in [
            (
                "PutResourcePolicy",
                RESOURCE_MISSING,
                put_policy(&router, &arn, &policy, REGION).await,
            ),
            (
                "GetResourcePolicy",
                RESOURCE_MISSING,
                get_policy(&router, &arn, REGION).await,
            ),
            (
                "DeleteResourcePolicy",
                RESOURCE_MISSING,
                delete_policy(&router, &arn, REGION).await,
            ),
        ] {
            assert_eq!(
                (status, error_code(&answer).as_str(), text(&answer, "message")),
                (
                    404,
                    "ResourceNotFoundException",
                    expected_message.to_string()
                ),
                "case {case:?} ({operation}): expected HTTP 404 ResourceNotFoundException \
                 {expected_message:?}, got {status} {} ({answer})",
                error_code(&answer)
            );
        }
    }

    // A resource this service holds that carries no policy: there is no document to report
    // or to remove, in the one sentence a policy lookup that resolves nothing answers.
    for (case, (status, answer)) in [
        (
            "GetResourcePolicy on a resource carrying no policy",
            get_policy(&router, &gateway, REGION).await,
        ),
        (
            "DeleteResourcePolicy on a resource carrying no policy",
            delete_policy(&router, &gateway, REGION).await,
        ),
    ] {
        assert_eq!(
            (status, error_code(&answer).as_str(), text(&answer, "message")),
            (
                404,
                "ResourceNotFoundException",
                POLICY_MISSING.to_string()
            ),
            "case {case:?}: expected HTTP 404 ResourceNotFoundException {POLICY_MISSING:?}, got \
             {status} {} ({answer})",
            error_code(&answer)
        );
    }

    // The published length range of the document, at each bound and one past it:
    // `ResourcePolicyBody` is `len 1..20480`.
    let too_long = policy_of_length(MAX_POLICY_LEN + 1);
    for (case, document, expected_status, expected_code) in [
        (
            "a document one character past the modelled maximum",
            too_long.as_str(),
            400,
            "ValidationException",
        ),
        ("an empty document", "", 400, "ValidationException"),
        (
            "a document at the modelled maximum",
            policy_of_length(MAX_POLICY_LEN).as_str(),
            201,
            "",
        ),
    ] {
        let (status, answer) = put_policy(&router, &runtime, document, REGION).await;
        let code = if expected_code.is_empty() {
            String::new()
        } else {
            error_code(&answer)
        };
        assert_eq!(
            (status, code.as_str()),
            (expected_status, expected_code),
            "case {case:?}: expected HTTP {expected_status} {expected_code:?}, got {status} {} \
             ({answer})",
            error_code(&answer)
        );
    }
}
