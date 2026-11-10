// SPDX-License-Identifier: CC0-1.0

//! The sentence a REVOKED session is refused with, on the token-authorization path
//! every access-token-bearing operation shares.
//!
//! RULE: an operation that identifies its caller by an access token alone refuses a
//! token whose session has been revoked with `NotAuthorizedException` and ONE fixed
//! sentence — "Amazon Cognito returns an `Access Token has been revoked` error when
//! your app attempts to authorize a user pools API request with a revoked access
//! token" (`GlobalSignOut` in this service's model) — whatever revoked that session
//! and whichever token of it is presented; a value that resolves to no access token
//! of this emulator at all is a DIFFERENT sentence, so a caller can tell a revoked
//! session apart from a corrupt or foreign credential. The companion refusal on the
//! refresh path carries the fixed `Refresh Token has been revoked`.
//!
//! No part of either sentence is taken from the request or from stored state: it
//! names neither the token, nor its `jti`/`origin_jti`, nor the app client, nor the
//! user.
//!
//! The rows span the operation asked (every self-service operation an access token
//! authorizes), which token of the revoked sign-in lineage is presented (the one the
//! sign-in minted and the one a refresh in the same `origin_jti` family minted), and
//! what revoked the session (the refresh token's revocation, a self-service
//! sign-out, an administrative sign-out, and deactivating the user). Each assertion
//! names its case and the value it got.

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice as cognito_idp;

/// This service's SigV4 signing name, the 4th field of a credential scope.
const SIGNING_NAME: &str = "cognito-idp";

/// The local name of this service's Smithy shape, which is the `X-Amz-Target` prefix a
/// client writes.
const TARGET_PREFIX: &str = "AWSCognitoIdentityProviderService";

/// This service's real regional endpoint host.
const ENDPOINT_HOST: &str = "cognito-idp.us-east-1.amazonaws.com";

/// The region the suite's calls are signed for.
const DEFAULT_REGION: &str = "us-east-1";

/// A password satisfying a default pool's password policy.
const GOOD_PASSWORD: &str = "Passw0rd!2024";

/// Another one, for the row that asks an operation which proposes a new password.
const OTHER_PASSWORD: &str = "Passw0rd!2025";

/// The fixed sentence a revoked access token is refused with.
const ACCESS_TOKEN_REVOKED: &str = "Access Token has been revoked";

/// The fixed sentence a revoked refresh token is refused with, on both operations a
/// refresh token authorizes.
const REFRESH_TOKEN_REVOKED: &str = "Refresh Token has been revoked";

/// The sentence a value that resolves to no access token of this emulator gets, which
/// the revoked case must not be collapsed into.
const INVALID_ACCESS_TOKEN: &str = "Invalid Access Token";

/// The error class every one of these refusals belongs to.
const NOT_AUTHORIZED: &str = "NotAuthorizedException";

/// A `TokenModelType`-shaped value (pattern `^[A-Za-z0-9-_=.]+$`) this emulator issued
/// to nobody.
const UNISSUED_TOKEN: &str = "eyJub3QiOiJhLXRva2VuIn0=.not-issued-here.AAAA";

/// A `DeviceKeyType`-shaped value (pattern `^[\w-]+_[0-9a-f-]+$`) for the rows whose
/// operation names a device: the token is judged before the device is resolved, so no
/// row depends on a device existing.
const DEVICE_KEY: &str = "us-east-1_00000000-0000-0000-0000-000000000000";

/// One emulator with just this service mounted over a fresh engine.
fn router() -> MultiServiceRouter {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(cognito_idp::EmulatedService::new(engine));
    router
}

/// Dispatch `operation` with `body` and return the status and JSON response body.
async fn dispatch(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", ENDPOINT_HOST)
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{DEFAULT_REGION}/{SIGNING_NAME}/aws4_request, \
                 SignedHeaders=host;x-amz-date, Signature=0000"
            ),
        );
    let response = router
        .dispatch(
            request
                .body(Bytes::from(body.to_string()))
                .expect("the request is well formed"),
        )
        .await;
    let status = response.status().as_u16();
    let payload = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or_else(|error| {
            panic!(
                "{operation} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(response.body())
            )
        })
    };
    (status, payload)
}

/// Call `operation`, requiring success, and return its response body.
async fn ok(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, payload) = dispatch(router, operation, body).await;
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

/// The `message` of an error body.
fn error_message(payload: &Value) -> &str {
    payload
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
}

