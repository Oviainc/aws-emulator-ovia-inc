// SPDX-License-Identifier: CC0-1.0

//! `GetUserAuthFactors` over the wire, driven exactly as an SDK drives it (`POST /`
//! with `X-Amz-Target: AWSCognitoIdentityProviderService.GetUserAuthFactors`).
//!
//! The rule under test: the operation reports, for the user its `AccessToken` was
//! issued to, the MFA settings that user holds and the `AuthFactorType` values that
//! user can authenticate with in a choice-based (`USER_AUTH`) sign-in — a factor the
//! pool admits as a first authentication factor and the user holds what it needs
//! for (a password, a verified address a one-time password is delivered to, a
//! registered passkey), or a factor the user has activated as an MFA factor. A
//! factor neither half offers is not reported, the reported list is in the order
//! the model declares `AuthFactorType`'s values in rather than the order the pool
//! lists them in, and the whole answer is computed from the STORED pool and the
//! STORED user, so asking twice answers the same and asking changes nothing a
//! later caller sees.
//!
//! The pools, app clients, users and access tokens are all real: they come from
//! `CreateUserPool`, `CreateUserPoolClient`, `AdminCreateUser`,
//! `AdminSetUserPassword`, `SetUserSettings` and `InitiateAuth`.
//!
//! Each table row names the case it asserts.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice as cognito_idp;

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

/// The `UserMFASettingList` value for SMS message MFA (`GetUserAuthFactors`'s
/// `UserMFASettingList`: "the possible values in this list are `SMS_MFA`,
/// `EMAIL_OTP`, and `SOFTWARE_TOKEN_MFA`").
const SMS_MFA: &str = "SMS_MFA";

/// `AuthFactorType`'s values, as `smithy_query(query="shape",
/// shape="AuthFactorType")` lists them. Every row below expects a subset of these
/// and nothing else.
const PASSWORD: &str = "PASSWORD";
const EMAIL_OTP: &str = "EMAIL_OTP";
const SMS_OTP: &str = "SMS_OTP";
const WEB_AUTHN: &str = "WEB_AUTHN";
const SOFTWARE_TOKEN: &str = "SOFTWARE_TOKEN";

/// One emulator with just this service mounted.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(cognito_idp::EmulatedService::new(engine));
    router.set_request_authentication(false);
    router
}

/// Dispatch `operation` with `body` and return its status and JSON response
/// body. A `signed` call carries a credential scope, as the SDK sends every
/// administrative operation; an unsigned one is addressed to the emulator's own
/// address, which is how a signed-in user's client calls `GetUserAuthFactors` —
/// the model marks the operation `@optionalAuth`.
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

