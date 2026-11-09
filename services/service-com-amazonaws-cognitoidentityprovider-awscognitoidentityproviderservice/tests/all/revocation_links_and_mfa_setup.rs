// SPDX-License-Identifier: CC0-1.0

//! The state a sign-out, a provider link and an MFA preference write, read back
//! through the operations that are supposed to consult it — driven through the
//! router exactly as an SDK drives them (`POST /` with
//! `X-Amz-Target: AWSCognitoIdentityProviderService.<Op>`).
//!
//! Four rules under test:
//!
//! 1. A sign-out invalidates every token this emulator issued to the user it names,
//!    so each later call authorized by one of those tokens is refused AS revoked —
//!    `NotAuthorizedException` with the sentence `GlobalSignOut` documents — and a
//!    refresh of a revoked refresh token is refused the same way. The tokens of
//!    another user, and tokens issued after the sign-out, are untouched.
//! 2. A federated identity links to ONE local profile: an identity the pool already
//!    holds a link for is refused — as the link it already holds when the request names
//!    that same profile, and as an invalid `SourceUser` when it points the identity at
//!    another profile, because the link makes the identity resolve to a local profile
//!    that signs in with a username and a password. Deactivating the identity removes the
//!    link, so it links again.
//! 3. A pool that REQUIRES MFA cannot complete the sign-in of a user that has
//!    activated no factor: the sign-in answers the `MFA_SETUP` challenge, with a
//!    session and the methods the pool offers for setting one up, and mints no
//!    token. A pool that does not require MFA signs the same user in.
//! 4. An MFA preference an administrator sets is the preference the self-service
//!    readers report, exactly as the user's own `SetUserMFAPreference` is.
//!
//! The pools, app clients, identity providers, users and tokens are all real: they
//! come from `CreateUserPool`, `CreateUserPoolClient`, `SetUserPoolMfaConfig`,
//! `CreateIdentityProvider`, `AdminCreateUser`, `AdminSetUserPassword` and
//! `InitiateAuth`.
//!
//! Each table row names the case it asserts and every assertion reports the actual
//! value.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice as cognito_idp;

/// A request's SigV4 credential scope is what routes an administrative call to this
/// service. The signature is never checked here: the suite turns request
/// authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/cognito-idp/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0000";

/// This service's regional endpoint host, which a signed call is addressed to.
const ENDPOINT_HOST: &str = "cognito-idp.us-east-1.amazonaws.com";

/// The emulator's own endpoint: an address, so an unsigned request addressed to it
/// names no service in its `Host` header and `X-Amz-Target` is the only routing
/// signal left.
const EMULATOR_HOST: &str = "127.0.0.1:9297";

/// A password satisfying a default pool's password policy.
const GOOD_PASSWORD: &str = "Passw0rd!2024";

/// The sentence a call authorized by a token a sign-out or a revocation invalidated
/// is refused with: "Amazon Cognito returns an `Access Token has been revoked` error
/// when your app attempts to authorize a user pools API request with a revoked
/// access token" (`GlobalSignOut` in this service's model).
const ACCESS_TOKEN_REVOKED: &str = "Access Token has been revoked";

/// The sentence a refresh of a refresh token a sign-out invalidated is refused with.
const REFRESH_TOKEN_REVOKED: &str = "Refresh Token has been revoked";

/// The sentence a second link of one federated identity is refused with when it
/// re-points the identity at the profile it ALREADY links to.
const ALREADY_LINKED: &str = "SourceUser is already linked to DestinationUser";

/// The sentence a `SourceUser` that resolves to one of the pool's own local profiles
/// is refused with — which a live link to a profile that signs in with a password makes
/// a federated identity
/// (`smithy_query(query="doc",shape="AdminLinkProviderForUserRequest",member="SourceUser")`:
/// the source "must be a federated user ..., not another native user").
const INVALID_SOURCE_USER: &str = "Invalid SourceUser: Cognito users with a username/password \
     may not be passed in as a SourceUser, only as a DestinationUser";

/// The `ChallengeNameType` value a user that must set up an MFA factor before it can
/// sign in is answered with.
const MFA_SETUP: &str = "MFA_SETUP";

/// The `ChallengeParameters` entries an `MFA_SETUP` challenge carries.
const MFAS_CAN_SETUP: &str = "MFAS_CAN_SETUP";
const USER_ID_FOR_SRP: &str = "USER_ID_FOR_SRP";

