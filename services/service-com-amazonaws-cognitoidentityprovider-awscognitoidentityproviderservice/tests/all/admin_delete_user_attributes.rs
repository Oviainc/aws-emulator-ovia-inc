// SPDX-License-Identifier: CC0-1.0

//! `AdminDeleteUserAttributes` over the wire, driven exactly as an SDK drives it
//! (`POST /` with `X-Amz-Target:
//! AWSCognitoIdentityProviderService.AdminDeleteUserAttributes`).
//!
//! The rule under test: an administrative deletion removes every attribute its
//! `UserAttributeNames` lists from the STORED user that its `UserPoolId` and
//! `Username` name — and nothing else — so a later read of that user renders
//! the remaining attributes and none of the deleted ones. Removing an `email`
//! or a `phone_number` takes the verified flag that vouched for it with it,
//! since the flag describes a value the user no longer holds. A name the user
//! holds no value of removes nothing and a name listed twice is one removal, so
//! the call is idempotent. A name the pool's schema does not declare, one it
//! declares immutable and one it marks required are all refused with 400
//! `InvalidParameterException`; a pool that does not exist with 400
//! `ResourceNotFoundException` and a user the pool does not hold with 400
//! `UserNotFoundException` — and a refused request writes nothing.
//!
//! The same shared plan serves the self-service `DeleteUserAttributes`, so one
//! case below deletes through each operation in turn and requires them to agree
//! on the one stored user.
//!
//! The pools, users and tokens are all real: they come from `CreateUserPool`,
//! `CreateUserPoolClient`, `AdminCreateUser`, `AdminSetUserPassword` and
//! `InitiateAuth`, and every deletion is read back through `AdminGetUser`.
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
/// address, which is how a signed-in user's own client calls the self-service
/// operations.
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
    // A pool that auto-verifies `phone_number` sends that verification over SMS,
    // so it needs the Amazon SNS caller SMS is sent through: `CreateUserPool`
    // refuses the pairing without it.
    if config.auto_verified.contains(&"phone_number") {
        body.insert(
            "SmsConfiguration".to_string(),
            json!({ "SnsCallerArn": "arn:aws:iam::000000000000:role/sms-caller" }),
        );
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

/// `attributes` as the modelled `AttributeListType` an SDK sends.
fn attribute_list(attributes: &[(&str, &str)]) -> Value {
    Value::Array(
        attributes
            .iter()
            .map(|(name, value)| json!({ "Name": name, "Value": value }))
            .collect(),
    )
}

/// Create `username` of `user_pool_id` holding `attributes`.
async fn user(
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
}

/// Call `AdminDeleteUserAttributes` as an SDK does and return its status and
/// body.
async fn delete(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    username: &str,
    names: &[&str],
) -> (u16, Value) {
    dispatch(
        router,
        "AdminDeleteUserAttributes",
        &json!({
            "UserPoolId": user_pool_id,
            "Username": username,
            "UserAttributeNames": names,
        }),
        true,
    )
    .await
}

/// A modelled `AttributeListType` as a name-to-value map.
fn attribute_map(attributes: &Value, operation: &str) -> BTreeMap<String, String> {
    attributes
        .as_array()
        .unwrap_or_else(|| panic!("{operation} reports the stored attributes, got {attributes}"))
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
    attribute_map(&user["UserAttributes"], "AdminGetUser")
}

/// The stored user's `UserLastModifiedDate`, which a deletion that removed a
/// value moves and one that removed nothing leaves where it was.
async fn last_modified(router: &MultiServiceRouter, user_pool_id: &str, username: &str) -> f64 {
    let user = ok(
        router,
        "AdminGetUser",
        &json!({ "UserPoolId": user_pool_id, "Username": username }),
    )
    .await;
    user["UserLastModifiedDate"]
        .as_f64()
        .unwrap_or_else(|| panic!("AdminGetUser reports a last-modified instant, got {user}"))
}

/// One row of the deletion table.
#[derive(Clone, Copy, Debug)]
struct DeleteCase {
    /// What this row asserts.
    case: &'static str,
    /// How the pool this row deletes in is configured.
    config: PoolConfig,
    /// The attributes the user already holds.
    stored: &'static [(&'static str, &'static str)],
    /// The names the row submits in `UserAttributeNames`.
    deleted: &'static [&'static str],
    /// What the stored user must hold afterwards: `Some(value)` for an
    /// attribute that holds it, `None` for one the store must not hold at all.
    expected: &'static [(&'static str, Option<&'static str>)],
}

