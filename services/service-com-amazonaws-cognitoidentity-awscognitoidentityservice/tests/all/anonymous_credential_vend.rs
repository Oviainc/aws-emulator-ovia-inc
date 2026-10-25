// SPDX-License-Identifier: CC0-1.0

//! Which OPERATIONS this endpoint serves with no credential scope at all.
//!
//! RULE: an operation whose model lists NO auth scheme is served without SigV4. In this
//! service that is `GetId`, `GetCredentialsForIdentity`, `GetOpenIdToken` and
//! `UnlinkIdentity` — `smithy_query(query="digest")` reports `auth: []` and
//! `optionalAuth: true` on exactly those four, and on no other — because a client
//! exchanging a federated login for credentials holds nothing to sign with. Each of them
//! must therefore reach a handler of THIS service when it carries only its
//! `X-Amz-Target`, rather than the router's `UnknownService` for a request no mounted
//! service claimed.
//!
//! This is the OPERATION dimension of the claim rule; the ADDRESSING dimension — which
//! credential scope, target, host and path reach this service — is the table in
//! [`super::anonymous_credential_exchange`], and the unit table beside the rule itself in
//! `src/service.rs`. Every row here runs through an AUTHENTICATING router, which is how
//! the emulator serves traffic for real: a row is served only because its own model
//! leaves it unsigned.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_cognitoidentity_awscognitoidentityservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The `X-Amz-Target` prefix a client writes: the local name of this service's Smithy
/// shape `com.amazonaws.cognitoidentity#AWSCognitoIdentityService`.
const TARGET_PREFIX: &str = "AWSCognitoIdentityService";

/// The emulator's own address, which is what a client is pointed at: an address, so the
/// `Host` header names no service and the target prefix is the only routing signal left.
const EMULATOR_HOST: &str = "127.0.0.1:9297";

/// This service's real regional endpoint host, which a signed setup call is addressed to.
const ENDPOINT_HOST: &str = "cognito-identity.us-east-1.amazonaws.com";

/// The router's own refusal for a request no mounted service claimed.
const UNKNOWN_SERVICE: &str = "UnknownService";

/// Dispatch `operation` with `body`, carrying a credential scope only when `signed` is
/// true, and return the status with the parsed JSON body (`Null` for an empty one).
///
/// The sibling suite's harness is deliberately not borrowed here: that file is the
/// ADDRESSING table's own, and these rows need just one addressing form each, so this
/// suite keeps a reader of its own rather than widening the sibling's.
async fn call(
    router: &MultiServiceRouter,
    signed: bool,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let host = if signed { ENDPOINT_HOST } else { EMULATOR_HOST };
    let mut request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", host)
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"));
    if signed {
        request = request.header(
            "authorization",
            "AWS4-HMAC-SHA256 \
             Credential=AKIDEXAMPLE/20260101/us-east-1/cognito-identity/aws4_request, \
             SignedHeaders=host, Signature=0000",
        );
    }
    let response = router
        .dispatch(
            request
                .body(bytes::Bytes::from(body.to_string()))
                .expect("the request is well formed"),
        )
        .await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let payload = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{operation} answered a body that is not JSON ({error}): {}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    (status, payload)
}

/// The wire error code an answer carries, and `None` for one that carries none — a
/// success, which is already this service's own answer.
///
/// `identity_pool_flow::error_code` is this test crate's reader for an answer known to be
/// an error (it requires a `__type` to be there); the rows below ask the question of
/// answers that may be either, so the absence has to be a value rather than a panic.
fn error_type(payload: &Value) -> Option<&str> {
    payload
        .get("__type")
        .and_then(Value::as_str)
        .map(|code| code.rsplit('#').next().unwrap_or(code))
}

/// What an anonymous call to one operation owes.
#[derive(Clone, Copy, Debug)]
enum Owed {
    /// HTTP 200 with the operation's own output: an operation this emulator implements.
    Served,
    /// Reached a handler of this service — whatever that handler answers. An operation
    /// still carrying the 501 scaffold is claimed and answered by THIS service, which is
    /// all the claim rule owes, so the row pins neither a status nor a refusal: it stays
    /// true the day the operation is implemented and answers its own output.
    Reached,
}

#[tokio::test]
async fn every_operation_the_model_leaves_unsigned_reaches_this_service() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());

    // `CreateIdentityPool` is a signed operation, so the fixture pool is created through
    // a non-authenticating router (its signature would otherwise have to be real).
    let mut credentialed = MultiServiceRouter::new();
    credentialed.register_service(EmulatedService::new(Arc::clone(&engine)));
    credentialed.set_request_authentication(false);
    let (status, pool) = call(
        &credentialed,
        true,
        "CreateIdentityPool",
        &json!({
            "IdentityPoolName": "unsigned-operations",
            "AllowUnauthenticatedIdentities": true,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: CreateIdentityPool must succeed, got {status} with {pool}"
    );
    let pool_id = pool["IdentityPoolId"]
        .as_str()
        .expect("CreateIdentityPool returns an IdentityPoolId")
        .to_string();

    // Request authentication ON from here on, as the emulator runs it: every row below
    // carries no signature at all.
    let mut serving = MultiServiceRouter::new();
    serving.register_service(EmulatedService::new(engine));
    serving.set_request_authentication(true);

    let (status, minted) = call(
        &serving,
        false,
        "GetId",
        &json!({ "IdentityPoolId": pool_id }),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: an anonymous GetId must be served, got {status} with {minted}"
    );
    let identity_id = minted["IdentityId"]
        .as_str()
        .expect("GetId returns an IdentityId")
        .to_string();

    /// One row: the operation, the request it is called with, and what it owes.
    struct Row {
        case: &'static str,
        operation: &'static str,
        body: Value,
        owed: Owed,
    }

    let rows = [
        Row {
            case: "an identity for a client that holds no credential",
            operation: "GetId",
            body: json!({ "IdentityPoolId": pool_id }),
            owed: Owed::Served,
        },
        Row {
            case: "the session that client signs its next calls with",
            operation: "GetCredentialsForIdentity",
            body: json!({ "IdentityId": identity_id }),
            owed: Owed::Served,
        },
        Row {
            case: "an OpenID token for a known identity",
            operation: "GetOpenIdToken",
            body: json!({ "IdentityId": identity_id }),
            owed: Owed::Reached,
        },
        Row {
            case: "unlinking a federated login from an identity",
            operation: "UnlinkIdentity",
            body: json!({
                "IdentityId": identity_id,
                "Logins": { "graph.facebook.com": "a.token.value" },
                "LoginsToRemove": ["graph.facebook.com"],
            }),
            owed: Owed::Reached,
        },
    ];

    for row in rows {
        let (status, payload) = call(&serving, false, row.operation, &row.body).await;
        // Whatever the answer is, it must not be the router's refusal for a request no
        // mounted service claimed — the one answer that means the rule is broken.
        assert_ne!(
            error_type(&payload),
            Some(UNKNOWN_SERVICE),
            "case {:?}: an anonymous {} must be answered by this service, got {status} \
             with {payload}",
            row.case,
            row.operation
        );
        if matches!(row.owed, Owed::Served) {
            assert_eq!(
                status, 200,
                "case {:?}: an anonymous {} must be served, got {status} with {payload}",
                row.case, row.operation
            );
        }
    }
}
