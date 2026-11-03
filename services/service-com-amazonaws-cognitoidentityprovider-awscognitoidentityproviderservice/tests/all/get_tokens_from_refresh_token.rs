// SPDX-License-Identifier: CC0-1.0

//! `GetTokensFromRefreshToken` over the wire: what exchanging a refresh token this
//! emulator issued answers, and what it refuses.
//!
//! Four rules are under test.
//!
//! * ROTATION. "This operation issues a new refresh token and invalidates the original
//!   refresh token after an optional grace period when refresh token rotation is
//!   enabled. If refresh token rotation is disabled, issues new ID and access tokens
//!   only" (this operation's own doc), and `RetryGracePeriodSeconds` of `0` means "the
//!   grace period is disabled and a successful request immediately invalidates the
//!   submitted refresh token" (`RefreshTokenRotationType`). The table below spans that
//!   member's published range — `0` and `60` — beside a client that configures no
//!   rotation at all and one that switches the feature off.
//! * DEVICES. "You must provide the confirmed device key in this request if device
//!   remembering is enabled in your user pool" (`GetTokensFromRefreshToken.DeviceKey`),
//!   so on such a pool the refresh is served only for a device key `ConfirmDevice` has
//!   confirmed for the token's user.
//! * TOKEN VALIDITY. "A valid refresh token that can authorize the request for new
//!   tokens" (`RefreshToken`): a live refresh token of the app client the request names,
//!   issued to a user that can still sign in.
//! * CLIENT SECRET. "The client secret of the requested app client, if the client has a
//!   secret" (`ClientSecret`).
//!
//! Each table row names the case it asserts.

use bytes::Bytes;
use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice as cognito_idp;
use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice::state;

/// This service's SigV4 signing name, the 4th field of a credential scope.
const SIGNING_NAME: &str = "cognito-idp";

/// The local name of this service's Smithy shape, which is the `X-Amz-Target` prefix a
/// client writes.
const TARGET_PREFIX: &str = "AWSCognitoIdentityProviderService";

/// This service's real regional endpoint host.
const ENDPOINT_HOST: &str = "cognito-idp.us-east-1.amazonaws.com";

/// The region the suite's calls are signed for, which is the region the pools they
/// create live in.
const DEFAULT_REGION: &str = "us-east-1";

/// A password satisfying a default pool's password policy.
const GOOD_PASSWORD: &str = "Passw0rd!2024";

/// The operation under test.
const REFRESH: &str = "GetTokensFromRefreshToken";

/// A `DeviceKeyType`-shaped key (pattern `^[\w-]+_[0-9a-f-]+$`, length 1..55) that no
/// sign-in in this suite minted.
const UNKNOWN_DEVICE_KEY: &str = "us-east-1_11111111-2222-3333-4444-555555555555";

/// A `ClientIdType`-shaped id (pattern `^[\w+]+$`, length 1..128) no create minted.
const UNKNOWN_CLIENT_ID: &str = "nosuchappclient0000";

/// A `ClientSecretType`-shaped secret (pattern `^[\w+]+$`, length 24..64) no app client
/// holds.
const WRONG_CLIENT_SECRET: &str = "wrongsecret0000000000000000";

