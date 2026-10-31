// SPDX-License-Identifier: CC0-1.0

//! Cover for the SHAPE of the token ARN a source-credential operation addresses a
//! credential by, driven over the wire the router serves: `POST /` with
//! `X-Amz-Target: CodeBuild_20161006.<Operation>` on awsJson1_1.
//!
//! The rule under test: **an operation whose only input is a resource ARN PARSES that ARN
//! before it reads any state, and a value that is not an ARN of the resource kind the
//! operation addresses is refused as invalid input rather than as an absent resource.**
//! For a source credential the ARN is
//! `arn:<partition>:codebuild:<region>:<account>:token/<server type>` — the server type
//! is the only request value it carries, which is why a credential is held per server
//! type — so a resource segment that is not `token/<a ServerType>` names no credential
//! this service could ever have minted, whatever the registry holds. CodeBuild refuses it
//! with HTTP 400 `InvalidInputException` and the fixed sentence
//! [`state::INVALID_TOKEN_ARN_MESSAGE`]; a WELL-FORMED token ARN that merely names
//! nothing stored is the other class, `ResourceNotFoundException` (see
//! `tests/all/source_credentials.rs`).
//!
//! The rows span the ways a value fails to be such an ARN — each ARN field in turn, the
//! resource type, the server-type segment (including every server type the model does NOT
//! declare that reads like one) and values that are no ARN at all — and are asserted
//! against both an account holding a credential and an empty one, since the refusal is
//! decided before the store is read.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_codebuild_codebuild_20161006::ops::types::SERVER_TYPE_VALUES;
use service_com_amazonaws_codebuild_codebuild_20161006::state;
use service_com_amazonaws_codebuild_codebuild_20161006::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below is signed for.
const REGION: &str = "us-west-2";

/// One router carrying only this service.
fn emulator() -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::new(engine::InMemoryEngine::new())));
    router
}

