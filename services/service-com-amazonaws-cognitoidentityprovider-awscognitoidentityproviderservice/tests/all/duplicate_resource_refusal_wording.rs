// SPDX-License-Identifier: CC0-1.0

//! The WORDING of the refusal a create gets when the resource already exists,
//! and of the refusal a write gets when the pool's schema freezes the attribute
//! it names, over the wire through the router exactly as an SDK drives it
//! (`POST /` with `X-Amz-Target: AWSCognitoIdentityProviderService.<Op>`).
//!
//! The rule under test: each of these refusals carries Amazon Cognito's own
//! message for its error class — that class's fixed text, and only the slots the
//! text declares, each filled from the REQUEST. A message that declares no slot
//! interpolates nothing, so it reports neither the value the caller asked for
//! nor anything about the stored resource it collided with; a message that
//! declares slots fills them from the request and from nothing else, so it never
//! discloses a stored value the caller did not send.
//!
//! One test per error class, each a table whose rows differ along what the rule
//! generalises over: the FORM of the name that collided (length, script,
//! punctuation, prefix), the configuration of the pool it collided in, the data
//! type of the attribute, the Region the pool lives in, which of two pools or
//! app clients the collision happened in, and — for the frozen attribute — which
//! operation attempted the change.
//!
//! Every row names the case it asserts and every assertion reports the message
//! the call actually answered.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice as cognito_idp;

/// The Region every call that does not name another one is signed for.
const DEFAULT_REGION: &str = "us-east-1";

/// The emulator's own endpoint: an address, so an unsigned request addressed to
/// it names no service in its `Host` header and `X-Amz-Target` is the only
/// routing signal left. That is how a signed-in user's own client calls the
/// self-service operations, which the model marks `@optionalAuth`.
const EMULATOR_HOST: &str = "127.0.0.1:9297";

/// A password satisfying a default pool's password policy.
const GOOD_PASSWORD: &str = "Passw0rd!2024";

/// One emulator with just this service mounted.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(cognito_idp::EmulatedService::new(engine));
    router.set_request_authentication(false);
    router
}

/// A SigV4 credential scope for `region`, which is what routes an
/// administrative call to this service and names the Region it is made in. The
/// signature itself is never checked here.
fn authorization(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/cognito-idp/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=0000"
    )
}

/// Call `operation` signed for `region` and return its status and JSON response
/// body.
async fn call_in(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("cognito-idp.{region}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "x-amz-target",
            format!("AWSCognitoIdentityProviderService.{operation}"),
        )
        .header("authorization", authorization(region))
        .body(Bytes::from(body.to_string()))
        .expect("the request is well formed");
    answer(router, operation, request).await
}

/// Call `operation` without a credential scope, as a signed-in user's own client
/// does, and return its status and JSON response body.
async fn call_unsigned(
    router: &MultiServiceRouter,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", EMULATOR_HOST)
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "x-amz-target",
            format!("AWSCognitoIdentityProviderService.{operation}"),
        )
        .body(Bytes::from(body.to_string()))
        .expect("the request is well formed");
    answer(router, operation, request).await
}

/// Dispatch `request` and read the status and JSON body back out.
async fn answer(
    router: &MultiServiceRouter,
    operation: &str,
    request: http::Request<Bytes>,
) -> (u16, Value) {
    let response = router.dispatch(request).await;
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

/// Call `operation` in [`DEFAULT_REGION`] and return its status and body.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    call_in(router, DEFAULT_REGION, operation, body).await
}

/// Call `operation` in `region`, requiring success, and return its body.
async fn ok_in(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: &Value,
) -> Value {
    let (status, payload) = call_in(router, region, operation, body).await;
    assert_eq!(
        status, 200,
        "{operation} must succeed, got {status} with {payload}"
    );
    payload
}