/// The `UserMFASettingList` values, in the order that member documents them
/// (`smithy_query(query="operation",operation="GetUserAuthFactors")`:
/// "the possible values in this list are `SMS_MFA`, `EMAIL_OTP`, and
/// `SOFTWARE_TOKEN_MFA`").
const SMS_MFA: &str = "SMS_MFA";
const EMAIL_OTP_MFA: &str = "EMAIL_OTP";
const SOFTWARE_TOKEN_MFA: &str = "SOFTWARE_TOKEN_MFA";

/// The modelled bounds of a `Session` (`smithy_query(query="shape",
/// shape="SessionType")`: length 20..4096), which the session a challenge carries
/// has to sit inside.
const SESSION_LENGTH: (usize, usize) = (20, 4096);

/// The Amazon SNS caller an SMS MFA configuration names
/// (`SmsConfigurationType.SnsCallerArn`).
const SNS_CALLER_ARN: &str = "arn:aws:iam::000000000000:role/service-role/cognito-sms";

/// One emulator with just this service mounted, and the engine behind it.
struct Emulator {
    router: MultiServiceRouter,
}

fn emulator() -> Emulator {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(cognito_idp::EmulatedService::new(engine));
    router.set_request_authentication(false);
    Emulator { router }
}

/// Dispatch `operation` with `body` and return its status and JSON response body. A
/// `signed` call carries a credential scope, as the SDK sends every administrative
/// operation; an unsigned one is addressed to the emulator's own address, which is
/// how a signed-in user's client calls the self-service half of this API.
async fn dispatch(
    emulator: &Emulator,
    operation: &str,
    body: &Value,
    signed: bool,
) -> (u16, Value) {
    let host = if signed { ENDPOINT_HOST } else { EMULATOR_HOST };
    let mut request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", host)
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "x-amz-target",
            format!("AWSCognitoIdentityProviderService.{operation}"),
        );
    if signed {
        request = request.header("authorization", AUTHORIZATION);
    }
    let response = emulator
        .router
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

