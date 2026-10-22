// SPDX-License-Identifier: CC0-1.0

//! The form of the tokens this service issues: the base64url of their segments
//! and the unsecured-JWS compact serialization they are serialized as.
//!
//! The encoding is pinned by PUBLISHED vectors rather than by this emulator's
//! own output: RFC 4648 §10's base64 test strings, plus inputs whose standard
//! encoding carries both characters RFC 7515 §2 substitutes. A second suite
//! signs in over the wire and reads the issued tokens back: three segments with
//! an EMPTY third one and an `alg` of `none`, exactly what RFC 7515 §6 defines
//! for a JWS that carries no signature — this emulator holds no JWKS key pair
//! and verifies no token, so it says so in the header instead of filling the
//! segment with bytes that would only look like a signature. Their claims come
//! from stored state, and each token resolves back to the stored user.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice as cognito_idp;
use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice::state;

/// A request's SigV4 credential scope is what routes it to this service; the
/// signature itself is never checked here.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/cognito-idp/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0000";

/// A password satisfying a default pool's password policy.
const GOOD_PASSWORD: &str = "Passw0rd!2024";

/// One emulator with just this service mounted, beside the engine its state
/// lives in.
fn router_and_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(cognito_idp::EmulatedService::new(Arc::clone(&engine)));
    router.set_request_authentication(false);
    (router, engine)
}

/// Call `operation`, requiring success, and return its response body.
async fn ok(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "cognito-idp.us-east-1.amazonaws.com")
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "x-amz-target",
            format!("AWSCognitoIdentityProviderService.{operation}"),
        )
        .header("authorization", AUTHORIZATION)
        .body(Bytes::from(body.to_string()))
        .expect("the request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload: Value = serde_json::from_slice(response.body()).unwrap_or_else(|error| {
        panic!(
            "{operation} answered a body that is not JSON ({error}): {:?}",
            String::from_utf8_lossy(response.body())
        )
    });
    assert_eq!(
        status, 200,
        "{operation} must succeed, got {status} with {payload}"
    );
    payload
}

/// A base64url segment's bytes, decoded with the codec the wire serializer uses
/// (`aws_smithy_types::base64`): the two alphabet substitutions and the padding
/// RFC 7515 §2 strips are restored first.
fn decode_base64url(segment: &str) -> Vec<u8> {
    let mut standard = segment.replace('-', "+").replace('_', "/");
    while !standard.len().is_multiple_of(4) {
        standard.push('=');
    }
    aws_smithy_types::base64::decode(&standard)
        .unwrap_or_else(|error| panic!("{segment:?} must be base64url ({error})"))
}

/// A JWS segment is base64url without padding (RFC 7515 §2): the standard
/// base64 of RFC 4648 §10 with `+`/`/` replaced and `=` dropped.
#[tokio::test]
async fn a_jws_segment_is_base64url_without_padding() {
    // The first rows are RFC 4648 §10's own base64 test vectors with the
    // padding stripped; the last two are byte strings whose standard encoding
    // contains the two characters RFC 7515 §2 substitutes.
    let cases: &[(&str, &[u8], &str)] = &[
        ("RFC 4648 \"\"", b"", ""),
        ("RFC 4648 \"f\"", b"f", "Zg"),
        ("RFC 4648 \"fo\"", b"fo", "Zm8"),
        ("RFC 4648 \"foo\"", b"foo", "Zm9v"),
        ("RFC 4648 \"foob\"", b"foob", "Zm9vYg"),
        ("RFC 4648 \"fooba\"", b"fooba", "Zm9vYmE"),
        ("RFC 4648 \"foobar\"", b"foobar", "Zm9vYmFy"),
        ("bytes whose encoding uses + and /", &[0xfb, 0xff], "-_8"),
        ("bytes whose encoding uses /", &[0x00, 0xff, 0xff], "AP__"),
    ];

    for (case, bytes, expected) in cases {
        let actual = state::base64_url(bytes);
        assert_eq!(
            &actual, expected,
            "case {case:?}: expected {expected:?}, got {actual:?}"
        );
        assert!(
            !actual.contains('+') && !actual.contains('/') && !actual.contains('='),
            "case {case:?}: a base64url segment carries none of + / =, got {actual:?}"
        );
        let decoded = decode_base64url(&actual);
        assert_eq!(
            decoded, *bytes,
            "case {case:?}: the segment must decode to the bytes it encoded, got {decoded:?}"
        );
    }
}