/// Call `operation` in [`DEFAULT_REGION`], requiring success, and return its
/// body.
async fn ok(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    ok_in(router, DEFAULT_REGION, operation, body).await
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

/// Create a pool in `region` whose request carries `extra` beside `PoolName`,
/// and return its minted `UserPoolId`.
async fn pool_in(
    router: &MultiServiceRouter,
    region: &str,
    name: &str,
    extra: Value,
) -> String {
    let mut body = Map::new();
    body.insert("PoolName".to_string(), json!(name));
    if let Value::Object(members) = extra {
        for (member, value) in members {
            body.insert(member, value);
        }
    }
    let created = ok_in(router, region, "CreateUserPool", &Value::Object(body)).await;
    created["UserPool"]["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateUserPool must mint an id, got {created}"))
        .to_string()
}

/// Create a pool in [`DEFAULT_REGION`] and return its minted `UserPoolId`.
async fn pool(router: &MultiServiceRouter, name: &str, extra: Value) -> String {
    pool_in(router, DEFAULT_REGION, name, extra).await
}

/// Create an app client in `user_pool_id` and return its minted `ClientId`.
/// `flows` is sent as `ExplicitAuthFlows` when it is not empty.
async fn client(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    name: &str,
    flows: &[&str],
) -> String {
    let mut body = Map::new();
    body.insert("UserPoolId".to_string(), json!(user_pool_id));
    body.insert("ClientName".to_string(), json!(name));
    if !flows.is_empty() {
        body.insert("ExplicitAuthFlows".to_string(), json!(flows));
    }
    let created = ok(router, "CreateUserPoolClient", &Value::Object(body)).await;
    created["UserPoolClient"]["ClientId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateUserPoolClient must mint a client id, got {created}"))
        .to_string()
}

// ---------------------------------------------------------------------------
// A sign-up for an account that already exists
// ---------------------------------------------------------------------------

/// The one sentence a `UsernameExistsException` from a self-service sign-up
/// carries, whatever name was submitted and however the pool resolves it.
const USER_ALREADY_EXISTS: &str = "User already exists";

/// One sign-up row: how the pool resolves sign-in names, the name that registers
/// the account and the name that collides with it.
struct SignUpCase {
    case: &'static str,
    /// The members the pool is created with beside `PoolName`.
    configuration: Value,
    first: String,
    second: String,
}

/// The rows. They differ along the pool's sign-in configuration (the username
/// itself, a case-insensitive username, and `email` as the sign-in attribute —
/// the forms `UsernameAttributes` and `UsernameConfiguration.CaseSensitive`
/// admit) and along the FORM of the name: both published length bounds of
/// `UsernameType` (length 1..128, from
/// `smithy_query(query="shape",shape="UsernameType")`), a non-ASCII name, an
/// email and a name re-cased.
fn sign_up_cases() -> Vec<SignUpCase> {
    vec![
        SignUpCase {
            case: "the submitted username is the sign-in name",
            configuration: json!({}),
            first: "alias-user-one".to_string(),
            second: "alias-user-one".to_string(),
        },
        SignUpCase {
            case: "the one-character length minimum",
            configuration: json!({}),
            first: "z".to_string(),
            second: "z".to_string(),
        },
        SignUpCase {
            case: "the 128-character length maximum",
            configuration: json!({}),
            first: "u".repeat(128),
            second: "u".repeat(128),
        },
        SignUpCase {
            case: "a non-ASCII username",
            configuration: json!({}),
            first: "ユーザー①".to_string(),
            second: "ユーザー①".to_string(),
        },
        SignUpCase {
            case: "the same name re-cased on a case-insensitive pool",
            configuration: json!({ "UsernameConfiguration": { "CaseSensitive": false } }),
            first: "Ada".to_string(),
            second: "aDA".to_string(),
        },
        SignUpCase {
            case: "the same email on a pool whose users sign in with their email",
            configuration: json!({ "UsernameAttributes": ["email"] }),
            first: "ada@lovelace.example".to_string(),
            second: "ada@lovelace.example".to_string(),
        },
    ]
}