/// Call the signed `operation`, requiring success, and return its response body.
async fn ok(emulator: &Emulator, operation: &str, body: &Value) -> Value {
    let (status, payload) = dispatch(emulator, operation, body, true).await;
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

/// Create a pool named `name`, with `extra` merged into the request, and return its
/// minted id.
async fn pool(emulator: &Emulator, name: &str, extra: Value) -> String {
    let mut body = Map::new();
    body.insert("PoolName".to_string(), json!(name));
    if let Some(members) = extra.as_object() {
        for (name, value) in members {
            body.insert(name.clone(), value.clone());
        }
    }
    let created = ok(emulator, "CreateUserPool", &Value::Object(body)).await;
    created["UserPool"]["Id"]
        .as_str()
        .expect("a minted pool id")
        .to_string()
}

/// Create an app client of `user_pool_id` that allows both password sign-in flows
/// and the refresh flow, with `extra` merged in, and return its minted id.
async fn app_client(emulator: &Emulator, user_pool_id: &str, name: &str, extra: Value) -> String {
    let mut body = Map::new();
    body.insert("UserPoolId".to_string(), json!(user_pool_id));
    body.insert("ClientName".to_string(), json!(name));
    body.insert(
        "ExplicitAuthFlows".to_string(),
        json!([
            "ALLOW_USER_PASSWORD_AUTH",
            "ALLOW_ADMIN_USER_PASSWORD_AUTH",
            "ALLOW_REFRESH_TOKEN_AUTH"
        ]),
    );
    if let Some(members) = extra.as_object() {
        for (name, value) in members {
            body.insert(name.clone(), value.clone());
        }
    }
    let created = ok(emulator, "CreateUserPoolClient", &Value::Object(body)).await;
    created["UserPoolClient"]["ClientId"]
        .as_str()
        .expect("a minted client id")
        .to_string()
}

/// Create `username` in `user_pool_id` with `attributes` and a permanent password.
async fn user(emulator: &Emulator, user_pool_id: &str, username: &str, attributes: &[(&str, &str)]) {
    ok(
        emulator,
        "AdminCreateUser",
        &json!({
            "UserPoolId": user_pool_id,
            "Username": username,
            "UserAttributes": Value::Array(
                attributes
                    .iter()
                    .map(|(name, value)| json!({ "Name": name, "Value": value }))
                    .collect(),
            ),
        }),
    )
    .await;
    ok(
        emulator,
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

/// The tokens one completed sign-in issued.
struct Tokens {
    access_token: String,
    refresh_token: String,
}

/// Sign `username` in through `client_id` and return the tokens, requiring a
/// completed sign-in.
async fn sign_in(emulator: &Emulator, client_id: &str, username: &str) -> Tokens {
    let (status, payload) = dispatch(
        emulator,
        "InitiateAuth",
        &json!({
            "ClientId": client_id,
            "AuthFlow": "USER_PASSWORD_AUTH",
            "AuthParameters": { "USERNAME": username, "PASSWORD": GOOD_PASSWORD },
        }),
        false,
    )
    .await;
    assert_eq!(
        status, 200,
        "signing {username} in must succeed, got {status} with {payload}"
    );
    let result = &payload["AuthenticationResult"];
    Tokens {
        access_token: result["AccessToken"]
            .as_str()
            .unwrap_or_else(|| panic!("a completed sign-in issues an access token: {payload}"))
            .to_string(),
        refresh_token: result["RefreshToken"]
            .as_str()
            .unwrap_or_else(|| panic!("a completed sign-in issues a refresh token: {payload}"))
            .to_string(),
    }
}

/// The self-service operations an access token authorizes that this suite drives
/// after a sign-out: the request each sends, carrying only that token.
const SELF_SERVICE_READERS: &[&str] = &["GetUser", "GetUserAuthFactors", "ListDevices"];

/// Assert that every self-service reader refuses `access_token` as REVOKED.
async fn refused_as_revoked(emulator: &Emulator, case: &str, access_token: &str) {
    for operation in SELF_SERVICE_READERS {
        let (status, payload) = dispatch(
            emulator,
            operation,
            &json!({ "AccessToken": access_token }),
            false,
        )
        .await;
        assert_eq!(
            (status, error_code(&payload), error_message(&payload)),
            (400, "NotAuthorizedException", ACCESS_TOKEN_REVOKED),
            "case {case:?}: {operation} must refuse a revoked token, got {status} with {payload}"
        );
    }
}

/// Assert that every self-service reader still serves `access_token`.
async fn still_authorizes(emulator: &Emulator, case: &str, access_token: &str, username: &str) {
    for operation in SELF_SERVICE_READERS {
        let (status, payload) = dispatch(
            emulator,
            operation,
            &json!({ "AccessToken": access_token }),
            false,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: {operation} must still serve this token, got {status} with {payload}"
        );
        if operation == &"GetUser" {
            assert_eq!(
                payload["Username"],
                json!(username),
                "case {case:?}: expected the token's own user, got {payload}"
            );
        }
    }
}

#[tokio::test]
async fn a_sign_out_revokes_every_token_it_issued_to_its_user() {
    let emulator = emulator();
    let user_pool_id = pool(&emulator, "sign-out-pool", json!({})).await;
    let client_id = app_client(&emulator, &user_pool_id, "sign-out-client", json!({})).await;
    for username in ["ada", "grace"] {
        user(&emulator, &user_pool_id, username, &[]).await;
    }

    // TWO sign-ins of the same user, so the sign-out is shown to reach every token
    // issued to it rather than the one that authorized the call, beside one sign-in
    // of ANOTHER user, which it must leave alone.
    let first = sign_in(&emulator, &client_id, "ada").await;
    let second = sign_in(&emulator, &client_id, "ada").await;
    let other = sign_in(&emulator, &client_id, "grace").await;

    let (status, payload) = dispatch(
        &emulator,
        "GlobalSignOut",
        &json!({ "AccessToken": first.access_token }),
        false,
    )
    .await;
    assert_eq!(
        status, 200,
        "the sign-out must be served, got {status} with {payload}"
    );
    assert_eq!(
        payload,
        json!({}),
        "GlobalSignOut models no output member, got {payload}"
    );

    // (case, the access token, the refresh token)
    let revoked: &[(&str, &Tokens)] = &[
        ("the session that signed out", &first),
        ("another session of the same user", &second),
    ];
    for (case, tokens) in revoked {
        refused_as_revoked(&emulator, case, &tokens.access_token).await;
        let (status, payload) = dispatch(
            &emulator,
            "GetTokensFromRefreshToken",
            &json!({ "ClientId": client_id, "RefreshToken": tokens.refresh_token }),
            false,
        )
        .await;
        assert_eq!(
            (status, error_code(&payload), error_message(&payload)),
            (400, "NotAuthorizedException", REFRESH_TOKEN_REVOKED),
            "case {case:?}: the refresh exchange must refuse a revoked refresh token, got \
             {status} with {payload}"
        );
        // The refresh flow of `InitiateAuth` reads the same revocation state.
        let (status, payload) = dispatch(
            &emulator,
            "InitiateAuth",
            &json!({
                "ClientId": client_id,
                "AuthFlow": "REFRESH_TOKEN_AUTH",
                "AuthParameters": { "REFRESH_TOKEN": tokens.refresh_token },
            }),
            false,
        )
        .await;
        assert_eq!(
            (status, error_code(&payload), error_message(&payload)),
            (400, "NotAuthorizedException", REFRESH_TOKEN_REVOKED),
            "case {case:?}: the refresh flow must refuse a revoked refresh token, got {status} \
             with {payload}"
        );
    }

    // Another user's session is untouched, and so is a sign-in made AFTER the
    // sign-out: the revocation acts on the tokens that existed, not on the user.
    still_authorizes(&emulator, "another user's session", &other.access_token, "grace").await;
    let again = sign_in(&emulator, &client_id, "ada").await;
    still_authorizes(&emulator, "a sign-in after the sign-out", &again.access_token, "ada").await;

    // Signing out again with a token the first sign-out revoked is refused as
    // revoked too, and revokes nothing further.
    let (status, payload) = dispatch(
        &emulator,
        "GlobalSignOut",
        &json!({ "AccessToken": first.access_token }),
        false,
    )
    .await;
    assert_eq!(
        (status, error_code(&payload), error_message(&payload)),
        (400, "NotAuthorizedException", ACCESS_TOKEN_REVOKED),
        "case \"a second sign-out with the revoked token\": got {status} with {payload}"
    );
    still_authorizes(
        &emulator,
        "the session made after the sign-out, after a refused second sign-out",
        &again.access_token,
        "ada",
    )
    .await;
}

/// One row of the provider-link table: the identity to link, and the destination the
/// SECOND link names.
struct LinkCase {
    case: &'static str,
    /// The source identity's provider name, attribute name and value.
    provider_name: &'static str,
    provider_attribute_name: &'static str,
    provider_attribute_value: &'static str,
    /// Whether the second link names the profile the first one linked to, or another
    /// profile of the same pool.
    same_destination: bool,
}

/// The rows differ along the provider the identity federates through, the attribute
/// that names it, the FORM of its value (a subject id, an email address, a long
/// opaque id), and whether the second link re-points the identity or names the
/// profile it already links to.
const LINK_CASES: &[LinkCase] = &[
    LinkCase {
        case: "a SAML identity named by its subject, linked again to the same profile",
        provider_name: "CorpSAML",
        provider_attribute_name: "Cognito_Subject",
        provider_attribute_value: "saml-subject-1",
        same_destination: true,
    },
    LinkCase {
        case: "the same identity re-pointed at another profile of the pool",
        provider_name: "CorpSAML",
        provider_attribute_name: "Cognito_Subject",
        provider_attribute_value: "saml-subject-2",
        same_destination: false,
    },
    LinkCase {
        case: "an OIDC identity named by an email claim, linked again to the same profile",
        provider_name: "CorpOIDC",
        provider_attribute_name: "email",
        provider_attribute_value: "ada@idp.example.com",
        same_destination: true,
    },
    LinkCase {
        case: "an OIDC identity named by a long opaque id, re-pointed at another profile",
        provider_name: "CorpOIDC",
        provider_attribute_name: "NameID",
        provider_attribute_value: "0123456789abcdef0123456789abcdef0123456789abcdef",
        same_destination: false,
    },
];

#[tokio::test]
async fn a_source_identity_links_to_one_profile_only() {
    let emulator = emulator();
    let user_pool_id = pool(&emulator, "links-pool", json!({})).await;
    for username in ["ada", "grace"] {
        user(&emulator, &user_pool_id, username, &[]).await;
    }
    for (provider_name, provider_type) in [("CorpSAML", "SAML"), ("CorpOIDC", "OIDC")] {
        ok(
            &emulator,
            "CreateIdentityProvider",
            &json!({
                "UserPoolId": user_pool_id,
                "ProviderName": provider_name,
                "ProviderType": provider_type,
                "ProviderDetails": {
                    "MetadataURL": "https://idp.example.com/metadata",
                },
            }),
        )
        .await;
    }

    for row in LINK_CASES {
        let case = row.case;
        let source = json!({
            "ProviderName": row.provider_name,
            "ProviderAttributeName": row.provider_attribute_name,
            "ProviderAttributeValue": row.provider_attribute_value,
        });
        let link = |destination: &'static str| {
            json!({
                "UserPoolId": user_pool_id,
                "SourceUser": source,
                "DestinationUser": {
                    "ProviderName": "Cognito",
                    "ProviderAttributeValue": destination,
                },
            })
        };

        let (status, payload) = dispatch(&emulator, "AdminLinkProviderForUser", &link("ada"), true)
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the first link must be served, got {status} with {payload}"
        );

        let destination = if row.same_destination { "ada" } else { "grace" };
        // A linked identity is spoken for either way: the call that names the
        // profile it already links to is told so, and the one that points it at
        // another profile submits an identity the live link has made local.
        let expected = if row.same_destination {
            ALREADY_LINKED
        } else {
            INVALID_SOURCE_USER
        };
        let (status, payload) =
            dispatch(&emulator, "AdminLinkProviderForUser", &link(destination), true).await;
        assert_eq!(
            (status, error_code(&payload), error_message(&payload)),
            (400, "InvalidParameterException", expected),
            "case {case:?}: a second link of one identity must be refused, got {status} with \
             {payload}"
        );

        // ROUND TRIP: the one stored link is still the first one — the refused
        // second call wrote nothing — so deactivating it succeeds exactly once.
        let disable = json!({ "UserPoolId": user_pool_id, "User": source });
        let (status, payload) =
            dispatch(&emulator, "AdminDisableProviderForUser", &disable, true).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the stored link must be removable, got {status} with {payload}"
        );
        let (status, payload) =
            dispatch(&emulator, "AdminDisableProviderForUser", &disable, true).await;
        assert_eq!(
            status, 400,
            "case {case:?}: the identity holds no second link, got {status} with {payload}"
        );

        // The deactivation removed the LINK, so the identity resolves to no local
        // profile and links where the next call points it.
        let (status, payload) =
            dispatch(&emulator, "AdminLinkProviderForUser", &link("grace"), true).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a de-linked identity links again, got {status} with {payload}"
        );
    }
}