/// Every token a sign-in returns is an unsecured JWS whose claims come from
/// stored state, and every one of them resolves back to the stored user.
#[tokio::test]
async fn an_issued_token_is_an_unsecured_jws_over_stored_claims() {
    let (router, engine) = router_and_engine();
    let pool = ok(&router, "CreateUserPool", &json!({ "PoolName": "token-pool" })).await;
    let user_pool_id = pool["UserPool"]["Id"]
        .as_str()
        .expect("a minted pool id")
        .to_string();
    let client = ok(
        &router,
        "CreateUserPoolClient",
        &json!({
            "UserPoolId": user_pool_id,
            "ClientName": "token-client",
            "ExplicitAuthFlows": ["ALLOW_ADMIN_USER_PASSWORD_AUTH"],
        }),
    )
    .await;
    let client_id = client["UserPoolClient"]["ClientId"]
        .as_str()
        .expect("a minted client id")
        .to_string();
    let created = ok(
        &router,
        "AdminCreateUser",
        &json!({ "UserPoolId": user_pool_id, "Username": "alice" }),
    )
    .await;
    let sub = created["User"]["Attributes"]
        .as_array()
        .expect("the stored attributes")
        .iter()
        .find(|attribute| attribute["Name"] == "sub")
        .and_then(|attribute| attribute["Value"].as_str())
        .expect("a generated sub")
        .to_string();
    ok(
        &router,
        "AdminSetUserPassword",
        &json!({
            "UserPoolId": user_pool_id,
            "Username": "alice",
            "Password": GOOD_PASSWORD,
            "Permanent": true,
        }),
    )
    .await;
    let result = ok(
        &router,
        "AdminInitiateAuth",
        &json!({
            "UserPoolId": user_pool_id,
            "ClientId": client_id,
            "AuthFlow": "ADMIN_USER_PASSWORD_AUTH",
            "AuthParameters": { "USERNAME": "alice", "PASSWORD": GOOD_PASSWORD },
        }),
    )
    .await;

    // Each row: the member of AuthenticationResult and the `token_use` claim
    // the stored token carries.
    let cases = [
        ("AccessToken", "access"),
        ("IdToken", "id"),
        ("RefreshToken", "refresh"),
    ];
    let mut jtis: Vec<String> = Vec::new();
    for (member, token_use) in cases {
        let token = result["AuthenticationResult"][member]
            .as_str()
            .unwrap_or_else(|| panic!("case {member:?}: the sign-in must return it, got {result}"));
        let segments: Vec<&str> = token.split('.').collect();
        assert_eq!(
            segments.len(),
            3,
            "case {member:?}: a JWS compact serialization has three segments, got {token}"
        );
        // RFC 7515 §6: an unsecured JWS has an EMPTY signature segment, and its
        // header says `alg: none` rather than naming an algorithm nothing ran.
        assert!(
            segments[2].is_empty(),
            "case {member:?}: an unsecured JWS carries no signature, got {:?}",
            segments[2]
        );
        let header: Value = serde_json::from_slice(&decode_base64url(segments[0]))
            .unwrap_or_else(|error| panic!("case {member:?}: the header must be JSON ({error})"));
        assert_eq!(
            header["alg"], "none",
            "case {member:?}: expected alg none, got {header}"
        );
        assert_eq!(
            header["typ"], "JWT",
            "case {member:?}: expected typ JWT, got {header}"
        );

        let claims: Value = serde_json::from_slice(&decode_base64url(segments[1]))
            .unwrap_or_else(|error| panic!("case {member:?}: the claims must be JSON ({error})"));
        assert_eq!(
            claims["sub"], sub,
            "case {member:?}: the sub claim must be the stored user's, got {claims}"
        );
        assert_eq!(
            claims["token_use"], token_use,
            "case {member:?}: expected token_use {token_use}, got {claims}"
        );
        let issued_at = claims["iat"].as_i64().unwrap_or_default();
        let expires_at = claims["exp"].as_i64().unwrap_or_default();
        assert!(
            expires_at > issued_at,
            "case {member:?}: a token must expire after it was issued, got {claims}"
        );
        let jti = claims["jti"]
            .as_str()
            .unwrap_or_else(|| panic!("case {member:?}: a token must carry a jti, got {claims}"))
            .to_string();
        assert!(
            !jtis.contains(&jti),
            "case {member:?}: every token gets its own jti, {jti} repeated"
        );
        jtis.push(jti);

        // The emulator records what it issued, so a later operation can resolve
        // the token back to the stored user.
        let service_state = state::service_state(&*engine).expect("this service's state");
        let guard = service_state.lock();
        let record = guard
            .token(token)
            .unwrap_or_else(|| panic!("case {member:?}: the token must be recorded, got None"));
        assert_eq!(
            record.username, "alice",
            "case {member:?}: the record must name the stored user, got {record:?}"
        );
        assert_eq!(
            record.client_id, client_id,
            "case {member:?}: the record must name the app client it was issued for"
        );
    }
}
