// SPDX-License-Identifier: CC0-1.0

//! Reading a user pool domain by name, and the two condition-specific refusals
//! whose message states the condition rather than the request. Driven through
//! the router exactly as an SDK drives it (`POST /` with
//! `X-Amz-Target: AWSCognitoIdentityProviderService.<Op>`).
//!
//! The rules under test:
//!
//! * `DescribeUserPoolDomain` takes `Domain` ALONE and REPORTS what the store
//!   holds under that name: a name some pool holds renders that pool's stored
//!   domain, and a name no pool holds — one never created, or one a delete took
//!   away — is reported as a 200 whose `DomainDescription` binds no member, not
//!   as a refusal.
//! * an operation addressed by the PAIR `UserPoolId` + `Domain`
//!   (`DeleteUserPoolDomain`, `UpdateUserPoolDomain`) resolves that pair as a
//!   unit, so a pair holding no stored domain is refused the same way whichever
//!   half of it is wrong, with one sentence that reports the pair and names
//!   neither half — `400 ResourceNotFoundException` where the operation
//!   reconfigures an existing domain record, and `400
//!   InvalidParameterException` where it removes one.
//! * the device surface of a pool that set no `DeviceConfiguration` is refused
//!   `400 InvalidUserPoolConfigurationException` with ONE fixed sentence, the
//!   same for every device operation and every pool, embedding no request value.
//! * `GetUserAttributeVerificationCode` refuses an `AttributeName` outside the
//!   verifiable set with ONE fixed sentence that names the permitted set and
//!   embeds no request value.
//!
//! Each table row names the case it asserts.
//!
//! Note on the domain strings below: `DomainType`'s modelled pattern
//! `^[a-z0-9](?:[a-z0-9\-]{0,61}[a-z0-9])?$` admits no dots, so even the
//! custom-domain rows name dotless domain strings.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice as cognito_idp;

/// A request's SigV4 credential scope is what routes it to this service. The
/// signature is never checked here: the suite turns request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/cognito-idp/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0000";

/// An ACM certificate ARN, which is what makes a create request a CUSTOM domain
/// (`CreateUserPoolDomain.CustomDomainConfig`).
const CERTIFICATE: &str =
    "arn:aws:acm:us-east-1:000000000000:certificate/11111111-2222-3333-4444-555555555555";

/// The sentence AWS answers when the `UserPoolId` + `Domain` pair a REMOVAL is
/// addressed by holds no stored domain.
const NO_SUCH_DOMAIN: &str = "No such domain or user pool exists.";

/// The sentence AWS answers when the `UserPoolId` + `Domain` pair a
/// RECONFIGURATION of an existing domain is addressed by holds no stored domain.
const DOMAIN_NOT_FOUND: &str = "User pool domain was not found.";

/// The sentence AWS answers when the pair names a stored pool and a domain name
/// ANOTHER stored pool holds, so the two parameters are mismatched rather than
/// unresolvable.
const DOMAIN_OF_OTHER_POOL: &str = "The domain is not configured for this user pool.";

/// The sentence AWS answers on the device surface of a pool that activated no
/// device remembering.
const DEVICE_TRACKING_OFF: &str = "Device tracking not currently enabled for this pool.";

/// The sentence AWS answers when a verification code is asked for an attribute
/// that is not one a message can be addressed to.
const UNVERIFIABLE_ATTRIBUTE: &str =
    "Invalid attribute name. Only phone_number and email can be verified.";

/// A password satisfying a default pool's password policy.
const GOOD_PASSWORD: &str = "Passw0rd!2024";

/// A well-formed `DeviceKey` (`DeviceKeyType`, pattern
/// `^[\w-]+_[0-9a-f-]+$`) that no sign-in minted.
const UNKNOWN_DEVICE_KEY: &str = "us-east-1_11111111-2222-3333-4444-555555555555";

/// One emulator with just this service mounted.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(cognito_idp::EmulatedService::new(engine));
    router.set_request_authentication(false);
    router
}

/// Call `operation` with `body` and return its status and JSON response body.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "cognito-idp.us-east-1.amazonaws.com")
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "x-amz-target",
            format!("AWSCognitoIdentityProviderService.{operation}"),
        )
        .header("authorization", AUTHORIZATION)
        .body(Bytes::from(body.to_string()))
        .expect("the request is well formed");
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