/// The rows differ along the attribute's kind (standard string, number,
/// custom, a verified value and its flag), the COUNT of names in one call
/// (none, one, three), whether the user HOLDS a value of the named attribute,
/// and whether a name is repeated. The standard attribute names come from this
/// service's `SchemaAttributeType` surface, which every pool starts with.
const DELETE_CASES: &[DeleteCase] = &[
    DeleteCase {
        case: "a standard string attribute the user holds is removed and its neighbour stays",
        config: PoolConfig {
            auto_verified: &[],
            custom_schema: &[],
            required: &[],
        },
        stored: &[("nickname", "Ada"), ("name", "Ada Lovelace")],
        deleted: &["nickname"],
        expected: &[("nickname", None), ("name", Some("Ada Lovelace"))],
    },
    DeleteCase {
        case: "three attributes in one call are all removed",
        config: PoolConfig {
            auto_verified: &[],
            custom_schema: &[],
            required: &[],
        },
        stored: &[
            ("name", "Grace Hopper"),
            ("given_name", "Grace"),
            ("family_name", "Hopper"),
            ("nickname", "Amazing Grace"),
        ],
        deleted: &["name", "given_name", "family_name"],
        expected: &[
            ("name", None),
            ("given_name", None),
            ("family_name", None),
            ("nickname", Some("Amazing Grace")),
        ],
    },
    DeleteCase {
        // `updated_at` is the standard schema's one Number attribute, so this
        // row deletes a value of a different data type.
        case: "a number attribute is removed",
        config: PoolConfig {
            auto_verified: &[],
            custom_schema: &[],
            required: &[],
        },
        stored: &[("updated_at", "1700000000"), ("zoneinfo", "UTC")],
        deleted: &["updated_at"],
        expected: &[("updated_at", None), ("zoneinfo", Some("UTC"))],
    },
    DeleteCase {
        case: "a custom attribute is removed",
        config: PoolConfig {
            auto_verified: &[],
            custom_schema: &["department"],
            required: &[],
        },
        stored: &[("custom:department", "research"), ("locale", "en-GB")],
        deleted: &["custom:department"],
        expected: &[("custom:department", None), ("locale", Some("en-GB"))],
    },
    DeleteCase {
        case: "deleting an email takes the verified flag that vouched for it",
        config: PoolConfig {
            auto_verified: &["email"],
            custom_schema: &[],
            required: &[],
        },
        stored: &[("email", "ada@lovelace.example"), ("email_verified", "true")],
        deleted: &["email"],
        expected: &[("email", None), ("email_verified", None)],
    },
    DeleteCase {
        case: "deleting a phone number takes the verified flag that vouched for it",
        config: PoolConfig {
            auto_verified: &["phone_number"],
            custom_schema: &[],
            required: &[],
        },
        stored: &[
            ("phone_number", "+15550123"),
            ("phone_number_verified", "true"),
        ],
        deleted: &["phone_number"],
        expected: &[("phone_number", None), ("phone_number_verified", None)],
    },
    DeleteCase {
        // The flag is an attribute of its own, so deleting it alone leaves the
        // address it described in place.
        case: "deleting the verified flag alone keeps the value it described",
        config: PoolConfig {
            auto_verified: &["email"],
            custom_schema: &[],
            required: &[],
        },
        stored: &[("email", "ada@lovelace.example"), ("email_verified", "true")],
        deleted: &["email_verified"],
        expected: &[
            ("email", Some("ada@lovelace.example")),
            ("email_verified", None),
        ],
    },
    DeleteCase {
        case: "an attribute the user holds no value of removes nothing",
        config: PoolConfig {
            auto_verified: &[],
            custom_schema: &[],
            required: &[],
        },
        stored: &[("nickname", "Ada")],
        deleted: &["birthdate"],
        expected: &[("birthdate", None), ("nickname", Some("Ada"))],
    },
    DeleteCase {
        case: "a mix of held and unheld names removes exactly what is held",
        config: PoolConfig {
            auto_verified: &[],
            custom_schema: &[],
            required: &[],
        },
        stored: &[("website", "https://lovelace.example"), ("gender", "female")],
        deleted: &["website", "picture"],
        expected: &[
            ("website", None),
            ("picture", None),
            ("gender", Some("female")),
        ],
    },
    DeleteCase {
        // A user holds one value per attribute, so naming it twice asks for
        // the same single removal.
        case: "the same attribute named twice is one removal",
        config: PoolConfig {
            auto_verified: &[],
            custom_schema: &[],
            required: &[],
        },
        stored: &[("nickname", "Ada"), ("profile", "mathematician")],
        deleted: &["nickname", "nickname"],
        expected: &[("nickname", None), ("profile", Some("mathematician"))],
    },
    DeleteCase {
        case: "a call that names no attribute removes nothing",
        config: PoolConfig {
            auto_verified: &[],
            custom_schema: &[],
            required: &[],
        },
        stored: &[("nickname", "Ada")],
        deleted: &[],
        expected: &[("nickname", Some("Ada"))],
    },
];