/// Assert that `operation` refuses `body` with the 400 `NotAuthorizedException` that
/// carries `expected_message`, and that the sentence is the FIXED one: it names no
/// value of the request or of stored state.
async fn refused_with(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    body: &Value,
    expected_message: &str,
    named_values: &[&str],
) {
    let (status, payload) = dispatch(router, operation, body).await;
    assert_eq!(
        (status, error_code(&payload), error_message(&payload)),
        (400, NOT_AUTHORIZED, expected_message),
        "case {case:?}: {operation} must answer (400, {NOT_AUTHORIZED:?}, \
         {expected_message:?}), got ({status}, {:?}, {:?})",
        error_code(&payload),
        error_message(&payload)
    );
    let message = error_message(&payload);
    for named in named_values {
        assert!(
            !message.contains(named),
            "case {case:?}: {operation}'s sentence is fixed text and must not carry \
             {named:?}, got {message:?}"
        );
    }
}

/// A created pool and one of its app clients.
struct Fixture {
    user_pool_id: String,
    client_id: String,
}

/// Create a pool, one app client that serves the password and refresh flows, and the
/// confirmed user `username`.
async fn fixture(router: &MultiServiceRouter, pool_name: &str, username: &str) -> Fixture {
    let created = ok(router, "CreateUserPool", &json!({ "PoolName": pool_name })).await;
    let user_pool_id = created["UserPool"]["Id"]
        .as_str()
        .expect("a minted pool id")
        .to_string();
    let client = ok(
        router,
        "CreateUserPoolClient",
        &json!({
            "UserPoolId": user_pool_id,
            "ClientName": "revoked-session-client",
            "ExplicitAuthFlows": [
                "ALLOW_USER_PASSWORD_AUTH",
                "ALLOW_REFRESH_TOKEN_AUTH",
                "ALLOW_ADMIN_USER_PASSWORD_AUTH",
            ],
            "GenerateSecret": false,
            "EnableTokenRevocation": true,
        }),
    )
    .await;
    let client_id = client["UserPoolClient"]["ClientId"]
        .as_str()
        .expect("a minted client id")
        .to_string();
    ok(
        router,
        "AdminCreateUser",
        &json!({ "UserPoolId": user_pool_id, "Username": username }),
    )
    .await;
    ok(
        router,
        "AdminSetUserPassword",
        &json!({
            "UserPoolId": user_pool_id,
            "Username": username,
            "Password": GOOD_PASSWORD,
            "Permanent": true,
        }),
    )
    .await;
    Fixture {
        user_pool_id,
        client_id,
    }
}

/// Sign `username` in with its password and return the `AuthenticationResult`, which
/// opens a sign-in lineage of its own.
async fn sign_in(router: &MultiServiceRouter, fixture: &Fixture, username: &str) -> Value {
    let payload = ok(
        router,
        "InitiateAuth",
        &json!({
            "ClientId": fixture.client_id,
            "AuthFlow": "USER_PASSWORD_AUTH",
            "AuthParameters": { "USERNAME": username, "PASSWORD": GOOD_PASSWORD },
        }),
    )
    .await;
    payload["AuthenticationResult"].clone()
}

/// Mint fresh tokens from `refresh_token` through the refresh flow of `InitiateAuth`,
/// which continues that refresh token's sign-in lineage.
async fn refresh_sign_in(
    router: &MultiServiceRouter,
    fixture: &Fixture,
    refresh_token: &str,
) -> Value {
    let payload = ok(
        router,
        "InitiateAuth",
        &json!({
            "ClientId": fixture.client_id,
            "AuthFlow": "REFRESH_TOKEN_AUTH",
            "AuthParameters": { "REFRESH_TOKEN": refresh_token },
        }),
    )
    .await;
    payload["AuthenticationResult"].clone()
}

/// The named token of an `AuthenticationResult`, which must be there.
fn token(case: &str, result: &Value, member: &str) -> String {
    result[member]
        .as_str()
        .filter(|token| !token.is_empty())
        .unwrap_or_else(|| panic!("case {case:?}: expected a {member}, got {result}"))
        .to_string()
}

/// The decoded claims of a compact JWS: the second segment is base64url (RFC 7515 §2,
/// whose padding is stripped), read with this crate's own codec — the one the token
/// serializer encodes the segment with (`state::base64_url_decode`).
fn claims(case: &str, jws: &str) -> Value {
    let segment = jws
        .split('.')
        .nth(1)
        .unwrap_or_else(|| panic!("case {case:?}: {jws:?} is not a compact JWS"));
    let bytes = cognito_idp::state::base64_url_decode(segment)
        .unwrap_or_else(|| panic!("case {case:?}: {segment:?} must be base64url"));
    serde_json::from_slice(&bytes)
        .unwrap_or_else(|error| panic!("case {case:?}: the claims must be JSON ({error})"))
}

