// SPDX-License-Identifier: CC0-1.0

//! `UpdateUserAttributes` over the wire, driven exactly as an SDK drives it
//! (`POST /` with `X-Amz-Target:
//! AWSCognitoIdentityProviderService.UpdateUserAttributes`).
//!
//! The rule under test: a self-service write whose only subject is the
//! `AccessToken` applies the submitted name/value pairs to the STORED user that
//! token was issued to — a value the pool's schema declares is stored as sent
//! and a blank value deletes the attribute — so a later read of that user
//! renders exactly what was written. A new value of an attribute the pool
//! verifies (`email`, `phone_number`) is no longer the value its verified flag
//! vouched for, so the flag goes false unless the request sets it itself, and
//! the reply reports one `CodeDeliveryDetails` per attribute a verification
//! message goes to; a pool that requires verification BEFORE the update holds
//! the new value pending and keeps the confirmed one. An attribute the schema
//! does not declare, one it declares immutable and blanking one it marks
//! required are all refused with 400 `InvalidParameterException`, a value
//! another user already holds as an alias no verified flag can take away from it
//! with 400 `AliasExistsException` — while one a verified flag CAN moves to the
//! taker and demotes the previous holder — and a token that names no signed-in
//! user with 400 `NotAuthorizedException` — and a refused request writes
//! nothing.
//!
//! The pools, app clients, users and access tokens are all real: they come from
//! `CreateUserPool`, `CreateUserPoolClient`, `AdminCreateUser`,
//! `AdminSetUserPassword` and `InitiateAuth`, and every update is read back
//! through `AdminGetUser`.
//!
//! Each table row names the case it asserts.

use std::collections::BTreeMap;
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
/// address, which is how a signed-in user's client calls
/// `UpdateUserAttributes` — the model marks the operation `@optionalAuth`.
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

/// How a pool this suite creates is configured. A row that leaves a list empty
/// creates a pool that does not configure it at all.
#[derive(Clone, Copy, Debug, Default)]
struct PoolConfig {
    /// `CreateUserPool.AutoVerifiedAttributes` — the attributes the pool
    /// verifies itself.
    auto_verified: &'static [&'static str],
    /// `UserAttributeUpdateSettings.AttributesRequireVerificationBeforeUpdate`
    /// — the attributes whose new value waits for a confirmation.
    verify_before_update: &'static [&'static str],
    /// `CreateUserPool.AliasAttributes` — the attributes a sign-in name
    /// resolves through.
    alias: &'static [&'static str],
    /// Names added to `CreateUserPool.Schema`, which the pool stores under the
    /// `custom:` prefix.
    custom_schema: &'static [&'static str],
    /// Standard attribute names the pool's schema marks required.
    required: &'static [&'static str],
}

/// Create a pool configured as `config` asks and return its minted
/// `UserPoolId`.
async fn pool(router: &MultiServiceRouter, name: &str, config: PoolConfig) -> String {
    let mut body = Map::new();
    body.insert("PoolName".to_string(), json!(name));
    if !config.auto_verified.is_empty() {
        body.insert(
            "AutoVerifiedAttributes".to_string(),
            json!(config.auto_verified),
        );
    }
    if !config.verify_before_update.is_empty() {
        body.insert(
            "UserAttributeUpdateSettings".to_string(),
            json!({
                "AttributesRequireVerificationBeforeUpdate": config.verify_before_update,
            }),
        );
    }
    if !config.alias.is_empty() {
        body.insert("AliasAttributes".to_string(), json!(config.alias));
    }
    let mut schema: Vec<Value> = config
        .custom_schema
        .iter()
        .map(|attribute| {
            json!({ "Name": attribute, "AttributeDataType": "String", "Mutable": true })
        })
        .collect();
    schema.extend(config.required.iter().map(|attribute| {
        json!({
            "Name": attribute,
            "AttributeDataType": "String",
            "Mutable": true,
            "Required": true,
        })
    }));
    if !schema.is_empty() {
        body.insert("Schema".to_string(), Value::Array(schema));
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
            "ClientName": "attribute-client",
            "ExplicitAuthFlows": ["ALLOW_USER_PASSWORD_AUTH", "ALLOW_REFRESH_TOKEN_AUTH"],
        }),
    )
    .await;
    created["UserPoolClient"]["ClientId"]
        .as_str()
        .expect("a minted client id")
        .to_string()
}