/// One row of the MFA-requirement table: the pool's MFA configuration, and the
/// methods an `MFA_SETUP` challenge must report as available for setup.
struct MfaCase {
    case: &'static str,
    /// The `SetUserPoolMfaConfig` request members, beside the pool id.
    configuration: fn() -> Value,
    /// The `MFAS_CAN_SETUP` entry the challenge must carry, in order, or an empty
    /// slice for a row that expects no challenge at all.
    can_setup: &'static [&'static str],
    /// Whether the sign-in is expected to be challenged rather than completed.
    challenged: bool,
}

/// The rows differ along WHICH methods the pool offers (TOTP alone, SMS alone, email
/// alone, all three) and along whether the pool REQUIRES a second factor at all —
/// `MfaConfiguration` is `ON`, `OPTIONAL` or `OFF`
/// (`smithy_query(query="shape",shape="UserPoolMfaType")`), and only the required
/// mode makes a factor a precondition of a sign-in.
const MFA_CASES: &[MfaCase] = &[
    MfaCase {
        case: "MFA required with the software token method offered",
        configuration: || {
            json!({
                "MfaConfiguration": "ON",
                "SoftwareTokenMfaConfiguration": { "Enabled": true },
            })
        },
        can_setup: &[SOFTWARE_TOKEN_MFA],
        challenged: true,
    },
    MfaCase {
        case: "MFA required with the SMS method offered",
        configuration: || {
            json!({
                "MfaConfiguration": "ON",
                "SmsMfaConfiguration": {
                    "SmsConfiguration": { "SnsCallerArn": SNS_CALLER_ARN },
                },
            })
        },
        can_setup: &[SMS_MFA],
        challenged: true,
    },
    MfaCase {
        case: "MFA required with the email method offered",
        configuration: || {
            json!({
                "MfaConfiguration": "ON",
                "EmailMfaConfiguration": { "Message": "Your code is {####}", "Subject": "Code" },
            })
        },
        can_setup: &[EMAIL_OTP_MFA],
        challenged: true,
    },
    MfaCase {
        case: "MFA required with all three methods offered",
        configuration: || {
            json!({
                "MfaConfiguration": "ON",
                "SmsMfaConfiguration": {
                    "SmsConfiguration": { "SnsCallerArn": SNS_CALLER_ARN },
                },
                "SoftwareTokenMfaConfiguration": { "Enabled": true },
                "EmailMfaConfiguration": { "Message": "Your code is {####}", "Subject": "Code" },
            })
        },
        can_setup: &[SMS_MFA, EMAIL_OTP_MFA, SOFTWARE_TOKEN_MFA],
        challenged: true,
    },
    MfaCase {
        case: "MFA optional, which a user with no factor signs in without",
        configuration: || {
            json!({
                "MfaConfiguration": "OPTIONAL",
                "SoftwareTokenMfaConfiguration": { "Enabled": true },
            })
        },
        can_setup: &[],
        challenged: false,
    },
    MfaCase {
        case: "MFA off, which asks for no second factor at all",
        configuration: || json!({ "MfaConfiguration": "OFF" }),
        can_setup: &[],
        challenged: false,
    },
];

