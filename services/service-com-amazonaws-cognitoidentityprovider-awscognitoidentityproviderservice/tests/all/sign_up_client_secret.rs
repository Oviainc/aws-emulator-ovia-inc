// SPDX-License-Identifier: CC0-1.0

//! `SignUp` against an app client that holds a client secret, over the wire.
//!
//! RULE under test: a request addressed to an app client that holds a client secret
//! registers nobody unless it carries the `SecretHash` that client's secret produces —
//! "a keyed-hash message authentication code (HMAC) calculated using the secret key of a
//! user pool client and username plus the client ID in the message"
//! (`smithy_query(query="doc",shape="SignUpRequest",member="SecretHash")`), carried
//! base64-encoded as the modelled `SecretHashType` pattern `^[\w+=/]+$` admits. A client
//! that holds NO secret has no such value to verify, so a request to it is served either
//! way.
//!
//! The table's rows differ along what the hash is derived from: the right secret, no hash
//! at all, a value that is not a hash, another client's secret, the two inputs in the
//! other order, and another username. The hash each row presents is computed in this test
//! from `ring`'s HMAC-SHA256 and the base64 codec — independently of the handler, so the
//! test pins the VALUE a client has to present and not merely the handler's own idea of
//! it.
//!
//! Every row names its own user, so one row's refusal cannot explain another's, and the
//! refused rows are read back afterwards: a `SignUp` the secret hash refused persists
//! nothing.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice as cognito_idp;

/// This service's SigV4 signing name, the 4th field of a credential scope.
const SIGNING_NAME: &str = "cognito-idp";

/// The local name of this service's Smithy shape, which is the `X-Amz-Target` prefix a
/// client writes (`smithy_query(query="service_info")`:
/// `com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService`).
const TARGET_PREFIX: &str = "AWSCognitoIdentityProviderService";

/// The emulator's own endpoint, which is what an unsigned client is pointed at.
const EMULATOR_HOST: &str = "127.0.0.1:9297";

/// This service's real regional endpoint host, which a signed call addresses.
const ENDPOINT_HOST: &str = "cognito-idp.us-east-1.amazonaws.com";

/// A password satisfying a default pool's password policy.
const GOOD_PASSWORD: &str = "Passw0rd!2024";

/// Whether a request carries a SigV4 credential scope.
#[derive(Clone, Copy, Debug)]
enum Addressing {
    /// A signed call, as the SDK sends every credentialed operation of this service.
    Signed,
    /// An unsigned call to the emulator's own endpoint: how `SignUp` is called.
    Anonymous,
}

/// The shared engine every router in one test mounts.
fn engine() -> Arc<engine::InMemoryEngine> {
    Arc::new(engine::InMemoryEngine::new())
}

/// One emulator with just this service mounted over `engine`.
fn router(engine: &Arc<engine::InMemoryEngine>, authenticating: bool) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.register_service(cognito_idp::EmulatedService::new(Arc::clone(engine)));
    router.set_request_authentication(authenticating);
    router
}

/// Dispatch `operation` with `body`, addressed as `addressing` says, and return the
/// status and JSON response body.
async fn dispatch(
    router: &MultiServiceRouter,
    addressing: Addressing,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let host = match addressing {
        Addressing::Signed => ENDPOINT_HOST,
        Addressing::Anonymous => EMULATOR_HOST,
    };
    let mut request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", host)
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"));
    if matches!(addressing, Addressing::Signed) {
        request = request.header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 \
                 Credential=AKIDEXAMPLE/20260101/us-east-1/{SIGNING_NAME}/aws4_request, \
                 SignedHeaders=host;x-amz-date, Signature=0000"
            ),
        );
    }
    let response = router
        .dispatch(
            request
                .body(Bytes::from(body.to_string()))
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

/// Call `operation` signed, requiring success, and return its response body.
async fn ok(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, payload) = dispatch(router, Addressing::Signed, operation, body).await;
    assert_eq!(
        status, 200,
        "{operation} must succeed, got {status} with {payload}"
    );
    payload
}

/// The `__type` of an error body, which is the wire error code.
fn error_code(payload: &Value) -> &str {
    payload
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
}

/// The `SecretHash` a request for `username` to the app client `client_id` carries when
/// that client's secret is `client_secret`: base64 of HMAC-SHA256, keyed with the secret,
/// over the username followed by the client id (`SignUpRequest.SecretHash`).
fn secret_hash(client_secret: &str, username: &str, client_id: &str) -> String {
    let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, client_secret.as_bytes());
    let tag = ring::hmac::sign(&key, format!("{username}{client_id}").as_bytes());
    aws_smithy_types::base64::encode(tag.as_ref())
}

/// A `SignUp` request body carrying `secret_hash` when the row presents one.
fn sign_up_body(client_id: &str, username: &str, secret_hash: Option<&str>) -> Value {
    let mut body = Map::new();
    body.insert("ClientId".to_string(), json!(client_id));
    body.insert("Username".to_string(), json!(username));
    body.insert("Password".to_string(), json!(GOOD_PASSWORD));
    if let Some(secret_hash) = secret_hash {
        body.insert("SecretHash".to_string(), json!(secret_hash));
    }
    Value::Object(body)
}

/// An app client as this test holds it: the id a request names and the secret it has to
/// prove, when it has one.
struct AppClient {
    client_id: String,
    client_secret: Option<String>,
}

