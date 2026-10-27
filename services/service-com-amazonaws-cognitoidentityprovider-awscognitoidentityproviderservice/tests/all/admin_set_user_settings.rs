// SPDX-License-Identifier: CC0-1.0

//! `AdminSetUserSettings` over the wire, driven exactly as an SDK drives it
//! (`POST /` with `X-Amz-Target:
//! AWSCognitoIdentityProviderService.AdminSetUserSettings`).
//!
//! The rule under test: an administrative write that names a pool and a user
//! stores the submitted `MFAOptions` as the SMS MFA configuration of that STORED
//! user, replacing the configuration the user held — so a later read of the same
//! user renders exactly what was written, SMS MFA counts as activated while an
//! option is stored for it, and an empty list leaves the user with no MFA option
//! and no activated factor at all. A submitted option that is not an SMS
//! configuration of `phone_number` — the only delivery medium and the only
//! attribute name an `MFAOptionType` may carry — is refused with 400
//! `InvalidParameterException`; a pool this emulator does not hold is refused
//! with 404 `ResourceNotFoundException` and a user that pool does not hold with
//! 404 `UserNotFoundException`. A refused request writes nothing.
//!
//! The pools, app clients, users and access tokens are all real: they come from
//! `CreateUserPool`, `CreateUserPoolClient`, `AdminCreateUser`,
//! `AdminSetUserPassword` and `InitiateAuth`, and every write is read back both
//! through the administrative `AdminGetUser` and through the signed-in user's own
//! `GetUser`.
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

/// The `UserMFASettingList` value for SMS message MFA (`AdminGetUser`'s
/// `UserMFASettingList`: "the possible values in this list are `SMS_MFA`,
/// `EMAIL_OTP`, and `SOFTWARE_TOKEN_MFA`").
const SMS_MFA: &str = "SMS_MFA";

/// The one option an `MFAOptionType` can carry: SMS delivery of `phone_number`
/// (`MFAOptionType.DeliveryMedium`: "you can use this parameter to set only the
/// `SMS` delivery medium value"; `MFAOptionType.AttributeName`: "the only valid
/// value is `phone_number`").
const SMS_OPTION: (Option<&str>, Option<&str>) = (Some("SMS"), Some("phone_number"));

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
/// address, which is how a signed-in user's client calls the self-service
/// operations this suite reads back through.
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

/// Create a pool and return its minted `UserPoolId`. A pool asked for
/// `case_sensitive` usernames gets the `UsernameConfiguration` that says so.
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

/// Create an app client that allows the password sign-in this suite uses, and
/// return its minted `ClientId`.
async fn client(router: &MultiServiceRouter, user_pool_id: &str) -> String {
    let created = ok(
        router,
        "CreateUserPoolClient",
        &json!({
            "UserPoolId": user_pool_id,
            "ClientName": "admin-settings-client",
            "ExplicitAuthFlows": ["ALLOW_USER_PASSWORD_AUTH", "ALLOW_REFRESH_TOKEN_AUTH"],
        }),
    )
    .await;
    created["UserPoolClient"]["ClientId"]
        .as_str()
        .expect("a minted client id")
        .to_string()
}

/// A pool with one app client, one signed-in user and that user's access token.
struct Fixture {
    user_pool_id: String,
    /// The username `AdminCreateUser` stored.
    username: String,
    /// The `sub` the created user holds, which names the same user.
    sub: String,
    access_token: String,
}

/// Create a pool holding `username` with `attributes`, sign that user in and
/// return the fixture.
async fn fixture(
    router: &MultiServiceRouter,
    name: &str,
    case_sensitive: bool,
    username: &str,
    attributes: &[(&str, &str)],
) -> Fixture {
    let user_pool_id = pool(router, name, case_sensitive).await;
    let client_id = client(router, &user_pool_id).await;
    let created = ok(
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
    let sub = attribute(&created["User"], "sub").unwrap_or_else(|| {
        panic!("a created user holds a sub, got {:?}", created["User"]);
    });
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
            "AuthParameters": { "USERNAME": username, "PASSWORD": GOOD_PASSWORD },
        }),
        false,
    )
    .await;
    assert_eq!(
        status, 200,
        "signing {username} in must succeed, got {status} with {payload}"
    );
    Fixture {
        user_pool_id,
        username: username.to_string(),
        sub,
        access_token: payload["AuthenticationResult"]["AccessToken"]
            .as_str()
            .expect("an access token")
            .to_string(),
    }
}