/// The `Session` a challenge response carries, requiring one inside the modelled
/// length bounds.
fn session_of(case: &str, payload: &Value) -> String {
    let session = payload["Session"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: a challenge carries a session: {payload}"));
    let (low, high) = SESSION_LENGTH;
    assert!(
        (low..=high).contains(&session.chars().count()),
        "case {case:?}: expected a session inside the modelled length {SESSION_LENGTH:?}, got \
         {} characters",
        session.chars().count()
    );
    session.to_string()
}

#[tokio::test]
async fn a_pool_that_requires_mfa_asks_a_user_with_no_factor_to_set_one_up() {
    for (index, row) in MFA_CASES.iter().enumerate() {
        let emulator = emulator();
        let case = row.case;
        let user_pool_id = pool(&emulator, &format!("mfa-pool-{index}"), json!({})).await;
        let client_id = app_client(&emulator, &user_pool_id, "mfa-client", json!({})).await;
        user(
            &emulator,
            &user_pool_id,
            "ada",
            &[("email", "ada@example.com"), ("email_verified", "true")],
        )
        .await;

        let mut configuration = Map::new();
        configuration.insert("UserPoolId".to_string(), json!(user_pool_id));
        for (name, value) in (row.configuration)()
            .as_object()
            .expect("a configuration is a JSON object")
        {
            configuration.insert(name.clone(), value.clone());
        }
        ok(
            &emulator,
            "SetUserPoolMfaConfig",
            &Value::Object(configuration),
        )
        .await;

        // The SAME rule holds for both password entry points: the self-service
        // sign-in and the administrative one.
        let signed_in = json!({
            "ClientId": client_id,
            "AuthFlow": "USER_PASSWORD_AUTH",
            "AuthParameters": { "USERNAME": "ada", "PASSWORD": GOOD_PASSWORD },
        });
        let admin_signed_in = json!({
            "UserPoolId": user_pool_id,
            "ClientId": client_id,
            "AuthFlow": "ADMIN_USER_PASSWORD_AUTH",
            "AuthParameters": { "USERNAME": "ada", "PASSWORD": GOOD_PASSWORD },
        });
        for (operation, body, signed) in [
            ("InitiateAuth", &signed_in, false),
            ("AdminInitiateAuth", &admin_signed_in, true),
        ] {
            let case = format!("{case}, through {operation}");
            let (status, payload) = dispatch(&emulator, operation, body, signed).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the sign-in is served, got {status} with {payload}"
            );

            if !row.challenged {
                assert_eq!(
                    payload["ChallengeName"],
                    Value::Null,
                    "case {case:?}: a pool that requires no second factor raises no challenge, \
                     got {payload}"
                );
                assert!(
                    payload["AuthenticationResult"]["AccessToken"].is_string(),
                    "case {case:?}: the sign-in completes, got {payload}"
                );
                continue;
            }

            assert_eq!(
                payload["ChallengeName"],
                json!(MFA_SETUP),
                "case {case:?}: expected the MFA_SETUP challenge, got {payload}"
            );
            assert_eq!(
                payload["AuthenticationResult"],
                Value::Null,
                "case {case:?}: a challenged sign-in mints no token, got {payload}"
            );
            let _session = session_of(&case, &payload);
            let parameters = &payload["ChallengeParameters"];
            assert_eq!(
                parameters[USER_ID_FOR_SRP],
                json!("ada"),
                "case {case:?}: the challenge names the user it is for, got {payload}"
            );
            let can_setup: Vec<String> = row
                .can_setup
                .iter()
                .map(|setting| (*setting).to_string())
                .collect();
            let reported: Vec<String> = serde_json::from_str(
                parameters[MFAS_CAN_SETUP]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case:?}: MFAS_CAN_SETUP is a JSON array in a string: {payload}")),
            )
            .unwrap_or_else(|error| {
                panic!("case {case:?}: MFAS_CAN_SETUP must parse as JSON ({error}): {payload}")
            });
            assert_eq!(
                reported, can_setup,
                "case {case:?}: expected {can_setup:?}, got {reported:?}"
            );
        }
    }
}

