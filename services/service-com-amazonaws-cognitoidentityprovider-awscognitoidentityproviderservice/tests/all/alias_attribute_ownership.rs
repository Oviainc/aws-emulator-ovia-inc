// SPDX-License-Identifier: CC0-1.0

//! Who an ALIAS value names, over the wire.
//!
//! The rule under test: in a pool that configures an attribute as a sign-in alias, a
//! value of that attribute names exactly ONE user at a time — the user that most
//! recently claimed it — so every operation that resolves a `Username` ("the value of
//! this parameter is typically your user's username, but it can be any of their alias
//! attributes", `AdminGetUser.Username` in this service's model) answers for the
//! CURRENT holder, and a claim takes the value away from whoever held it before ("a
//! value of true migrates the alias from an existing user to the new user if an
//! existing user already has the phone number or email address as an alias",
//! `ConfirmSignUp.ForceAliasCreation`). The previous holder keeps its own record and
//! stays reachable by its own username; it only stops answering to the alias value.
//!
//! The alias attributes are `phone_number`, `email` and `preferred_username`
//! (`smithy_query(query="shape",shape="AliasAttributeType")`). The first two carry a
//! verified flag, so a claim demotes the previous holder's flag; `preferred_username`
//! carries none, so there is nothing to demote and the claim is refused instead —
//! both halves are driven below.
//!
//! Each table row names the case it asserts.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice as cognito_idp;

use cognito_idp::state;

/// This service's SigV4 signing name, the 4th field of a credential scope.
const SIGNING_NAME: &str = "cognito-idp";

/// The local name of this service's Smithy shape, which is the `X-Amz-Target` prefix a
/// client writes.
const TARGET_PREFIX: &str = "AWSCognitoIdentityProviderService";

/// The emulator's own endpoint, which is what an unsigned client is pointed at.
const EMULATOR_HOST: &str = "127.0.0.1:9297";

/// This service's real regional endpoint host.
const ENDPOINT_HOST: &str = "cognito-idp.us-east-1.amazonaws.com";

/// A password satisfying a default pool's password policy.
const GOOD_PASSWORD: &str = "Passw0rd!2024";

/// The user that signs up first and holds the alias value until it is claimed.
const FIRST_USER: &str = "alias-user-one";

/// The user whose confirmed sign-up claims the alias value.
const SECOND_USER: &str = "alias-user-two";

/// The IAM role a pool's `SmsConfiguration` names as the Amazon SNS caller that SMS
/// is sent through ("the ARN of the Amazon SNS caller",
/// `SmsConfigurationType.SnsCallerArn`).
const SNS_CALLER_ARN: &str = "arn:aws:iam::000000000000:role/sms-caller";

/// Whether a request carries a SigV4 credential scope.
#[derive(Clone, Copy, Debug)]
enum Addressing {
    /// A signed call, as the SDK sends every credentialed operation of this service.
    Signed,
    /// An unsigned call to the emulator's own endpoint: how the self-service
    /// operations are called.
    Anonymous,
}

/// The shared engine every router in one test mounts.
fn engine() -> Arc<engine::InMemoryEngine> {
    Arc::new(engine::InMemoryEngine::new())
}

/// One emulator with just this service mounted over `engine`.
fn router(engine: &Arc<engine::InMemoryEngine>, authenticating: bool) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.register_service(cognito_idp::EmulatedService::new(Arc::clone(engine)));
    router.set_request_authentication(authenticating);
    router
}

/// Dispatch `operation` with `body`, addressed as `addressing` says, and return the
/// status and JSON response body.
async fn dispatch(
    router: &MultiServiceRouter,
    addressing: Addressing,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let host = match addressing {
        Addressing::Signed => ENDPOINT_HOST,
        Addressing::Anonymous => EMULATOR_HOST,
    };
    let mut request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", host)
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"));
    if matches!(addressing, Addressing::Signed) {
        request = request.header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 \
                 Credential=AKIDEXAMPLE/20260101/us-east-1/{SIGNING_NAME}/aws4_request, \
                 SignedHeaders=host;x-amz-date, Signature=0000"
            ),
        );
    }
    let response = router
        .dispatch(
            request
                .body(Bytes::from(body.to_string()))
                .expect("the request is well formed"),
        )
        .await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let payload = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{operation} answered a body that is not JSON ({error}): {}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    (status, payload)
}

