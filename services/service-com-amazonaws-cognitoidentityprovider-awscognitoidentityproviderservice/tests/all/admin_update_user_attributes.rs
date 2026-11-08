// SPDX-License-Identifier: CC0-1.0

//! `AdminUpdateUserAttributes` over the wire, driven exactly as an SDK drives it
//! (`POST /` with `X-Amz-Target:
//! AWSCognitoIdentityProviderService.AdminUpdateUserAttributes`).
//!
//! The rule under test: an administrative write applies the submitted name/value
//! pairs to the STORED user the `UserPoolId` and `Username` name — a value the
//! pool's schema declares is stored as sent, a blank value DELETES the attribute
//! so a later read renders it absent rather than empty, and the user is resolved
//! by username or by `sub`. A new `email` or `phone_number` is no longer the
//! value its verified flag vouched for, so the flag goes false, a pool that
//! requires verification first keeps the confirmed value and holds the new one
//! pending, and a request that submits the verified flag as `true` skips that
//! hold and applies the new value at once. An attribute the schema does not
//! declare, one it declares immutable, blanking one it marks required, a name
//! submitted twice and a list longer than the schema are all refused with 400
//! `InvalidParameterException`, a value another user holds as an alias no
//! verified flag can take away from it with 400 `AliasExistsException` — while
//! one a verified flag CAN moves to the taker and demotes the previous holder —
//! an absent pool with 400
//! `ResourceNotFoundException` and an absent user with 400
//! `UserNotFoundException` — and a refused request writes nothing.
//!
//! The pools, their schemas and their users are all real: they come from
//! `CreateUserPool`, `AddCustomAttributes` and `AdminCreateUser`, and every
//! update is read back through `AdminGetUser`.
//!
//! Each table row names the case it asserts.

use std::collections::BTreeMap;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice as cognito_idp;

use cognito_idp::state;

/// A request's SigV4 credential scope is what routes an administrative call to
/// this service. The signature is never checked here: the suite turns request
/// authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/cognito-idp/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0000";

/// This service's regional endpoint host, which a signed call is addressed to.
const ENDPOINT_HOST: &str = "cognito-idp.us-east-1.amazonaws.com";

/// One emulator with just this service mounted.
fn emulator() -> MultiServiceRouter {
    emulator_over(&Arc::new(engine::InMemoryEngine::new()))
}

/// One emulator with just this service mounted over `engine`, so a test can
/// also read the state a delivery made out of band left behind.
fn emulator_over(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.register_service(cognito_idp::EmulatedService::new(Arc::clone(engine)));
    router.set_request_authentication(false);
    router
}

