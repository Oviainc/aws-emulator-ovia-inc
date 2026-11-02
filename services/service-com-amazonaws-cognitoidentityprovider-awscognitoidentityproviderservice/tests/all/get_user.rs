// SPDX-License-Identifier: CC0-1.0

//! `GetUser` over the wire, driven exactly as an SDK drives it (`POST /` with
//! `X-Amz-Target: AWSCognitoIdentityProviderService.GetUser`).
//!
//! The rule under test: a self-service read whose only subject is an
//! `AccessToken` renders the STORED user that token was issued to — that user's
//! stored username, its whole stored attribute list and its stored MFA
//! configuration — and writes nothing, so a second read answers exactly what the
//! first one did and every other reader of the same user sees the same thing. A
//! token that names no signed-in user is refused instead: one this emulator never
//! issued, one that is not an ACCESS token, a revoked one and one the clock has
//! expired with 400 `NotAuthorizedException`, one whose pool is gone with 400
//! `ResourceNotFoundException` and one whose user is gone with 400
//! `UserNotFoundException`.
//!
//! The pools, app clients, custom attributes, users, MFA configurations and
//! access tokens are all real: they come from `CreateUserPool`,
//! `CreateUserPoolClient`, `AddCustomAttributes`, `AdminCreateUser`,
//! `AdminSetUserPassword`, `SetUserSettings` and `InitiateAuth`, and every read
//! is cross-checked against the administrative `AdminGetUser`. A token record
//! whose state no read of this suite writes — a revoked token, one the clock has
//! expired, one naming a pool or a user that is
//! gone — is PLANTED in the store, the way the other suites of this crate plant
//! state for writers that are not implemented yet.
//!
//! Each table row names the case it asserts and every assertion reports the
//! actual value.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice as cognito_idp;
use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice::state;

/// A request's SigV4 credential scope is what routes an administrative call to
/// this service. The signature is never checked here: the suite turns request
/// authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/cognito-idp/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0000";

/// This service's regional endpoint host, which a signed call is addressed to.
const ENDPOINT_HOST: &str = "cognito-idp.us-east-1.amazonaws.com";

/// The emulator's own endpoint: an address, so an unsigned request addressed to
/// it names no service in its `Host` header and `X-Amz-Target` is the only
/// routing signal left.
const EMULATOR_HOST: &str = "127.0.0.1:9297";

/// A password satisfying a default pool's password policy.
const GOOD_PASSWORD: &str = "Passw0rd!2024";

/// The `UserMFASettingList` value for SMS message MFA (`GetUser`'s
/// `UserMFASettingList`: "the possible values in this list are `SMS_MFA`,
/// `EMAIL_OTP`, and `SOFTWARE_TOKEN_MFA`").
const SMS_MFA: &str = "SMS_MFA";

/// Every member `GetUserResponse` models
/// (`smithy_query(query="operation",operation="GetUser")`), so a response that
/// carries anything else is reporting something the operation does not model.
const MODELLED_OUTPUT_MEMBERS: [&str; 5] = [
    "MFAOptions",
    "PreferredMfaSetting",
    "UserAttributes",
    "UserMFASettingList",
    "Username",
];

/// One emulator with just this service mounted, and the engine its state lives
/// in, so a suite can plant a token record no call can hand out.
struct Emulator {
    engine: Arc<engine::InMemoryEngine>,
    router: MultiServiceRouter,
}

fn emulator() -> Emulator {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(cognito_idp::EmulatedService::new(Arc::clone(&engine)));
    router.set_request_authentication(false);
    Emulator { engine, router }
}

/// Dispatch `operation` with `body` and return its status and JSON response
/// body. A `signed` call carries a credential scope, as the SDK sends every
/// administrative operation; an unsigned one is addressed to the emulator's own
/// address, which is how a signed-in user's client calls `GetUser` — the model
/// marks the operation `@optionalAuth`.
async fn dispatch(
    router: &MultiServiceRouter,
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

/// Call the signed `operation`, requiring success, and return its response body.
async fn ok(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, payload) = dispatch(router, operation, body, true).await;
    assert_eq!(
        status, 200,
        "{operation} must succeed, got {status} with {payload}"
    );
    payload
}

/// The wire error code of an error body: the `__type` without the shape
/// namespace an AWS JSON error carries it under.
fn error_code(payload: &Value) -> &str {
    let raw = payload
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>");
    raw.split('#').next_back().unwrap_or(raw)
}