/// Create an app client in `user_pool_id`, with a generated secret when `with_secret`.
async fn create_client(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    client_name: &str,
    with_secret: bool,
) -> AppClient {
    let created = ok(
        router,
        "CreateUserPoolClient",
        &json!({
            "UserPoolId": user_pool_id,
            "ClientName": client_name,
            "GenerateSecret": with_secret,
        }),
    )
    .await;
    let client = &created["UserPoolClient"];
    let client_secret = client["ClientSecret"].as_str().map(str::to_string);
    assert_eq!(
        client_secret.is_some(),
        with_secret,
        "GenerateSecret={with_secret} must decide whether a secret is minted, got {client}"
    );
    AppClient {
        client_id: client["ClientId"]
            .as_str()
            .expect("a minted client id")
            .to_string(),
        client_secret,
    }
}

/// RULE: a `SignUp` addressed to an app client that holds a client secret is served only
/// when it carries the `SecretHash` that secret, the username and the client id produce;
/// a client with no secret has nothing to verify.
#[tokio::test]
async fn sign_up_requires_the_secret_hash_of_an_app_client_that_holds_a_secret() {
    let engine = engine();
    let setup = router(&engine, false);
    let emulator = router(&engine, true);

    let user_pool_id = ok(&setup, "CreateUserPool", &json!({ "PoolName": "secret-hash" }))
        .await["UserPool"]["Id"]
        .as_str()
        .expect("a minted pool id")
        .to_string();

    // Three clients of ONE pool: one with no secret, and two with their own secrets, so a
    // hash is tied to the client whose secret made it rather than to the pool.
    let open = create_client(&setup, &user_pool_id, "no-secret", false).await;
    let confidential = create_client(&setup, &user_pool_id, "with-secret", true).await;
    let other = create_client(&setup, &user_pool_id, "another-secret", true).await;
    let secret = confidential
        .client_secret
        .as_deref()
        .expect("GenerateSecret=true mints a secret");
    let other_secret = other
        .client_secret
        .as_deref()
        .expect("GenerateSecret=true mints a secret");

    // (case, client id, username, presented SecretHash, expected status, expected code)
    type Row = (
        &'static str,
        String,
        &'static str,
        Option<String>,
        u16,
        Option<&'static str>,
    );
    let rows: Vec<Row> = vec![
        (
            "a client with no secret and no hash",
            open.client_id.clone(),
            "nohash",
            None,
            200,
            None,
        ),
        (
            "a client with no secret, and a hash it has no secret to verify",
            open.client_id.clone(),
            "strayhash",
            Some(secret_hash(secret, "strayhash", &open.client_id)),
            200,
            None,
        ),
        (
            "a secret client and the hash its own secret produces",
            confidential.client_id.clone(),
            "alice",
            Some(secret_hash(secret, "alice", &confidential.client_id)),
            200,
            None,
        ),
        (
            "a secret client and no hash at all",
            confidential.client_id.clone(),
            "bob",
            None,
            403,
            Some("NotAuthorizedException"),
        ),
        (
            "a secret client and a well-formed value that is not a hash",
            confidential.client_id.clone(),
            "carol",
            Some("notTheHash".to_string()),
            403,
            Some("NotAuthorizedException"),
        ),
        (
            "a secret client and the hash of ANOTHER client's secret",
            confidential.client_id.clone(),
            "dave",
            Some(secret_hash(
                other_secret,
                "dave",
                &confidential.client_id,
            )),
            403,
            Some("NotAuthorizedException"),
        ),
        (
            "a secret client and the hash over the client id before the username",
            confidential.client_id.clone(),
            "erin",
            Some(secret_hash(secret, &confidential.client_id, "erin")),
            403,
            Some("NotAuthorizedException"),
        ),
        (
            "a secret client and the hash of a DIFFERENT username",
            confidential.client_id.clone(),
            "frank",
            Some(secret_hash(secret, "grace", &confidential.client_id)),
            403,
            Some("NotAuthorizedException"),
        ),
        (
            "the other secret client and the hash its own secret produces",
            other.client_id.clone(),
            "heidi",
            Some(secret_hash(other_secret, "heidi", &other.client_id)),
            200,
            None,
        ),
    ];

    for (case, client_id, username, presented, expected_status, expected_code) in &rows {
        let (status, payload) = dispatch(
            &emulator,
            Addressing::Anonymous,
            "SignUp",
            &sign_up_body(client_id, username, presented.as_deref()),
        )
        .await;
        assert_eq!(
            status, *expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with {payload}"
        );
        match expected_code {
            Some(code) => assert_eq!(
                error_code(&payload),
                *code,
                "case {case:?}: expected error code {code}, got {payload}"
            ),
            None => {
                let user_sub = payload["UserSub"].as_str().unwrap_or_default();
                assert!(
                    !user_sub.is_empty(),
                    "case {case:?}: a served registration reports the sub it minted, \
                     got {payload}"
                );
            }
        }
    }

    // What each row did to the POOL: the served rows stored their user, and every row the
    // secret hash refused stored nothing — read back through an implemented reader.
    for (case, _, username, _, expected_status, _) in &rows {
        let (status, payload) = dispatch(
            &setup,
            Addressing::Signed,
            "AdminGetUser",
            &json!({ "UserPoolId": user_pool_id, "Username": username }),
        )
        .await;
        if *expected_status == 200 {
            assert_eq!(
                status, 200,
                "case {case:?}: the registered user must read back, got {status} with {payload}"
            );
            assert_eq!(
                payload["Username"],
                json!(username),
                "case {case:?}: expected the registered username, got {payload}"
            );
            assert_eq!(
                payload["UserStatus"],
                json!("UNCONFIRMED"),
                "case {case:?}: a registered user is UNCONFIRMED, got {payload}"
            );
        } else {
            assert_eq!(
                status, 404,
                "case {case:?}: a refused registration stores no user, got {status} with \
                 {payload}"
            );
            assert_eq!(
                error_code(&payload),
                "UserNotFoundException",
                "case {case:?}: a refused registration stores no user, got {payload}"
            );
        }
    }
}