/// The claim `name` of `jws`, which an access token this emulator minted carries.
fn claim(case: &str, jws: &str, name: &str) -> String {
    claims(case, jws)[name]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: an access token carries a {name} claim, got {}",
                claims(case, jws)
            )
        })
        .to_string()
}

/// Every self-service operation whose caller is named by an `AccessToken` alone, with
/// the other members each one's input requires. The token check runs before any
/// operation-specific state is resolved, so none of these bodies depends on a device,
/// a passkey or a verification code existing.
fn access_token_operations(access_token: &str) -> Vec<(&'static str, Value)> {
    vec![
        ("GetUser", json!({ "AccessToken": access_token })),
        ("GetUserAuthFactors", json!({ "AccessToken": access_token })),
        ("ListDevices", json!({ "AccessToken": access_token })),
        (
            "ListWebAuthnCredentials",
            json!({ "AccessToken": access_token }),
        ),
        (
            "StartWebAuthnRegistration",
            json!({ "AccessToken": access_token }),
        ),
        (
            "CompleteWebAuthnRegistration",
            json!({ "AccessToken": access_token, "Credential": { "id": "a-credential" } }),
        ),
        (
            "DeleteWebAuthnCredential",
            json!({ "AccessToken": access_token, "CredentialId": "a-credential" }),
        ),
        (
            "ChangePassword",
            json!({
                "AccessToken": access_token,
                "PreviousPassword": GOOD_PASSWORD,
                "ProposedPassword": OTHER_PASSWORD,
            }),
        ),
        (
            "UpdateUserAttributes",
            json!({
                "AccessToken": access_token,
                "UserAttributes": [{ "Name": "email", "Value": "revoked@example.com" }],
            }),
        ),
        (
            "DeleteUserAttributes",
            json!({ "AccessToken": access_token, "UserAttributeNames": ["email"] }),
        ),
        (
            "SetUserSettings",
            json!({
                "AccessToken": access_token,
                "MFAOptions": [
                    { "DeliveryMedium": "SMS", "AttributeName": "phone_number" },
                ],
            }),
        ),
        (
            "SetUserMFAPreference",
            json!({
                "AccessToken": access_token,
                "SMSMfaSettings": { "Enabled": false, "PreferredMfa": false },
            }),
        ),
        (
            "GetUserAttributeVerificationCode",
            json!({ "AccessToken": access_token, "AttributeName": "email" }),
        ),
        (
            "VerifyUserAttribute",
            json!({
                "AccessToken": access_token,
                "AttributeName": "email",
                "Code": "123456",
            }),
        ),
        (
            "GetDevice",
            json!({ "AccessToken": access_token, "DeviceKey": DEVICE_KEY }),
        ),
        (
            "ForgetDevice",
            json!({ "AccessToken": access_token, "DeviceKey": DEVICE_KEY }),
        ),
        (
            "ConfirmDevice",
            json!({ "AccessToken": access_token, "DeviceKey": DEVICE_KEY }),
        ),
        (
            "UpdateDeviceStatus",
            json!({
                "AccessToken": access_token,
                "DeviceKey": DEVICE_KEY,
                "DeviceRememberedStatus": "remembered",
            }),
        ),
        ("GlobalSignOut", json!({ "AccessToken": access_token })),
        ("DeleteUser", json!({ "AccessToken": access_token })),
    ]
}

/// Assert that `access_token` still names `username`, through the reader that resolves
/// its caller from the token alone.
async fn still_signed_in(
    router: &MultiServiceRouter,
    case: &str,
    access_token: &str,
    username: &str,
) {
    let payload = ok(router, "GetUser", &json!({ "AccessToken": access_token })).await;
    assert_eq!(
        payload["Username"].as_str(),
        Some(username),
        "case {case:?}: the untouched sign-in's token must still resolve {username:?}, \
         got {payload}"
    );
}