/// Create `username` holding `attributes`, with a PERMANENT password so the
/// user is `CONFIRMED` and can sign in.
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
            "UserAttributes": attribute_list(attributes),
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

/// `attributes` as the modelled `AttributeListType` an SDK sends.
fn attribute_list(attributes: &[(&str, &str)]) -> Value {
    Value::Array(
        attributes
            .iter()
            .map(|(name, value)| json!({ "Name": name, "Value": value }))
            .collect(),
    )
}

/// Sign `username` in through an unsigned `InitiateAuth` and return the
/// `AuthenticationResult` it answered.
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
        "signing {username} in must succeed, got {status} with {payload}"
    );
    payload["AuthenticationResult"].clone()
}

/// A pool with one app client, one signed-in user and that user's access token.
struct Fixture {
    user_pool_id: String,
    client_id: String,
    access_token: String,
}

/// Create a pool configured as `config` asks, holding `username` with
/// `attributes`, and sign that user in.
async fn fixture(
    router: &MultiServiceRouter,
    name: &str,
    config: PoolConfig,
    username: &str,
    attributes: &[(&str, &str)],
) -> Fixture {
    let user_pool_id = pool(router, name, config).await;
    let client_id = client(router, &user_pool_id).await;
    confirmed_user(router, &user_pool_id, username, attributes).await;
    let access_token = sign_in(router, &client_id, username).await["AccessToken"]
        .as_str()
        .expect("an access token")
        .to_string();
    Fixture {
        user_pool_id,
        client_id,
        access_token,
    }
}

/// Call `UpdateUserAttributes` unsigned, as a signed-in user's client does, and
/// return its status and body.
async fn update(
    router: &MultiServiceRouter,
    access_token: &str,
    attributes: &[(&str, &str)],
) -> (u16, Value) {
    dispatch(
        router,
        "UpdateUserAttributes",
        &json!({
            "AccessToken": access_token,
            "UserAttributes": attribute_list(attributes),
        }),
        false,
    )
    .await
}