/// One row of the administrative-preference table: what the request activates, and
/// what the user must report afterwards.
struct PreferenceCase {
    case: &'static str,
    /// The attributes the user holds, so the factor the row activates is one it can
    /// receive.
    attributes: &'static [(&'static str, &'static str)],
    /// The settings the request carries: (member, enabled, preferred).
    settings: &'static [(&'static str, bool, bool)],
    /// The `UserMFASettingList` the readers must report, in order.
    activated: &'static [&'static str],
    /// The `PreferredMfaSetting` the readers must report.
    preferred: Option<&'static str>,
}

/// A verified phone number and a verified email address, so each delivered factor is
/// one the user can receive.
const REACHABLE: &[(&str, &str)] = &[
    ("email", "ada@example.com"),
    ("email_verified", "true"),
    ("phone_number", "+15550100001"),
    ("phone_number_verified", "true"),
];

/// The rows differ along WHICH factor an administrator activates, how many at once,
/// and whether one of them is preferred.
const PREFERENCE_CASES: &[PreferenceCase] = &[
    PreferenceCase {
        case: "SMS message MFA activated and preferred",
        attributes: REACHABLE,
        settings: &[("SMSMfaSettings", true, true)],
        activated: &[SMS_MFA],
        preferred: Some(SMS_MFA),
    },
    PreferenceCase {
        case: "email message MFA activated and preferred",
        attributes: REACHABLE,
        settings: &[("EmailMfaSettings", true, true)],
        activated: &[EMAIL_OTP_MFA],
        preferred: Some(EMAIL_OTP_MFA),
    },
    PreferenceCase {
        case: "two factors activated at once, the email one preferred",
        attributes: REACHABLE,
        settings: &[("SMSMfaSettings", true, false), ("EmailMfaSettings", true, true)],
        activated: &[SMS_MFA, EMAIL_OTP_MFA],
        preferred: Some(EMAIL_OTP_MFA),
    },
    PreferenceCase {
        case: "a factor activated without a preference leaves the user preferring none",
        attributes: REACHABLE,
        settings: &[("SMSMfaSettings", true, false)],
        activated: &[SMS_MFA],
        preferred: None,
    },
];