#[tokio::test]
async fn a_deletion_removes_exactly_the_named_attributes_of_the_named_user() {
    let router = emulator();
    for (index, case) in DELETE_CASES.iter().enumerate() {
        let DeleteCase {
            case,
            config,
            stored,
            deleted,
            expected,
        } = case;
        let username = format!("user-{index}");
        let user_pool_id = pool(&router, &format!("admin-delete-pool-{index}"), *config).await;
        user(&router, &user_pool_id, &username, stored).await;
        // A second user of the same pool, holding the same attributes: a
        // deletion names ONE user, so this one must be untouched.
        let bystander = format!("bystander-{index}");
        user(&router, &user_pool_id, &bystander, stored).await;
        let bystander_before = stored_attributes(&router, &user_pool_id, &bystander).await;

        let (status, payload) = delete(&router, &user_pool_id, &username, deleted).await;
        assert_eq!(
            status, 200,
            "case {case:?}: AdminDeleteUserAttributes must succeed, got {status} with {payload}"
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
        // `sub` is the user's own id: no deletion may take it.
        assert!(
            attributes.contains_key("sub"),
            "case {case:?}: the user must still hold its sub, got {attributes:?}"
        );
        let bystander_after = stored_attributes(&router, &user_pool_id, &bystander).await;
        assert_eq!(
            bystander_after, bystander_before,
            "case {case:?}: another user of the pool must be untouched, but its stored \
             attributes went from {bystander_before:?} to {bystander_after:?}"
        );

        // Deleting the same names again asks for a removal that has already
        // happened, which is accepted and leaves the user as it is.
        let (status, payload) = delete(&router, &user_pool_id, &username, deleted).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a repeated deletion must succeed, got {status} with {payload}"
        );
        let again = stored_attributes(&router, &user_pool_id, &username).await;
        assert_eq!(
            again, attributes,
            "case {case:?}: a repeated deletion must change nothing, but the stored \
             attributes went from {attributes:?} to {again:?}"
        );
    }
}

/// One row of the refusal table: a request the operation must refuse, and the
/// status and wire error code it must answer.
#[derive(Clone, Copy, Debug)]
struct RefusalCase {
    /// What this row asserts.
    case: &'static str,
    /// How the pool this row deletes in is configured.
    config: PoolConfig,
    /// The attributes the user already holds.
    stored: &'static [(&'static str, &'static str)],
    /// The names the row submits.
    deleted: &'static [&'static str],
    status: u16,
    code: &'static str,
}

/// The rows differ along WHICH rule the request breaks: an attribute no schema
/// declares, one the schema freezes, one it marks required, a bad name beside a
/// good one, and a list of more distinct names than the pool's schema declares.
const REFUSAL_CASES: &[RefusalCase] = &[
    RefusalCase {
        case: "an attribute the pool's schema does not declare",
        config: PoolConfig {
            auto_verified: &[],
            custom_schema: &[],
            required: &[],
        },
        stored: &[("nickname", "Ada")],
        deleted: &["custom:undeclared"],
        status: 400,
        code: "InvalidParameterException",
    },
    RefusalCase {
        // `sub` is the user's own id: the standard schema declares it
        // immutable, so no deletion may take it.
        case: "an attribute the schema declares immutable",
        config: PoolConfig {
            auto_verified: &[],
            custom_schema: &[],
            required: &[],
        },
        stored: &[("nickname", "Ada")],
        deleted: &["sub"],
        status: 400,
        code: "InvalidParameterException",
    },
    RefusalCase {
        case: "an attribute the schema marks required",
        config: PoolConfig {
            auto_verified: &[],
            custom_schema: &[],
            required: &["email"],
        },
        stored: &[("email", "ada@lovelace.example")],
        deleted: &["email"],
        status: 400,
        code: "InvalidParameterException",
    },
    RefusalCase {
        case: "one bad name refuses the whole call, including the good one beside it",
        config: PoolConfig {
            auto_verified: &[],
            custom_schema: &[],
            required: &[],
        },
        stored: &[("nickname", "Ada")],
        deleted: &["nickname", "custom:undeclared"],
        status: 400,
        code: "InvalidParameterException",
    },
    RefusalCase {
        // The standard schema declares 20 attributes and this pool adds none,
        // so a list of 21 distinct names cannot name 21 attributes of it.
        case: "more distinct names in one call than the pool's schema declares",
        config: PoolConfig {
            auto_verified: &[],
            custom_schema: &[],
            required: &[],
        },
        stored: &[("nickname", "Ada")],
        deleted: &[
            "name",
            "given_name",
            "family_name",
            "middle_name",
            "nickname",
            "preferred_username",
            "profile",
            "picture",
            "website",
            "email",
            "email_verified",
            "gender",
            "birthdate",
            "zoneinfo",
            "locale",
            "phone_number",
            "phone_number_verified",
            "address",
            "updated_at",
            "sub",
            "custom:n",
        ],
        status: 400,
        code: "InvalidParameterException",
    },
];