/// Call one operation and return its HTTP status and decoded body.
async fn call(router: &MultiServiceRouter, operation: &str, body: Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("CodeBuild_20161006.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDCODEBUILDARN/20260101/{REGION}/codebuild/\
                 aws4_request, SignedHeaders=host;x-amz-target, Signature=0f0f"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(&body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let decoded = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, decoded)
}

/// The `__type` an error body carries.
fn error_type(body: &Value) -> String {
    body["__type"].as_str().unwrap_or_default().to_string()
}

/// A well-formed token ARN of this service for `server_type`, the way
/// `ImportSourceCredentials` mints one.
fn token_arn(server_type: &str) -> String {
    format!(
        "arn:aws:codebuild:{REGION}:{}:token/{}",
        engine::EMULATOR_ACCOUNT_ID,
        server_type.to_ascii_lowercase()
    )
}

/// Hold a credential for `server_type`, so the table below runs against an account that
/// really has one.
async fn import(router: &MultiServiceRouter, server_type: &str) {
    let (status, body) = call(
        router,
        "ImportSourceCredentials",
        json!({
            "serverType": server_type,
            "authType": "PERSONAL_ACCESS_TOKEN",
            "token": "an-access-token",
            "shouldOverwrite": true,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the setup import for {server_type} must succeed, got {status} with {body}"
    );
}

/// Values that are not a token ARN of this service, each with what is wrong with it.
///
/// The server-type rows are the point of the class: a segment that READS like a provider
/// but is none the model declares (`ServerType`:
/// `smithy_query(query="shape", shape="ServerType")`) is still not an ARN this service
/// could have minted.
fn malformed_arns() -> Vec<(&'static str, String)> {
    let account = engine::EMULATOR_ACCOUNT_ID;
    vec![
        (
            "a server type no ServerType value names",
            token_arn("github_does_not_exist"),
        ),
        ("an empty server type", token_arn("")),
        (
            "a server type that is a prefix of a real one",
            token_arn("git"),
        ),
        (
            "a server type carrying a real one as a suffix",
            token_arn("my-github"),
        ),
        (
            "another resource kind of this service",
            format!("arn:aws:codebuild:{REGION}:{account}:project/github"),
        ),
        (
            "the report-group resource kind",
            format!("arn:aws:codebuild:{REGION}:{account}:report-group/github"),
        ),
        (
            "no resource segment at all",
            format!("arn:aws:codebuild:{REGION}:{account}:"),
        ),
        (
            "another service's namespace",
            format!("arn:aws:codecommit:{REGION}:{account}:token/github"),
        ),
        (
            "an empty partition",
            format!("arn::codebuild:{REGION}:{account}:token/github"),
        ),
        (
            "too few ARN fields",
            format!("arn:aws:codebuild:{account}:token/github"),
        ),
        ("not an ARN at all", "GITHUB".to_string()),
        ("a bare server type path", "token/github".to_string()),
        ("a single space", " ".to_string()),
    ]
}

/// Every malformed value is refused as invalid input, with CodeBuild's fixed sentence,
/// before any credential is looked up — whether or not the account holds one, and with
/// every stored credential left in place.
#[tokio::test]
async fn a_value_that_is_not_a_token_arn_is_refused_as_invalid_input() {
    for holds_a_credential in [false, true] {
        let router = emulator();
        if holds_a_credential {
            import(&router, "GITHUB").await;
        }

        for (why, arn) in malformed_arns() {
            let case = format!("{why} (account holds a credential: {holds_a_credential})");
            let (status, body) =
                call(&router, "DeleteSourceCredentials", json!({"arn": arn})).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected 400, got {status} with {body}"
            );
            assert_eq!(
                error_type(&body),
                "InvalidInputException",
                "case {case:?}: expected InvalidInputException, got {:?}",
                error_type(&body)
            );
            assert_eq!(
                body["message"].as_str(),
                Some(state::INVALID_TOKEN_ARN_MESSAGE),
                "case {case:?}: expected CodeBuild's invalid-token-ARN sentence, got {:?}",
                body["message"]
            );
        }

        // The refused calls read nothing and removed nothing: the credential the account
        // holds is still the one it holds.
        let (status, body) = call(&router, "ListSourceCredentials", json!({})).await;
        assert_eq!(
            status, 200,
            "ListSourceCredentials must be served, got {status} with {body}"
        );
        let listed = body["sourceCredentialsInfos"]
            .as_array()
            .map(Vec::len)
            .unwrap_or_default();
        assert_eq!(
            listed,
            usize::from(holds_a_credential),
            "a refused delete must leave the account's credentials alone, got {body}"
        );
    }
}

/// The other side of the same rule: every server type the model DOES declare makes a
/// token ARN this operation acts on — it deletes the credential held for it, and reports
/// the well-formed-but-absent case as the not-found class instead.
#[tokio::test]
async fn every_modelled_server_type_makes_an_arn_the_operation_acts_on() {
    for server_type in SERVER_TYPE_VALUES {
        let router = emulator();
        let arn = token_arn(server_type);

        // Well-formed and nothing stored: the resource is absent, not the request wrong.
        let (status, body) = call(
            &router,
            "DeleteSourceCredentials",
            json!({"arn": arn.clone()}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {server_type:?}: expected 400 for an absent credential, got {status} with {body}"
        );
        assert_eq!(
            error_type(&body),
            "ResourceNotFoundException",
            "case {server_type:?}: a well-formed token ARN naming nothing is the not-found \
             class, got {:?}",
            error_type(&body)
        );

        // And with the credential held, the same ARN deletes it.
        import(&router, server_type).await;
        let (status, body) = call(
            &router,
            "DeleteSourceCredentials",
            json!({"arn": arn.clone()}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {server_type:?}: expected 200 deleting the held credential, got {status} \
             with {body}"
        );
        assert_eq!(
            body["arn"].as_str(),
            Some(arn.as_str()),
            "case {server_type:?}: the answer must report the ARN it deleted, got {body}"
        );
    }
}