/// Create a pool and return its minted `UserPoolId`. `first_auth_factors` is the
/// `Policies.SignInPolicy.AllowedFirstAuthFactors` the pool is created with, left
/// unset when empty; a pool asked for case-insensitive usernames gets the
/// `UsernameConfiguration` that says so.
async fn pool(
    router: &MultiServiceRouter,
    name: &str,
    case_sensitive: bool,
    first_auth_factors: &[&str],
) -> String {
    let mut body = Map::new();
    body.insert("PoolName".to_string(), json!(name));
    if !first_auth_factors.is_empty() {
        body.insert(
            "Policies".to_string(),
            json!({ "SignInPolicy": { "AllowedFirstAuthFactors": first_auth_factors } }),
        );
    }
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

/// Create an app client that allows the password sign-in this suite uses, and
/// return its minted `ClientId`.
async fn client(router: &MultiServiceRouter, user_pool_id: &str) -> String {
    let created = ok(
        router,
        "CreateUserPoolClient",
        &json!({
            "UserPoolId": user_pool_id,
            "ClientName": "factors-client",
            "ExplicitAuthFlows": ["ALLOW_USER_PASSWORD_AUTH", "ALLOW_REFRESH_TOKEN_AUTH"],
        }),
    )
    .await;
    created["UserPoolClient"]["ClientId"]
        .as_str()
        .expect("a minted client id")
        .to_string()
}

/// A pool with one app client, one signed-in user and that user's tokens.
struct Fixture {
    user_pool_id: String,
    username: String,
    access_token: String,
    id_token: String,
}

/// Create a pool holding `username` with `attributes`, sign that user in as
/// `signed_in_as` and return the fixture.
async fn fixture(
    router: &MultiServiceRouter,
    name: &str,
    case_sensitive: bool,
    first_auth_factors: &[&str],
    username: &str,
    signed_in_as: &str,
    attributes: &[(&str, &str)],
) -> Fixture {
    let user_pool_id = pool(router, name, case_sensitive, first_auth_factors).await;
    let client_id = client(router, &user_pool_id).await;
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
    let (status, payload) = dispatch(
        router,
        "InitiateAuth",
        &json!({
            "ClientId": client_id,
            "AuthFlow": "USER_PASSWORD_AUTH",
            "AuthParameters": { "USERNAME": signed_in_as, "PASSWORD": GOOD_PASSWORD },
        }),
        false,
    )
    .await;
    assert_eq!(
        status, 200,
        "signing {signed_in_as} in must succeed, got {status} with {payload}"
    );
    let result = &payload["AuthenticationResult"];
    Fixture {
        user_pool_id,
        username: username.to_string(),
        access_token: result["AccessToken"]
            .as_str()
            .expect("an access token")
            .to_string(),
        id_token: result["IdToken"].as_str().expect("an id token").to_string(),
    }
}

/// What one `GetUserAuthFactors` answer reports.
#[derive(Debug, PartialEq, Eq)]
struct Factors {
    username: String,
    /// `ConfiguredUserAuthFactors`; empty where the response carries none.
    configured: Vec<String>,
    /// `UserMFASettingList`; empty where the response carries none.
    settings: Vec<String>,
    /// `PreferredMfaSetting`, absent where the response carries none.
    preferred: Option<String>,
}

/// The string members of `payload[member]`, empty when the member is absent.
fn string_list(payload: &Value, member: &str) -> Vec<String> {
    payload[member]
        .as_array()
        .map(|values| {
            values
                .iter()
                .map(|value| value.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// Call `GetUserAuthFactors` unsigned, as a signed-in user's client does, and
/// return what it reported. Requires success.
async fn factors(router: &MultiServiceRouter, access_token: &str) -> Factors {
    let (status, payload) = dispatch(
        router,
        "GetUserAuthFactors",
        &json!({ "AccessToken": access_token }),
        false,
    )
    .await;
    assert_eq!(
        status, 200,
        "GetUserAuthFactors must succeed, got {status} with {payload}"
    );
    Factors {
        username: payload["Username"]
            .as_str()
            .unwrap_or("<no Username>")
            .to_string(),
        configured: string_list(&payload, "ConfiguredUserAuthFactors"),
        settings: string_list(&payload, "UserMFASettingList"),
        preferred: payload["PreferredMfaSetting"]
            .as_str()
            .map(str::to_string),
    }
}

/// One row of the reported-factors table.
#[derive(Clone, Copy, Debug)]
struct Case {
    /// What this row asserts.
    case: &'static str,
    /// The `AllowedFirstAuthFactors` the pool is created with; empty leaves the
    /// sign-in policy unset, which admits `PASSWORD` alone.
    first_auth_factors: &'static [&'static str],
    /// The attributes `AdminCreateUser` gives the user.
    attributes: &'static [(&'static str, &'static str)],
    /// Whether `SetUserSettings` activates SMS MFA for the user first.
    activate_sms_mfa: bool,
    /// The `WebAuthnConfiguration.FactorConfiguration`
    /// `SetUserPoolMfaConfig` puts on the pool first, when the row sets one.
    passkey_mfa: Option<&'static str>,
    /// The `ConfiguredUserAuthFactors` the answer must carry, in order.
    configured: &'static [&'static str],
    /// The `UserMFASettingList` the answer must carry.
    settings: &'static [&'static str],
}

/// The rows differ along WHICH factors the pool admits and in which order it
/// lists them, HOW MANY of them the user is set up for, WHAT the user holds (a
/// verified address, an unverified one, none at all, no passkey), whether passkey
/// MFA is enabled at the pool level, and whether the factor is offered as a first
/// factor or activated as an MFA factor.
const CASES: &[Case] = &[
    Case {
        case: "a pool that configures no sign-in policy offers the password factor alone",
        first_auth_factors: &[],
        attributes: &[],
        activate_sms_mfa: false,
        passkey_mfa: None,
        configured: &[PASSWORD],
        settings: &[],
    },
    Case {
        case: "a verified email makes the email one-time-password factor available",
        first_auth_factors: &[PASSWORD, EMAIL_OTP],
        attributes: &[("email", "ada@example.com"), ("email_verified", "true")],
        activate_sms_mfa: false,
        passkey_mfa: None,
        configured: &[PASSWORD, EMAIL_OTP],
        settings: &[],
    },
    Case {
        case: "an unverified email is no address a one-time password is delivered to",
        first_auth_factors: &[PASSWORD, EMAIL_OTP],
        attributes: &[("email", "grace@example.com")],
        activate_sms_mfa: false,
        passkey_mfa: None,
        configured: &[PASSWORD],
        settings: &[],
    },
    Case {
        case: "a user holding no email at all is not offered that factor either",
        first_auth_factors: &[PASSWORD, EMAIL_OTP],
        attributes: &[],
        activate_sms_mfa: false,
        passkey_mfa: None,
        configured: &[PASSWORD],
        settings: &[],
    },
    Case {
        case: "a verified phone number makes the SMS one-time-password factor available",
        first_auth_factors: &[PASSWORD, SMS_OTP],
        attributes: &[
            ("phone_number", "+15550100001"),
            ("phone_number_verified", "true"),
        ],
        activate_sms_mfa: false,
        passkey_mfa: None,
        configured: &[PASSWORD, SMS_OTP],
        settings: &[],
    },
    Case {
        case: "a factor the pool does not admit is not offered though the user holds what it \
               needs",
        first_auth_factors: &[],
        attributes: &[
            ("email", "edsger@example.com"),
            ("email_verified", "true"),
            ("phone_number", "+15550100002"),
            ("phone_number_verified", "true"),
        ],
        activate_sms_mfa: false,
        passkey_mfa: None,
        configured: &[PASSWORD],
        settings: &[],
    },
    Case {
        case: "the passkey factor needs a registered passkey, which this user has none of",
        first_auth_factors: &[PASSWORD, WEB_AUTHN],
        attributes: &[("email", "alan@example.com"), ("email_verified", "true")],
        activate_sms_mfa: false,
        passkey_mfa: None,
        configured: &[PASSWORD],
        settings: &[],
    },
    Case {
        case: "passkey MFA enabled at the pool offers no passkey factor to a user holding no \
               passkey",
        first_auth_factors: &[PASSWORD],
        attributes: &[],
        activate_sms_mfa: false,
        passkey_mfa: Some("MULTI_FACTOR_WITH_USER_VERIFICATION"),
        configured: &[PASSWORD],
        settings: &[],
    },
    Case {
        case: "passkeys configured as a single factor are no MFA factor of the pool at all",
        first_auth_factors: &[PASSWORD],
        attributes: &[],
        activate_sms_mfa: false,
        passkey_mfa: Some("SINGLE_FACTOR"),
        configured: &[PASSWORD],
        settings: &[],
    },
    Case {
        case: "the software token factor is an MFA factor only, so no sign-in policy entry \
               offers it",
        first_auth_factors: &[PASSWORD, SOFTWARE_TOKEN],
        attributes: &[],
        activate_sms_mfa: false,
        passkey_mfa: None,
        configured: &[PASSWORD],
        settings: &[],
    },
    Case {
        case: "every admitted factor the user is set up for is reported, in the order the model \
               declares them and not the order the pool lists them",
        first_auth_factors: &[SMS_OTP, EMAIL_OTP, PASSWORD],
        attributes: &[
            ("email", "barbara@example.com"),
            ("email_verified", "true"),
            ("phone_number", "+15550100003"),
            ("phone_number_verified", "true"),
        ],
        activate_sms_mfa: false,
        passkey_mfa: None,
        configured: &[PASSWORD, EMAIL_OTP, SMS_OTP],
        settings: &[],
    },
    Case {
        case: "an activated MFA factor is available to the user though the pool admits it as no \
               first factor",
        first_auth_factors: &[],
        attributes: &[
            ("phone_number", "+15550100004"),
            ("phone_number_verified", "true"),
        ],
        activate_sms_mfa: true,
        passkey_mfa: None,
        configured: &[PASSWORD, SMS_OTP],
        settings: &[SMS_MFA],
    },
];

#[tokio::test]
async fn get_user_auth_factors_reports_the_factors_the_stored_user_can_sign_in_with() {
    let router = emulator();
    for (index, row) in CASES.iter().enumerate() {
        let case = row.case;
        let user = fixture(
            &router,
            &format!("factors-pool-{index}"),
            true,
            row.first_auth_factors,
            "ada",
            "ada",
            row.attributes,
        )
        .await;
        if let Some(factor_configuration) = row.passkey_mfa {
            // `WebAuthnConfigurationType.FactorConfiguration` is what enables
            // passkey MFA at the user pool level; a relying party and a
            // user-verification requirement go with it.
            ok(
                &router,
                "SetUserPoolMfaConfig",
                &json!({
                    "UserPoolId": user.user_pool_id,
                    "WebAuthnConfiguration": {
                        "RelyingPartyId": "auth.example.com",
                        "UserVerification": "required",
                        "FactorConfiguration": factor_configuration,
                    },
                }),
            )
            .await;
        }
        if row.activate_sms_mfa {
            let (status, payload) = dispatch(
                &router,
                "SetUserSettings",
                &json!({
                    "AccessToken": user.access_token,
                    "MFAOptions": [{ "DeliveryMedium": "SMS", "AttributeName": "phone_number" }],
                }),
                false,
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: activating SMS MFA must succeed, got {status} with {payload}"
            );
        }

        let expected = Factors {
            username: user.username.clone(),
            configured: row.configured.iter().map(|f| (*f).to_string()).collect(),
            settings: row.settings.iter().map(|s| (*s).to_string()).collect(),
            // Nothing in these inputs expresses a preference, so no factor is
            // reported as the preferred one.
            preferred: None,
        };
        let reported = factors(&router, &user.access_token).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn get_user_auth_factors_is_a_pure_read_of_the_stored_user() {
    let router = emulator();
    let user = fixture(
        &router,
        "pure-read-pool",
        false,
        &[PASSWORD, EMAIL_OTP],
        "Barbara",
        "barbara",
        &[("email", "barbara@example.com"), ("email_verified", "true")],
    )
    .await;

    // The signed-in user is the STORED one, whatever casing the sign-in used.
    let first = factors(&router, &user.access_token).await;
    assert_eq!(
        first.username, user.username,
        "case \"the stored username is reported\": expected {:?}, got {:?}",
        user.username, first.username
    );
    assert_eq!(
        first.configured,
        vec![PASSWORD.to_string(), EMAIL_OTP.to_string()],
        "case \"both admitted factors are reported\": got {:?}",
        first.configured
    );

    // Asking again answers the same, and the user the first call read is
    // unchanged: a read moves no setting and consumes nothing.
    let second = factors(&router, &user.access_token).await;
    assert_eq!(
        second, first,
        "case \"asking twice answers the same\": expected {first:?}, got {second:?}"
    );
    let stored = ok(
        &router,
        "AdminGetUser",
        &json!({ "UserPoolId": user.user_pool_id, "Username": user.username }),
    )
    .await;
    assert!(
        stored["UserMFASettingList"].is_null() && stored["PreferredMfaSetting"].is_null(),
        "case \"a read activates no MFA factor\": got {stored}"
    );
}

/// One row of the token table: a value submitted as the `AccessToken` that names
/// no signed-in user.
#[derive(Clone, Copy, Debug)]
struct TokenCase {
    /// What this row asserts.
    case: &'static str,
    /// Whether the row submits the sign-in's id token; otherwise it submits a
    /// value this emulator never issued at all.
    id_token: bool,
    /// The message the refusal must carry.
    message: &'static str,
}

/// The rows differ along WHY the token names no signed-in user: it was never
/// issued at all, or it was issued as a token of another kind.
const TOKEN_CASES: &[TokenCase] = &[
    TokenCase {
        case: "a token this emulator never issued",
        id_token: false,
        message: "Invalid Access Token",
    },
    TokenCase {
        case: "the sign-in's id token, which is not the access token the member asks for",
        id_token: true,
        message: "Invalid Access Token",
    },
];

#[tokio::test]
async fn get_user_auth_factors_refuses_a_token_that_names_no_signed_in_user() {
    let router = emulator();
    let user = fixture(
        &router,
        "token-pool",
        true,
        &[PASSWORD],
        "ada",
        "ada",
        &[("email", "ada@example.com"), ("email_verified", "true")],
    )
    .await;

    for row in TOKEN_CASES {
        let case = row.case;
        let token = if row.id_token {
            user.id_token.clone()
        } else {
            // Satisfies `TokenModelType`'s pattern, so the request reaches the
            // handler and the refusal is the handler's own.
            "not-a-token-this-emulator-issued".to_string()
        };
        let (status, payload) = dispatch(
            &router,
            "GetUserAuthFactors",
            &json!({ "AccessToken": token }),
            false,
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: the refusal is a 400, got {status} with {payload}"
        );
        let code = error_code(&payload);
        assert!(
            code.ends_with("NotAuthorizedException"),
            "case {case:?}: expected NotAuthorizedException, got {code:?} with {payload}"
        );
        let message = error_message(&payload);
        assert_eq!(
            message, row.message,
            "case {case:?}: expected the refusal to report {:?}, got {message:?}",
            row.message
        );
    }
}