#[tokio::test]
async fn a_refused_deletion_answers_the_modelled_error_and_writes_nothing() {
    let router = emulator();
    for (index, case) in REFUSAL_CASES.iter().enumerate() {
        let RefusalCase {
            case,
            config,
            stored,
            deleted,
            status: expected_status,
            code,
        } = case;
        let username = format!("refused-{index}");
        let user_pool_id = pool(&router, &format!("admin-refusal-pool-{index}"), *config).await;
        user(&router, &user_pool_id, &username, stored).await;
        let before = stored_attributes(&router, &user_pool_id, &username).await;
        let modified_before = last_modified(&router, &user_pool_id, &username).await;

        let (status, payload) = delete(&router, &user_pool_id, &username, deleted).await;
        assert_eq!(
            (status, error_code(&payload)),
            (*expected_status, *code),
            "case {case:?}: expected {expected_status} {code}, got {status} with {payload}"
        );

        let after = stored_attributes(&router, &user_pool_id, &username).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused deletion must write nothing, but the stored attributes \
             went from {before:?} to {after:?}"
        );
        let modified_after = last_modified(&router, &user_pool_id, &username).await;
        assert_eq!(
            modified_after, modified_before,
            "case {case:?}: a refused deletion must not touch the user, but its last-modified \
             instant went from {modified_before} to {modified_after}"
        );
    }
}

#[tokio::test]
async fn a_deletion_of_a_resource_that_does_not_exist_is_refused() {
    let router = emulator();
    let user_pool_id = pool(&router, "admin-missing-pool", PoolConfig::default()).await;
    user(&router, &user_pool_id, "ada", &[("nickname", "Ada")]).await;

    // A pool id of the modelled `UserPoolIdType` form that no pool was created
    // under, so the request names a pool this emulator does not hold.
    let absent_pool = format!("{user_pool_id}zz");
    for (case, pool_id, username, expected) in [
        (
            "a user pool that does not exist",
            absent_pool.as_str(),
            "ada",
            (400u16, "ResourceNotFoundException"),
        ),
        (
            "a user the pool does not hold",
            user_pool_id.as_str(),
            "grace",
            (400, "UserNotFoundException"),
        ),
    ] {
        let (status, payload) = delete(&router, pool_id, username, &["nickname"]).await;
        assert_eq!(
            (status, error_code(&payload)),
            expected,
            "case {case:?}: expected {expected:?}, got {status} with {payload}"
        );
    }

    // Nothing the refused calls named reached the stored user.
    let attributes = stored_attributes(&router, &user_pool_id, "ada").await;
    let nickname = attributes.get("nickname");
    assert_eq!(
        nickname.map(String::as_str),
        Some("Ada"),
        "a refused deletion must write nothing, got nickname {nickname:?} in {attributes:?}"
    );
}