/// Call `operation`, requiring success, and return its response body.
async fn ok(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, payload) = call(router, operation, body).await;
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

/// Create a pool, optionally carrying `device_configuration`, and return its
/// minted `UserPoolId`.
async fn pool(
    router: &MultiServiceRouter,
    name: &str,
    device_configuration: Option<Value>,
) -> String {
    let mut body = Map::new();
    body.insert("PoolName".to_string(), json!(name));
    if let Some(configuration) = device_configuration {
        body.insert("DeviceConfiguration".to_string(), configuration);
    }
    let created = ok(router, "CreateUserPool", &Value::Object(body)).await;
    created["UserPool"]["Id"]
        .as_str()
        .expect("a minted pool id")
        .to_string()
}

/// `DescribeUserPoolDomain(Domain=domain)`, requiring the 200 this reporting
/// operation always answers, and returning the `DomainDescription` it bound.
async fn describe(router: &MultiServiceRouter, domain: &str) -> Value {
    let read = ok(
        router,
        "DescribeUserPoolDomain",
        &json!({ "Domain": domain }),
    )
    .await;
    read["DomainDescription"].clone()
}

/// One row of the by-name read table. The rows differ along what the rule
/// generalises over: the form and length of the domain string, whether any pool
/// ever held it, and — for one that was held — whether it was a prefix or a
/// custom domain (which is also which of the pool's two slots held it).
struct ReadCase {
    case: &'static str,
    domain: String,
    /// The `CustomDomainConfig` the create carries, for a row that creates one;
    /// `None` makes the created domain a prefix domain.
    custom_domain_config: Option<Value>,
    /// Whether this row creates the domain and deletes it again, rather than
    /// only reading a name nothing ever created.
    created_then_deleted: bool,
}

/// The table. The domain strings span the digits, letters and hyphens
/// `DomainType`'s pattern admits and both ends of its modelled length 1..63
/// (`smithy_query(query="operation",operation="DescribeUserPoolDomain,CreateUserPoolDomain")`).
fn read_cases() -> Vec<ReadCase> {
    vec![
        ReadCase {
            case: "a one-character name nothing created, the length minimum",
            domain: "q".to_string(),
            custom_domain_config: None,
            created_then_deleted: false,
        },
        ReadCase {
            case: "a single digit nothing created",
            domain: "7".to_string(),
            custom_domain_config: None,
            created_then_deleted: false,
        },
        ReadCase {
            case: "a 63-character name nothing created, the length maximum",
            domain: format!("a{}z", "b".repeat(61)),
            custom_domain_config: None,
            created_then_deleted: false,
        },
        ReadCase {
            case: "a hyphenated name starting with a digit that nothing created",
            domain: "9-absent-prefix-0".to_string(),
            custom_domain_config: None,
            created_then_deleted: false,
        },
        ReadCase {
            case: "a prefix domain created and then deleted",
            domain: "read-prefix".to_string(),
            custom_domain_config: None,
            created_then_deleted: true,
        },
        ReadCase {
            case: "a custom domain created and then deleted",
            domain: "read-custom".to_string(),
            custom_domain_config: Some(json!({ "CertificateArn": CERTIFICATE })),
            created_then_deleted: true,
        },
        ReadCase {
            case: "a one-character prefix domain created and then deleted",
            domain: "k".to_string(),
            custom_domain_config: None,
            created_then_deleted: true,
        },
    ]
}