/// The value of the `name` attribute a `UserType` carries (`UserType.Attributes`
/// in this service's model, which `AdminCreateUser` answers under `User`).
fn attribute(user: &Value, name: &str) -> Option<String> {
    user["Attributes"]
        .as_array()?
        .iter()
        .find(|attribute| attribute["Name"].as_str() == Some(name))?["Value"]
        .as_str()
        .map(str::to_string)
}

/// `options` as the modelled `MFAOptionListType` an SDK sends: a member left
/// `None` is left out of the option entirely.
fn option_list(options: &[(Option<&str>, Option<&str>)]) -> Value {
    Value::Array(
        options
            .iter()
            .map(|(medium, attribute)| {
                let mut option = Map::new();
                if let Some(medium) = medium {
                    option.insert("DeliveryMedium".to_string(), json!(medium));
                }
                if let Some(attribute) = attribute {
                    option.insert("AttributeName".to_string(), json!(attribute));
                }
                Value::Object(option)
            })
            .collect(),
    )
}

/// Call `AdminSetUserSettings` as the SDK does, and return its status and body.
async fn admin_set_settings(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    username: &str,
    options: &[(Option<&str>, Option<&str>)],
) -> (u16, Value) {
    dispatch(
        router,
        "AdminSetUserSettings",
        &json!({
            "UserPoolId": user_pool_id,
            "Username": username,
            "MFAOptions": option_list(options),
        }),
        true,
    )
    .await
}

/// The MFA state one read reports, rendered from the stored user.
#[derive(Debug, PartialEq, Eq)]
struct MfaState {
    /// `MFAOptions` as `(DeliveryMedium, AttributeName)` pairs; empty where the
    /// response carries no `MFAOptions` member at all.
    options: Vec<(String, String)>,
    /// `UserMFASettingList`; empty where the response carries none.
    settings: Vec<String>,
    /// `PreferredMfaSetting`, absent where the response carries none.
    preferred: Option<String>,
}