/// One emulator with just this service mounted, beside the engine holding its state.
fn router_and_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(cognito_idp::EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// One emulator with just this service mounted over a fresh engine.
fn router() -> MultiServiceRouter {
    router_and_engine().0
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

/// The `message` member of an error body.
fn error_message(payload: &Value) -> &str {
    payload
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
}

/// Call `operation` expecting a refusal, and assert its status and wire error code.
async fn refused(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    body: &Value,
    expected_status: u16,
    expected_code: &str,
) {
    let (status, payload) = dispatch(router, operation, body).await;
    assert_eq!(
        (status, error_code(&payload)),
        (expected_status, expected_code),
        "case {case:?}: expected ({expected_status}, {expected_code:?}), \
         got ({status}, {:?}) with {payload}",
        error_code(&payload)
    );
}

/// A created pool and one of its app clients.
struct Fixture {
    user_pool_id: String,
    client_id: String,
    /// The secret the app client was created with, when it asked for one.
    client_secret: Option<String>,
}

/// What an app client of this suite is created with.
#[derive(Default)]
struct ClientSetup {
    /// The pool's `DeviceConfiguration`, which is what activates device remembering.
    device_configuration: Option<Value>,
    /// The client's `RefreshTokenRotation`.
    rotation: Option<Value>,
    /// Whether the client holds a secret.
    generate_secret: bool,
}

/// Create a pool and one app client as `setup` asks, and return both ids.
async fn pool_with_client(
    router: &MultiServiceRouter,
    pool_name: &str,
    setup: &ClientSetup,
) -> Fixture {
    let mut pool_body = Map::new();
    pool_body.insert("PoolName".to_string(), json!(pool_name));
    if let Some(configuration) = setup.device_configuration.clone() {
        pool_body.insert("DeviceConfiguration".to_string(), configuration);
    }
    let created = ok(router, "CreateUserPool", &Value::Object(pool_body)).await;
    let user_pool_id = created["UserPool"]["Id"]
        .as_str()
        .expect("a minted pool id")
        .to_string();

    let mut client_body = Map::new();
    client_body.insert("UserPoolId".to_string(), json!(user_pool_id));
    client_body.insert("ClientName".to_string(), json!("refresh-client"));
    client_body.insert(
        "ExplicitAuthFlows".to_string(),
        json!(["ALLOW_USER_PASSWORD_AUTH", "ALLOW_REFRESH_TOKEN_AUTH"]),
    );
    if let Some(rotation) = setup.rotation.clone() {
        client_body.insert("RefreshTokenRotation".to_string(), rotation);
    }
    if setup.generate_secret {
        client_body.insert("GenerateSecret".to_string(), json!(true));
    }
    let client = ok(router, "CreateUserPoolClient", &Value::Object(client_body)).await;
    Fixture {
        user_pool_id,
        client_id: client["UserPoolClient"]["ClientId"]
            .as_str()
            .expect("a minted client id")
            .to_string(),
        client_secret: client["UserPoolClient"]["ClientSecret"]
            .as_str()
            .map(str::to_string),
    }
}

/// Create `username` in `user_pool_id` with a PERMANENT password, so it is `CONFIRMED`
/// and can sign in.
async fn create_confirmed_user(router: &MultiServiceRouter, user_pool_id: &str, username: &str) {
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
}

/// Sign `username` in with its password and return the `AuthenticationResult`.
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

/// The named token of an `AuthenticationResult`, which must be there.
fn token(case: &str, result: &Value, member: &str) -> String {
    result[member]
        .as_str()
        .filter(|token| !token.is_empty())
        .unwrap_or_else(|| panic!("case {case:?}: expected a {member}, got {result}"))
        .to_string()
}

/// A `GetTokensFromRefreshToken` body for `fixture`, carrying the app client's secret
/// when it holds one and the `DeviceKey` when one is given.
fn refresh_body(fixture: &Fixture, refresh_token: &str, device_key: Option<&str>) -> Value {
    let mut body = Map::new();
    body.insert("ClientId".to_string(), json!(fixture.client_id));
    body.insert("RefreshToken".to_string(), json!(refresh_token));
    if let Some(secret) = fixture.client_secret.as_deref() {
        body.insert("ClientSecret".to_string(), json!(secret));
    }
    if let Some(device_key) = device_key {
        body.insert("DeviceKey".to_string(), json!(device_key));
    }
    Value::Object(body)
}

/// One row of the rotation table: what the app client configured for refresh token
/// rotation, and what a refresh on it owes.
struct RotationRow {
    case: &'static str,
    rotation: Option<Value>,
    /// Whether a refresh issues a new refresh token beside the new id and access ones.
    rotates: bool,
    /// Whether the SUBMITTED refresh token is already invalidated for a retry made
    /// right after a successful refresh.
    invalidated_at_once: bool,
}

/// A refresh renders fresh tokens for the user the submitted refresh token was issued
/// to, mints a new refresh token exactly when the app client rotates them, and
/// invalidates the submitted one when its retry grace period has run out.
#[tokio::test]
async fn rotation_decides_what_a_refresh_issues_and_invalidates() {
    let rows = [
        RotationRow {
            case: "no RefreshTokenRotation configured at all",
            rotation: None,
            rotates: false,
            invalidated_at_once: false,
        },
        RotationRow {
            case: "Feature DISABLED",
            rotation: Some(json!({ "Feature": "DISABLED" })),
            rotates: false,
            invalidated_at_once: false,
        },
        RotationRow {
            case: "Feature DISABLED with a grace period",
            rotation: Some(json!({ "Feature": "DISABLED", "RetryGracePeriodSeconds": 30 })),
            rotates: false,
            invalidated_at_once: false,
        },
        RotationRow {
            case: "Feature ENABLED with no RetryGracePeriodSeconds",
            rotation: Some(json!({ "Feature": "ENABLED" })),
            rotates: true,
            invalidated_at_once: true,
        },
        RotationRow {
            // The published minimum of `RetryGracePeriodSeconds` (range 0..60), which
            // the member documents as disabling the grace period.
            case: "Feature ENABLED with RetryGracePeriodSeconds 0",
            rotation: Some(json!({ "Feature": "ENABLED", "RetryGracePeriodSeconds": 0 })),
            rotates: true,
            invalidated_at_once: true,
        },
        RotationRow {
            // The published maximum of `RetryGracePeriodSeconds` (range 0..60).
            case: "Feature ENABLED with RetryGracePeriodSeconds 60",
            rotation: Some(json!({ "Feature": "ENABLED", "RetryGracePeriodSeconds": 60 })),
            rotates: true,
            invalidated_at_once: false,
        },
    ];

    for row in rows {
        let case = row.case;
        let router = router();
        let fixture = pool_with_client(
            &router,
            "rotation-pool",
            &ClientSetup {
                rotation: row.rotation.clone(),
                ..ClientSetup::default()
            },
        )
        .await;
        create_confirmed_user(&router, &fixture.user_pool_id, "alice").await;

        let signed_in = sign_in(&router, &fixture, "alice").await;
        let first_access = token(case, &signed_in, "AccessToken");
        let first_id = token(case, &signed_in, "IdToken");
        let first_refresh = token(case, &signed_in, "RefreshToken");

        let refreshed = ok(&router, REFRESH, &refresh_body(&fixture, &first_refresh, None)).await;
        let result = &refreshed["AuthenticationResult"];
        let access = token(case, result, "AccessToken");
        let id_token = token(case, result, "IdToken");
        assert_ne!(
            access, first_access,
            "case {case:?}: the refresh must issue a new access token, got the sign-in's"
        );
        assert_ne!(
            id_token, first_id,
            "case {case:?}: the refresh must issue a new id token, got the sign-in's"
        );
        assert_eq!(
            result["TokenType"].as_str(),
            Some("Bearer"),
            "case {case:?}: expected TokenType Bearer, got {result}"
        );
        assert_eq!(
            result["ExpiresIn"].as_i64(),
            Some(3_600),
            "case {case:?}: expected the app client's default access token lifetime \
             in ExpiresIn, got {result}"
        );
        assert!(
            result.get("NewDeviceMetadata").is_none(),
            "case {case:?}: a pool that remembers no devices owes no NewDeviceMetadata, \
             got {result}"
        );

        // The minted access token resolves to the STORED user, read back through the
        // reader that identifies its caller by access token alone.
        let read = ok(&router, "GetUser", &json!({ "AccessToken": access })).await;
        assert_eq!(
            read["Username"].as_str(),
            Some("alice"),
            "case {case:?}: the new access token must identify the stored user, got {read}"
        );

        let rotated = result["RefreshToken"].as_str().map(str::to_string);
        assert_eq!(
            rotated.is_some(),
            row.rotates,
            "case {case:?}: expected RefreshToken present={}, got {result}",
            row.rotates
        );
        if let Some(rotated) = rotated.as_deref() {
            assert_ne!(
                rotated, first_refresh,
                "case {case:?}: a rotated refresh token must not be the submitted one"
            );
            // The token the rotation handed out is the one in force now.
            let again = ok(&router, REFRESH, &refresh_body(&fixture, rotated, None)).await;
            assert!(
                again["AuthenticationResult"]["AccessToken"]
                    .as_str()
                    .is_some_and(|token| !token.is_empty()),
                "case {case:?}: the rotated refresh token must refresh, got {again}"
            );
        }

        // What the submitted token is worth right after the refresh: nothing once its
        // grace period has run out, and another refresh while it still has one.
        let retry = refresh_body(&fixture, &first_refresh, None);
        if row.invalidated_at_once {
            refused(
                &router,
                case,
                REFRESH,
                &retry,
                400,
                "RefreshTokenReuseException",
            )
            .await;
        } else {
            let retried = ok(&router, REFRESH, &retry).await;
            assert!(
                retried["AuthenticationResult"]["AccessToken"]
                    .as_str()
                    .is_some_and(|token| !token.is_empty()),
                "case {case:?}: the submitted token must still refresh, got {retried}"
            );
        }
    }
}

/// `RetryGracePeriodSeconds` is modelled with a range of 0..60, so no app client can be
/// configured to leave a rotated-out refresh token valid for longer than the published
/// maximum.
#[tokio::test]
async fn a_grace_period_past_the_published_maximum_is_refused() {
    let router = router();
    let pool = ok(&router, "CreateUserPool", &json!({ "PoolName": "range-pool" })).await;
    let user_pool_id = pool["UserPool"]["Id"].as_str().expect("a minted pool id");
    for seconds in [-1, 61] {
        let case = format!("RetryGracePeriodSeconds {seconds}");
        let (status, payload) = dispatch(
            &router,
            "CreateUserPoolClient",
            &json!({
                "UserPoolId": user_pool_id,
                "ClientName": "range-client",
                "RefreshTokenRotation": {
                    "Feature": "ENABLED",
                    "RetryGracePeriodSeconds": seconds,
                },
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a grace period outside the modelled range must be refused, \
             got {status} with {payload}"
        );
    }
}

/// One row of the device table: what the pool configured, which `DeviceKey` the refresh
/// presents, and whether the refresh is served.
struct DeviceRow {
    case: &'static str,
    device_configuration: Option<Value>,
    /// Whether `ConfirmDevice` confirms the device the sign-in minted the key for.
    confirm: bool,
    /// The key the refresh presents, built from (this user's minted key, another user's).
    presented: fn(&str, &str) -> Option<String>,
    served: bool,
}

/// On a pool that remembers devices the refresh is served only for a device key
/// `ConfirmDevice` has confirmed for the token's user; a pool that remembers none has no
/// device to authenticate, so the member is nothing to check against there.
#[tokio::test]
async fn device_remembering_decides_whether_a_refresh_needs_a_device_key() {
    let rows = [
        DeviceRow {
            case: "device remembering on, no DeviceKey presented",
            device_configuration: Some(json!({})),
            confirm: true,
            presented: |_, _| None,
            served: false,
        },
        DeviceRow {
            case: "device remembering on, the user's confirmed device",
            device_configuration: Some(json!({})),
            confirm: true,
            presented: |mine, _| Some(mine.to_string()),
            served: true,
        },
        DeviceRow {
            case: "device remembering on, the user's device before ConfirmDevice confirms it",
            device_configuration: Some(json!({ "ChallengeRequiredOnNewDevice": true })),
            confirm: false,
            presented: |mine, _| Some(mine.to_string()),
            served: false,
        },
        DeviceRow {
            case: "device remembering on, another user's device",
            device_configuration: Some(json!({})),
            confirm: true,
            presented: |_, theirs| Some(theirs.to_string()),
            served: false,
        },
        DeviceRow {
            case: "device remembering on, a device key nothing minted",
            device_configuration: Some(json!({ "DeviceOnlyRememberedOnUserPrompt": true })),
            confirm: true,
            presented: |_, _| Some(UNKNOWN_DEVICE_KEY.to_string()),
            served: false,
        },
        DeviceRow {
            case: "no DeviceConfiguration, no DeviceKey presented",
            device_configuration: None,
            confirm: false,
            presented: |_, _| None,
            served: true,
        },
        DeviceRow {
            case: "no DeviceConfiguration, a device key nothing minted",
            device_configuration: None,
            confirm: false,
            presented: |_, _| Some(UNKNOWN_DEVICE_KEY.to_string()),
            served: true,
        },
    ];

    for row in rows {
        let case = row.case;
        let router = router();
        let fixture = pool_with_client(
            &router,
            "device-pool",
            &ClientSetup {
                device_configuration: row.device_configuration.clone(),
                // A rotating client, so the refresh under test mints the whole triple.
                rotation: Some(json!({ "Feature": "ENABLED", "RetryGracePeriodSeconds": 30 })),
                ..ClientSetup::default()
            },
        )
        .await;
        create_confirmed_user(&router, &fixture.user_pool_id, "alice").await;
        create_confirmed_user(&router, &fixture.user_pool_id, "bob").await;

        let alice = sign_in(&router, &fixture, "alice").await;
        let bob = sign_in(&router, &fixture, "bob").await;
        let refresh_token = token(case, &alice, "RefreshToken");
        let mine = alice["NewDeviceMetadata"]["DeviceKey"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let theirs = bob["NewDeviceMetadata"]["DeviceKey"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            mine.is_empty(),
            row.device_configuration.is_none(),
            "case {case:?}: a pool that remembers devices owes the sign-in a device key, \
             got {alice}"
        );
        if row.confirm && !mine.is_empty() {
            ok(
                &router,
                "ConfirmDevice",
                &json!({
                    "AccessToken": token(case, &alice, "AccessToken"),
                    "DeviceKey": mine,
                    // The device credentials a confirm registers: "a Secure
                    // Remote Password (SRP) value that your application
                    // generates when you register a user's device"
                    // (`smithy_query(query="shape",shape="DeviceSecretVerifierConfigType")`).
                    "DeviceSecretVerifierConfig": {
                        "PasswordVerifier": "ZGV2aWNlLXBhc3N3b3JkLXZlcmlmaWVy",
                        "Salt": "ZGV2aWNlLXNhbHQ",
                    },
                }),
            )
            .await;
        }

        let presented = (row.presented)(&mine, &theirs);
        let body = refresh_body(&fixture, &refresh_token, presented.as_deref());
        if row.served {
            let refreshed = ok(&router, REFRESH, &body).await;
            assert!(
                refreshed["AuthenticationResult"]["AccessToken"]
                    .as_str()
                    .is_some_and(|token| !token.is_empty()),
                "case {case:?}: the refresh must be served, got {refreshed}"
            );
        } else {
            let (status, payload) = dispatch(&router, REFRESH, &body).await;
            // The token itself is one this service issued for this app client: what does
            // not authorize the refresh is the DEVICE, so the refusal is the one a
            // resolved token that does not authorize this request owes.
            assert_eq!(
                (status, error_code(&payload), error_message(&payload)),
                (400, "NotAuthorizedException", "Invalid Refresh Token."),
                "case {case:?}: expected the unauthorized-token refusal, got {status} with \
                 {payload}"
            );
        }
    }
}

/// One row of the token table: the `RefreshToken` and `ClientId` a refresh presents, and
/// the refusal it owes.
struct TokenRow {
    case: &'static str,
    /// Built from the tokens of the sign-ins the setup performed.
    body: fn(&Tokens) -> Value,
    status: u16,
    code: &'static str,
}

/// The tokens a refusal row builds its request from.
struct Tokens {
    client_id: String,
    other_client_id: String,
    access: String,
    id: String,
    refresh: String,
    other_client_refresh: String,
    expired_refresh: String,
}

/// Only a live refresh token of the app client the request names refreshes: an access or
/// id token of the same sign-in does not, a refresh token of another app client does not,
/// a token the clock has expired does not, and no token refreshes against an app client
/// nothing created.
#[tokio::test]
async fn only_a_live_refresh_token_of_the_app_client_refreshes() {
    let (router, engine) = router_and_engine();
    let fixture = pool_with_client(&router, "token-pool", &ClientSetup::default()).await;
    create_confirmed_user(&router, &fixture.user_pool_id, "alice").await;
    let signed_in = sign_in(&router, &fixture, "alice").await;

    // A second app client of the SAME pool, whose refresh token is not this client's.
    let other = ok(
        &router,
        "CreateUserPoolClient",
        &json!({
            "UserPoolId": fixture.user_pool_id,
            "ClientName": "other-client",
            "ExplicitAuthFlows": ["ALLOW_USER_PASSWORD_AUTH", "ALLOW_REFRESH_TOKEN_AUTH"],
        }),
    )
    .await;
    let other_fixture = Fixture {
        user_pool_id: fixture.user_pool_id.clone(),
        client_id: other["UserPoolClient"]["ClientId"]
            .as_str()
            .expect("a minted client id")
            .to_string(),
        client_secret: None,
    };
    let other_signed_in = sign_in(&router, &other_fixture, "alice").await;

    // A refresh token record the clock has already expired, planted because no app
    // client can be configured with a refresh token validity that is already past.
    let expired_refresh = "expired.refresh.token".to_string();
    {
        let now = SystemTime::now();
        let state = state::service_state(&*engine).expect("this service's state");
        let mut guard = state.lock();
        let sub = guard
            .pool(&fixture.user_pool_id)
            .and_then(|pool| pool.user("alice"))
            .expect("the stored user")
            .sub
            .clone();
        guard.issued_tokens.insert(
            expired_refresh.clone(),
            state::TokenRecord {
                user_pool_id: fixture.user_pool_id.clone(),
                client_id: fixture.client_id.clone(),
                username: "alice".to_string(),
                sub,
                kind: state::TokenKind::Refresh,
                // A refresh token carries no `scope` claim.
                scopes: Vec::new(),
                // The sign-in lineage this planted record belongs to.
                origin: "planted-sign-in".to_string(),
                issued_at: now - Duration::from_secs(7_200),
                expires_at: now - Duration::from_secs(60),
                revoked: false,
                rotated_out_at: None,
            },
        );
    }

    let tokens = Tokens {
        client_id: fixture.client_id.clone(),
        other_client_id: other_fixture.client_id.clone(),
        access: token("setup", &signed_in, "AccessToken"),
        id: token("setup", &signed_in, "IdToken"),
        refresh: token("setup", &signed_in, "RefreshToken"),
        other_client_refresh: token("setup", &other_signed_in, "RefreshToken"),
        expired_refresh,
    };

    let rows = [
        TokenRow {
            case: "a refresh token nothing issued",
            body: |t| json!({ "ClientId": t.client_id, "RefreshToken": "not.a.token" }),
            status: 400,
            code: "NotAuthorizedException",
        },
        TokenRow {
            case: "the access token of the same sign-in",
            body: |t| json!({ "ClientId": t.client_id, "RefreshToken": t.access }),
            status: 400,
            code: "NotAuthorizedException",
        },
        TokenRow {
            case: "the id token of the same sign-in",
            body: |t| json!({ "ClientId": t.client_id, "RefreshToken": t.id }),
            status: 400,
            code: "NotAuthorizedException",
        },
        TokenRow {
            case: "a refresh token of another app client of the pool",
            body: |t| json!({ "ClientId": t.client_id, "RefreshToken": t.other_client_refresh }),
            status: 400,
            code: "NotAuthorizedException",
        },
        TokenRow {
            case: "this app client's refresh token against the other app client",
            body: |t| json!({ "ClientId": t.other_client_id, "RefreshToken": t.refresh }),
            status: 400,
            code: "NotAuthorizedException",
        },
        TokenRow {
            case: "a refresh token the clock has expired",
            body: |t| json!({ "ClientId": t.client_id, "RefreshToken": t.expired_refresh }),
            status: 400,
            code: "NotAuthorizedException",
        },
        TokenRow {
            case: "a ClientId nothing created",
            body: |t| json!({ "ClientId": UNKNOWN_CLIENT_ID, "RefreshToken": t.refresh }),
            status: 400,
            code: "ResourceNotFoundException",
        },
    ];

    for row in rows {
        refused(
            &router,
            row.case,
            REFRESH,
            &(row.body)(&tokens),
            row.status,
            row.code,
        )
        .await;
    }

    // The live refresh token of this app client is served, so every refusal above is
    // the row's own and not a refresh that cannot work at all.
    ok(&router, REFRESH, &refresh_body(&fixture, &tokens.refresh, None)).await;
}

/// The state the token's user is in decides the refresh too: a disabled user cannot
/// refresh, nor one in a status that cannot sign in, nor a profile that is gone.
#[tokio::test]
async fn the_users_own_state_decides_the_refresh() {
    let (router, engine) = router_and_engine();
    let fixture = pool_with_client(&router, "user-state-pool", &ClientSetup::default()).await;
    create_confirmed_user(&router, &fixture.user_pool_id, "alice").await;
    let refresh_token = token(
        "setup",
        &sign_in(&router, &fixture, "alice").await,
        "RefreshToken",
    );
    let body = refresh_body(&fixture, &refresh_token, None);

    // The baseline: while the user can sign in, its refresh token refreshes.
    ok(&router, REFRESH, &body).await;

    ok(
        &router,
        "AdminDisableUser",
        &json!({ "UserPoolId": fixture.user_pool_id, "Username": "alice" }),
    )
    .await;
    refused(
        &router,
        "the token's user is disabled",
        REFRESH,
        &body,
        400,
        "NotAuthorizedException",
    )
    .await;

    // A second sign-in of the re-enabled user, since deactivating it revoked the
    // tokens the first one issued.
    ok(
        &router,
        "AdminEnableUser",
        &json!({ "UserPoolId": fixture.user_pool_id, "Username": "alice" }),
    )
    .await;
    let reissued = token(
        "re-enabled",
        &sign_in(&router, &fixture, "alice").await,
        "RefreshToken",
    );
    let reissued_body = refresh_body(&fixture, &reissued, None);
    ok(&router, REFRESH, &reissued_body).await;

    // The stored statuses a sign-in is refused for. Each is planted, since a user in
    // either cannot sign in to be issued a token first, and restored afterwards, so the
    // refusal is shown to come from the status and nothing else.
    for (case, status) in [
        (
            "the token's user is in RESET_REQUIRED",
            state::UserStatus::ResetRequired,
        ),
        (
            "the token's user is UNCONFIRMED",
            state::UserStatus::Unconfirmed,
        ),
    ] {
        for planted in [status, state::UserStatus::Confirmed] {
            {
                let state = state::service_state(&*engine).expect("this service's state");
                let mut guard = state.lock();
                let user = guard
                    .pool_mut(&fixture.user_pool_id)
                    .and_then(|pool| pool.user_mut("alice"))
                    .expect("the stored user");
                user.status = planted;
            }
            if planted == state::UserStatus::Confirmed {
                ok(&router, REFRESH, &reissued_body).await;
            } else {
                refused(
                    &router,
                    case,
                    REFRESH,
                    &reissued_body,
                    400,
                    "NotAuthorizedException",
                )
                .await;
            }
        }
    }

    ok(
        &router,
        "AdminDeleteUser",
        &json!({ "UserPoolId": fixture.user_pool_id, "Username": "alice" }),
    )
    .await;
    refused(
        &router,
        "the token's user is gone",
        REFRESH,
        &reissued_body,
        400,
        "NotAuthorizedException",
    )
    .await;
}

/// One row of the client-secret table: which `ClientSecret` the refresh presents, and
/// whether the refresh is served.
struct SecretRow {
    case: &'static str,
    /// Built from the secret the app client was created with, when it holds one.
    presented: fn(Option<&str>) -> Option<String>,
    served: bool,
}

/// An app client that holds a secret serves a refresh only to a request carrying that
/// secret; one that holds none has nothing for a request to present.
#[tokio::test]
async fn a_client_secret_is_required_exactly_when_the_app_client_holds_one() {
    let rows = [
        SecretRow {
            case: "no ClientSecret presented",
            presented: |_| None,
            served: false,
        },
        SecretRow {
            case: "the app client's own secret",
            presented: |secret| secret.map(str::to_string),
            served: true,
        },
        SecretRow {
            case: "a secret no app client holds",
            presented: |_| Some(WRONG_CLIENT_SECRET.to_string()),
            served: false,
        },
    ];

    for generate_secret in [true, false] {
        for row in &rows {
            let case = format!("{} (GenerateSecret={generate_secret})", row.case);
            let router = router();
            let fixture = pool_with_client(
                &router,
                "secret-pool",
                &ClientSetup {
                    generate_secret,
                    ..ClientSetup::default()
                },
            )
            .await;
            assert_eq!(
                fixture.client_secret.is_some(),
                generate_secret,
                "case {case:?}: the created app client must hold a secret only when it \
                 asked for one"
            );
            create_confirmed_user(&router, &fixture.user_pool_id, "alice").await;
            let refresh_token = token(
                &case,
                &sign_in(&router, &fixture, "alice").await,
                "RefreshToken",
            );

            let mut body = Map::new();
            body.insert("ClientId".to_string(), json!(fixture.client_id));
            body.insert("RefreshToken".to_string(), json!(refresh_token));
            if let Some(secret) = (row.presented)(fixture.client_secret.as_deref()) {
                body.insert("ClientSecret".to_string(), json!(secret));
            }
            let body = Value::Object(body);

            // A client that holds no secret accepts every row: there is nothing to
            // verify a presented value against.
            if row.served || !generate_secret {
                let refreshed = ok(&router, REFRESH, &body).await;
                assert!(
                    refreshed["AuthenticationResult"]["AccessToken"]
                        .as_str()
                        .is_some_and(|token| !token.is_empty()),
                    "case {case:?}: the refresh must be served, got {refreshed}"
                );
            } else {
                refused(&router, &case, REFRESH, &body, 400, "NotAuthorizedException").await;
            }
        }
    }
}