/// RULE: `DescribeUserPoolDomain` reports the store under the name it is given —
/// the stored domain when some pool holds that name, and an EMPTY description
/// when none does, whether the name was never created or a delete took it away.
#[tokio::test]
async fn a_read_of_a_domain_no_pool_holds_reports_an_empty_description() {
    let router = router();

    for case in read_cases() {
        let label = case.case;
        let domain = case.domain.as_str();

        // Nothing holds the name yet, so the reader binds no member — and says
        // so twice, since a read changes nothing a later caller sees.
        let before = describe(&router, domain).await;
        assert_eq!(
            before,
            json!({}),
            "case {label:?}: a name no pool holds binds no member, got {before}"
        );
        let again = describe(&router, domain).await;
        assert_eq!(
            again, before,
            "case {label:?}: a read must not change what the next read sees, got {again}"
        );

        if !case.created_then_deleted {
            continue;
        }

        // A pool per row: a pool holds at most one prefix and at most one
        // custom domain.
        let user_pool_id = pool(&router, &format!("read-pool-{domain}"), None).await;
        let mut create = Map::new();
        create.insert("UserPoolId".to_string(), json!(user_pool_id));
        create.insert("Domain".to_string(), json!(domain));
        if let Some(configuration) = &case.custom_domain_config {
            create.insert("CustomDomainConfig".to_string(), configuration.clone());
        }
        ok(&router, "CreateUserPoolDomain", &Value::Object(create)).await;

        // Now the name resolves, and what it renders is the stored record of
        // the pool it was attached to.
        let held = describe(&router, domain).await;
        assert_eq!(
            held["UserPoolId"],
            json!(user_pool_id),
            "case {label:?}: the description names the pool the domain is attached to, \
             got {held}"
        );
        assert_eq!(
            held["Domain"],
            json!(domain),
            "case {label:?}: the description names the stored domain string, got {held}"
        );
        assert!(
            held["CloudFrontDistribution"]
                .as_str()
                .is_some_and(|endpoint| endpoint.ends_with(".cloudfront.net")),
            "case {label:?}: every hosted domain is served through a CloudFront endpoint, \
             got {held}"
        );

        ok(
            &router,
            "DeleteUserPoolDomain",
            &json!({ "UserPoolId": user_pool_id, "Domain": domain }),
        )
        .await;

        // The delete took the name away, so the reader is back to binding no
        // member — the same answer a name nothing ever created gets.
        let after = describe(&router, domain).await;
        assert_eq!(
            after,
            json!({}),
            "case {label:?}: a deleted name binds no member again, got {after}"
        );
    }
}

/// RULE: an operation addressed by the `UserPoolId` + `Domain` pair resolves
/// that pair as a unit, so every way the pair can fail to RESOLVE is refused
/// identically for that operation, with one sentence that names neither half:
/// the operation that reconfigures an existing domain answers the resource it
/// cannot find (400), and the one that removes a domain answers its bad-parameter
/// refusal (400). The one pairing that does resolve without naming this pool's
/// domain — a stored pool and a name ANOTHER stored pool holds — is a mismatch of
/// the two parameters, which both operations refuse as a bad parameter with the
/// sentence for it. Over a table spanning both operations and every way the pair
/// can fail.
#[tokio::test]
async fn an_unresolved_pool_and_domain_pair_is_refused_the_same_way_however_it_fails() {
    let router = router();
    let user_pool_id = pool(&router, "pair-pool", None).await;
    let other_pool_id = pool(&router, "pair-other-pool", None).await;
    // One name the pool took and gave up again, so that name is a pair that
    // USED to resolve; a pool holds at most one prefix domain at a time, so the
    // name it keeps is created after that one is deleted.
    ok(
        &router,
        "CreateUserPoolDomain",
        &json!({ "UserPoolId": user_pool_id, "Domain": "pair-deleted" }),
    )
    .await;
    ok(
        &router,
        "DeleteUserPoolDomain",
        &json!({ "UserPoolId": user_pool_id, "Domain": "pair-deleted" }),
    )
    .await;
    ok(
        &router,
        "CreateUserPoolDomain",
        &json!({ "UserPoolId": user_pool_id, "Domain": "pair-held" }),
    )
    .await;

    // (case, the pair the request carries, whether the pair resolves to a stored
    // pool and a domain ANOTHER stored pool holds)
    let pairs = [
        (
            "a pool that does not exist, holding a name another pool does",
            json!({ "UserPoolId": "us-east-1_absent", "Domain": "pair-held" }),
            false,
        ),
        (
            "a pool that does not exist, holding a name nothing created",
            json!({ "UserPoolId": "us-east-1_absent", "Domain": "pair-never" }),
            false,
        ),
        (
            "a stored pool and a name nothing created",
            json!({ "UserPoolId": user_pool_id, "Domain": "pair-never" }),
            false,
        ),
        (
            "a stored pool and a name a delete took away",
            json!({ "UserPoolId": user_pool_id, "Domain": "pair-deleted" }),
            false,
        ),
        (
            "a stored pool and a name ANOTHER pool holds",
            json!({ "UserPoolId": other_pool_id, "Domain": "pair-held" }),
            true,
        ),
    ];

    // Each operation's own refusal of an unresolvable pair: the status, the
    // error code and the one sentence it carries.
    let refusals = [
        (
            "DeleteUserPoolDomain",
            400,
            "InvalidParameterException",
            NO_SUCH_DOMAIN,
        ),
        (
            "UpdateUserPoolDomain",
            400,
            "ResourceNotFoundException",
            DOMAIN_NOT_FOUND,
        ),
    ];

    for (label, pair, of_other_pool) in &pairs {
        for (operation, expected_status, expected_code, expected_message) in &refusals {
            // A pair naming a stored pool and another pool's domain resolves, so
            // both operations refuse it as the parameter mismatch it is rather
            // than with their own not-found sentence.
            let (expected_status, expected_code, expected_message) = if *of_other_pool {
                (&400, &"InvalidParameterException", &DOMAIN_OF_OTHER_POOL)
            } else {
                (expected_status, expected_code, expected_message)
            };
            // The update carries a member to change, so the row measures the
            // pair and not an empty request.
            let mut body = pair.as_object().expect("the pair is an object").clone();
            if *operation == "UpdateUserPoolDomain" {
                body.insert("ManagedLoginVersion".to_string(), json!(1));
            }
            let (status, payload) = call(&router, operation, &Value::Object(body)).await;
            assert_eq!(
                (status, error_code(&payload)),
                (*expected_status, *expected_code),
                "case {label:?} on {operation}: expected a {expected_status} \
                 {expected_code}, got {status} with {payload}"
            );
            assert_eq!(
                error_message(&payload),
                *expected_message,
                "case {label:?} on {operation}: expected the one sentence for an \
                 unresolved pair, got {payload}"
            );
            let message = error_message(&payload).to_string();
            assert!(
                !message.contains(&user_pool_id)
                    && !message.contains(&other_pool_id)
                    && !message.contains("pair-"),
                "case {label:?} on {operation}: the sentence names neither half of the \
                 pair, got {message:?}"
            );
        }
    }

    // Every refusal left the stored domain where it was.
    let held = describe(&router, "pair-held").await;
    assert_eq!(
        held["UserPoolId"],
        json!(user_pool_id),
        "a refused request must not move the stored domain, got {held}"
    );
}