/// The attributes the store holds for `username`, read back through
/// `AdminGetUser`.
async fn stored_attributes(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    username: &str,
) -> BTreeMap<String, String> {
    let user = ok(
        router,
        "AdminGetUser",
        &json!({ "UserPoolId": user_pool_id, "Username": username }),
    )
    .await;
    user["UserAttributes"]
        .as_array()
        .expect("AdminGetUser reports the stored attributes")
        .iter()
        .map(|attribute| {
            (
                attribute["Name"]
                    .as_str()
                    .expect("an attribute name")
                    .to_string(),
                attribute["Value"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// The `CodeDeliveryDetailsList` of one reply as
/// `(AttributeName, DeliveryMedium, Destination)` triples.
///
/// A reply that sent no message carries no list at all, so an absent member
/// reads as no deliveries rather than as a malformed reply.
fn deliveries_of(payload: &Value, case: &str) -> Vec<(String, String, String)> {
    let entries = match payload["CodeDeliveryDetailsList"].as_array() {
        Some(entries) => entries.clone(),
        None => {
            assert!(
                payload["CodeDeliveryDetailsList"].is_null(),
                "case {case:?}: CodeDeliveryDetailsList must be a list or absent, got {payload}"
            );
            Vec::new()
        }
    };
    entries
        .iter()
        .map(|details| {
            (
                details["AttributeName"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                details["DeliveryMedium"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                details["Destination"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            )
        })
        .collect()
}

/// One row of the update table.
#[derive(Clone, Copy, Debug)]
struct UpdateCase {
    /// What this row asserts.
    case: &'static str,
    /// How the pool this row updates in is configured.
    config: PoolConfig,
    /// The attributes the user already holds.
    stored: &'static [(&'static str, &'static str)],
    /// What the row submits to `UpdateUserAttributes`.
    submitted: &'static [(&'static str, &'static str)],
    /// What the stored user must hold afterwards: `Some(value)` for an
    /// attribute that holds it, `None` for one the store must not hold at all.
    expected: &'static [(&'static str, Option<&'static str>)],
    /// The `(AttributeName, DeliveryMedium, Destination)` the reply must carry,
    /// in order.
    deliveries: &'static [(&'static str, &'static str, &'static str)],
}

/// The rows differ along the attribute's kind (standard string, number, custom,
/// verified), the VALUE submitted (a new one, the one already stored, a blank
/// one), the COUNT of attributes in one call, and the pool's configuration (no
/// verification, auto-verification, verification required first).
const UPDATE_CASES: &[UpdateCase] = &[
    UpdateCase {
        case: "a standard string attribute is stored as submitted",
        config: PoolConfig {
            auto_verified: &[],
            verify_before_update: &[],
            alias: &[],
            custom_schema: &[],
            required: &[],
        },
        stored: &[],
        submitted: &[("name", "Ada Lovelace")],
        expected: &[("name", Some("Ada Lovelace"))],
        deliveries: &[],
    },
    UpdateCase {
        case: "three attributes in one call are all stored",
        config: PoolConfig {
            auto_verified: &[],
            verify_before_update: &[],
            alias: &[],
            custom_schema: &[],
            required: &[],
        },
        stored: &[("given_name", "Augusta")],
        submitted: &[
            ("given_name", "Ada"),
            ("family_name", "Lovelace"),
            ("locale", "en-GB"),
        ],
        expected: &[
            ("given_name", Some("Ada")),
            ("family_name", Some("Lovelace")),
            ("locale", Some("en-GB")),
        ],
        deliveries: &[],
    },
    UpdateCase {
        // A `Number` attribute of the standard schema, whose value travels as
        // the string every attribute value is.
        case: "a number attribute is stored as the submitted digits",
        config: PoolConfig {
            auto_verified: &[],
            verify_before_update: &[],
            alias: &[],
            custom_schema: &[],
            required: &[],
        },
        stored: &[],
        submitted: &[("updated_at", "1700000000")],
        expected: &[("updated_at", Some("1700000000"))],
        deliveries: &[],
    },
    UpdateCase {
        case: "a custom attribute the pool's schema declares is stored",
        config: PoolConfig {
            auto_verified: &[],
            verify_before_update: &[],
            alias: &[],
            custom_schema: &["department"],
            required: &[],
        },
        stored: &[],
        submitted: &[("custom:department", "research")],
        expected: &[("custom:department", Some("research"))],
        deliveries: &[],
    },
    UpdateCase {
        case: "a blank value deletes the stored attribute",
        config: PoolConfig {
            auto_verified: &[],
            verify_before_update: &[],
            alias: &[],
            custom_schema: &[],
            required: &[],
        },
        stored: &[("nickname", "Ada"), ("website", "https://example.test")],
        submitted: &[("nickname", "")],
        expected: &[
            ("nickname", None),
            ("website", Some("https://example.test")),
        ],
        deliveries: &[],
    },
    UpdateCase {
        case: "a blank value for an attribute that holds none leaves it absent",
        config: PoolConfig {
            auto_verified: &[],
            verify_before_update: &[],
            alias: &[],
            custom_schema: &[],
            required: &[],
        },
        stored: &[],
        submitted: &[("profile", "")],
        expected: &[("profile", None)],
        deliveries: &[],
    },
    UpdateCase {
        case: "deleting an email takes its verified flag with it",
        config: PoolConfig {
            auto_verified: &["email"],
            verify_before_update: &[],
            alias: &[],
            custom_schema: &[],
            required: &[],
        },
        stored: &[("email", "ada@old.example"), ("email_verified", "true")],
        submitted: &[("email", "")],
        expected: &[("email", None), ("email_verified", None)],
        deliveries: &[],
    },
    UpdateCase {
        case: "deleting an email takes its verified flag with it, even one the request sets",
        config: PoolConfig {
            auto_verified: &["email"],
            verify_before_update: &[],
            alias: &[],
            custom_schema: &[],
            required: &[],
        },
        stored: &[("email", "ada@old.example"), ("email_verified", "true")],
        submitted: &[("email", ""), ("email_verified", "true")],
        expected: &[("email", None), ("email_verified", None)],
        deliveries: &[],
    },
    UpdateCase {
        case: "a new email in a pool that verifies it applies at once, unverified, with a mailed code",
        config: PoolConfig {
            auto_verified: &["email"],
            verify_before_update: &[],
            alias: &[],
            custom_schema: &[],
            required: &[],
        },
        stored: &[("email", "ada@old.example"), ("email_verified", "true")],
        submitted: &[("email", "ada@new.example")],
        expected: &[
            ("email", Some("ada@new.example")),
            ("email_verified", Some("false")),
        ],
        deliveries: &[("email", "EMAIL", "a***@n***")],
    },
    UpdateCase {
        case: "a new phone number in a pool that verifies it applies at once, unverified, with a texted code",
        config: PoolConfig {
            auto_verified: &["phone_number"],
            verify_before_update: &[],
            alias: &[],
            custom_schema: &[],
            required: &[],
        },
        stored: &[
            ("phone_number", "+15550100"),
            ("phone_number_verified", "true"),
        ],
        submitted: &[("phone_number", "+15550199")],
        expected: &[
            ("phone_number", Some("+15550199")),
            ("phone_number_verified", Some("false")),
        ],
        deliveries: &[("phone_number", "SMS", "+****0199")],
    },
    UpdateCase {
        case: "a pool that requires verification first keeps the confirmed email and sends a code",
        config: PoolConfig {
            auto_verified: &["email"],
            verify_before_update: &["email"],
            alias: &[],
            custom_schema: &[],
            required: &[],
        },
        stored: &[("email", "ada@old.example"), ("email_verified", "true")],
        submitted: &[("email", "ada@new.example")],
        expected: &[
            ("email", Some("ada@old.example")),
            ("email_verified", Some("true")),
        ],
        deliveries: &[("email", "EMAIL", "a***@n***")],
    },
    UpdateCase {
        case: "a pool that neither verifies nor defers stores the new email and sends nothing",
        config: PoolConfig {
            auto_verified: &[],
            verify_before_update: &[],
            alias: &[],
            custom_schema: &[],
            required: &[],
        },
        stored: &[("email", "ada@old.example"), ("email_verified", "true")],
        submitted: &[("email", "ada@new.example")],
        expected: &[
            ("email", Some("ada@new.example")),
            ("email_verified", Some("false")),
        ],
        deliveries: &[],
    },
    UpdateCase {
        case: "resubmitting the stored email sends nothing and leaves it verified",
        config: PoolConfig {
            auto_verified: &["email"],
            verify_before_update: &[],
            alias: &[],
            custom_schema: &[],
            required: &[],
        },
        stored: &[("email", "ada@same.example"), ("email_verified", "true")],
        submitted: &[("email", "ada@same.example")],
        expected: &[
            ("email", Some("ada@same.example")),
            ("email_verified", Some("true")),
        ],
        deliveries: &[],
    },
    UpdateCase {
        case: "resubmitting the stored email to a pool that defers it holds nothing pending",
        config: PoolConfig {
            auto_verified: &["email"],
            verify_before_update: &["email"],
            alias: &[],
            custom_schema: &[],
            required: &[],
        },
        stored: &[("email", "ada@same.example"), ("email_verified", "true")],
        submitted: &[("email", "ada@same.example")],
        expected: &[
            ("email", Some("ada@same.example")),
            ("email_verified", Some("true")),
        ],
        deliveries: &[],
    },
    UpdateCase {
        case: "a request that sets the verified flag itself keeps the submitted flag",
        config: PoolConfig {
            auto_verified: &["email"],
            verify_before_update: &[],
            alias: &[],
            custom_schema: &[],
            required: &[],
        },
        stored: &[],
        submitted: &[("email", "ada@new.example"), ("email_verified", "true")],
        expected: &[
            ("email", Some("ada@new.example")),
            ("email_verified", Some("true")),
        ],
        deliveries: &[("email", "EMAIL", "a***@n***")],
    },
    UpdateCase {
        case: "an email and a phone number in one call each report their own delivery",
        config: PoolConfig {
            auto_verified: &["email", "phone_number"],
            verify_before_update: &[],
            alias: &[],
            custom_schema: &[],
            required: &[],
        },
        stored: &[],
        submitted: &[
            ("email", "grace@hopper.example"),
            ("phone_number", "+15550123"),
        ],
        expected: &[
            ("email", Some("grace@hopper.example")),
            ("email_verified", Some("false")),
            ("phone_number", Some("+15550123")),
            ("phone_number_verified", Some("false")),
        ],
        deliveries: &[
            ("email", "EMAIL", "g***@h***"),
            ("phone_number", "SMS", "+****0123"),
        ],
    },
];

#[tokio::test]
async fn an_update_stores_the_submitted_values_on_the_signed_in_user() {
    let router = emulator();
    for (index, case) in UPDATE_CASES.iter().enumerate() {
        let UpdateCase {
            case,
            config,
            stored,
            submitted,
            expected,
            deliveries,
        } = case;
        let username = format!("user-{index}");
        let fixture = fixture(
            &router,
            &format!("update-pool-{index}"),
            *config,
            &username,
            stored,
        )
        .await;

        let (status, payload) = update(&router, &fixture.access_token, submitted).await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateUserAttributes must succeed, got {status} with {payload}"
        );
        // Nothing delivered means no delivery member at all: the reply is the
        // empty object, not an empty list.
        assert_eq!(
            payload.get("CodeDeliveryDetailsList").is_some(),
            !deliveries.is_empty(),
            "case {case:?}: CodeDeliveryDetailsList is reported only when a message was sent, \
             got {payload}"
        );
        let answered = deliveries_of(&payload, case);
        let wanted: Vec<(String, String, String)> = deliveries
            .iter()
            .map(|(attribute, medium, destination)| {
                (
                    (*attribute).to_string(),
                    (*medium).to_string(),
                    (*destination).to_string(),
                )
            })
            .collect();
        assert_eq!(
            answered, wanted,
            "case {case:?}: expected deliveries {wanted:?}, got {answered:?}"
        );

        // The round trip: what a later read of the user renders.
        let attributes = stored_attributes(&router, &fixture.user_pool_id, &username).await;
        for (name, value) in *expected {
            let actual = attributes.get(*name);
            assert_eq!(
                actual.map(String::as_str),
                *value,
                "case {case:?}: expected {name} to be {value:?}, got {actual:?} in {attributes:?}"
            );
        }
    }
}

/// One row of the refusal table: a request the operation must refuse, and the
/// status and wire error code it must answer.
#[derive(Clone, Copy, Debug)]
struct RefusalCase {
    /// What this row asserts.
    case: &'static str,
    /// How the pool this row updates in is configured.
    config: PoolConfig,
    /// The attributes the user already holds.
    stored: &'static [(&'static str, &'static str)],
    /// What the row submits.
    submitted: &'static [(&'static str, &'static str)],
    status: u16,
    code: &'static str,
}

/// The rows differ along WHICH rule the request breaks: an attribute no schema
/// declares, one the schema freezes, a required one a blank value would
/// delete, an attribute named twice, and a list longer than the pool's schema.
const REFUSAL_CASES: &[RefusalCase] = &[
    RefusalCase {
        case: "an attribute the pool's schema does not declare",
        config: PoolConfig {
            auto_verified: &[],
            verify_before_update: &[],
            alias: &[],
            custom_schema: &[],
            required: &[],
        },
        stored: &[],
        submitted: &[("custom:undeclared", "nowhere")],
        status: 400,
        code: "InvalidParameterException",
    },
    RefusalCase {
        // `sub` is the user's own id: the standard schema declares it
        // immutable, so no update may change it.
        case: "an attribute the schema declares immutable",
        config: PoolConfig {
            auto_verified: &[],
            verify_before_update: &[],
            alias: &[],
            custom_schema: &[],
            required: &[],
        },
        stored: &[],
        submitted: &[("sub", "11111111-2222-3333-4444-555555555555")],
        status: 400,
        code: "InvalidParameterException",
    },
    RefusalCase {
        case: "a blank value for an attribute the schema marks required",
        config: PoolConfig {
            auto_verified: &[],
            verify_before_update: &[],
            alias: &[],
            custom_schema: &[],
            required: &["email"],
        },
        stored: &[("email", "ada@old.example")],
        submitted: &[("email", "")],
        status: 400,
        code: "InvalidParameterException",
    },
    RefusalCase {
        case: "one bad attribute refuses the whole call, including the good one beside it",
        config: PoolConfig {
            auto_verified: &[],
            verify_before_update: &[],
            alias: &[],
            custom_schema: &[],
            required: &[],
        },
        stored: &[],
        submitted: &[("nickname", "Ada"), ("custom:undeclared", "nowhere")],
        status: 400,
        code: "InvalidParameterException",
    },
    RefusalCase {
        // One value per attribute: naming it twice asks for two values of it.
        case: "the same attribute submitted twice in one call",
        config: PoolConfig {
            auto_verified: &[],
            verify_before_update: &[],
            alias: &[],
            custom_schema: &[],
            required: &[],
        },
        stored: &[],
        submitted: &[("nickname", "Ada"), ("nickname", "Augusta")],
        status: 400,
        code: "InvalidParameterException",
    },
    RefusalCase {
        // The standard schema declares 20 attributes and this pool adds none,
        // so a list of 21 entries cannot name 21 attributes of it. The entries
        // are distinct names so that the count is what refuses the call.
        case: "more attributes in one call than the pool's schema declares",
        config: PoolConfig {
            auto_verified: &[],
            verify_before_update: &[],
            alias: &[],
            custom_schema: &[],
            required: &[],
        },
        stored: &[],
        submitted: &[
            ("name", "a"),
            ("given_name", "b"),
            ("family_name", "c"),
            ("middle_name", "d"),
            ("nickname", "e"),
            ("preferred_username", "f"),
            ("profile", "g"),
            ("picture", "h"),
            ("website", "i"),
            ("email", "j@example.test"),
            ("email_verified", "false"),
            ("gender", "k"),
            ("birthdate", "1815-12-10"),
            ("zoneinfo", "UTC"),
            ("locale", "en-GB"),
            ("phone_number", "+15550111"),
            ("phone_number_verified", "false"),
            ("address", "l"),
            ("updated_at", "1700000000"),
            ("sub", "m"),
            ("custom:n", "o"),
        ],
        status: 400,
        code: "InvalidParameterException",
    },
];

#[tokio::test]
async fn a_refused_update_answers_the_modelled_error_and_writes_nothing() {
    let router = emulator();
    for (index, case) in REFUSAL_CASES.iter().enumerate() {
        let RefusalCase {
            case,
            config,
            stored,
            submitted,
            status: expected_status,
            code,
        } = case;
        let username = format!("refused-{index}");
        let fixture = fixture(
            &router,
            &format!("refusal-pool-{index}"),
            *config,
            &username,
            stored,
        )
        .await;
        let before = stored_attributes(&router, &fixture.user_pool_id, &username).await;

        let (status, payload) = update(&router, &fixture.access_token, submitted).await;
        assert_eq!(
            (status, error_code(&payload)),
            (*expected_status, *code),
            "case {case:?}: expected {expected_status} {code}, got {status} with {payload}"
        );

        let after = stored_attributes(&router, &fixture.user_pool_id, &username).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused update must write nothing, but the stored attributes \
             went from {before:?} to {after:?}"
        );
    }
}

#[tokio::test]
async fn a_token_that_names_no_signed_in_user_is_refused() {
    let router = emulator();
    let fixture = fixture(
        &router,
        "token-pool",
        PoolConfig::default(),
        "dave",
        &[("nickname", "Dave")],
    )
    .await;
    let result = sign_in(&router, &fixture.client_id, "dave").await;
    let id_token = result["IdToken"]
        .as_str()
        .expect("an id token")
        .to_string();
    let refresh_token = result["RefreshToken"]
        .as_str()
        .expect("a refresh token")
        .to_string();

    for (case, token) in [
        (
            "a token this emulator never issued",
            "not-an-issued-token".to_string(),
        ),
        (
            "the id token of the same sign-in, which is not an access token",
            id_token,
        ),
        (
            "the refresh token of the same sign-in, which is not an access token",
            refresh_token,
        ),
    ] {
        let (status, payload) = update(&router, &token, &[("nickname", "Changed")]).await;
        assert_eq!(
            (status, error_code(&payload)),
            (400, "NotAuthorizedException"),
            "case {case:?}: expected 400 NotAuthorizedException, got {status} with {payload}"
        );
    }

    // Nothing the refused calls submitted reached the stored user.
    let attributes = stored_attributes(&router, &fixture.user_pool_id, "dave").await;
    let nickname = attributes.get("nickname");
    assert_eq!(
        nickname.map(String::as_str),
        Some("Dave"),
        "a refused token must write nothing, got nickname {nickname:?} in {attributes:?}"
    );
}

#[tokio::test]
async fn an_alias_value_another_user_holds_moves_to_the_taker_or_is_refused() {
    let router = emulator();
    // A pool that resolves a sign-in name through `preferred_username` and
    // `email`, so a value of either names one user of the pool.
    let config = PoolConfig {
        auto_verified: &["email"],
        verify_before_update: &[],
        alias: &["preferred_username", "email"],
        custom_schema: &[],
        required: &[],
    };
    let holder = fixture(
        &router,
        "alias-pool",
        config,
        "ada",
        &[
            ("preferred_username", "lovelace"),
            ("email", "ada@alias.example"),
            ("email_verified", "true"),
        ],
    )
    .await;
    confirmed_user(&router, &holder.user_pool_id, "grace", &[]).await;
    let other = sign_in(&router, &holder.client_id, "grace").await["AccessToken"]
        .as_str()
        .expect("an access token")
        .to_string();

    // `preferred_username` carries no verified flag, so every stored value of
    // it is an alias and a second user may not take it.
    let (status, payload) = update(&router, &other, &[("preferred_username", "lovelace")]).await;
    assert_eq!(
        (status, error_code(&payload)),
        (400, "AliasExistsException"),
        "case {:?}: expected 400 AliasExistsException, got {status} with {payload}",
        "a preferred_username another user already holds"
    );

    // The same value is free once the user holding it gives it up.
    let (status, payload) = update(&router, &holder.access_token, &[("preferred_username", "")]).await;
    assert_eq!(
        status, 200,
        "giving up an alias value must succeed, got {status} with {payload}"
    );
    let (status, payload) = update(&router, &other, &[("preferred_username", "lovelace")]).await;
    assert_eq!(
        status, 200,
        "case {:?}: the value is free now, got {status} with {payload}",
        "a preferred_username the holder has given up"
    );

    // An email is an alias only while it is verified, so taking an address
    // another user holds UNVERIFIED succeeds and leaves it unverified.
    let (status, payload) = update(&router, &other, &[("email", "ada@alias.example")]).await;
    assert_eq!(
        status, 200,
        "case {:?}: an unverified duplicate is not an alias collision, got {status} with {payload}",
        "an email another user holds verified, taken unverified"
    );
    let attributes = stored_attributes(&router, &holder.user_pool_id, "grace").await;
    let verified = attributes.get("email_verified");
    assert_eq!(
        verified.map(String::as_str),
        Some("false"),
        "case {:?}: the new address is not verified yet, got {verified:?} in {attributes:?}",
        "an email another user holds verified, taken unverified"
    );

    // Asking for that same address AS VERIFIED takes the alias: an alias names
    // ONE user of the pool, so the write is accepted and the previous holder's
    // copy is demoted out of verified status.
    let (status, payload) = update(
        &router,
        &other,
        &[("email", "ada@alias.example"), ("email_verified", "true")],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {:?}: taking a verified alias must succeed, got {status} with {payload}",
        "an email another user holds verified, taken verified"
    );
    let taker = stored_attributes(&router, &holder.user_pool_id, "grace").await;
    assert_eq!(
        taker.get("email_verified").map(String::as_str),
        Some("true"),
        "case {:?}: the taker holds the alias verified, got {taker:?}",
        "an email another user holds verified, taken verified"
    );
    let demoted = stored_attributes(&router, &holder.user_pool_id, "ada").await;
    assert_eq!(
        (
            demoted.get("email").map(String::as_str),
            demoted.get("email_verified").map(String::as_str),
        ),
        (Some("ada@alias.example"), Some("false")),
        "case {:?}: the previous holder keeps the value but loses the verified flag, got {demoted:?}",
        "an email another user holds verified, taken verified"
    );
}