#[tokio::test]
async fn an_mfa_preference_an_administrator_sets_is_what_the_readers_report() {
    for (index, row) in PREFERENCE_CASES.iter().enumerate() {
        let emulator = emulator();
        let case = row.case;
        let user_pool_id = pool(&emulator, &format!("admin-preference-{index}"), json!({})).await;
        let client_id = app_client(&emulator, &user_pool_id, "preference-client", json!({})).await;
        user(&emulator, &user_pool_id, "ada", row.attributes).await;
        let tokens = sign_in(&emulator, &client_id, "ada").await;

        let mut body = Map::new();
        body.insert("UserPoolId".to_string(), json!(user_pool_id));
        body.insert("Username".to_string(), json!("ada"));
        for (member, enabled, preferred) in row.settings {
            body.insert(
                (*member).to_string(),
                json!({ "Enabled": enabled, "PreferredMfa": preferred }),
            );
        }
        let (status, payload) = dispatch(
            &emulator,
            "AdminSetUserMFAPreference",
            &Value::Object(body),
            true,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the preference must be stored, got {status} with {payload}"
        );
        assert_eq!(
            payload,
            json!({}),
            "case {case:?}: this operation models no output member, got {payload}"
        );

        // ROUND TRIP: the stored preference is what every reader of this user
        // reports — the self-service factor listing, the self-service profile and
        // the administrative profile alike.
        let activated: Vec<String> = row
            .activated
            .iter()
            .map(|setting| (*setting).to_string())
            .collect();
        let preferred = row.preferred.map(|setting| json!(setting));
        for (operation, body, signed) in [
            (
                "GetUserAuthFactors",
                json!({ "AccessToken": tokens.access_token }),
                false,
            ),
            (
                "GetUser",
                json!({ "AccessToken": tokens.access_token }),
                false,
            ),
            (
                "AdminGetUser",
                json!({ "UserPoolId": user_pool_id, "Username": "ada" }),
                true,
            ),
        ] {
            let case = format!("{case}, read through {operation}");
            let (status, payload) = dispatch(&emulator, operation, &body, signed).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the read is served, got {status} with {payload}"
            );
            let reported: Vec<String> = payload["UserMFASettingList"]
                .as_array()
                .map(|values| {
                    values
                        .iter()
                        .map(|value| value.as_str().unwrap_or_default().to_string())
                        .collect()
                })
                .unwrap_or_default();
            assert_eq!(
                reported, activated,
                "case {case:?}: expected {activated:?}, got {reported:?}"
            );
            let reported_preference = payload.get("PreferredMfaSetting").cloned();
            assert_eq!(
                reported_preference,
                preferred.clone(),
                "case {case:?}: expected {preferred:?}, got {reported_preference:?}"
            );
        }
    }
}