/// RULE: the device surface of a pool that activated no device remembering is
/// refused with ONE fixed sentence, the same for every device operation and
/// every such pool, naming nothing the request asked for — while a pool that DID
/// set a `DeviceConfiguration` is not refused on its configuration at all.
#[tokio::test]
async fn the_device_surface_of_a_pool_with_no_device_configuration_states_one_sentence() {
    let router = router();

    // Two pools that activated no device remembering — neither set a
    // `DeviceConfiguration` — with different ids and different users, so the
    // sentence cannot be coming from either.
    let bare = device_pool(&router, "device-bare", None).await;
    let mut cases = device_rows(&bare, "the pool that set no DeviceConfiguration");
    let other = device_pool(&router, "device-other", None).await;
    cases.extend(device_rows(
        &other,
        "another pool that set no DeviceConfiguration",
    ));

    for (label, operation, body) in &cases {
        let (status, payload) = call(&router, operation, body).await;
        assert_eq!(
            (status, error_code(&payload)),
            (400, "InvalidUserPoolConfigurationException"),
            "case {label:?}: expected a 400 InvalidUserPoolConfigurationException, \
             got {status} with {payload}"
        );
        assert_eq!(
            error_message(&payload),
            DEVICE_TRACKING_OFF,
            "case {label:?}: expected the one sentence for a pool with no device \
             tracking, got {payload}"
        );
        assert!(
            !error_message(&payload).contains(&bare.user_pool_id)
                && !error_message(&payload).contains(&other.user_pool_id)
                && !error_message(&payload).contains(UNKNOWN_DEVICE_KEY),
            "case {label:?}: the sentence names nothing the request asked for, got {payload}"
        );
    }

    // The positive control: the same operations on a pool that DID activate
    // device remembering are answered on their own terms — a listing succeeds,
    // and a key nothing minted is reported as the missing device it is.
    let remembering = device_pool(
        &router,
        "device-on",
        Some(json!({ "ChallengeRequiredOnNewDevice": true })),
    )
    .await;
    let listed = ok(
        &router,
        "AdminListDevices",
        &json!({
            "UserPoolId": remembering.user_pool_id,
            "Username": remembering.username,
        }),
    )
    .await;
    assert!(
        listed["Devices"].as_array().is_some_and(Vec::is_empty),
        "a pool with device remembering lists its devices, got {listed}"
    );
    let (status, payload) = call(
        &router,
        "AdminGetDevice",
        &json!({
            "UserPoolId": remembering.user_pool_id,
            "Username": remembering.username,
            "DeviceKey": UNKNOWN_DEVICE_KEY,
        }),
    )
    .await;
    assert_eq!(
        (status, error_code(&payload)),
        (400, "ResourceNotFoundException"),
        "a pool with device remembering refuses an unknown key as a missing device, \
         got {status} with {payload}"
    );
}