/// Call `operation` signed, requiring success, and return its response body.
async fn ok(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, payload) = dispatch(router, Addressing::Signed, operation, body).await;
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

/// Create a pool whose `AliasAttributes` and `AutoVerifiedAttributes` are
/// `alias_attributes`, plus an app client, and return both minted ids.
async fn pool_with_client(
    router: &MultiServiceRouter,
    pool_name: &str,
    alias_attributes: &[&str],
) -> (String, String) {
    // Only an attribute a verification message can be SENT to is auto-verified:
    // `preferred_username` carries no verified flag and is addressed by nothing.
    let auto_verified: Vec<&str> = alias_attributes
        .iter()
        .copied()
        .filter(|name| *name != "preferred_username")
        .collect();
    let mut pool = serde_json::Map::new();
    pool.insert("PoolName".to_string(), json!(pool_name));
    pool.insert("AliasAttributes".to_string(), json!(alias_attributes));
    pool.insert("AutoVerifiedAttributes".to_string(), json!(auto_verified));
    // A pool that auto-verifies `phone_number` sends that verification over SMS, so
    // it needs the Amazon SNS caller SMS is sent through: `CreateUserPool` refuses
    // the pairing without one.
    if auto_verified.contains(&"phone_number") {
        pool.insert(
            "SmsConfiguration".to_string(),
            json!({ "SnsCallerArn": SNS_CALLER_ARN }),
        );
    }
    let created = ok(router, "CreateUserPool", &Value::Object(pool)).await;
    let user_pool_id = created["UserPool"]["Id"]
        .as_str()
        .expect("a minted pool id")
        .to_string();
    let client = ok(
        router,
        "CreateUserPoolClient",
        &json!({
            "UserPoolId": user_pool_id,
            "ClientName": "app",
            "ExplicitAuthFlows": ["ALLOW_USER_PASSWORD_AUTH", "ALLOW_REFRESH_TOKEN_AUTH"],
        }),
    )
    .await;
    let client_id = client["UserPoolClient"]["ClientId"]
        .as_str()
        .expect("a minted client id")
        .to_string();
    (user_pool_id, client_id)
}

