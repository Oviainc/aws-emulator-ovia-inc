// SPDX-License-Identifier: CC0-1.0

//! One credential, two services: the principal Billing names in an
//! `AccessDeniedException` is the identity STS reports for the SAME credential.
//!
//! Billing refuses a billing-view ARN outside the caller's account with a message
//! built from the request and the caller — "User: <identity> is not authorized to
//! perform: billing:<Operation> on resource: <arn> …". The identity in it is the
//! caller's own, which is to say the `Arn` that `sts:GetCallerIdentity` answers
//! for the credential the request was signed with. A client that parses the denied
//! principal out of that message, or asserts it equals its own caller identity,
//! must see ONE principal for one credential — and only a cross-service suite can
//! pin that: each crate's own tests can only ever assert its own half.
//!
//! The rows vary what the rule generalises over: the operation refusing (every
//! implemented Billing operation that identifies a view by ARN), and the account,
//! partition and resource id of the foreign ARN.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// Reading one uniquely named element out of an awsQuery XML body already has a
// helper in this test crate, so this suite imports it instead of hand-rolling a
// third copy.
use crate::iam_role_id_agrees_with_sts::xml_element;

/// A SigV4 `Authorization` header whose credential scope names `service`. The
/// signature is never checked: the router here runs with request authentication
/// off, and the access key id is what resolves the caller's identity.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-east-1/{service}/aws4_request, \
         SignedHeaders=host, Signature=0"
    )
}

/// The emulator with Billing and STS mounted on ONE engine, as
/// `core/router/src/lib.rs` mounts them.
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_billing_awsbilling::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_sts_awssecuritytokenservicev20110615::EmulatedService::new(engine),
    );
    router
}

/// The `Arn` STS reports for the credential this suite signs with, over STS's own
/// awsQuery wire.
async fn caller_identity_arn(router: &MultiServiceRouter) -> String {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "sts.amazonaws.com")
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("sts"))
        .body(Bytes::from("Action=GetCallerIdentity&Version=2011-06-15"))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = String::from_utf8_lossy(response.body()).to_string();
    assert_eq!(
        status, 200,
        "expected HTTP 200 from GetCallerIdentity, got {status} with body {payload}"
    );
    xml_element(&payload, "Arn").unwrap_or_else(|| panic!("no <Arn> in {payload}"))
}

/// Send one Billing awsJson1_0 call and return its status and decoded body.
async fn billing(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("AWSBilling.{operation}"))
        .header("authorization", authorization("billing"))
        .body(Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let decoded = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, decoded)
}

#[tokio::test]
async fn billing_denies_in_the_name_of_the_identity_sts_reports() {
    let router = build_router();
    let caller = caller_identity_arn(&router).await;

    type BodyOf = fn(&str) -> Value;
    let operations: [(&str, BodyOf); 4] = [
        ("GetBillingView", |arn| json!({ "arn": arn })),
        ("UpdateBillingView", |arn| {
            json!({ "arn": arn, "name": "renamed" })
        }),
        ("DeleteBillingView", |arn| json!({ "arn": arn })),
        ("ListBillingViews", |arn| json!({ "arns": [arn] })),
    ];
    let foreign_arns = [
        "arn:aws:billing::111122223333:billingview/primary",
        "arn:aws:billing::123456789012:billingview/does-not-exist",
        "arn:aws-cn:billing::999999999999:billingview/custom-3",
    ];

    for arn in foreign_arns {
        for (operation, body_of) in operations {
            let case = format!("{operation} on {arn}");
            let (status, body) = billing(&router, operation, &body_of(arn)).await;
            assert_eq!(
                status, 403,
                "case {case:?}: expected HTTP 403, got {status} {body:?}"
            );
            let stated = body["message"].as_str().unwrap_or_default();
            assert!(
                stated.starts_with(&format!("User: {caller} is not authorized to perform:")),
                "case {case:?}: expected the refusal to name the identity STS reports \
                 ({caller:?}), got {stated:?}"
            );
            assert!(
                stated.contains(arn),
                "case {case:?}: the refusal is built from the requested ARN, got {stated:?}"
            );
        }
    }
}