/// RULE, over the whole set of operations an access token authorizes: after the refresh
/// token of a sign-in is revoked, EVERY one of them refuses either access token of that
/// `origin_jti` family with the fixed revoked sentence, and the same operations refuse a
/// value this emulator never issued with the different invalid-token sentence.
#[tokio::test]
async fn every_access_token_operation_reports_a_revoked_session_as_revoked() {
    let case = "a revoked refresh token's origin_jti family";
    let router = router();
    let fixture = fixture(&router, "revoked-session-pool", "alice").await;

    // Two independent sign-ins: one whose lineage is revoked, and a twin that is not
    // touched, so each refusal below is the revocation's doing and not the token
    // check refusing everything.
    let revoked_sign_in = sign_in(&router, &fixture, "alice").await;
    let twin_sign_in = sign_in(&router, &fixture, "alice").await;
    let revoked_refresh = token(case, &revoked_sign_in, "RefreshToken");
    let twin_refresh = token(case, &twin_sign_in, "RefreshToken");

    // A refresh in the revoked lineage mints a SECOND access token of the same
    // family: it keeps the sign-in's `origin_jti` and carries a `jti` of its own.
    let refreshed = refresh_sign_in(&router, &fixture, &revoked_refresh).await;
    let family = [
        token(case, &revoked_sign_in, "AccessToken"),
        token(case, &refreshed, "AccessToken"),
    ];
    let twin_access = token(case, &twin_sign_in, "AccessToken");
    let origin = claim(case, &family[0], "origin_jti");
    assert_eq!(
        claim(case, &family[1], "origin_jti"),
        origin,
        "case {case:?}: a refresh keeps the sign-in's origin_jti, got {}",
        claims(case, &family[1])
    );
    assert_ne!(
        claim(case, &family[1], "jti"),
        claim(case, &family[0], "jti"),
        "case {case:?}: a refresh mints a new jti, got {}",
        claims(case, &family[1])
    );
    assert_ne!(
        claim(case, &twin_access, "origin_jti"),
        origin,
        "case {case:?}: the twin sign-in opens a different origin_jti family, got {}",
        claims(case, &twin_access)
    );

    // Both tokens of the family resolve the signed-in user while the lineage is live.
    for access_token in &family {
        still_signed_in(&router, case, access_token, "alice").await;
    }

    ok(
        &router,
        "RevokeToken",
        &json!({ "ClientId": fixture.client_id, "Token": revoked_refresh }),
    )
    .await;

    // The values the fixed sentences must not name: nothing of the request, and
    // nothing of stored state.
    let named = [
        family[0].as_str(),
        family[1].as_str(),
        origin.as_str(),
        fixture.client_id.as_str(),
        fixture.user_pool_id.as_str(),
        "alice",
    ];

    for (index, access_token) in family.iter().enumerate() {
        let token_case = format!("{case}, token {index} of the family");
        for (operation, body) in access_token_operations(access_token) {
            refused_with(
                &router,
                &token_case,
                operation,
                &body,
                ACCESS_TOKEN_REVOKED,
                &named,
            )
            .await;
        }
    }

    // The same operations tell a value that resolves to no access token at all apart
    // from a revoked one, so the revoked case is not collapsed into the invalid case.
    for (operation, body) in access_token_operations(UNISSUED_TOKEN) {
        refused_with(
            &router,
            "a value this emulator never issued",
            operation,
            &body,
            INVALID_ACCESS_TOKEN,
            &named,
        )
        .await;
    }

    // Nothing above was served, so the twin sign-in is exactly where it was: its
    // access token still names the user and its refresh token still mints tokens.
    still_signed_in(&router, case, &twin_access, "alice").await;
    let twin_refreshed = refresh_sign_in(&router, &fixture, &twin_refresh).await;
    still_signed_in(
        &router,
        case,
        &token(case, &twin_refreshed, "AccessToken"),
        "alice",
    )
    .await;

    // The companion refusal on the refresh path: the revoked lineage's refresh token
    // is refused as revoked by both operations a refresh token authorizes.
    for (operation, body) in [
        (
            "InitiateAuth",
            json!({
                "ClientId": fixture.client_id,
                "AuthFlow": "REFRESH_TOKEN_AUTH",
                "AuthParameters": { "REFRESH_TOKEN": revoked_refresh },
            }),
        ),
        (
            "GetTokensFromRefreshToken",
            json!({ "ClientId": fixture.client_id, "RefreshToken": revoked_refresh }),
        ),
    ] {
        refused_with(
            &router,
            case,
            operation,
            &body,
            REFRESH_TOKEN_REVOKED,
            &named,
        )
        .await;
    }
}

/// What revoked the session one row of the table asks about.
#[derive(Clone, Copy, Debug)]
enum Revoker {
    /// `RevokeToken` naming the refresh token of the sign-in lineage.
    RevokeToken,
    /// The self-service sign-out, authorized by the access token itself.
    GlobalSignOut,
    /// The administrative sign-out, which names the user.
    AdminUserGlobalSignOut,
    /// Deactivating the user, which "revokes all access tokens for the user"
    /// (`AdminDisableUser` in this service's model).
    AdminDisableUser,
}