/// Register `username` with `attribute` set to `value` through the self-service
/// `SignUp`, which leaves the account UNCONFIRMED and the value unverified.
async fn sign_up(
    emulator: &MultiServiceRouter,
    client_id: &str,
    username: &str,
    attribute: &str,
    value: &str,
) {
    let (status, payload) = dispatch(
        emulator,
        Addressing::Anonymous,
        "SignUp",
        &json!({
            "ClientId": client_id,
            "Username": username,
            "Password": GOOD_PASSWORD,
            "UserAttributes": [{ "Name": attribute, "Value": value }],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "SignUp for {username:?} must succeed, got {status} with {payload}"
    );
}

/// The sign-up confirmation code outstanding for `username`, read out of the
/// emulator's own state: the real service delivers it out of band, so no response
/// carries it.
fn outstanding_code(
    engine: &Arc<engine::InMemoryEngine>,
    user_pool_id: &str,
    username: &str,
) -> String {
    let service_state = state::service_state(&**engine).expect("this service's state");
    let guard = service_state.lock();
    guard
        .pool(user_pool_id)
        .and_then(|pool| pool.user(username))
        .and_then(|user| user.outstanding_code(&state::CodePurpose::SignUpConfirmation))
        .map(|code| code.code.clone())
        .unwrap_or_else(|| panic!("a sign-up code must be outstanding for {username:?}"))
}

/// Confirm `username`'s sign-up with `code`, asking for the alias migration exactly
/// when `force` says so, and return the status and body.
async fn confirm_sign_up(
    emulator: &MultiServiceRouter,
    client_id: &str,
    username: &str,
    code: &str,
    force: bool,
) -> (u16, Value) {
    dispatch(
        emulator,
        Addressing::Anonymous,
        "ConfirmSignUp",
        &json!({
            "ClientId": client_id,
            "Username": username,
            "ConfirmationCode": code,
            "ForceAliasCreation": force,
        }),
    )
    .await
}

/// Read the user `name` resolves to in `user_pool_id`, requiring a 200.
async fn admin_get_user(
    setup: &MultiServiceRouter,
    user_pool_id: &str,
    name: &str,
) -> Value {
    ok(
        setup,
        "AdminGetUser",
        &json!({ "UserPoolId": user_pool_id, "Username": name }),
    )
    .await
}

/// The `(name, value)` pairs of a user read's `UserAttributes`.
fn attributes_of(read: &Value) -> Vec<(String, String)> {
    read["UserAttributes"]
        .as_array()
        .expect("a user read reports the stored attributes")
        .iter()
        .map(|attribute| {
            (
                attribute["Name"].as_str().unwrap_or_default().to_string(),
                attribute["Value"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// The value a user read reports for `name`, or `None` when it holds none.
fn attribute_of(read: &Value, name: &str) -> Option<String> {
    attributes_of(read)
        .into_iter()
        .find(|(stored, _)| stored == name)
        .map(|(_, value)| value)
}

/// The service-generated `sub` a user read reports.
fn sub_of(read: &Value) -> String {
    attribute_of(read, "sub").expect("every stored user carries the sub the pool minted")
}

/// The group names `AdminListGroupsForUser` reports for `name`.
async fn groups_of(setup: &MultiServiceRouter, user_pool_id: &str, name: &str) -> Vec<String> {
    let listed = ok(
        setup,
        "AdminListGroupsForUser",
        &json!({ "UserPoolId": user_pool_id, "Username": name }),
    )
    .await;
    listed["Groups"]
        .as_array()
        .map(|groups| {
            groups
                .iter()
                .map(|group| group["GroupName"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// RULE: a confirmed duplicate sign-up for an alias value MOVES that value's
/// ownership to the confirming user, so every later lookup by it — a read and a
/// mutation alike — addresses that user and not the previous holder. Rows differ along
/// which alias attribute of the pool carries the value.
#[tokio::test]
async fn a_confirmed_duplicate_sign_up_moves_the_alias_to_the_claiming_user() {
    let engine = engine();
    let setup = router(&engine, false);
    let emulator = router(&engine, true);

    // The alias attributes a confirmation code can be sent to, which are the ones that
    // carry a verified flag: `email` and `phone_number` of `AliasAttributeType`
    // (`smithy_query(query="shape",shape="AliasAttributeType")`). A phone number is
    // held in E.164 form ("a phone number must be in E.164 format",
    // `AdminCreateUser.UserAttributes`).
    //
    // (case, alias attribute, the shared value, its verified flag)
    let rows: Vec<(&str, &str, &str, &str)> = vec![
        (
            "an email alias",
            "email",
            "shared-address@example.com",
            "email_verified",
        ),
        (
            "a phone-number alias",
            "phone_number",
            "+15550100200",
            "phone_number_verified",
        ),
    ];

    for (case, attribute, value, flag) in &rows {
        let (user_pool_id, client_id) =
            pool_with_client(&setup, &format!("alias-claim-{attribute}"), &[attribute]).await;

        // The first user signs up with the value and confirms it, which is what makes
        // the value its alias.
        sign_up(&emulator, &client_id, FIRST_USER, attribute, value).await;
        let first_code = outstanding_code(&engine, &user_pool_id, FIRST_USER);
        let (status, payload) =
            confirm_sign_up(&emulator, &client_id, FIRST_USER, &first_code, false).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the first confirmation must succeed, got {status} with {payload}"
        );

        let first_read = admin_get_user(&setup, &user_pool_id, FIRST_USER).await;
        let first_sub = sub_of(&first_read);
        let before = admin_get_user(&setup, &user_pool_id, value).await;
        assert_eq!(
            before["Username"],
            json!(FIRST_USER),
            "case {case:?}: before any claim the value names its first owner, got {before}"
        );

        // A second sign-up for the SAME value is registered but claims nothing yet: the
        // value becomes an alias only when a confirmation confirms it.
        sign_up(&emulator, &client_id, SECOND_USER, attribute, value).await;
        let second_code = outstanding_code(&engine, &user_pool_id, SECOND_USER);
        let (refused_status, refused) =
            confirm_sign_up(&emulator, &client_id, SECOND_USER, &second_code, false).await;
        assert_eq!(
            refused_status, 400,
            "case {case:?}: a confirmation that does not ask for the migration is refused, \
             got {refused_status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "AliasExistsException",
            "case {case:?}: expected AliasExistsException, got {refused}"
        );
        let unmoved = admin_get_user(&setup, &user_pool_id, value).await;
        assert_eq!(
            unmoved["Username"],
            json!(FIRST_USER),
            "case {case:?}: a refused confirmation moves nothing, got {unmoved}"
        );

        // Asking for the migration claims the alias for the confirming user.
        let (claimed_status, claimed) =
            confirm_sign_up(&emulator, &client_id, SECOND_USER, &second_code, true).await;
        assert_eq!(
            claimed_status, 200,
            "case {case:?}: the forced confirmation must succeed, got {claimed_status} \
             with {claimed}"
        );

        let second_read = admin_get_user(&setup, &user_pool_id, SECOND_USER).await;
        let second_sub = sub_of(&second_read);
        assert_ne!(
            first_sub, second_sub,
            "case {case:?}: two users of one pool hold two different subs, got {first_sub}"
        );

        // The lookup by the shared value answers with the CURRENT owner: its own
        // username and its own sub.
        let after = admin_get_user(&setup, &user_pool_id, value).await;
        assert_eq!(
            after["Username"],
            json!(SECOND_USER),
            "case {case:?}: after the claim the value names the claiming user, got {after}"
        );
        assert_eq!(
            sub_of(&after),
            second_sub,
            "case {case:?}: expected the claiming user's own sub {second_sub}, got {after}"
        );

        // The previous holder keeps its record and stays reachable by its own username;
        // what it loses is the flag that made the value its alias.
        let first_after = admin_get_user(&setup, &user_pool_id, FIRST_USER).await;
        assert_eq!(
            first_after["UserStatus"],
            json!("CONFIRMED"),
            "case {case:?}: the previous holder stays confirmed, got {first_after}"
        );
        assert_eq!(
            attribute_of(&first_after, attribute).as_deref(),
            Some(*value),
            "case {case:?}: the previous holder keeps its stored value, got {first_after}"
        );
        assert_eq!(
            attribute_of(&first_after, flag).as_deref(),
            Some("false"),
            "case {case:?}: the previous holder's {flag} no longer vouches for the value, \
             got {first_after}"
        );
        assert_eq!(
            attribute_of(&second_read, flag).as_deref(),
            Some("true"),
            "case {case:?}: the claiming user's {flag} vouches for the value, got \
             {second_read}"
        );

        // A MUTATION addressed by the alias value lands on the current owner too: the
        // grant and the disable below name the value, never a username.
        ok(
            &setup,
            "CreateGroup",
            &json!({ "UserPoolId": user_pool_id, "GroupName": "operators" }),
        )
        .await;
        ok(
            &setup,
            "AdminAddUserToGroup",
            &json!({
                "UserPoolId": user_pool_id,
                "Username": value,
                "GroupName": "operators",
            }),
        )
        .await;
        assert_eq!(
            groups_of(&setup, &user_pool_id, SECOND_USER).await,
            vec!["operators".to_string()],
            "case {case:?}: the grant addressed by the alias must reach the current owner"
        );
        assert!(
            groups_of(&setup, &user_pool_id, FIRST_USER).await.is_empty(),
            "case {case:?}: the previous holder must be granted nothing, got {:?}",
            groups_of(&setup, &user_pool_id, FIRST_USER).await
        );

        ok(
            &setup,
            "AdminDisableUser",
            &json!({ "UserPoolId": user_pool_id, "Username": value }),
        )
        .await;
        let first_enabled = admin_get_user(&setup, &user_pool_id, FIRST_USER).await;
        let second_enabled = admin_get_user(&setup, &user_pool_id, SECOND_USER).await;
        assert_eq!(
            second_enabled["Enabled"],
            json!(false),
            "case {case:?}: the disable addressed by the alias must reach the current \
             owner, got {second_enabled}"
        );
        assert_eq!(
            first_enabled["Enabled"],
            json!(true),
            "case {case:?}: the previous holder must stay enabled, got {first_enabled}"
        );
    }
}

/// RULE: the same one-holder rule governs an administrative attribute write — a write
/// that VOUCHES for an alias value takes it from the user that held it, and an alias
/// attribute that carries no verified flag has nothing to demote, so taking its value
/// is refused instead. Rows differ along which alias attribute of the pool the value
/// belongs to, which is what decides whether there is a flag to move.
#[tokio::test]
async fn an_admin_attribute_write_moves_an_alias_only_where_a_flag_can_be_demoted() {
    let engine = engine();
    let setup = router(&engine, false);

    // Every value of `AliasAttributeType`
    // (`smithy_query(query="shape",shape="AliasAttributeType")`): `phone_number` and
    // `email` carry the verified flags of `VerifiedAttributeType`, and
    // `preferred_username` carries none.
    //
    // (case, alias attribute, the shared value, its verified flag, expected status,
    //  expected error code)
    type Row = (
        &'static str,
        &'static str,
        &'static str,
        Option<&'static str>,
        u16,
        Option<&'static str>,
    );
    let rows: Vec<Row> = vec![
        (
            "an email alias, whose flag the write demotes",
            "email",
            "moved-address@example.com",
            Some("email_verified"),
            200,
            None,
        ),
        (
            "a phone-number alias, whose flag the write demotes",
            "phone_number",
            "+15550300400",
            Some("phone_number_verified"),
            200,
            None,
        ),
        (
            "a preferred-username alias, which carries no flag to demote",
            "preferred_username",
            "shared-handle",
            None,
            400,
            Some("AliasExistsException"),
        ),
    ];

    for (case, attribute, value, flag, expected_status, expected_code) in &rows {
        let (user_pool_id, _) =
            pool_with_client(&setup, &format!("alias-write-{attribute}"), &[attribute]).await;

        // The holder is created with the value, vouched for where the attribute has a
        // flag to vouch with.
        let mut held = vec![json!({ "Name": attribute, "Value": value })];
        if let Some(flag) = flag {
            held.push(json!({ "Name": flag, "Value": "true" }));
        }
        ok(
            &setup,
            "AdminCreateUser",
            &json!({
                "UserPoolId": user_pool_id,
                "Username": FIRST_USER,
                "UserAttributes": held,
            }),
        )
        .await;
        ok(
            &setup,
            "AdminCreateUser",
            &json!({ "UserPoolId": user_pool_id, "Username": SECOND_USER }),
        )
        .await;
        let holder = admin_get_user(&setup, &user_pool_id, value).await;
        assert_eq!(
            holder["Username"],
            json!(FIRST_USER),
            "case {case:?}: the created holder answers to the value, got {holder}"
        );

        let mut written = vec![json!({ "Name": attribute, "Value": value })];
        if let Some(flag) = flag {
            written.push(json!({ "Name": flag, "Value": "true" }));
        }
        let (status, payload) = dispatch(
            &setup,
            Addressing::Signed,
            "AdminUpdateUserAttributes",
            &json!({
                "UserPoolId": user_pool_id,
                "Username": SECOND_USER,
                "UserAttributes": written,
            }),
        )
        .await;
        assert_eq!(
            status, *expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with {payload}"
        );

        let resolved = admin_get_user(&setup, &user_pool_id, value).await;
        match expected_code {
            Some(code) => {
                assert_eq!(
                    error_code(&payload),
                    *code,
                    "case {case:?}: expected error code {code}, got {payload}"
                );
                // A refused write leaves the value where it was, and leaves the user it
                // was refused for holding nothing for that attribute.
                assert_eq!(
                    resolved["Username"],
                    json!(FIRST_USER),
                    "case {case:?}: a refused write moves nothing, got {resolved}"
                );
                let claimant = admin_get_user(&setup, &user_pool_id, SECOND_USER).await;
                assert_eq!(
                    attribute_of(&claimant, attribute),
                    None,
                    "case {case:?}: the refused write stored nothing, got {claimant}"
                );
            }
            None => {
                assert_eq!(
                    resolved["Username"],
                    json!(SECOND_USER),
                    "case {case:?}: the write that vouches for the value takes it, got \
                     {resolved}"
                );
                let demoted = admin_get_user(&setup, &user_pool_id, FIRST_USER).await;
                let flag = flag.expect("this row's attribute carries a verified flag");
                assert_eq!(
                    attribute_of(&demoted, flag).as_deref(),
                    Some("false"),
                    "case {case:?}: the previous holder's {flag} is demoted with the \
                     alias, got {demoted}"
                );
                assert_eq!(
                    attribute_of(&demoted, attribute).as_deref(),
                    Some(*value),
                    "case {case:?}: the previous holder keeps its stored value, got \
                     {demoted}"
                );
            }
        }
    }
}