/// The `message` of an error body.
fn error_message(payload: &Value) -> &str {
    payload
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
}

/// Create a pool and return its minted `UserPoolId`. A pool asked for
/// case-insensitive usernames gets the `UsernameConfiguration` that says so.
async fn pool(router: &MultiServiceRouter, name: &str, case_sensitive: bool) -> String {
    let mut body = Map::new();
    body.insert("PoolName".to_string(), json!(name));
    if !case_sensitive {
        body.insert(
            "UsernameConfiguration".to_string(),
            json!({ "CaseSensitive": false }),
        );
    }
    let created = ok(router, "CreateUserPool", &Value::Object(body)).await;
    created["UserPool"]["Id"]
        .as_str()
        .expect("a minted pool id")
        .to_string()
}

/// Declare `names` as custom attributes of `user_pool_id`, so a user may hold
/// them under the `custom:` prefix.
async fn declare_custom_attributes(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    names: &[&str],
) {
    ok(
        router,
        "AddCustomAttributes",
        &json!({
            "UserPoolId": user_pool_id,
            "CustomAttributes": Value::Array(
                names.iter().map(|name| json!({ "Name": name })).collect(),
            ),
        }),
    )
    .await;
}

/// Create an app client that allows the password sign-in this suite uses, and
/// return its minted `ClientId`.
async fn client(router: &MultiServiceRouter, user_pool_id: &str) -> String {
    let created = ok(
        router,
        "CreateUserPoolClient",
        &json!({
            "UserPoolId": user_pool_id,
            "ClientName": "profile-client",
            "ExplicitAuthFlows": ["ALLOW_USER_PASSWORD_AUTH", "ALLOW_REFRESH_TOKEN_AUTH"],
        }),
    )
    .await;
    created["UserPoolClient"]["ClientId"]
        .as_str()
        .expect("a minted client id")
        .to_string()
}