/// One row of the revoker table.
#[derive(Clone, Copy, Debug)]
struct RevokerRow {
    case: &'static str,
    revoker: Revoker,
    /// Whether the lineage is refreshed first, so the token presented afterwards is the
    /// one a refresh minted rather than the one the sign-in did.
    refresh_first: bool,
}

/// RULE, over what revoked the session: the sentence is the same fixed text whatever
/// invalidated the token — a revocation of its refresh token, either sign-out, or
/// deactivating its user — and whichever token of the lineage is presented.
#[tokio::test]
async fn the_revoked_sentence_is_the_same_whatever_revoked_the_session() {
    let rows = [
        RevokerRow {
            case: "the refresh token's revocation, the sign-in's own access token",
            revoker: Revoker::RevokeToken,
            refresh_first: false,
        },
        RevokerRow {
            case: "the refresh token's revocation, the access token a refresh minted",
            revoker: Revoker::RevokeToken,
            refresh_first: true,
        },
        RevokerRow {
            case: "the self-service sign-out",
            revoker: Revoker::GlobalSignOut,
            refresh_first: false,
        },
        RevokerRow {
            case: "the self-service sign-out, the access token a refresh minted",
            revoker: Revoker::GlobalSignOut,
            refresh_first: true,
        },
        RevokerRow {
            case: "the administrative sign-out",
            revoker: Revoker::AdminUserGlobalSignOut,
            refresh_first: false,
        },
        RevokerRow {
            case: "deactivating the user",
            revoker: Revoker::AdminDisableUser,
            refresh_first: false,
        },
    ];

    // A spread of the operations the token check answers for: three readers and two
    // writers, so the sentence is the shared check's and not one handler's.
    let asked = [
        "GetUser",
        "GetUserAuthFactors",
        "ListDevices",
        "UpdateUserAttributes",
        "DeleteUser",
    ];

    for row in rows {
        let case = row.case;
        let router = router();
        let fixture = fixture(&router, "revoker-pool", "alice").await;
        let signed_in = sign_in(&router, &fixture, "alice").await;
        let refresh_token = token(case, &signed_in, "RefreshToken");
        let presented = if row.refresh_first {
            let refreshed = refresh_sign_in(&router, &fixture, &refresh_token).await;
            token(case, &refreshed, "AccessToken")
        } else {
            token(case, &signed_in, "AccessToken")
        };
        still_signed_in(&router, case, &presented, "alice").await;

        match row.revoker {
            Revoker::RevokeToken => {
                ok(
                    &router,
                    "RevokeToken",
                    &json!({ "ClientId": fixture.client_id, "Token": refresh_token }),
                )
                .await;
            }
            Revoker::GlobalSignOut => {
                ok(
                    &router,
                    "GlobalSignOut",
                    &json!({ "AccessToken": presented }),
                )
                .await;
            }
            Revoker::AdminUserGlobalSignOut => {
                ok(
                    &router,
                    "AdminUserGlobalSignOut",
                    &json!({ "UserPoolId": fixture.user_pool_id, "Username": "alice" }),
                )
                .await;
            }
            Revoker::AdminDisableUser => {
                ok(
                    &router,
                    "AdminDisableUser",
                    &json!({ "UserPoolId": fixture.user_pool_id, "Username": "alice" }),
                )
                .await;
            }
        }

        let named = [
            presented.as_str(),
            fixture.client_id.as_str(),
            fixture.user_pool_id.as_str(),
            "alice",
        ];
        let mut rows_asked = 0;
        for (operation, body) in access_token_operations(&presented)
            .into_iter()
            .filter(|(operation, _)| asked.contains(operation))
        {
            rows_asked += 1;
            refused_with(
                &router,
                case,
                operation,
                &body,
                ACCESS_TOKEN_REVOKED,
                &named,
            )
            .await;
        }
        assert_eq!(
            rows_asked,
            asked.len(),
            "case {case:?}: every operation this row asks must have a body, asked {}",
            asked.len()
        );

        // The refresh token of the same lineage is refused as revoked too, with its
        // own fixed sentence.
        refused_with(
            &router,
            case,
            "GetTokensFromRefreshToken",
            &json!({ "ClientId": fixture.client_id, "RefreshToken": refresh_token }),
            REFRESH_TOKEN_REVOKED,
            &named,
        )
        .await;
    }
}