/// A pool whose device surface a row drives, and the user it drives it for.
struct DevicePool {
    user_pool_id: String,
    username: String,
    access_token: String,
}

/// Create a pool carrying `device_configuration`, a signed-in user in it, and
/// return what a device request needs to address that user.
async fn device_pool(
    router: &MultiServiceRouter,
    name: &str,
    device_configuration: Option<Value>,
) -> DevicePool {
    let user_pool_id = pool(router, name, device_configuration).await;
    let client = ok(
        router,
        "CreateUserPoolClient",
        &json!({
            "UserPoolId": user_pool_id,
            "ClientName": "device-client",
            "ExplicitAuthFlows": ["ALLOW_ADMIN_USER_PASSWORD_AUTH", "ALLOW_REFRESH_TOKEN_AUTH"],
        }),
    )
    .await;
    let client_id = client["UserPoolClient"]["ClientId"]
        .as_str()
        .expect("a minted client id")
        .to_string();
    let username = "alice".to_string();
    let access_token = signed_in(router, &user_pool_id, &client_id, &username, &[]).await;
    DevicePool {
        user_pool_id,
        username,
        access_token,
    }
}

/// Every device operation, admin and self-service, addressed at `pool`'s user:
/// the whole surface the pool's device configuration gates.
fn device_rows(pool: &DevicePool, label: &str) -> Vec<(String, &'static str, Value)> {
    let admin = json!({
        "UserPoolId": pool.user_pool_id,
        "Username": pool.username,
        "DeviceKey": UNKNOWN_DEVICE_KEY,
    });
    let mut admin_status = admin.as_object().expect("an object").clone();
    admin_status.insert("DeviceRememberedStatus".to_string(), json!("remembered"));
    let token = json!({ "AccessToken": pool.access_token, "DeviceKey": UNKNOWN_DEVICE_KEY });
    let mut token_status = token.as_object().expect("an object").clone();
    token_status.insert("DeviceRememberedStatus".to_string(), json!("remembered"));
    vec![
        (
            format!("AdminListDevices on {label}"),
            "AdminListDevices",
            json!({ "UserPoolId": pool.user_pool_id, "Username": pool.username }),
        ),
        (
            format!("AdminGetDevice on {label}"),
            "AdminGetDevice",
            admin.clone(),
        ),
        (
            format!("AdminUpdateDeviceStatus on {label}"),
            "AdminUpdateDeviceStatus",
            Value::Object(admin_status),
        ),
        (
            format!("AdminForgetDevice on {label}"),
            "AdminForgetDevice",
            admin,
        ),
        (
            format!("GetDevice on {label}"),
            "GetDevice",
            token.clone(),
        ),
        (
            format!("ConfirmDevice on {label}"),
            "ConfirmDevice",
            token.clone(),
        ),
        (
            format!("UpdateDeviceStatus on {label}"),
            "UpdateDeviceStatus",
            Value::Object(token_status),
        ),
        (
            format!("ForgetDevice on {label}"),
            "ForgetDevice",
            token,
        ),
    ]
}

