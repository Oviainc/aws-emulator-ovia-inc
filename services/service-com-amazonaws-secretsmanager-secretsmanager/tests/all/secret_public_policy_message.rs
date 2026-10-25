// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what this service SAYS about a resource policy that
//! grants public access to a secret, driven over the wire through the
//! multi-service router.
//!
//! The rule under test: the service says one fixed sentence about that
//! condition, and the same sentence wherever it says it — reported as the
//! `ErrorMessage` of the broad-access entry `ValidateResourcePolicy` answers
//! with on HTTP 200, and carried by the `PublicPolicyException` a
//! `PutResourcePolicy` under `BlockPublicPolicy` is refused with. No part of the
//! request or of the stored state is interpolated into it: not the secret's
//! name, id or ARN, not a statement's `Sid`, not the principal, not the action.
//!
//! The tables span what the rule generalises over: the form of the wildcard
//! principal (bare string, a principal type, the wildcard among concrete
//! identifiers), the number of statements and the position of the offending one,
//! the `Sid` and action a statement carries, and — on the validating channel —
//! whether the request names a `SecretId` at all.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_secretsmanager_secretsmanager::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 header naming `secretsmanager` in `us-west-2`; the router routes on
/// the credential scope and the emulator verifies signatures only when an
/// authenticator is registered, which this harness does not do.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-west-2/secretsmanager/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=deadbeef";

/// The one sentence this service says about a resource policy granting public
/// access to a secret, written out here rather than imported from the crate, so
/// the TEXT is what is pinned and not merely the constant's reuse.
const PUBLIC_ACCESS_SENTENCE: &str =
    "You can not attach a resource policy that grants Public access to your secret.";

/// The name the broad-access check is reported under.
const PUBLIC_ACCESS_CHECK: &str = "BlockPublicPolicyCheck";

/// A fresh emulator holding only this service, over its own engine.
fn emulator() -> MultiServiceRouter {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(EmulatedService::new(engine));
    router
}

/// Send one `awsJson1_1` request and return its status and JSON body.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("secretsmanager.{operation}"))
        .header("authorization", AUTHORIZATION)
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("the request body is serializable"),
        ))
        .expect("a valid request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// The account every identity this emulator can resolve belongs to.
fn this_account() -> &'static str {
    engine::emulator_account_id()
}

/// A one-statement policy document naming this principal, with a `Sid` and an
/// action of the row's choosing, rendered through `serde_json` so it is a
/// document the service has to parse rather than one it can match on.
fn policy(sid: &str, action: &str, principal: Value) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": sid,
            "Effect": "Allow",
            "Principal": principal,
            "Action": action,
            "Resource": "*"
        }]
    })
    .to_string()
}