#[tokio::test]
async fn a_deleted_attribute_can_be_set_again_by_an_administrator() {
    let router = emulator();
    let user_pool_id = pool(&router, "admin-round-trip-pool", PoolConfig::default()).await;
    user(&router, &user_pool_id, "ada", &[("nickname", "Ada")]).await;

    let (status, payload) = delete(&router, &user_pool_id, "ada", &["nickname"]).await;
    assert_eq!(
        status, 200,
        "the deletion must succeed, got {status} with {payload}"
    );
    let attributes = stored_attributes(&router, &user_pool_id, "ada").await;
    assert!(
        !attributes.contains_key("nickname"),
        "the deleted attribute must be gone, got {attributes:?}"
    );

    ok(
        &router,
        "AdminUpdateUserAttributes",
        &json!({
            "UserPoolId": user_pool_id,
            "Username": "ada",
            "UserAttributes": attribute_list(&[("nickname", "Ada again")]),
        }),
    )
    .await;
    let attributes = stored_attributes(&router, &user_pool_id, "ada").await;
    let nickname = attributes.get("nickname");
    assert_eq!(
        nickname.map(String::as_str),
        Some("Ada again"),
        "a deleted attribute must be settable again, got {nickname:?} in {attributes:?}"
    );
}

#[tokio::test]
async fn the_administrative_and_self_service_deletions_agree_on_the_stored_user() {
    let router = emulator();
    let user_pool_id = pool(&router, "shared-plan-pool", PoolConfig::default()).await;
    let client = ok(
        &router,
        "CreateUserPoolClient",
        &json!({
            "UserPoolId": user_pool_id,
            "ClientName": "attribute-client",
            "ExplicitAuthFlows": ["ALLOW_USER_PASSWORD_AUTH", "ALLOW_REFRESH_TOKEN_AUTH"],
        }),
    )
    .await;
    let client_id = client["UserPoolClient"]["ClientId"]
        .as_str()
        .expect("a minted client id")
        .to_string();
    user(
        &router,
        &user_pool_id,
        "ada",
        &[
            ("nickname", "Ada"),
            ("profile", "mathematician"),
            ("locale", "en-GB"),
        ],
    )
    .await;
    ok(
        &router,
        "AdminSetUserPassword",
        &json!({
            "UserPoolId": user_pool_id,
            "Username": "ada",
            "Password": GOOD_PASSWORD,
            "Permanent": true,
        }),
    )
    .await;
    let (status, payload) = dispatch(
        &router,
        "InitiateAuth",
        &json!({
            "ClientId": client_id,
            "AuthFlow": "USER_PASSWORD_AUTH",
            "AuthParameters": { "USERNAME": "ada", "PASSWORD": GOOD_PASSWORD },
        }),
        false,
    )
    .await;
    assert_eq!(
        status, 200,
        "signing ada in must succeed, got {status} with {payload}"
    );
    let access_token = payload["AuthenticationResult"]["AccessToken"]
        .as_str()
        .expect("an access token")
        .to_string();

    // The administrative deletion removes one attribute.
    let (status, payload) = delete(&router, &user_pool_id, "ada", &["nickname"]).await;
    assert_eq!(
        status, 200,
        "the administrative deletion must succeed, got {status} with {payload}"
    );
    // The self-service deletion removes another from the SAME stored user.
    let (status, payload) = dispatch(
        &router,
        "DeleteUserAttributes",
        &json!({ "AccessToken": access_token, "UserAttributeNames": ["profile"] }),
        false,
    )
    .await;
    assert_eq!(
        status, 200,
        "the self-service deletion must succeed, got {status} with {payload}"
    );

    let attributes = stored_attributes(&router, &user_pool_id, "ada").await;
    for (name, expected) in [
        ("nickname", None),
        ("profile", None),
        ("locale", Some("en-GB")),
    ] {
        let actual = attributes.get(name);
        assert_eq!(
            actual.map(String::as_str),
            expected,
            "both deletions apply to one stored user: expected {name} to be {expected:?}, \
             got {actual:?} in {attributes:?}"
        );
    }

    // Both operations judge a request by the same rules, so the attribute the
    // schema freezes is refused through either one.
    let (status, payload) = delete(&router, &user_pool_id, "ada", &["sub"]).await;
    assert_eq!(
        (status, error_code(&payload)),
        (400, "InvalidParameterException"),
        "the administrative deletion must refuse an immutable attribute, \
         got {status} with {payload}"
    );
    let (status, payload) = dispatch(
        &router,
        "DeleteUserAttributes",
        &json!({ "AccessToken": access_token, "UserAttributeNames": ["sub"] }),
        false,
    )
    .await;
    assert_eq!(
        (status, error_code(&payload)),
        (400, "InvalidParameterException"),
        "the self-service deletion must refuse an immutable attribute, \
         got {status} with {payload}"
    );
}
