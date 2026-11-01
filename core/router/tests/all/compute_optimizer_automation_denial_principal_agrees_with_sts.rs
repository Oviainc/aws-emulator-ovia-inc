// SPDX-License-Identifier: CC0-1.0

//! One credential, two services: the principal Compute Optimizer Automation names in
//! its `AccessDeniedException` is the identity STS reports for the SAME credential.
//!
//! Compute Optimizer Automation evaluates authorization against the resource string a
//! request supplies before it parses that string, and refuses one that names no
//! automation rule of the service with the IAM denial built from the request — "User:
//! <identity> is not authorized to perform: aco-automation:<Operation> on resource:
//! <the string as sent>". The identity in it is the caller's own, which is to say the
//! `Arn` that `sts:GetCallerIdentity` answers for the credential the request was signed
//! with. A client that parses the denied principal out of that message, or asserts it
//! equals its own caller identity, must see ONE principal for one credential — and only
//! a cross-service suite can pin that: the account's access-key records live in IAM, the
//! identity a presented key resolves to is STS's answer, and each crate's own tests can
//! only ever assert its own half.
//!
//! The rows vary what the rule generalises over: the CREDENTIAL presented (the key this
//! emulator publishes, and a key `iam:CreateAccessKey` minted for a user of this
//! account), the OPERATION refusing (the operations whose input names a resource by ARN)
//! and the FORM of the resource string.
//!
//! The awsQuery encoder and the XML element reader are this test crate's own
//! (`query_wire`).

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::{form_encode, opt_element};

/// The access key id an administrative call presents: the account holds no record of
/// it, so it is the ambient caller this emulator publishes.
const AMBIENT_KEY: &str = "AKIAIOSFODNN7EXAMPLE";

/// The signing name Compute Optimizer Automation's requests carry, which is also the
/// IAM service prefix of its actions (`sigv4Name`, equal to its `endpointPrefix`, from
/// `smithy_query(query="service_info")`).
const SIGNING_NAME: &str = "aco-automation";

/// A SigV4 `Authorization` header whose credential scope names `service` and
/// `access_key_id`. The signature is never checked: this router runs with request
/// authentication off, and the access key id is what resolves the caller's identity.
fn authorization(access_key_id: &str, service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/us-east-1/{service}/aws4_request, \
         SignedHeaders=host, Signature=0"
    )
}

/// The emulator with Compute Optimizer Automation, IAM and STS mounted on ONE engine,
/// as `core/router/src/lib.rs` mounts them: the access-key registry `iam:CreateAccessKey`
/// writes is the one both STS and the refusal resolve a presented key against.
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_computeoptimizerautomation_computeoptimizerautomationservice::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_sts_awssecuritytokenservicev20110615::EmulatedService::new(engine),
    );
    router
}

/// One awsQuery call against `signing_name`, presenting `access_key_id`.
async fn query(
    router: &MultiServiceRouter,
    access_key_id: &str,
    signing_name: &str,
    version: &str,
    action: &str,
    parameters: &[(&str, String)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version={version}");
    for (name, value) in parameters {
        body.push('&');
        body.push_str(name);
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_name}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization(access_key_id, signing_name))
        .body(Bytes::from(body))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// The `Arn` STS reports for `access_key_id`, over STS's own awsQuery wire.
async fn caller_identity_arn(
    router: &MultiServiceRouter,
    case: &str,
    access_key_id: &str,
) -> String {
    let (status, payload) = query(
        router,
        access_key_id,
        "sts",
        "2011-06-15",
        "GetCallerIdentity",
        &[],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from GetCallerIdentity, got {status} with body {payload}"
    );
    opt_element(&payload, "Arn")
        .unwrap_or_else(|| panic!("case {case:?}: no <Arn> in {payload}"))
}

/// Mint one IAM user and an access key for it, answering that key's id. Nothing here
/// quotes the minting answer, so the secret it carries never reaches the test output.
async fn user_with_a_key(router: &MultiServiceRouter, case: &str, user_name: &str) -> String {
    let (status, body) = query(
        router,
        AMBIENT_KEY,
        "iam",
        "2010-05-08",
        "CreateUser",
        &[("UserName", user_name.to_string())],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateUser, got {status} with body {body}"
    );
    let (status, minted) = query(
        router,
        AMBIENT_KEY,
        "iam",
        "2010-05-08",
        "CreateAccessKey",
        &[("UserName", user_name.to_string())],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateAccessKey, got status {status}"
    );
    opt_element(&minted, "AccessKeyId").unwrap_or_else(|| {
        panic!("case {case:?}: the minted-credential answer carries no <AccessKeyId> element")
    })
}

/// One Compute Optimizer Automation awsJson1_0 call, presenting `access_key_id`.
async fn automation(
    router: &MultiServiceRouter,
    access_key_id: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.0")
        .header(
            "x-amz-target",
            format!("ComputeOptimizerAutomationService.{operation}"),
        )
        .header("authorization", authorization(access_key_id, SIGNING_NAME))
        .body(Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let decoded = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, decoded)
}

/// The request `operation` takes for `resource`, with every other required member of
/// that operation filled in.
fn request_for(operation: &str, resource: &str) -> Value {
    match operation {
        "GetAutomationRule" => json!({"ruleArn": resource}),
        "DeleteAutomationRule" => json!({"ruleArn": resource, "ruleRevision": 1}),
        "ListTagsForResource" => json!({"resourceArn": resource}),
        other => panic!("no request is defined for {other}"),
    }
}

#[tokio::test]
async fn the_automation_refusal_names_the_identity_sts_reports() {
    let router = build_router();

    // (case, the access key id presented)
    let minted = user_with_a_key(&router, "a user's own key", "automation-reconciler").await;
    let credentials = [
        ("the key the emulator publishes", AMBIENT_KEY.to_string()),
        ("a key minted for an IAM user", minted),
    ];
    let operations = [
        "GetAutomationRule",
        "DeleteAutomationRule",
        "ListTagsForResource",
    ];
    let resources = [
        "not-an-arn",
        "arn:aws:compute-optimizer::1234:automation-rule/42d39f82e52fa35c",
        "arn:aws:ec2::000000000000:automation-rule/42d39f82e52fa35c",
    ];

    for (credential, access_key_id) in &credentials {
        let caller = caller_identity_arn(&router, credential, access_key_id).await;
        for operation in operations {
            for resource in resources {
                let case = format!("{credential}: {operation} on {resource}");
                let (status, body) =
                    automation(&router, access_key_id, operation, &request_for(operation, resource))
                        .await;
                assert_eq!(
                    status, 403,
                    "case {case:?}: expected HTTP 403, got {status} {body:?}"
                );
                let stated = body["message"].as_str().unwrap_or_default();
                assert_eq!(
                    stated,
                    format!(
                        "User: {caller} is not authorized to perform: \
                         {SIGNING_NAME}:{operation} on resource: {resource}"
                    ),
                    "case {case:?}: expected the refusal to name the identity STS reports \
                     ({caller:?}), the action and the resource as sent, got {stated:?}"
                );
            }
        }
    }

    // The two credentials are two different principals, so the rows above are not one
    // constant sentence asserted twice.
    let ambient = caller_identity_arn(&router, "the publisher", AMBIENT_KEY).await;
    let user = caller_identity_arn(&router, "the user", &credentials[1].1).await;
    assert_ne!(
        ambient, user,
        "the minted key must speak for the user it was minted for, not for the publisher: \
         got {user:?} for both"
    );
}