/// The MFA state a `GetUser` / `AdminGetUser` body reports.
fn mfa_state(payload: &Value) -> MfaState {
    MfaState {
        options: payload["MFAOptions"]
            .as_array()
            .map(|options| {
                options
                    .iter()
                    .map(|option| {
                        (
                            option["DeliveryMedium"]
                                .as_str()
                                .unwrap_or_default()
                                .to_string(),
                            option["AttributeName"]
                                .as_str()
                                .unwrap_or_default()
                                .to_string(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default(),
        settings: payload["UserMFASettingList"]
            .as_array()
            .map(|settings| {
                settings
                    .iter()
                    .map(|setting| setting.as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default(),
        preferred: payload["PreferredMfaSetting"]
            .as_str()
            .map(str::to_string),
    }
}

/// The MFA state the store holds for `username`, read back through
/// `AdminGetUser`.
async fn stored_mfa_state(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    username: &str,
) -> MfaState {
    let user = ok(
        router,
        "AdminGetUser",
        &json!({ "UserPoolId": user_pool_id, "Username": username }),
    )
    .await;
    mfa_state(&user)
}

/// The MFA state the signed-in user's own `GetUser` reports.
async fn own_mfa_state(router: &MultiServiceRouter, access_token: &str) -> MfaState {
    let (status, payload) =
        dispatch(router, "GetUser", &json!({ "AccessToken": access_token }), false).await;
    assert_eq!(
        status, 200,
        "GetUser must succeed, got {status} with {payload}"
    );
    mfa_state(&payload)
}

/// How a row's `AdminSetUserSettings` call names the user it configures. Every
/// form names the SAME stored user.
#[derive(Clone, Copy, Debug)]
enum NamedBy {
    /// The username exactly as `AdminCreateUser` stored it.
    StoredUsername,
    /// The username in another case, which a case-insensitive pool resolves to
    /// the stored one.
    LowercasedUsername,
    /// The user's `sub`, which "must be the sub of a local user" where the
    /// submitted value is no alias attribute (this operation's `Username` in
    /// this service's model).
    Sub,
}

/// One `AdminSetUserSettings` call of an accepted row, and the configuration a
/// later read of the same user must then render.
#[derive(Clone, Copy, Debug)]
struct Step {
    /// What the call submits in `MFAOptions`.
    submitted: &'static [(Option<&'static str>, Option<&'static str>)],
    /// The `MFAOptions` a later read must render, as `(DeliveryMedium,
    /// AttributeName)` pairs.
    options: &'static [(&'static str, &'static str)],
    /// The `UserMFASettingList` a later read must render.
    settings: &'static [&'static str],
}

/// One row of the accepted-configuration table: a user, and the calls applied to
/// it in order.
#[derive(Clone, Copy, Debug)]
struct AcceptedCase {
    /// What this row asserts.
    case: &'static str,
    /// Whether the pool compares usernames case-sensitively.
    case_sensitive: bool,
    /// The username `AdminCreateUser` creates.
    username: &'static str,
    /// How the administrative call names that user.
    named_by: NamedBy,
    /// The attributes the user already holds.
    attributes: &'static [(&'static str, &'static str)],
    /// The calls this row makes, in order.
    steps: &'static [Step],
}

/// The rows differ along the COUNT of options one call submits (one, the same
/// one twice, none), the ORDER of calls over one user (activate, deactivate,
/// activate again), whether the user HOLDS a phone number at all, and the FORM
/// the request names the user by (the stored username, another case of it, the
/// user's `sub`).
const ACCEPTED_CASES: &[AcceptedCase] = &[
    AcceptedCase {
        case: "one SMS option activates SMS MFA for a user holding a phone number",
        case_sensitive: true,
        username: "ada",
        named_by: NamedBy::StoredUsername,
        attributes: &[("phone_number", "+15550100001")],
        steps: &[Step {
            submitted: &[SMS_OPTION],
            options: &[("SMS", "phone_number")],
            settings: &[SMS_MFA],
        }],
    },
    AcceptedCase {
        case: "a user holding no phone number takes the same configuration",
        case_sensitive: true,
        username: "grace",
        named_by: NamedBy::StoredUsername,
        attributes: &[],
        steps: &[Step {
            submitted: &[SMS_OPTION],
            options: &[("SMS", "phone_number")],
            settings: &[SMS_MFA],
        }],
    },
    AcceptedCase {
        case: "the same option submitted twice is one stored configuration",
        case_sensitive: true,
        username: "edsger",
        named_by: NamedBy::StoredUsername,
        attributes: &[("phone_number", "+15550100002")],
        steps: &[Step {
            submitted: &[SMS_OPTION, SMS_OPTION],
            options: &[("SMS", "phone_number")],
            settings: &[SMS_MFA],
        }],
    },
    AcceptedCase {
        case: "an empty list deactivates what a previous call set, and a later call sets it again",
        case_sensitive: true,
        username: "alan",
        named_by: NamedBy::StoredUsername,
        attributes: &[("phone_number", "+15550100003")],
        steps: &[
            Step {
                submitted: &[SMS_OPTION],
                options: &[("SMS", "phone_number")],
                settings: &[SMS_MFA],
            },
            Step {
                submitted: &[],
                options: &[],
                settings: &[],
            },
            Step {
                submitted: &[SMS_OPTION],
                options: &[("SMS", "phone_number")],
                settings: &[SMS_MFA],
            },
        ],
    },
    AcceptedCase {
        case: "a call that cases the username differently configures the stored user of a \
               case-insensitive pool",
        case_sensitive: false,
        username: "Barbara",
        named_by: NamedBy::LowercasedUsername,
        attributes: &[("phone_number", "+15550100004")],
        steps: &[Step {
            submitted: &[SMS_OPTION],
            options: &[("SMS", "phone_number")],
            settings: &[SMS_MFA],
        }],
    },
    AcceptedCase {
        case: "a call that names the user by its sub configures the same stored user",
        case_sensitive: true,
        username: "donald",
        named_by: NamedBy::Sub,
        attributes: &[("phone_number", "+15550100005")],
        steps: &[Step {
            submitted: &[SMS_OPTION],
            options: &[("SMS", "phone_number")],
            settings: &[SMS_MFA],
        }],
    },
];

#[tokio::test]
async fn admin_set_user_settings_stores_the_submitted_sms_mfa_configuration() {
    let router = emulator();
    for (index, row) in ACCEPTED_CASES.iter().enumerate() {
        let case = row.case;
        let user = fixture(
            &router,
            &format!("admin-accepted-pool-{index}"),
            row.case_sensitive,
            row.username,
            row.attributes,
        )
        .await;
        let named = match row.named_by {
            NamedBy::StoredUsername => user.username.clone(),
            NamedBy::LowercasedUsername => user.username.to_lowercase(),
            NamedBy::Sub => user.sub.clone(),
        };

        for (step_index, step) in row.steps.iter().enumerate() {
            let (status, payload) =
                admin_set_settings(&router, &user.user_pool_id, &named, step.submitted).await;
            assert_eq!(
                status, 200,
                "case {case:?} step {step_index}: AdminSetUserSettings must succeed, got \
                 {status} with {payload}"
            );
            assert!(
                payload.is_null() || payload.as_object().is_some_and(Map::is_empty),
                "case {case:?} step {step_index}: AdminSetUserSettings models no output member, \
                 got {payload:?}"
            );

            let expected = MfaState {
                options: step
                    .options
                    .iter()
                    .map(|(medium, attribute)| ((*medium).to_string(), (*attribute).to_string()))
                    .collect(),
                settings: step.settings.iter().map(|s| (*s).to_string()).collect(),
                // Nothing in this input expresses a preference, so no factor is
                // reported as the preferred one.
                preferred: None,
            };
            let stored = stored_mfa_state(&router, &user.user_pool_id, &user.username).await;
            assert_eq!(
                stored, expected,
                "case {case:?} step {step_index}: AdminGetUser must render the stored \
                 configuration, got {stored:?}"
            );
            let own = own_mfa_state(&router, &user.access_token).await;
            assert_eq!(
                own, expected,
                "case {case:?} step {step_index}: the configured user's own GetUser must render \
                 the stored configuration, got {own:?}"
            );
        }
    }
}

/// One row of the refusal table: a submitted list that is no SMS MFA
/// configuration.
#[derive(Clone, Copy, Debug)]
struct RefusedCase {
    /// What this row asserts.
    case: &'static str,
    /// What the call submits in `MFAOptions`.
    submitted: &'static [(Option<&'static str>, Option<&'static str>)],
    /// Text the refusal's message must carry: the member the submitted option
    /// cannot carry, and the submitted value of it.
    message: &'static str,
}

/// The rows differ along WHICH member is wrong (the delivery medium, the
/// attribute name), whether it is wrong or absent, the kind of value submitted
/// for it (the other modelled `DeliveryMediumType` value, another standard
/// attribute, a custom one), and the POSITION of the bad option in the list.
const REFUSED_CASES: &[RefusedCase] = &[
    RefusedCase {
        case: "the other modelled delivery medium is not one an MFA option can set",
        submitted: &[(Some("EMAIL"), Some("email"))],
        message: "Invalid DeliveryMedium: EMAIL",
    },
    RefusedCase {
        case: "a non-SMS medium is refused even beside the one valid attribute name",
        submitted: &[(Some("EMAIL"), Some("phone_number"))],
        message: "Invalid DeliveryMedium: EMAIL",
    },
    RefusedCase {
        case: "an option that sets no delivery medium configures no SMS delivery",
        submitted: &[(None, Some("phone_number"))],
        message: "an MFA option must set SMS as its DeliveryMedium",
    },
    RefusedCase {
        case: "another standard attribute is not one an MFA option can name",
        submitted: &[(Some("SMS"), Some("email"))],
        message: "Invalid AttributeName: email",
    },
    RefusedCase {
        case: "a custom attribute is not one an MFA option can name either",
        submitted: &[(Some("SMS"), Some("custom:mobile"))],
        message: "Invalid AttributeName: custom:mobile",
    },
    RefusedCase {
        case: "an option that names no attribute names none an MFA code is sent for",
        submitted: &[(Some("SMS"), None)],
        message: "an MFA option must name phone_number as its AttributeName",
    },
    RefusedCase {
        case: "a bad option refuses the whole list, whatever precedes it",
        submitted: &[SMS_OPTION, (Some("SMS"), Some("address"))],
        message: "Invalid AttributeName: address",
    },
];

#[tokio::test]
async fn admin_set_user_settings_refuses_an_option_that_is_no_sms_configuration() {
    let router = emulator();
    let user = fixture(
        &router,
        "admin-refused-pool",
        true,
        "ada",
        &[("phone_number", "+15550100006")],
    )
    .await;
    // The configuration every refusal below has to leave standing.
    let (status, payload) = admin_set_settings(
        &router,
        &user.user_pool_id,
        &user.username,
        &[SMS_OPTION],
    )
    .await;
    assert_eq!(
        status, 200,
        "the configuration a refusal must leave standing has to be set first, got {status} with \
         {payload}"
    );
    let configured = MfaState {
        options: vec![("SMS".to_string(), "phone_number".to_string())],
        settings: vec![SMS_MFA.to_string()],
        preferred: None,
    };

    for row in REFUSED_CASES {
        let case = row.case;
        let (status, payload) =
            admin_set_settings(&router, &user.user_pool_id, &user.username, row.submitted).await;
        assert_eq!(
            status, 400,
            "case {case:?}: the refusal is a 400, got {status} with {payload}"
        );
        let code = error_code(&payload);
        assert!(
            code.ends_with("InvalidParameterException"),
            "case {case:?}: expected InvalidParameterException, got {code:?} with {payload}"
        );
        let message = error_message(&payload);
        assert!(
            message.contains(row.message),
            "case {case:?}: expected a message carrying {:?}, got {message:?}",
            row.message
        );
        let stored = stored_mfa_state(&router, &user.user_pool_id, &user.username).await;
        assert_eq!(
            stored, configured,
            "case {case:?}: a refused request writes nothing, got {stored:?}"
        );
    }
}

/// One row of the absent-subject table: a pool or a user the store does not
/// hold.
#[derive(Clone, Copy, Debug)]
struct AbsentCase {
    /// What this row asserts.
    case: &'static str,
    /// Whether the call names the pool the fixture created; otherwise it names a
    /// pool id this emulator never minted.
    existing_pool: bool,
    /// The username the call names, where it is not the stored one.
    username: &'static str,
    /// The error code the refusal must carry.
    code: &'static str,
    /// The expected status of the refusal.
    status: u16,
}

/// A well-formed pool id (`UserPoolIdType`'s pattern) that this emulator never
/// minted, so it names no stored pool.
const ABSENT_POOL_ID: &str = "us-east-1_absentpool";

/// The rows differ along WHICH subject is absent (the pool, the user) and the
/// FORM the absent user is named by (a username, a value shaped like a `sub`).
const ABSENT_CASES: &[AbsentCase] = &[
    AbsentCase {
        case: "a pool id this emulator never minted",
        existing_pool: false,
        username: "ada",
        code: "ResourceNotFoundException",
        status: 404,
    },
    AbsentCase {
        case: "a username the pool does not hold",
        existing_pool: true,
        username: "nobody",
        code: "UserNotFoundException",
        status: 404,
    },
    AbsentCase {
        case: "a sub no user of the pool holds",
        existing_pool: true,
        username: "11111111-2222-3333-4444-555555555555",
        code: "UserNotFoundException",
        status: 404,
    },
];

#[tokio::test]
async fn admin_set_user_settings_refuses_a_pool_or_user_the_store_does_not_hold() {
    let router = emulator();
    let user = fixture(
        &router,
        "admin-absent-pool",
        true,
        "ada",
        &[("phone_number", "+15550100007")],
    )
    .await;

    for row in ABSENT_CASES {
        let case = row.case;
        let user_pool_id = if row.existing_pool {
            user.user_pool_id.clone()
        } else {
            ABSENT_POOL_ID.to_string()
        };
        let (status, payload) =
            admin_set_settings(&router, &user_pool_id, row.username, &[SMS_OPTION]).await;
        assert_eq!(
            status, row.status,
            "case {case:?}: expected {} , got {status} with {payload}",
            row.status
        );
        let code = error_code(&payload);
        assert!(
            code.ends_with(row.code),
            "case {case:?}: expected {:?}, got {code:?} with {payload}",
            row.code
        );
        // The user the call would have configured is still configured by
        // nothing.
        let stored = stored_mfa_state(&router, &user.user_pool_id, &user.username).await;
        assert_eq!(
            stored,
            MfaState {
                options: Vec::new(),
                settings: Vec::new(),
                preferred: None,
            },
            "case {case:?}: a refused request writes nothing, got {stored:?}"
        );
    }
}