/// Create `username` holding `attributes`, with a permanent password, so the
/// user is confirmed and can sign in.
async fn confirmed_user(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    username: &str,
    attributes: &[(&str, &str)],
) {
    ok(
        router,
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

/// Sign `username` in on `client_id` and return the `AuthenticationResult` the
/// sign-in issued.
async fn sign_in(router: &MultiServiceRouter, client_id: &str, username: &str) -> Value {
    let (status, payload) = dispatch(
        router,
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
        "signing {username:?} in must succeed, got {status} with {payload}"
    );
    payload["AuthenticationResult"].clone()
}

/// The access token a sign-in of `username` issued.
async fn access_token(router: &MultiServiceRouter, client_id: &str, username: &str) -> String {
    sign_in(router, client_id, username).await["AccessToken"]
        .as_str()
        .expect("a sign-in issues an access token")
        .to_string()
}

/// Call `GetUser` unsigned, as a signed-in user's client does, and return its
/// status and body.
async fn get_user(router: &MultiServiceRouter, token: &str) -> (u16, Value) {
    dispatch(router, "GetUser", &json!({ "AccessToken": token }), false).await
}

/// The `UserAttributes` of a `GetUser` / `AdminGetUser` body, as a name-to-value
/// map.
fn attribute_map(payload: &Value, who: &str) -> BTreeMap<String, String> {
    payload["UserAttributes"]
        .as_array()
        .unwrap_or_else(|| panic!("{who} must render UserAttributes, got {payload}"))
        .iter()
        .map(|attribute| {
            (
                attribute["Name"]
                    .as_str()
                    .unwrap_or_else(|| panic!("{who} renders a named attribute, got {attribute}"))
                    .to_string(),
                attribute["Value"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// Configure SMS MFA for the signed-in user `token` was issued to, through the
/// self-service writer.
async fn set_sms_mfa(router: &MultiServiceRouter, token: &str) {
    let (status, payload) = dispatch(
        router,
        "SetUserSettings",
        &json!({
            "AccessToken": token,
            "MFAOptions": [{ "DeliveryMedium": "SMS", "AttributeName": "phone_number" }],
        }),
        false,
    )
    .await;
    assert_eq!(
        status, 200,
        "SetUserSettings must succeed, got {status} with {payload}"
    );
}

/// One row of the rendering table: a stored user, and how its own access token
/// came to be.
#[derive(Clone, Copy, Debug)]
struct RenderCase {
    /// What this row asserts.
    case: &'static str,
    /// Whether the pool compares usernames case-sensitively.
    case_sensitive: bool,
    /// Custom attributes the pool declares before the user is created.
    custom_attributes: &'static [&'static str],
    /// The username `AdminCreateUser` stores.
    username: &'static str,
    /// The username the sign-in names, which a case-insensitive pool may case
    /// differently from the stored one.
    signed_in_as: &'static str,
    /// The attributes the stored user holds, beside its own `sub`.
    attributes: &'static [(&'static str, &'static str)],
    /// Whether SMS MFA is configured for the user before the read.
    sms_mfa: bool,
}

/// The rows differ along the COUNT of stored attributes (none, one, several),
/// their KIND (standard, a verified flag, a custom one under its `custom:`
/// prefix), the FORM of the username (plain, the punctuation an email address
/// carries, non-ASCII letters), whether the sign-in cased the username as the
/// store did, and whether the user has an MFA configuration at all.
const RENDER_CASES: &[RenderCase] = &[
    RenderCase {
        case: "a user holding no attribute but its own sub",
        case_sensitive: true,
        custom_attributes: &[],
        username: "ada",
        signed_in_as: "ada",
        attributes: &[],
        sms_mfa: false,
    },
    RenderCase {
        case: "a user holding one standard attribute",
        case_sensitive: true,
        custom_attributes: &[],
        username: "grace",
        signed_in_as: "grace",
        attributes: &[("email", "grace@example.com")],
        sms_mfa: false,
    },
    RenderCase {
        case: "a user holding several attributes, one of them a verified flag",
        case_sensitive: true,
        custom_attributes: &[],
        username: "edsger",
        signed_in_as: "edsger",
        attributes: &[
            ("email", "edsger@example.com"),
            ("email_verified", "true"),
            ("family_name", "Dijkstra"),
            ("given_name", "Edsger"),
            ("phone_number", "+15550100001"),
        ],
        sms_mfa: false,
    },
    RenderCase {
        case: "a custom attribute is rendered under its custom: prefix",
        case_sensitive: true,
        custom_attributes: &["department", "seats"],
        username: "alan",
        signed_in_as: "alan",
        attributes: &[
            ("custom:department", "research"),
            ("custom:seats", "12"),
            ("email", "alan@example.com"),
        ],
        sms_mfa: false,
    },
    RenderCase {
        case: "a username carrying the punctuation an email address does",
        case_sensitive: true,
        custom_attributes: &[],
        username: "ada.lovelace+work@example.com",
        signed_in_as: "ada.lovelace+work@example.com",
        attributes: &[("given_name", "Ada")],
        sms_mfa: false,
    },
    RenderCase {
        case: "a username of non-ASCII letters",
        case_sensitive: true,
        custom_attributes: &[],
        username: "Ада-Лавлейс",
        signed_in_as: "Ада-Лавлейс",
        attributes: &[("given_name", "Ада")],
        sms_mfa: false,
    },
    RenderCase {
        case: "a case-insensitive pool stores the lower-cased name, whatever case created it",
        case_sensitive: false,
        custom_attributes: &[],
        username: "Barbara",
        signed_in_as: "barbara",
        attributes: &[("email", "barbara@example.com")],
        sms_mfa: false,
    },
    RenderCase {
        case: "a user with SMS MFA configured renders its option and its activated factor",
        case_sensitive: true,
        custom_attributes: &[],
        username: "katherine",
        signed_in_as: "katherine",
        attributes: &[("phone_number", "+15550100002")],
        sms_mfa: true,
    },
];

#[tokio::test]
async fn get_user_renders_the_stored_user_its_access_token_was_issued_to() {
    let Emulator { router, .. } = emulator();

    for (index, row) in RENDER_CASES.iter().enumerate() {
        let case = row.case;
        let user_pool_id = pool(&router, &format!("render-pool-{index}"), row.case_sensitive).await;
        if !row.custom_attributes.is_empty() {
            declare_custom_attributes(&router, &user_pool_id, row.custom_attributes).await;
        }
        let client_id = client(&router, &user_pool_id).await;
        confirmed_user(&router, &user_pool_id, row.username, row.attributes).await;
        let token = access_token(&router, &client_id, row.signed_in_as).await;
        if row.sms_mfa {
            set_sms_mfa(&router, &token).await;
        }

        let (status, payload) = get_user(&router, &token).await;
        assert_eq!(
            status, 200,
            "case {case:?}: an access token must name its user, got {status} with {payload}"
        );

        // Nothing beyond what the operation models is reported.
        let rendered_members: Vec<String> = payload
            .as_object()
            .unwrap_or_else(|| panic!("case {case:?}: GetUser answers an object, got {payload}"))
            .keys()
            .cloned()
            .collect();
        for member in &rendered_members {
            assert!(
                MODELLED_OUTPUT_MEMBERS.contains(&member.as_str()),
                "case {case:?}: GetUser models no {member:?} member, got {rendered_members:?}"
            );
        }

        // A pool whose usernames are case-insensitive lower-cases the name on
        // the way in, so the stored username is the lower-cased one whatever
        // case the create submitted; a case-sensitive pool stores it as sent.
        let stored_username = if row.case_sensitive {
            row.username.to_string()
        } else {
            row.username.to_lowercase()
        };
        assert_eq!(
            payload["Username"],
            json!(stored_username),
            "case {case:?}: expected the stored username {stored_username:?}, got {payload}"
        );

        // The attribute list is the STORED one: exactly what the user holds,
        // including the `sub` the store minted and nothing the request invented.
        let rendered = attribute_map(&payload, "GetUser");
        let sub = rendered.get("sub").cloned().unwrap_or_else(|| {
            panic!("case {case:?}: every stored user carries its own sub, got {rendered:?}")
        });
        assert!(
            !sub.is_empty(),
            "case {case:?}: the stored sub is an id, got {sub:?}"
        );
        let mut expected: BTreeMap<String, String> = row
            .attributes
            .iter()
            .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
            .collect();
        expected.insert("sub".to_string(), sub);
        assert_eq!(
            rendered, expected,
            "case {case:?}: expected the stored attributes {expected:?}, got {rendered:?}"
        );

        // The administrative reader of the same stored user agrees.
        let administrative = ok(
            &router,
            "AdminGetUser",
            &json!({ "UserPoolId": user_pool_id, "Username": row.username }),
        )
        .await;
        let stored = attribute_map(&administrative, "AdminGetUser");
        assert_eq!(
            rendered, stored,
            "case {case:?}: GetUser and AdminGetUser must report one stored user, got {rendered:?} \
             and {stored:?}"
        );

        // The MFA members are the stored configuration, and a user with none
        // carries no empty list.
        if row.sms_mfa {
            assert_eq!(
                payload["MFAOptions"],
                json!([{ "DeliveryMedium": "SMS", "AttributeName": "phone_number" }]),
                "case {case:?}: expected the stored MFA option, got {payload}"
            );
            assert_eq!(
                payload["UserMFASettingList"],
                json!([SMS_MFA]),
                "case {case:?}: expected SMS MFA activated, got {payload}"
            );
        } else {
            for member in ["MFAOptions", "UserMFASettingList"] {
                assert!(
                    payload.get(member).is_none(),
                    "case {case:?}: a user with no MFA configuration carries no {member}, got \
                     {payload}"
                );
            }
        }
        assert!(
            payload.get("PreferredMfaSetting").is_none(),
            "case {case:?}: nothing expressed a preferred factor, got {payload}"
        );

        // A pure read: asking again answers exactly the same thing, and the
        // administrative reader still sees what the first read saw.
        let (again_status, again) = get_user(&router, &token).await;
        assert_eq!(
            (again_status, &again),
            (200, &payload),
            "case {case:?}: reading twice must answer the same stored user, got {again_status} \
             with {again}"
        );
        let after = ok(
            &router,
            "AdminGetUser",
            &json!({ "UserPoolId": user_pool_id, "Username": row.username }),
        )
        .await;
        assert_eq!(
            after, administrative,
            "case {case:?}: a read changes nothing a later caller sees, got {after}"
        );
    }
}

/// One row of the token table: a value submitted as the `AccessToken` that names
/// no signed-in user, and the refusal it owes.
#[derive(Clone, Copy, Debug)]
struct TokenCase {
    /// What this row asserts.
    case: &'static str,
    /// Which token the row submits.
    token: SubmittedToken,
    /// The HTTP status of the refusal.
    status: u16,
    /// The wire error code of the refusal.
    code: &'static str,
    /// The message the refusal reports.
    message: &'static str,
}

/// Which token one row of the table submits.
#[derive(Clone, Copy, Debug)]
enum SubmittedToken {
    /// A value satisfying `TokenModelType`'s pattern that this emulator never
    /// issued, so the request reaches the handler.
    NeverIssued,
    /// The id token of a real sign-in.
    IdToken,
    /// The refresh token of a real sign-in.
    RefreshToken,
    /// A planted record, by the token value it is planted under.
    Planted(&'static str),
}

/// An access token record planted as revoked, so the row that reads it does not
/// depend on which revoking operation wrote that state.
const REVOKED: &str = "revoked.access.token";

/// An access token record planted with a lifetime the clock is already past.
const EXPIRED: &str = "expired.access.token";

/// An access token record planted against a pool this emulator does not hold.
const POOL_GONE: &str = "pool-gone.access.token";

/// An access token record planted against a user its pool does not hold.
const USER_GONE: &str = "user-gone.access.token";

/// The rows differ along WHY the token names no signed-in user — never issued,
/// issued as a token of another kind, revoked, past its lifetime, naming a pool
/// that is gone, naming a user that is gone — and so along the error each one
/// owes.
const TOKEN_CASES: &[TokenCase] = &[
    TokenCase {
        case: "a token this emulator never issued",
        token: SubmittedToken::NeverIssued,
        status: 400,
        code: "NotAuthorizedException",
        message: "Invalid Access Token",
    },
    TokenCase {
        case: "the id token of the same sign-in, which is not the token the member asks for",
        token: SubmittedToken::IdToken,
        status: 400,
        code: "NotAuthorizedException",
        message: "Invalid Access Token",
    },
    TokenCase {
        case: "the refresh token of the same sign-in",
        token: SubmittedToken::RefreshToken,
        status: 400,
        code: "NotAuthorizedException",
        message: "Invalid Access Token",
    },
    TokenCase {
        case: "a revoked access token",
        token: SubmittedToken::Planted(REVOKED),
        status: 400,
        code: "NotAuthorizedException",
        message: "Access Token has been revoked",
    },
    TokenCase {
        case: "an access token the clock has expired",
        token: SubmittedToken::Planted(EXPIRED),
        status: 400,
        code: "NotAuthorizedException",
        message: "Access Token has expired",
    },
    TokenCase {
        case: "an access token whose user pool is gone",
        token: SubmittedToken::Planted(POOL_GONE),
        status: 400,
        code: "ResourceNotFoundException",
        message: "User pool does not exist.",
    },
    TokenCase {
        case: "an access token whose user is gone",
        token: SubmittedToken::Planted(USER_GONE),
        status: 400,
        code: "UserNotFoundException",
        message: "User does not exist.",
    },
];

#[tokio::test]
async fn get_user_refuses_a_token_that_names_no_signed_in_user() {
    let Emulator { engine, router } = emulator();
    let user_pool_id = pool(&router, "token-pool", true).await;
    let client_id = client(&router, &user_pool_id).await;
    confirmed_user(
        &router,
        &user_pool_id,
        "ada",
        &[("email", "ada@example.com")],
    )
    .await;

    let result = sign_in(&router, &client_id, "ada").await;
    let id_token = result["IdToken"].as_str().expect("an id token").to_string();
    let refresh_token = result["RefreshToken"]
        .as_str()
        .expect("a refresh token")
        .to_string();
    // The token the table must leave working: every refusal below writes
    // nothing, so this read answers the same user before and after.
    let live_token = access_token(&router, &client_id, "ada").await;
    let before = get_user(&router, &live_token).await;
    assert_eq!(
        before.0, 200,
        "the token the table must leave working has to work first, got {} with {}",
        before.0, before.1
    );

    {
        let service_state = state::service_state(&*engine).expect("this service's state");
        let mut guard = service_state.lock();
        let now = SystemTime::now();
        // The id of the stored user a planted record names, so a record naming a
        // user this pool HOLDS names that user's id too; one naming a user the
        // pool does not hold names an id it does not hold either.
        let ada_sub = guard
            .pool(&user_pool_id)
            .and_then(|pool| pool.user("ada"))
            .expect("the stored user")
            .sub
            .clone();
        let record =
            |pool: &str, username: &str, sub: &str, expires_at: SystemTime, revoked: bool| {
                state::TokenRecord {
                    user_pool_id: pool.to_string(),
                    client_id: client_id.clone(),
                    username: username.to_string(),
                    sub: sub.to_string(),
                    kind: state::TokenKind::Access,
                    scopes: vec![state::USER_ADMIN_SCOPE.to_string()],
                    // The sign-in lineage this planted record belongs to.
                    origin: "planted-sign-in".to_string(),
                    issued_at: now - Duration::from_secs(60),
                    expires_at,
                    revoked,
                    rotated_out_at: None,
                }
            };
        let live = now + Duration::from_secs(3_600);
        for (token, planted) in [
            (REVOKED, record(&user_pool_id, "ada", &ada_sub, live, true)),
            (
                EXPIRED,
                record(
                    &user_pool_id,
                    "ada",
                    &ada_sub,
                    now - Duration::from_secs(1),
                    false,
                ),
            ),
            (
                POOL_GONE,
                record("us-east-1_absent", "ada", &ada_sub, live, false),
            ),
            (
                USER_GONE,
                record(&user_pool_id, "nobody", "nobody-has-this-id", live, false),
            ),
        ] {
            guard.issued_tokens.insert(token.to_string(), planted);
        }
    }

    for row in TOKEN_CASES {
        let case = row.case;
        let token = match row.token {
            // Satisfies `TokenModelType`'s pattern, so the request reaches the
            // handler and the refusal is the handler's own.
            SubmittedToken::NeverIssued => "not-a-token-this-emulator-issued".to_string(),
            SubmittedToken::IdToken => id_token.clone(),
            SubmittedToken::RefreshToken => refresh_token.clone(),
            SubmittedToken::Planted(planted) => planted.to_string(),
        };
        let (status, payload) = get_user(&router, &token).await;
        assert_eq!(
            status, row.status,
            "case {case:?}: expected HTTP {}, got {status} with {payload}",
            row.status
        );
        assert_eq!(
            error_code(&payload),
            row.code,
            "case {case:?}: expected {}, got {payload}",
            row.code
        );
        assert_eq!(
            error_message(&payload),
            row.message,
            "case {case:?}: expected the refusal to report {:?}, got {payload}",
            row.message
        );
    }

    let after = get_user(&router, &live_token).await;
    assert_eq!(
        after, before,
        "a refused read changes nothing a later caller sees, got {} with {}",
        after.0, after.1
    );
}

#[tokio::test]
async fn an_access_token_answers_for_its_own_user_only() {
    let Emulator { router, .. } = emulator();

    // The SAME username in two pools is two users, each with its own sub, and
    // two users in one pool are two profiles: in neither case may one token
    // answer for the other user.
    let mut reads: Vec<(String, String, Value)> = Vec::new();
    for (index, pool_name) in ["crossing-pool-a", "crossing-pool-b"].iter().enumerate() {
        let user_pool_id = pool(&router, pool_name, true).await;
        let client_id = client(&router, &user_pool_id).await;
        for username in ["ada", "grace"] {
            let email = format!("{username}@{index}.example.com");
            confirmed_user(
                &router,
                &user_pool_id,
                username,
                &[("email", email.as_str())],
            )
            .await;
            let token = access_token(&router, &client_id, username).await;
            let (status, payload) = get_user(&router, &token).await;
            let case = format!("{pool_name}/{username}");
            assert_eq!(
                status, 200,
                "case {case:?}: an access token must name its user, got {status} with {payload}"
            );
            assert_eq!(
                payload["Username"],
                json!(username),
                "case {case:?}: expected the token's own user, got {payload}"
            );
            let attributes = attribute_map(&payload, "GetUser");
            assert_eq!(
                attributes.get("email").map(String::as_str),
                Some(email.as_str()),
                "case {case:?}: expected the stored email {email:?}, got {attributes:?}"
            );
            reads.push((case, token, payload));
        }
    }

    // Every read answered a distinct stored user: four distinct subs.
    let subs: std::collections::BTreeSet<String> = reads
        .iter()
        .map(|(case, _, payload)| {
            attribute_map(payload, "GetUser")
                .get("sub")
                .cloned()
                .unwrap_or_else(|| panic!("case {case:?}: a stored user carries a sub"))
        })
        .collect();
    assert_eq!(
        subs.len(),
        reads.len(),
        "each token must answer its own stored user, got {subs:?} for {} reads",
        reads.len()
    );

    // And re-reading every token in turn still answers what it answered first,
    // whatever was read in between.
    for (case, token, expected) in &reads {
        let (status, payload) = get_user(&router, token).await;
        assert_eq!(
            (status, &payload),
            (200, expected),
            "case {case:?}: a token keeps answering its own user, got {status} with {payload}"
        );
    }
}