/// Create `username` with `attributes` in `user_pool_id`, give it a permanent
/// password, sign it in through the admin password flow and return the access
/// token of that sign-in.
async fn signed_in(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    client_id: &str,
    username: &str,
    attributes: &[(&str, &str)],
) -> String {
    let user_attributes: Vec<Value> = attributes
        .iter()
        .map(|(name, value)| json!({ "Name": name, "Value": value }))
        .collect();
    ok(
        router,
        "AdminCreateUser",
        &json!({
            "UserPoolId": user_pool_id,
            "Username": username,
            "UserAttributes": user_attributes,
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
    let signed_in = ok(
        router,
        "AdminInitiateAuth",
        &json!({
            "UserPoolId": user_pool_id,
            "ClientId": client_id,
            "AuthFlow": "ADMIN_USER_PASSWORD_AUTH",
            "AuthParameters": { "USERNAME": username, "PASSWORD": GOOD_PASSWORD },
        }),
    )
    .await;
    signed_in["AuthenticationResult"]["AccessToken"]
        .as_str()
        .expect("a sign-in issues an access token")
        .to_string()
}

/// RULE: a verification code is asked for an attribute that IS a destination a
/// message can be addressed to; every other attribute name earns one fixed
/// sentence that states the permitted set and quotes nothing the request carried
/// — over a table of attribute names spanning the standard profile, a custom
/// attribute, and both ends of `AttributeNameType`'s modelled length 1..32.
#[tokio::test]
async fn a_code_for_an_unverifiable_attribute_states_the_permitted_set() {
    let router = router();
    let user_pool_id = pool(&router, "verifiable-pool", None).await;
    let client = ok(
        &router,
        "CreateUserPoolClient",
        &json!({
            "UserPoolId": user_pool_id,
            "ClientName": "app",
            "ExplicitAuthFlows": ["ALLOW_ADMIN_USER_PASSWORD_AUTH", "ALLOW_REFRESH_TOKEN_AUTH"],
        }),
    )
    .await;
    let client_id = client["UserPoolClient"]["ClientId"]
        .as_str()
        .expect("a minted client id")
        .to_string();
    let access_token = signed_in(
        &router,
        &user_pool_id,
        &client_id,
        "carol",
        &[
            ("email", "carol@example.com"),
            ("phone_number", "+15550100007"),
            ("family_name", "Carol"),
            ("address", "1 Example Street"),
        ],
    )
    .await;

    // Attribute names outside the verifiable set: one the user stores, ones it
    // does not, a custom attribute, a name of one character and one of 32 — the
    // ends of `AttributeNameType`'s modelled length
    // (`smithy_query(query="operation",operation="GetUserAttributeVerificationCode")`).
    for name in [
        "family_name",
        "given_name",
        "address",
        "preferred_username",
        "custom:pager",
        "sub",
        "z",
        "a".repeat(32).as_str(),
    ] {
        let (status, payload) = call(
            &router,
            "GetUserAttributeVerificationCode",
            &json!({ "AccessToken": access_token, "AttributeName": name }),
        )
        .await;
        assert_eq!(
            (status, error_code(&payload)),
            (400, "InvalidParameterException"),
            "case {name:?}: expected a 400 InvalidParameterException, got {status} \
             with {payload}"
        );
        assert_eq!(
            error_message(&payload),
            UNVERIFIABLE_ATTRIBUTE,
            "case {name:?}: expected the one sentence naming the permitted set, \
             got {payload}"
        );
        assert!(
            !error_message(&payload).contains(name),
            "case {name:?}: the sentence quotes nothing the request carried, got {payload}"
        );
    }

    // The positive control: each attribute the sentence names IS verifiable, and
    // a code goes to the value the user stores for it.
    for (name, medium) in [("email", "EMAIL"), ("phone_number", "SMS")] {
        let sent = ok(
            &router,
            "GetUserAttributeVerificationCode",
            &json!({ "AccessToken": access_token, "AttributeName": name }),
        )
        .await;
        assert_eq!(
            sent["CodeDeliveryDetails"]["DeliveryMedium"],
            json!(medium),
            "case {name:?}: expected delivery over {medium}, got {sent}"
        );
        assert_eq!(
            sent["CodeDeliveryDetails"]["AttributeName"],
            json!(name),
            "case {name:?}: the delivery reports the attribute it was sent for, got {sent}"
        );
    }

    // And the sentence is specific to the condition: a VERIFIABLE attribute the
    // user holds no value for is refused with a different one, since the
    // attribute name is not what is wrong with that request.
    let (status, payload) = call(
        &router,
        "GetUserAttributeVerificationCode",
        &json!({ "AccessToken": signed_in(&router, &user_pool_id, &client_id, "dave", &[]).await,
                 "AttributeName": "email" }),
    )
    .await;
    assert_eq!(
        status, 400,
        "a user holding no address is refused too, got {status} with {payload}"
    );
    assert_ne!(
        error_message(&payload),
        UNVERIFIABLE_ATTRIBUTE,
        "a verifiable attribute with no stored value is a different condition, got {payload}"
    );
}