#[tokio::test]
async fn a_sign_up_for_an_existing_account_is_refused_with_one_fixed_sentence() {
    let router = router();

    for (index, row) in sign_up_cases().into_iter().enumerate() {
        let case = row.case;
        let user_pool_id = pool(
            &router,
            &format!("sign-up-exists-{index}"),
            row.configuration,
        )
        .await;
        let client_id = client(&router, &user_pool_id, &format!("app-{index}"), &[]).await;

        ok(
            &router,
            "SignUp",
            &json!({
                "ClientId": client_id,
                "Username": row.first,
                "Password": GOOD_PASSWORD,
            }),
        )
        .await;

        let (status, payload) = call(
            &router,
            "SignUp",
            &json!({
                "ClientId": client_id,
                "Username": row.second,
                "Password": GOOD_PASSWORD,
            }),
        )
        .await;
        assert_eq!(
            (status, error_code(&payload)),
            (400, "UsernameExistsException"),
            "case {case:?}: expected a 400 UsernameExistsException, got {status} with {payload}"
        );
        let message = error_message(&payload);
        assert_eq!(
            message, USER_ALREADY_EXISTS,
            "case {case:?}: expected {USER_ALREADY_EXISTS:?}, got {message:?}"
        );
        for (what, value) in [
            ("the submitted name", row.second.as_str()),
            ("the registered name", row.first.as_str()),
            ("the pool", user_pool_id.as_str()),
            ("the app client", client_id.as_str()),
        ] {
            assert!(
                !message.contains(value),
                "case {case:?}: the refusal must not name {what} ({value:?}), got {message:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// A group name the pool already holds
// ---------------------------------------------------------------------------

/// The one sentence a `GroupExistsException` carries.
const GROUP_ALREADY_EXISTS: &str = "A group with the name already exists.";

#[tokio::test]
async fn a_duplicate_group_name_is_refused_with_one_fixed_sentence() {
    let router = router();
    let user_pool_id = pool(&router, "duplicate-group-pool", json!({})).await;

    // The names span what `GroupNameType` admits (length 1..128, pattern
    // `^[\p{L}\p{M}\p{S}\p{N}\p{P}]+$`, from
    // `smithy_query(query="shape",shape="GroupNameType")`): a plain name, both
    // published length bounds, a non-ASCII name and a punctuation-heavy one.
    let group_names = [
        ("a plain name", "staff".to_string()),
        ("the one-character length minimum", "z".to_string()),
        ("the 128-character length maximum", "g".repeat(128)),
        ("a non-ASCII name", "グループ①".to_string()),
        ("a punctuation-heavy name", "ops.team-1_a/x".to_string()),
    ];

    for (case, group_name) in &group_names {
        ok(
            &router,
            "CreateGroup",
            &json!({ "UserPoolId": user_pool_id, "GroupName": group_name }),
        )
        .await;

        // A second group under the same name, with a description the stored one
        // does not carry, so the collision is on the name alone.
        let (status, payload) = call(
            &router,
            "CreateGroup",
            &json!({
                "UserPoolId": user_pool_id,
                "GroupName": group_name,
                "Description": "a second group under a name the pool holds",
            }),
        )
        .await;
        assert_eq!(
            (status, error_code(&payload)),
            (400, "GroupExistsException"),
            "case {case:?}: expected a 400 GroupExistsException, got {status} with {payload}"
        );
        let message = error_message(&payload);
        assert_eq!(
            message, GROUP_ALREADY_EXISTS,
            "case {case:?}: expected {GROUP_ALREADY_EXISTS:?}, got {message:?}"
        );
        assert!(
            !message.contains(group_name.as_str()),
            "case {case:?}: the refusal must not name the group, got {message:?}"
        );
        assert!(
            !message.contains(user_pool_id.as_str()),
            "case {case:?}: the refusal must not name the pool, got {message:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// An identity provider the pool already supports
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_duplicate_identity_provider_names_the_provider_and_the_tenant() {
    let router = router();
    // TWO pools, so the tenant slot is shown to follow the pool the request
    // named rather than being fixed to one pool.
    let first_pool = pool(&router, "duplicate-provider-pool-one", json!({})).await;
    let second_pool = pool(&router, "duplicate-provider-pool-two", json!({})).await;

    // `ProviderType` spans the modelled catalog
    // (`smithy_query(query="shape",shape="IdentityProviderTypeType")`), and the
    // names span `ProviderNameType`'s length 1..32 including its maximum.
    let providers = [
        (
            "a social provider",
            "Facebook".to_string(),
            "Facebook",
            json!({ "client_id": "1", "client_secret": "2", "authorize_scopes": "email" }),
        ),
        (
            "another social provider",
            "Google".to_string(),
            "Google",
            json!({ "client_id": "3", "client_secret": "4", "authorize_scopes": "openid" }),
        ),
        (
            "a SAML provider",
            "CorpSAML".to_string(),
            "SAML",
            json!({ "MetadataFile": "<EntityDescriptor/>" }),
        ),
        (
            "a name at the 32-character length maximum",
            "s".repeat(32),
            "OIDC",
            json!({ "oidc_issuer": "https://auth.example.com" }),
        ),
    ];

    for user_pool_id in [&first_pool, &second_pool] {
        for (case, provider_name, provider_type, details) in &providers {
            ok(
                &router,
                "CreateIdentityProvider",
                &json!({
                    "UserPoolId": user_pool_id,
                    "ProviderName": provider_name,
                    "ProviderType": provider_type,
                    "ProviderDetails": details,
                }),
            )
            .await;

            // A second provider under the same name, of a DIFFERENT type: the
            // name is what collides.
            let (status, payload) = call(
                &router,
                "CreateIdentityProvider",
                &json!({
                    "UserPoolId": user_pool_id,
                    "ProviderName": provider_name,
                    "ProviderType": "OIDC",
                    "ProviderDetails": { "oidc_issuer": "https://second.example.com" },
                }),
            )
            .await;
            assert_eq!(
                (status, error_code(&payload)),
                (400, "DuplicateProviderException"),
                "case {case:?} in {user_pool_id}: expected a 400 DuplicateProviderException, got \
                 {status} with {payload}"
            );
            let message = error_message(&payload);
            let expected = format!("{provider_name} already exists for tenant {user_pool_id}.");
            assert_eq!(
                message, expected,
                "case {case:?} in {user_pool_id}: expected {expected:?}, got {message:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// A managed login style the app client already has
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_duplicate_managed_login_style_names_the_app_client() {
    let router = router();
    let first_pool = pool(&router, "duplicate-branding-pool-one", json!({})).await;
    let second_pool = pool(&router, "duplicate-branding-pool-two", json!({})).await;

    // The rows differ along the app client the style belongs to and the pool
    // that client lives in: the slot follows the `ClientId` the request named.
    let clients = [
        (
            "the first app client of a pool",
            client(&router, &first_pool, "branding-one", &[]).await,
            first_pool.clone(),
        ),
        (
            "a second app client of the same pool",
            client(&router, &first_pool, "branding-two", &[]).await,
            first_pool.clone(),
        ),
        (
            "an app client of another pool",
            client(&router, &second_pool, "branding-three", &[]).await,
            second_pool.clone(),
        ),
    ];

    for (case, client_id, user_pool_id) in &clients {
        ok(
            &router,
            "CreateManagedLoginBranding",
            &json!({
                "UserPoolId": user_pool_id,
                "ClientId": client_id,
                "UseCognitoProvidedValues": true,
            }),
        )
        .await;

        let (status, payload) = call(
            &router,
            "CreateManagedLoginBranding",
            &json!({
                "UserPoolId": user_pool_id,
                "ClientId": client_id,
                "Settings": { "components": { "secondaryButton": { "lightMode": {} } } },
            }),
        )
        .await;
        assert_eq!(
            (status, error_code(&payload)),
            (400, "ManagedLoginBrandingExistsException"),
            "case {case:?}: expected a 400 ManagedLoginBrandingExistsException, got {status} with \
             {payload}"
        );
        let message = error_message(&payload);
        let expected = format!("A ManagedLoginBranding already exists for client {client_id}");
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
        assert!(
            !message.contains(user_pool_id.as_str()),
            "case {case:?}: the refusal must not name the pool, got {message:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// A schema attribute name the pool already declares
// ---------------------------------------------------------------------------

/// One `AddCustomAttributes` row: the attribute the request carries, the bare
/// name the refusal must report and the data type it must report in upper case.
struct SchemaCase {
    case: &'static str,
    requested: Value,
    bare_name: &'static str,
    data_type: &'static str,
}

/// The rows. The data types are the whole `AttributeDataType` enum (`String |
/// Number | DateTime | Boolean`, from
/// `smithy_query(query="shape",shape="AttributeDataType")`), including the entry
/// a request that names none defaults to, and the name forms span what
/// `CustomAttributeNameType` admits (length 1..20, pattern
/// `^[\p{L}\p{M}\p{S}\p{N}\p{P}]+$`, from
/// `smithy_query(query="shape",shape="CustomAttributeNameType")`) together with
/// the two prefixes `SchemaAttributeType.Name` documents a stored attribute
/// carries.
fn schema_cases() -> Vec<SchemaCase> {
    vec![
        SchemaCase {
            case: "an attribute whose data type is defaulted",
            requested: json!({ "Name": "department" }),
            bare_name: "department",
            data_type: "STRING",
        },
        SchemaCase {
            case: "a Number attribute",
            requested: json!({ "Name": "seats", "AttributeDataType": "Number" }),
            bare_name: "seats",
            data_type: "NUMBER",
        },
        SchemaCase {
            case: "a DateTime attribute",
            requested: json!({ "Name": "joined", "AttributeDataType": "DateTime" }),
            bare_name: "joined",
            data_type: "DATETIME",
        },
        SchemaCase {
            case: "a Boolean attribute",
            requested: json!({ "Name": "member", "AttributeDataType": "Boolean" }),
            bare_name: "member",
            data_type: "BOOLEAN",
        },
        SchemaCase {
            case: "a name that already carries the custom prefix",
            requested: json!({ "Name": "custom:prefixed", "AttributeDataType": "String" }),
            bare_name: "prefixed",
            data_type: "STRING",
        },
        SchemaCase {
            case: "a developer-only attribute, stored under the dev prefix",
            requested: json!({ "Name": "internal", "DeveloperOnlyAttribute": true }),
            bare_name: "internal",
            data_type: "STRING",
        },
        SchemaCase {
            case: "a non-ASCII name",
            requested: json!({ "Name": "名前", "AttributeDataType": "String" }),
            bare_name: "名前",
            data_type: "STRING",
        },
        SchemaCase {
            case: "the one-character name minimum",
            requested: json!({ "Name": "q", "AttributeDataType": "Number" }),
            bare_name: "q",
            data_type: "NUMBER",
        },
    ]
}

/// The record the refusal owes for `bare_name` of `data_type`.
fn not_unique_message(bare_name: &str, data_type: &str) -> String {
    format!(
        "Entity Type: USER, Message: AttributeName: {bare_name}, DataType: {data_type}, Error: \
         Attribute name is not unique"
    )
}

#[tokio::test]
async fn a_schema_attribute_name_already_declared_is_refused_with_the_structured_record() {
    let router = router();
    let user_pool_id = pool(&router, "duplicate-schema-pool", json!({})).await;

    for row in schema_cases() {
        let case = row.case;
        let request =
            json!({ "UserPoolId": user_pool_id, "CustomAttributes": [row.requested.clone()] });
        ok(&router, "AddCustomAttributes", &request).await;

        // The SAME request a second time: the name is now one the pool's schema
        // declares.
        let (status, payload) = call(&router, "AddCustomAttributes", &request).await;
        assert_eq!(
            (status, error_code(&payload)),
            (400, "InvalidParameterException"),
            "case {case:?}: expected a 400 InvalidParameterException, got {status} with {payload}"
        );
        let message = error_message(&payload);
        let expected = not_unique_message(row.bare_name, row.data_type);
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
        // The record reports the BARE name, so neither prefix a stored
        // attribute carries appears in it.
        for prefix in ["custom:", "dev:"] {
            assert!(
                !message.contains(prefix),
                "case {case:?}: the record reports the bare name, got {message:?}"
            );
        }
    }

    // A name repeated WITHIN one request collides the same way: it is taken by
    // the entry ahead of it, so the same record is owed.
    let (status, payload) = call(
        &router,
        "AddCustomAttributes",
        &json!({
            "UserPoolId": user_pool_id,
            "CustomAttributes": [
                { "Name": "twice", "AttributeDataType": "Number" },
                { "Name": "twice", "AttributeDataType": "Number" },
            ],
        }),
    )
    .await;
    let case = "the same name twice in one request";
    assert_eq!(
        (status, error_code(&payload)),
        (400, "InvalidParameterException"),
        "case {case:?}: expected a 400 InvalidParameterException, got {status} with {payload}"
    );
    let message = error_message(&payload);
    let expected = not_unique_message("twice", "NUMBER");
    assert_eq!(
        message, expected,
        "case {case:?}: expected {expected:?}, got {message:?}"
    );
}

// ---------------------------------------------------------------------------
// A domain another pool already holds
// ---------------------------------------------------------------------------

/// The one sentence the refusal of an already-taken domain carries.
const DOMAIN_ALREADY_ASSOCIATED: &str = "Domain already exists.";

#[tokio::test]
async fn a_domain_another_pool_holds_is_refused_without_naming_that_pool() {
    let router = router();

    // The domains span what `DomainType` admits (length 1..63, pattern
    // `^[a-z0-9](?:[a-z0-9\-]{0,61}[a-z0-9])?$`, from
    // `smithy_query(query="shape",shape="DomainType")`): a plain prefix, both
    // published length bounds and a hyphenated one. Each row runs on its own
    // pair of pools, since a pool holds at most one prefix domain.
    let longest = format!("d{}", "x".repeat(62));
    let domains = [
        ("a plain prefix", "sim-login".to_string()),
        ("the one-character length minimum", "a".to_string()),
        ("the 63-character length maximum", longest),
        ("a hyphenated prefix", "login-2-example".to_string()),
    ];

    for (index, (case, domain)) in domains.iter().enumerate() {
        let holder = pool(&router, &format!("domain-holder-{index}"), json!({})).await;
        let claimant = pool(&router, &format!("domain-claimant-{index}"), json!({})).await;
        ok(
            &router,
            "CreateUserPoolDomain",
            &json!({ "UserPoolId": holder, "Domain": domain }),
        )
        .await;

        let (status, payload) = call(
            &router,
            "CreateUserPoolDomain",
            &json!({ "UserPoolId": claimant, "Domain": domain }),
        )
        .await;
        assert_eq!(
            (status, error_code(&payload)),
            (400, "InvalidParameterException"),
            "case {case:?}: expected a 400 InvalidParameterException, got {status} with {payload}"
        );
        let message = error_message(&payload);
        assert_eq!(
            message, DOMAIN_ALREADY_ASSOCIATED,
            "case {case:?}: expected {DOMAIN_ALREADY_ASSOCIATED:?}, got {message:?}"
        );
        // The fixed text above is what proves the sentence interpolates
        // nothing; these name the values it must not disclose. A domain of one
        // or two characters can occur inside an ordinary English word, so the
        // substring check is made for the rows whose domain cannot.
        let mut forbidden = vec![
            ("the pool holding it", holder.as_str()),
            ("the pool that asked", claimant.as_str()),
        ];
        if domain.len() > 3 {
            forbidden.push(("the domain", domain.as_str()));
        }
        for (what, value) in forbidden {
            assert!(
                !message.contains(value),
                "case {case:?}: the refusal must not name {what} ({value:?}), got {message:?}"
            );
        }

        // The domain still belongs to the pool that took it: the refusal moved
        // nothing.
        let read = ok(
            &router,
            "DescribeUserPoolDomain",
            &json!({ "Domain": domain }),
        )
        .await;
        assert_eq!(
            read["DomainDescription"]["UserPoolId"],
            json!(holder),
            "case {case:?}: the stored domain must still belong to the pool that took it, got \
             {read}"
        );
    }
}

// ---------------------------------------------------------------------------
// A replica asked for in the pool's own Region
// ---------------------------------------------------------------------------

/// The one sentence a replica asked for in the pool's own Region is refused
/// with.
const REPLICA_NEEDS_ANOTHER_REGION: &str = "Replica must be created in a different region";

#[tokio::test]
async fn a_replica_in_the_pools_own_region_is_refused_with_the_fixed_rule() {
    let router = router();

    // A pool's Region is the one its create was signed for, so the rows differ
    // along the Region the pool lives in — which is also the Region the replica
    // asks for, and the one the message must NOT interpolate.
    for (case, region) in [
        ("a pool in us-east-1", "us-east-1"),
        ("a pool in us-west-2", "us-west-2"),
        ("a pool in eu-central-1", "eu-central-1"),
    ] {
        // The pool is encrypted under a customer-managed key, which is what a
        // pool has to hold to be replicated at all: "you must use a
        // multi-region key to enable multi-region replication for a user pool"
        // (`smithy_query(query="doc",shape="KeyConfigurationType",member="KeyType")`),
        // so the Region the replica asks for is the only thing left to refuse.
        let user_pool_id = pool_in(
            &router,
            region,
            &format!("replica-pool-{region}"),
            json!({
                "KeyConfiguration": {
                    "KeyType": "CUSTOMER_MANAGED_KEY",
                    "KmsKeyArn": "arn:aws:kms:us-east-1:123456789012:key/multi-region-example",
                },
            }),
        )
        .await;
        let (status, payload) = call_in(
            &router,
            region,
            "CreateUserPoolReplica",
            &json!({ "UserPoolId": user_pool_id, "RegionName": region }),
        )
        .await;
        assert_eq!(
            (status, error_code(&payload)),
            (400, "InvalidParameterException"),
            "case {case:?}: expected a 400 InvalidParameterException, got {status} with {payload}"
        );
        let message = error_message(&payload);
        assert_eq!(
            message, REPLICA_NEEDS_ANOTHER_REGION,
            "case {case:?}: expected {REPLICA_NEEDS_ANOTHER_REGION:?}, got {message:?}"
        );
        assert!(
            !message.contains(region),
            "case {case:?}: the refusal must not name the Region asked for, got {message:?}"
        );
        assert!(
            !message.contains(user_pool_id.as_str()),
            "case {case:?}: the refusal must not name the pool, got {message:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// An attribute the pool's schema freezes
// ---------------------------------------------------------------------------

/// One frozen-attribute row: the operation that attempts the change and the
/// attribute it names.
struct FrozenCase {
    case: &'static str,
    operation: &'static str,
    attribute: &'static str,
    /// Whether the caller is the signed-in user, named by an access token,
    /// rather than an administrator naming the pool and the user.
    self_service: bool,
    /// Whether the row submits a VALUE for the attribute (an update) rather
    /// than its name alone (a deletion).
    updates: bool,
}

/// The request `row` sends against the pool, user and access token the fixture
/// holds.
fn frozen_request(row: &FrozenCase, user_pool_id: &str, username: &str, token: &str) -> Value {
    let value = "a value the schema will not take";
    match (row.updates, row.self_service) {
        (false, true) => json!({
            "AccessToken": token,
            "UserAttributeNames": [row.attribute],
        }),
        (false, false) => json!({
            "UserPoolId": user_pool_id,
            "Username": username,
            "UserAttributeNames": [row.attribute],
        }),
        (true, true) => json!({
            "AccessToken": token,
            "UserAttributes": [{ "Name": row.attribute, "Value": value }],
        }),
        (true, false) => json!({
            "UserPoolId": user_pool_id,
            "Username": username,
            "UserAttributes": [{ "Name": row.attribute, "Value": value }],
        }),
    }
}

/// The rows: every implemented operation that writes a user's attributes, each
/// against a standard attribute the pool's schema freezes (`sub`, the user's own
/// id) and a custom one the pool declared with `Mutable: false`.
fn frozen_cases() -> Vec<FrozenCase> {
    let mut rows = Vec::new();
    for attribute in ["sub", "custom:frozen"] {
        rows.push(FrozenCase {
            case: "the self-service deletion",
            operation: "DeleteUserAttributes",
            attribute,
            self_service: true,
            updates: false,
        });
        rows.push(FrozenCase {
            case: "the administrative deletion",
            operation: "AdminDeleteUserAttributes",
            attribute,
            self_service: false,
            updates: false,
        });
        rows.push(FrozenCase {
            case: "the self-service update",
            operation: "UpdateUserAttributes",
            attribute,
            self_service: true,
            updates: true,
        });
        rows.push(FrozenCase {
            case: "the administrative update",
            operation: "AdminUpdateUserAttributes",
            attribute,
            self_service: false,
            updates: true,
        });
    }
    rows
}

#[tokio::test]
async fn an_attribute_the_schema_freezes_is_refused_by_name_on_every_write() {
    let router = router();
    let user_pool_id = pool(
        &router,
        "frozen-attribute-pool",
        json!({
            "Schema": [
                { "Name": "frozen", "AttributeDataType": "String", "Mutable": false },
                { "Name": "editable", "AttributeDataType": "String", "Mutable": true },
            ],
        }),
    )
    .await;
    let client_id = client(
        &router,
        &user_pool_id,
        "frozen-attribute-client",
        &["ALLOW_USER_PASSWORD_AUTH"],
    )
    .await;
    let username = "ada";
    ok(
        &router,
        "AdminCreateUser",
        &json!({ "UserPoolId": user_pool_id, "Username": username }),
    )
    .await;
    ok(
        &router,
        "AdminSetUserPassword",
        &json!({
            "UserPoolId": user_pool_id,
            "Username": username,
            "Password": GOOD_PASSWORD,
            "Permanent": true,
        }),
    )
    .await;
    let (status, payload) = call_unsigned(
        &router,
        "InitiateAuth",
        &json!({
            "ClientId": client_id,
            "AuthFlow": "USER_PASSWORD_AUTH",
            "AuthParameters": { "USERNAME": username, "PASSWORD": GOOD_PASSWORD },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "signing the user in must succeed, got {status} with {payload}"
    );
    let token = payload["AuthenticationResult"]["AccessToken"]
        .as_str()
        .unwrap_or_else(|| panic!("a completed sign-in returns an access token, got {payload}"))
        .to_string();

    for row in frozen_cases() {
        let case = format!("{} of {}", row.case, row.attribute);
        let body = frozen_request(&row, &user_pool_id, username, &token);
        let (status, payload) = if row.self_service {
            call_unsigned(&router, row.operation, &body).await
        } else {
            call(&router, row.operation, &body).await
        };
        assert_eq!(
            (status, error_code(&payload)),
            (400, "InvalidParameterException"),
            "case {case:?}: expected a 400 InvalidParameterException, got {status} with {payload}"
        );
        let message = error_message(&payload);
        // A write that submits a VALUE is validated against the pool's schema
        // value by value, so it is refused with that report's own line for the
        // finding; a DELETION, which carries names alone, is refused with the
        // sentence about the attribute. Both name the attribute and nothing else.
        let expected = if row.updates {
            format!("user.{}: Attribute cannot be updated.\n", row.attribute)
        } else {
            format!("Cannot modify the non-mutable attribute {}", row.attribute)
        };
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
        assert!(
            !message.contains(user_pool_id.as_str()) && !message.contains(username),
            "case {case:?}: the refusal reports the attribute alone, got {message:?}"
        );
    }

    // The attribute the schema does NOT freeze is still writable, so the
    // refusal above is the schema's answer and not every write's.
    ok(
        &router,
        "AdminUpdateUserAttributes",
        &json!({
            "UserPoolId": user_pool_id,
            "Username": username,
            "UserAttributes": [{ "Name": "custom:editable", "Value": "yes" }],
        }),
    )
    .await;
    let read = ok(
        &router,
        "AdminGetUser",
        &json!({ "UserPoolId": user_pool_id, "Username": username }),
    )
    .await;
    let attributes = read["UserAttributes"]
        .as_array()
        .unwrap_or_else(|| panic!("AdminGetUser renders the stored attributes, got {read}"));
    assert!(
        attributes.contains(&json!({ "Name": "custom:editable", "Value": "yes" })),
        "a mutable attribute must still be written, got {read}"
    );
}