/// One row of the table: a case name and a document whose statements grant
/// public access.
type PublicDocument = (&'static str, String);

/// Documents that grant public access, spanning the form of the wildcard
/// principal, the statement count and the position of the offending statement,
/// and the `Sid`/action each statement carries.
fn public_documents() -> Vec<PublicDocument> {
    let account = this_account();
    let root = format!("arn:aws:iam::{account}:root");
    vec![
        (
            "bare-wildcard",
            policy("PublicRead", "secretsmanager:GetSecretValue", json!("*")),
        ),
        (
            "wildcard-under-a-principal-type",
            policy("OpenToAll", "secretsmanager:*", json!({"AWS": "*"})),
        ),
        (
            "wildcard-among-concrete-identifiers",
            policy(
                "RootAndEveryone",
                "secretsmanager:DescribeSecret",
                json!({"AWS": [&root, "*"]}),
            ),
        ),
        (
            "wildcard-allow-last-of-three-statements",
            json!({
                "Version": "2012-10-17",
                "Statement": [
                    {"Sid": "DenyRole", "Effect": "Deny",
                     "Principal": {"AWS": format!("arn:aws:iam::{account}:role/reader")},
                     "Action": "secretsmanager:GetSecretValue", "Resource": "*"},
                    {"Sid": "AllowRoot", "Effect": "Allow", "Principal": {"AWS": &root},
                     "Action": "secretsmanager:GetSecretValue", "Resource": "*"},
                    {"Sid": "AllowEveryone", "Effect": "Allow", "Principal": "*",
                     "Action": "secretsmanager:GetSecretValue", "Resource": "*"}
                ]
            })
            .to_string(),
        ),
        (
            "wildcard-allow-as-a-single-statement-object",
            json!({
                "Version": "2012-10-17",
                "Statement": {
                    "Sid": "LoneStatement", "Effect": "Allow", "Principal": {"AWS": "*"},
                    "Action": "secretsmanager:GetSecretValue", "Resource": "*"
                }
            })
            .to_string(),
        ),
    ]
}

/// Nothing from the request or from state may appear in the sentence: the
/// secret's name and ARN, the account id, and every `Sid`, action and principal
/// the documents above carry.
fn assert_nothing_interpolated(message: &str, case: &str, request_values: &[&str]) {
    for value in request_values {
        assert!(
            !message.contains(value),
            "case {case:?}: the fixed sentence must interpolate nothing, but {value:?} appears \
             in {message:?}"
        );
    }
}

/// `ValidateResourcePolicy` reports the broad-access check with the service's
/// fixed sentence, for every form of a public document and whether or not the
/// request names a secret.
#[tokio::test]
async fn the_reported_broad_access_message_is_the_services_fixed_sentence() {
    let router = emulator();
    let name = "public-policy-message-secret";
    let created = call(
        &router,
        "CreateSecret",
        &json!({"Name": name, "SecretString": "s3cret"}),
    )
    .await;
    assert_eq!(
        created.0, 200,
        "setup: expected 200 from CreateSecret, got {} with body {}",
        created.0, created.1
    );
    let arn = created
        .1
        .get("ARN")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("setup: no ARN in {}", created.1))
        .to_string();

    for (case, document) in public_documents() {
        for secret_id in [Some(name), None] {
            let row = format!("{case}/SecretId={secret_id:?}");
            let mut body = json!({"ResourcePolicy": &document});
            if let Some(secret_id) = secret_id {
                body["SecretId"] = json!(secret_id);
            }
            let (status, response) = call(&router, "ValidateResourcePolicy", &body).await;
            assert_eq!(
                status, 200,
                "case {row:?}: a valid policy granting public access is reported on, not \
                 refused; got {status} with body {response}"
            );
            assert_eq!(
                response.get("PolicyValidationPassed").and_then(Value::as_bool),
                Some(false),
                "case {row:?}: expected PolicyValidationPassed=false, got {response}"
            );
            let reported: Vec<(String, String)> = response
                .get("ValidationErrors")
                .and_then(Value::as_array)
                .unwrap_or_else(|| panic!("case {row:?}: no ValidationErrors in {response}"))
                .iter()
                .map(|entry| {
                    let text = |member: &str| {
                        entry
                            .get(member)
                            .and_then(Value::as_str)
                            .unwrap_or_else(|| {
                                panic!("case {row:?}: no {member} in entry {entry}")
                            })
                            .to_string()
                    };
                    (text("CheckName"), text("ErrorMessage"))
                })
                .collect();
            let messages: Vec<&str> = reported
                .iter()
                .filter(|(check, _)| check == PUBLIC_ACCESS_CHECK)
                .map(|(_, message)| message.as_str())
                .collect();
            assert_eq!(
                messages,
                vec![PUBLIC_ACCESS_SENTENCE],
                "case {row:?}: expected one {PUBLIC_ACCESS_CHECK} entry carrying \
                 {PUBLIC_ACCESS_SENTENCE:?}, got {reported:?}"
            );
            assert_nothing_interpolated(
                messages[0],
                &row,
                &[
                    name,
                    &arn,
                    this_account(),
                    "PublicRead",
                    "OpenToAll",
                    "RootAndEveryone",
                    "AllowEveryone",
                    "LoneStatement",
                    "secretsmanager:",
                    "\"*\"",
                ],
            );
        }
    }
}

/// `PutResourcePolicy` refuses the same documents under `BlockPublicPolicy` with
/// `PublicPolicyException` carrying the very sentence the validating channel
/// reports, so a caller using the validator as a pre-flight guard reads the text
/// the refusal will later carry.
#[tokio::test]
async fn the_refusal_carries_the_same_sentence_the_validator_reports() {
    let router = emulator();
    let name = "blocked-public-policy-secret";
    let created = call(
        &router,
        "CreateSecret",
        &json!({"Name": name, "SecretString": "s3cret"}),
    )
    .await;
    assert_eq!(
        created.0, 200,
        "setup: expected 200 from CreateSecret, got {} with body {}",
        created.0, created.1
    );

    for (case, document) in public_documents() {
        let (status, response) = call(
            &router,
            "PutResourcePolicy",
            &json!({
                "SecretId": name,
                "ResourcePolicy": &document,
                "BlockPublicPolicy": true
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a 400 refusal from PutResourcePolicy, got {status} with \
             body {response}"
        );
        let raw = response
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: no __type in error body {response}"));
        let code = raw.split('#').next_back().unwrap_or(raw);
        assert_eq!(
            code, "PublicPolicyException",
            "case {case:?}: expected PublicPolicyException, got {code} with body {response}"
        );
        let message = response
            .get("message")
            .or_else(|| response.get("Message"))
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: no message in error body {response}"));
        assert_eq!(
            message, PUBLIC_ACCESS_SENTENCE,
            "case {case:?}: expected the refusal to carry {PUBLIC_ACCESS_SENTENCE:?}, got \
             {message:?}"
        );
        assert_nothing_interpolated(message, case, &[name, this_account(), "secretsmanager:"]);
    }
}