/// Dispatch the signed `operation` with `body` and return its status and JSON
/// response body.
async fn dispatch(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", ENDPOINT_HOST)
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "x-amz-target",
            format!("AWSCognitoIdentityProviderService.{operation}"),
        )
        .header("authorization", AUTHORIZATION);
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
    /// Names added to the pool's schema AFTER it was created, through
    /// `AddCustomAttributes`: the schema is append-only, so an attribute added
    /// that way is as writable as one the create declared.
    added_custom: &'static [&'static str],
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
    // A pool that auto-verifies `phone_number` sends that verification over SMS,
    // so it needs the Amazon SNS caller SMS is sent through: `CreateUserPool`
    // refuses the pairing without it.
    if config.auto_verified.contains(&"phone_number") {
        body.insert(
            "SmsConfiguration".to_string(),
            json!({ "SnsCallerArn": "arn:aws:iam::000000000000:role/sms-caller" }),
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
    let user_pool_id = created["UserPool"]["Id"]
        .as_str()
        .expect("a minted pool id")
        .to_string();
    if !config.added_custom.is_empty() {
        ok(
            router,
            "AddCustomAttributes",
            &json!({
                "UserPoolId": user_pool_id,
                "CustomAttributes": config
                    .added_custom
                    .iter()
                    .map(|attribute| json!({
                        "Name": attribute,
                        "AttributeDataType": "String",
                        "Mutable": true,
                    }))
                    .collect::<Vec<Value>>(),
            }),
        )
        .await;
    }
    user_pool_id
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

/// Create `username` holding `attributes` and return its minted `sub`.
async fn user(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    username: &str,
    attributes: &[(&str, &str)],
) -> String {
    let created = ok(
        router,
        "AdminCreateUser",
        &json!({
            "UserPoolId": user_pool_id,
            "Username": username,
            "UserAttributes": attribute_list(attributes),
        }),
    )
    .await;
    created["User"]["Attributes"]
        .as_array()
        .expect("AdminCreateUser reports the stored attributes")
        .iter()
        .find(|attribute| attribute["Name"] == json!("sub"))
        .and_then(|attribute| attribute["Value"].as_str())
        .expect("a minted sub")
        .to_string()
}

/// Call `AdminUpdateUserAttributes` and return its status and body.
async fn update(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    username: &str,
    attributes: &[(&str, &str)],
) -> (u16, Value) {
    dispatch(
        router,
        "AdminUpdateUserAttributes",
        &json!({
            "UserPoolId": user_pool_id,
            "Username": username,
            "UserAttributes": attribute_list(attributes),
        }),
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
    let stored = ok(
        router,
        "AdminGetUser",
        &json!({ "UserPoolId": user_pool_id, "Username": username }),
    )
    .await;
    stored["UserAttributes"]
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

/// One row of the update table.
#[derive(Clone, Copy, Debug)]
struct UpdateCase {
    /// What this row asserts.
    case: &'static str,
    /// How the pool this row updates in is configured.
    config: PoolConfig,
    /// The attributes the user already holds.
    stored: &'static [(&'static str, &'static str)],
    /// What the row submits to `AdminUpdateUserAttributes`.
    submitted: &'static [(&'static str, &'static str)],
    /// What the stored user must hold afterwards: `Some(value)` for an
    /// attribute that holds it, `None` for one the store must not hold at all.
    expected: &'static [(&'static str, Option<&'static str>)],
}

/// The rows differ along the attribute's kind (standard string, number, custom
/// declared at create, custom appended later, verified), the VALUE submitted (a
/// new one, a blank one), the COUNT of attributes in one call, and the pool's
/// configuration (no verification, auto-verification, verification required
/// first).
const UPDATE_CASES: &[UpdateCase] = &[
    UpdateCase {
        case: "a standard string attribute is stored as submitted",
        config: PoolConfig {
            auto_verified: &[],
            verify_before_update: &[],
            alias: &[],
            custom_schema: &[],
            added_custom: &[],
            required: &[],
        },
        stored: &[],
        submitted: &[("name", "Ada Lovelace")],
        expected: &[("name", Some("Ada Lovelace"))],
    },
    UpdateCase {
        case: "three attributes in one call are all stored, over what was there",
        config: PoolConfig {
            auto_verified: &[],
            verify_before_update: &[],
            alias: &[],
            custom_schema: &[],
            added_custom: &[],
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
            added_custom: &[],
            required: &[],
        },
        stored: &[],
        submitted: &[("updated_at", "1700000000")],
        expected: &[("updated_at", Some("1700000000"))],
    },
    UpdateCase {
        case: "a custom attribute the create declared is stored",
        config: PoolConfig {
            auto_verified: &[],
            verify_before_update: &[],
            alias: &[],
            custom_schema: &["department"],
            added_custom: &[],
            required: &[],
        },
        stored: &[],
        submitted: &[("custom:department", "research")],
        expected: &[("custom:department", Some("research"))],
    },
    UpdateCase {
        case: "a custom attribute appended to the schema after the create is stored",
        config: PoolConfig {
            auto_verified: &[],
            verify_before_update: &[],
            alias: &[],
            custom_schema: &[],
            added_custom: &["badge"],
            required: &[],
        },
        stored: &[],
        submitted: &[("custom:badge", "42")],
        expected: &[("custom:badge", Some("42"))],
    },
    UpdateCase {
        case: "a blank value deletes the stored attribute, leaving it absent and not empty",
        config: PoolConfig {
            auto_verified: &[],
            verify_before_update: &[],
            alias: &[],
            custom_schema: &[],
            added_custom: &[],
            required: &[],
        },
        stored: &[("nickname", "Ada"), ("website", "https://example.test")],
        submitted: &[("nickname", "")],
        expected: &[
            ("nickname", None),
            ("website", Some("https://example.test")),
        ],
    },
    UpdateCase {
        case: "a blank value deletes a custom attribute too",
        config: PoolConfig {
            auto_verified: &[],
            verify_before_update: &[],
            alias: &[],
            custom_schema: &["department"],
            added_custom: &[],
            required: &[],
        },
        stored: &[("custom:department", "research")],
        submitted: &[("custom:department", "")],
        expected: &[("custom:department", None)],
    },
    UpdateCase {
        case: "a blank value for an attribute that holds none leaves it absent",
        config: PoolConfig {
            auto_verified: &[],
            verify_before_update: &[],
            alias: &[],
            custom_schema: &[],
            added_custom: &[],
            required: &[],
        },
        stored: &[],
        submitted: &[("profile", "")],
        expected: &[("profile", None)],
    },
    UpdateCase {
        case: "deleting an email takes its verified flag with it",
        config: PoolConfig {
            auto_verified: &["email"],
            verify_before_update: &[],
            alias: &[],
            custom_schema: &[],
            added_custom: &[],
            required: &[],
        },
        stored: &[("email", "ada@old.example"), ("email_verified", "true")],
        submitted: &[("email", "")],
        expected: &[("email", None), ("email_verified", None)],
    },
    UpdateCase {
        case: "a new email the request says nothing about applies unverified",
        config: PoolConfig {
            auto_verified: &["email"],
            verify_before_update: &[],
            alias: &[],
            custom_schema: &[],
            added_custom: &[],
            required: &[],
        },
        stored: &[("email", "ada@old.example"), ("email_verified", "true")],
        submitted: &[("email", "ada@new.example")],
        expected: &[
            ("email", Some("ada@new.example")),
            ("email_verified", Some("false")),
        ],
    },
    UpdateCase {
        case: "a new phone number the request says nothing about applies unverified",
        config: PoolConfig {
            auto_verified: &["phone_number"],
            verify_before_update: &[],
            alias: &[],
            custom_schema: &[],
            added_custom: &[],
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
    },
    UpdateCase {
        case: "a pool that requires verification first keeps the confirmed email",
        config: PoolConfig {
            auto_verified: &["email"],
            verify_before_update: &["email"],
            alias: &[],
            custom_schema: &[],
            added_custom: &[],
            required: &[],
        },
        stored: &[("email", "ada@old.example"), ("email_verified", "true")],
        submitted: &[("email", "ada@new.example")],
        expected: &[
            ("email", Some("ada@old.example")),
            ("email_verified", Some("true")),
        ],
    },
    UpdateCase {
        // The documented skip: an administrator that submits the verified flag
        // as `true` asserts the new value is verified, so the pool neither
        // holds it pending nor waits for a confirmation.
        case: "a verified flag of true applies the new email at once, verified, past the hold",
        config: PoolConfig {
            auto_verified: &["email"],
            verify_before_update: &["email"],
            alias: &[],
            custom_schema: &[],
            added_custom: &[],
            required: &[],
        },
        stored: &[("email", "ada@old.example"), ("email_verified", "true")],
        submitted: &[("email", "ada@new.example"), ("email_verified", "true")],
        expected: &[
            ("email", Some("ada@new.example")),
            ("email_verified", Some("true")),
        ],
    },
    UpdateCase {
        // Only `true` skips the hold: a flag the request submits as `false`
        // asserts nothing, so the pool still waits for a confirmation and the
        // new value is not the user's value yet.
        case: "a verified flag of false does not skip the hold, so the new phone number waits",
        config: PoolConfig {
            auto_verified: &["phone_number"],
            verify_before_update: &["phone_number"],
            alias: &[],
            custom_schema: &[],
            added_custom: &[],
            required: &[],
        },
        stored: &[],
        submitted: &[
            ("phone_number", "+15550123"),
            ("phone_number_verified", "false"),
        ],
        expected: &[
            ("phone_number", None),
            ("phone_number_verified", Some("false")),
        ],
    },
    UpdateCase {
        case: "an email and a phone number in one call both apply",
        config: PoolConfig {
            auto_verified: &["email", "phone_number"],
            verify_before_update: &[],
            alias: &[],
            custom_schema: &[],
            added_custom: &[],
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
    },
];

#[tokio::test]
async fn an_admin_update_stores_the_submitted_values_on_the_named_user() {
    let router = emulator();
    for (index, case) in UPDATE_CASES.iter().enumerate() {
        let UpdateCase {
            case,
            config,
            stored,
            submitted,
            expected,
        } = case;
        let username = format!("user-{index}");
        let user_pool_id = pool(&router, &format!("admin-update-pool-{index}"), *config).await;
        user(&router, &user_pool_id, &username, stored).await;

        let (status, payload) = update(&router, &user_pool_id, &username, submitted).await;
        assert_eq!(
            status, 200,
            "case {case:?}: AdminUpdateUserAttributes must succeed, got {status} with {payload}"
        );
        assert_eq!(
            payload,
            json!({}),
            "case {case:?}: the response models no members, got {payload}"
        );

        // The round trip: what a later read of the user renders.
        let attributes = stored_attributes(&router, &user_pool_id, &username).await;
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

/// Whether a verification code is outstanding for `attribute` of `username`,
/// read out of the emulator's own state: a code travels out of band and this
/// operation's response models no member to report its delivery in, so state is
/// where a message that was sent can be observed at all.
fn code_outstanding(
    engine: &Arc<engine::InMemoryEngine>,
    user_pool_id: &str,
    username: &str,
    attribute: &str,
) -> bool {
    let service_state = state::service_state(&**engine).expect("this service's state");
    let guard = service_state.lock();
    guard
        .pool(user_pool_id)
        .and_then(|stored| stored.user(username))
        .and_then(|stored| {
            stored.outstanding_code(&state::CodePurpose::AttributeVerification(
                attribute.to_string(),
            ))
        })
        .is_some()
}

/// One row of the verification-message table: what an update does about the
/// code that confirms a new email address or phone number.
#[derive(Clone, Copy, Debug)]
struct MessageCase {
    /// What this row asserts.
    case: &'static str,
    /// How the pool this row updates in is configured.
    config: PoolConfig,
    /// What the row submits.
    submitted: &'static [(&'static str, &'static str)],
    /// The attribute a code must be outstanding for afterwards, or `None` when
    /// this update sends nothing at all.
    sends_for: Option<&'static str>,
}

/// The rows differ along the pool's configuration (verifying the attribute
/// itself, holding the update pending, doing neither), the verified flag the
/// request submits, and whether the attribute is one a message can be addressed
/// to at all.
const MESSAGE_CASES: &[MessageCase] = &[
    MessageCase {
        case: "a pool that verifies the attribute sends a code for the new address",
        config: PoolConfig {
            auto_verified: &["email"],
            verify_before_update: &[],
            alias: &[],
            custom_schema: &[],
            added_custom: &[],
            required: &[],
        },
        submitted: &[("email", "ada@new.example")],
        sends_for: Some("email"),
    },
    MessageCase {
        case: "a pool that holds the update pending sends a code for the pending number",
        config: PoolConfig {
            auto_verified: &[],
            verify_before_update: &["phone_number"],
            alias: &[],
            custom_schema: &[],
            added_custom: &[],
            required: &[],
        },
        submitted: &[("phone_number", "+15550123")],
        sends_for: Some("phone_number"),
    },
    MessageCase {
        // The documented skip: the administrator asserted the value verified,
        // so there is nothing left to confirm and no message goes out.
        case: "a verified flag of true sends no code at all",
        config: PoolConfig {
            auto_verified: &["email"],
            verify_before_update: &["email"],
            alias: &[],
            custom_schema: &[],
            added_custom: &[],
            required: &[],
        },
        submitted: &[("email", "ada@new.example"), ("email_verified", "true")],
        sends_for: None,
    },
    MessageCase {
        case: "a pool that neither verifies nor defers sends no code",
        config: PoolConfig {
            auto_verified: &[],
            verify_before_update: &[],
            alias: &[],
            custom_schema: &[],
            added_custom: &[],
            required: &[],
        },
        submitted: &[("email", "ada@new.example")],
        sends_for: None,
    },
    MessageCase {
        case: "an attribute no message can be addressed to sends no code",
        config: PoolConfig {
            auto_verified: &["email"],
            verify_before_update: &[],
            alias: &[],
            custom_schema: &[],
            added_custom: &[],
            required: &[],
        },
        submitted: &[("nickname", "Ada")],
        sends_for: None,
    },
];

#[tokio::test]
async fn an_admin_update_records_the_code_of_each_message_it_sends() {
    let shared: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_over(&shared);
    for (index, case) in MESSAGE_CASES.iter().enumerate() {
        let MessageCase {
            case,
            config,
            submitted,
            sends_for,
        } = case;
        let username = format!("messaged-{index}");
        let user_pool_id = pool(&router, &format!("admin-message-pool-{index}"), *config).await;
        user(&router, &user_pool_id, &username, &[]).await;

        let (status, payload) = update(&router, &user_pool_id, &username, submitted).await;
        assert_eq!(
            status, 200,
            "case {case:?}: AdminUpdateUserAttributes must succeed, got {status} with {payload}"
        );

        // A code is recorded for the attribute its message was addressed to,
        // and for no other: that record is what `VerifyUserAttribute` accepts.
        for attribute in ["email", "phone_number"] {
            let outstanding = code_outstanding(&shared, &user_pool_id, &username, attribute);
            let wanted = *sends_for == Some(attribute);
            assert_eq!(
                outstanding, wanted,
                "case {case:?}: expected a code outstanding for {attribute} to be {wanted}, \
                 got {outstanding}"
            );
        }
    }
}

#[tokio::test]
async fn an_admin_update_reaches_the_user_named_by_its_sub() {
    let router = emulator();
    let user_pool_id = pool(&router, "sub-pool", PoolConfig::default()).await;
    let sub = user(&router, &user_pool_id, "ada", &[("nickname", "Ada")]).await;

    // `Username` "must be the sub of a local user" where it is not a username
    // or an alias (`AdminUpdateUserAttributes.Username`), so the same user is
    // reachable under either name.
    let (status, payload) = update(&router, &user_pool_id, &sub, &[("nickname", "Augusta")]).await;
    assert_eq!(
        status, 200,
        "case {:?}: an update by sub must succeed, got {status} with {payload}",
        "the user named by its sub"
    );
    let attributes = stored_attributes(&router, &user_pool_id, "ada").await;
    let nickname = attributes.get("nickname");
    assert_eq!(
        nickname.map(String::as_str),
        Some("Augusta"),
        "case {:?}: expected nickname Augusta, got {nickname:?} in {attributes:?}",
        "the user named by its sub"
    );
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
/// declares, one the schema freezes, a required one a blank value would delete,
/// an attribute named twice, and a list longer than the pool's schema.
const REFUSAL_CASES: &[RefusalCase] = &[
    RefusalCase {
        case: "an attribute the pool's schema does not declare",
        config: PoolConfig {
            auto_verified: &[],
            verify_before_update: &[],
            alias: &[],
            custom_schema: &[],
            added_custom: &[],
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
            added_custom: &[],
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
            added_custom: &[],
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
            added_custom: &[],
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
            added_custom: &[],
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
            added_custom: &[],
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
async fn a_refused_admin_update_answers_the_modelled_error_and_writes_nothing() {
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
        let user_pool_id = pool(&router, &format!("admin-refusal-pool-{index}"), *config).await;
        user(&router, &user_pool_id, &username, stored).await;
        let before = stored_attributes(&router, &user_pool_id, &username).await;

        let (status, payload) = update(&router, &user_pool_id, &username, submitted).await;
        assert_eq!(
            (status, error_code(&payload)),
            (*expected_status, *code),
            "case {case:?}: expected {expected_status} {code}, got {status} with {payload}"
        );

        let after = stored_attributes(&router, &user_pool_id, &username).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused update must write nothing, but the stored attributes \
             went from {before:?} to {after:?}"
        );
    }
}

#[tokio::test]
async fn a_pool_or_user_the_store_does_not_hold_is_refused() {
    let router = emulator();
    let user_pool_id = pool(&router, "absent-pool", PoolConfig::default()).await;
    user(&router, &user_pool_id, "ada", &[]).await;

    for (case, pool_id, username, expected_status, code) in [
        (
            "a user pool this emulator never created",
            "us-east-1_absentpool".to_string(),
            "ada".to_string(),
            400,
            "ResourceNotFoundException",
        ),
        (
            "a user the pool does not hold",
            user_pool_id.clone(),
            "grace".to_string(),
            400,
            "UserNotFoundException",
        ),
    ] {
        let (status, payload) = update(&router, &pool_id, &username, &[("nickname", "Ada")]).await;
        assert_eq!(
            (status, error_code(&payload)),
            (expected_status, code),
            "case {case:?}: expected {expected_status} {code}, got {status} with {payload}"
        );
    }
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
        added_custom: &[],
        required: &[],
    };
    let user_pool_id = pool(&router, "admin-alias-pool", config).await;
    user(
        &router,
        &user_pool_id,
        "ada",
        &[
            ("preferred_username", "lovelace"),
            ("email", "ada@alias.example"),
            ("email_verified", "true"),
        ],
    )
    .await;
    user(&router, &user_pool_id, "grace", &[]).await;

    // `preferred_username` carries no verified flag, so every stored value of
    // it is an alias and a second user may not take it.
    let (status, payload) = update(
        &router,
        &user_pool_id,
        "grace",
        &[("preferred_username", "lovelace")],
    )
    .await;
    assert_eq!(
        (status, error_code(&payload)),
        (400, "AliasExistsException"),
        "case {:?}: expected 400 AliasExistsException, got {status} with {payload}",
        "a preferred_username another user already holds"
    );

    // The same value is free once the user holding it gives it up.
    let (status, payload) = update(
        &router,
        &user_pool_id,
        "ada",
        &[("preferred_username", "")],
    )
    .await;
    assert_eq!(
        status, 200,
        "giving up an alias value must succeed, got {status} with {payload}"
    );
    let (status, payload) = update(
        &router,
        &user_pool_id,
        "grace",
        &[("preferred_username", "lovelace")],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {:?}: the value is free now, got {status} with {payload}",
        "a preferred_username the holder has given up"
    );

    // An email is an alias only while it is verified, so taking an address
    // another user holds UNVERIFIED succeeds and leaves it unverified.
    let (status, payload) = update(
        &router,
        &user_pool_id,
        "grace",
        &[("email", "ada@alias.example")],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {:?}: an unverified duplicate is not an alias collision, got {status} with {payload}",
        "an email another user holds verified, taken unverified"
    );

    // Asking for that same address AS VERIFIED takes the alias: an alias names
    // ONE user of the pool, so the write is accepted and the previous holder's
    // copy is demoted out of verified status.
    let (status, payload) = update(
        &router,
        &user_pool_id,
        "grace",
        &[("email", "ada@alias.example"), ("email_verified", "true")],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {:?}: taking a verified alias must succeed, got {status} with {payload}",
        "an email another user holds verified, taken verified"
    );
    let taker = stored_attributes(&router, &user_pool_id, "grace").await;
    assert_eq!(
        taker.get("email_verified").map(String::as_str),
        Some("true"),
        "case {:?}: the taker holds the alias verified, got {taker:?}",
        "an email another user holds verified, taken verified"
    );
    let demoted = stored_attributes(&router, &user_pool_id, "ada").await;
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