/// One row of the simulated-delivery table: the username asked about, which no call
/// of this suite registers.
const GHOST_USERNAMES: &[(&str, &str)] = &[
    ("a plain name", "nobody-here"),
    ("an email-shaped name", "nobody@example.com"),
    ("a name shaped like a generated sub", "11111111-2222-3333-4444-555555555555"),
    ("a non-ASCII name", "ユーザー-不在"),
];

/// The `Destination` a simulated delivery reports: it identifies nobody, and it is
/// the same value whatever the request named, so no two usernames can be told apart
/// by the answer they get.
const SIMULATED_DESTINATION: &str = "a***@b***";

#[tokio::test]
async fn a_recovery_request_for_a_user_nobody_registered_reports_a_simulated_delivery() {
    let emulator = emulator();
    let user_pool_id = pool(
        &emulator,
        "ghost-pool",
        json!({ "AutoVerifiedAttributes": ["email"] }),
    )
    .await;
    let hiding = app_client(
        &emulator,
        &user_pool_id,
        "hiding",
        json!({ "PreventUserExistenceErrors": "ENABLED" }),
    )
    .await;
    let legacy = app_client(
        &emulator,
        &user_pool_id,
        "legacy",
        json!({ "PreventUserExistenceErrors": "LEGACY" }),
    )
    .await;

    for (name_case, username) in GHOST_USERNAMES {
        for operation in ["ForgotPassword", "ResendConfirmationCode"] {
            // The client that suppresses user-existence errors reports a delivery,
            // to a simulated destination.
            let case = format!("{operation} for {name_case}, through the hiding client");
            let (status, payload) = dispatch(
                &emulator,
                operation,
                &json!({ "ClientId": hiding, "Username": username }),
                false,
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected a reported delivery, got {status} with {payload}"
            );
            assert_eq!(
                payload["CodeDeliveryDetails"],
                json!({
                    "AttributeName": "email",
                    "DeliveryMedium": "EMAIL",
                    "Destination": SIMULATED_DESTINATION,
                }),
                "case {case:?}: expected a simulated delivery, got {payload}"
            );

            // The legacy client, which does not suppress them, reports that the
            // pair resolved nothing.
            let case = format!("{operation} for {name_case}, through the legacy client");
            let (status, payload) = dispatch(
                &emulator,
                operation,
                &json!({ "ClientId": legacy, "Username": username }),
                false,
            )
            .await;
            assert_eq!(
                (status, error_code(&payload)),
                (400, "UserNotFoundException"),
                "case {case:?}: expected a 400 UserNotFoundException, got {status} with {payload}"
            );
        }
    }

    // Nothing was stored for any of them: a user nobody registered is still
    // unknown to the administrative reader, so no later call can sign in as one.
    for (name_case, username) in GHOST_USERNAMES {
        let (status, payload) = dispatch(
            &emulator,
            "AdminGetUser",
            &json!({ "UserPoolId": user_pool_id, "Username": username }),
            true,
        )
        .await;
        assert_eq!(
            (status, error_code(&payload)),
            (400, "UserNotFoundException"),
            "case {name_case:?}: a simulated delivery registers nobody, got {status} with \
             {payload}"
        );
    }
}
