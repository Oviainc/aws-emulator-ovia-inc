// SPDX-License-Identifier: CC0-1.0

//! `AdminConfirmSignUp` over the wire: the administrative confirmation of a
//! sign-up, for a pool "that requires confirmation of new user accounts before
//! they can sign in".
//!
//! The rules under test:
//!
//! * the call sets the account of a user whose sign-up is still awaiting
//!   confirmation ACTIVE, in the pool the request names, and nothing else: no
//!   code was presented, so no attribute value was proven to be the user's and no
//!   verified flag moves — and the code the sign-up sent has nothing left to
//!   confirm;
//! * a user in any OTHER status is not one a confirmation confirms again: the
//!   call is refused and names the status the user is in;
//! * a username the pool holds no user for, and a pool id no stored pool holds,
//!   are each refused with that refusal's own status, error code and message, and
//!   persist nothing.
//!
//! Each table row names the case it asserts, and every state assertion reads the
//! user back through an implemented reader.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice as cognito_idp;

use cognito_idp::state;

/// This service's SigV4 signing name, the 4th field of a credential scope.
const SIGNING_NAME: &str = "cognito-idp";

/// The local name of this service's Smithy shape, which is the `X-Amz-Target`
/// prefix a client writes.
const TARGET_PREFIX: &str = "AWSCognitoIdentityProviderService";

/// The emulator's own endpoint, which is what an unsigned client is pointed at.
const EMULATOR_HOST: &str = "127.0.0.1:9297";

/// This service's real regional endpoint host.
const ENDPOINT_HOST: &str = "cognito-idp.us-east-1.amazonaws.com";

/// A password satisfying a default pool's password policy.
const GOOD_PASSWORD: &str = "Passw0rd!2024";

/// The one sentence a `UserNotFoundException` says for a lookup against a pool
/// the request names.
const USER_NOT_FOUND_MESSAGE: &str = "User does not exist.";

/// A `UserPoolIdType` value (len 1..55, pattern `^[\w-]+_[0-9a-zA-Z]+$`) that no
/// stored pool holds.
const ABSENT_POOL_ID: &str = "us-east-1_4bs3ntp00l";

/// Whether a request carries a SigV4 credential scope.
#[derive(Clone, Copy, Debug)]
enum Addressing {
    /// A signed call, as the SDK sends every administrative operation.
    Signed,
    /// An unsigned call to the emulator's own endpoint: how the self-service
    /// operations in this suite's setup are called.
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

/// Dispatch `operation` with `body`, addressed as `addressing` says, and return
/// the status and JSON response body.
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

/// Call `operation` unsigned, requiring success, and return its response body.
async fn ok_anonymous(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, payload) = dispatch(router, Addressing::Anonymous, operation, body).await;
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

/// The `message` member of an error body.
fn error_message(payload: &Value) -> &str {
    payload
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
}

/// Create a pool with `extra` merged into its configuration and return its id.
async fn pool(router: &MultiServiceRouter, pool_name: &str, extra: Value) -> String {
    let mut body = json!({ "PoolName": pool_name });
    if let Value::Object(members) = extra {
        for (member, value) in members {
            body[member] = value;
        }
    }
    let created = ok(router, "CreateUserPool", &body).await;
    created["UserPool"]["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateUserPool must mint an id, got {created}"))
        .to_string()
}

/// Create an app client of `user_pool_id` and return its minted id.
async fn app_client(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    client_name: &str,
) -> String {
    let created = ok(
        router,
        "CreateUserPoolClient",
        &json!({ "UserPoolId": user_pool_id, "ClientName": client_name }),
    )
    .await;
    created["UserPoolClient"]["ClientId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateUserPoolClient must mint a client id, got {created}"))
        .to_string()
}

/// Register `username` through `client_id` with an email address, leaving the
/// user where a sign-up leaves it: unconfirmed, with a code outstanding.
async fn sign_up(router: &MultiServiceRouter, client_id: &str, username: &str) {
    ok_anonymous(
        router,
        "SignUp",
        &json!({
            "ClientId": client_id,
            "Username": username,
            "Password": GOOD_PASSWORD,
            "UserAttributes": [{ "Name": "email", "Value": format!("{username}@example.test") }],
        }),
    )
    .await;
}

/// The sign-up confirmation code outstanding for `username`, read out of the
/// emulator's own state: the real service delivers it out of band, so no
/// response carries it.
fn outstanding_code(
    engine: &Arc<engine::InMemoryEngine>,
    user_pool_id: &str,
    username: &str,
) -> Option<String> {
    let service_state = state::service_state(&**engine).expect("this service's state");
    let guard = service_state.lock();
    guard
        .pool(user_pool_id)
        .and_then(|pool| pool.user(username))
        .and_then(|user| user.outstanding_code(&state::CodePurpose::SignUpConfirmation))
        .map(|code| code.code.clone())
}

/// Leave `username` in `status` in the emulator's own state. The statuses a
/// refusal row needs are ones no implemented operation of this service produces,
/// so the row writes the state it is about rather than calling one.
fn set_status(
    engine: &Arc<engine::InMemoryEngine>,
    user_pool_id: &str,
    username: &str,
    status: state::UserStatus,
) {
    let service_state = state::service_state(&**engine).expect("this service's state");
    let mut guard = service_state.lock();
    let user = guard
        .pool_mut(user_pool_id)
        .and_then(|pool| pool.user_mut(username))
        .expect("the signed-up user is stored");
    user.status = status;
}

/// The attributes `AdminGetUser` reports for `username`, flattened to pairs.
async fn stored_attributes(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    username: &str,
) -> Vec<(String, String)> {
    let read_back = ok(
        router,
        "AdminGetUser",
        &json!({ "UserPoolId": user_pool_id, "Username": username }),
    )
    .await;
    read_back["UserAttributes"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|attribute| {
            (
                attribute["Name"].as_str().unwrap_or_default().to_string(),
                attribute["Value"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// The `UserStatus` `AdminGetUser` reports for `username`.
async fn stored_status(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    username: &str,
) -> String {
    let read_back = ok(
        router,
        "AdminGetUser",
        &json!({ "UserPoolId": user_pool_id, "Username": username }),
    )
    .await;
    read_back["UserStatus"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// RULE: an administrative confirmation sets an awaiting sign-up active and
/// proves no attribute value, and the code that sign-up sent is spent with it.
#[tokio::test]
async fn an_administrative_confirmation_activates_the_account_and_verifies_nothing() {
    let engine = engine();
    let setup = router(&engine, false);
    let emulator = router(&engine, true);
    let user_pool_id = pool(
        &setup,
        "admin-confirm",
        json!({ "AutoVerifiedAttributes": ["email"] }),
    )
    .await;
    let client_id = app_client(&setup, &user_pool_id, "open").await;
    sign_up(&emulator, &client_id, "newcomer").await;

    assert_eq!(
        stored_status(&setup, &user_pool_id, "newcomer").await,
        "UNCONFIRMED",
        "a sign-up leaves the user awaiting confirmation"
    );
    let code = outstanding_code(&engine, &user_pool_id, "newcomer")
        .expect("a sign-up leaves a code outstanding");

    let confirmed = ok(
        &setup,
        "AdminConfirmSignUp",
        &json!({ "UserPoolId": user_pool_id, "Username": "newcomer" }),
    )
    .await;
    assert_eq!(
        confirmed,
        json!({}),
        "an administrative confirmation reports nothing of its own, got {confirmed}"
    );

    // Read back through an implemented reader: the account is active.
    let status_now = stored_status(&setup, &user_pool_id, "newcomer").await;
    assert_eq!(
        status_now, "CONFIRMED",
        "expected the account to be active, got {status_now}"
    );
    // Nothing was proven about the address: no code was answered, so the
    // verified flag of the attribute the sign-up carried must not say true.
    let attributes = stored_attributes(&setup, &user_pool_id, "newcomer").await;
    let email_verified = attributes
        .iter()
        .find(|(name, _)| name == "email_verified")
        .map(|(_, value)| value.as_str());
    assert!(
        email_verified.is_none_or(|value| value != "true"),
        "an administrative confirmation proves no ownership, so email_verified must not say \
         true, got {attributes:?}"
    );
    // The code the sign-up sent has nothing left to confirm.
    let code_now = outstanding_code(&engine, &user_pool_id, "newcomer");
    assert!(
        code_now.is_none(),
        "the sign-up's code is spent with the confirmation, got {code_now:?}"
    );
    let (status, payload) = dispatch(
        &emulator,
        Addressing::Anonymous,
        "ConfirmSignUp",
        &json!({
            "ClientId": client_id,
            "Username": "newcomer",
            "ConfirmationCode": code,
        }),
    )
    .await;
    assert_eq!(
        (status, error_code(&payload)),
        (403, "NotAuthorizedException"),
        "the code of an already confirmed account confirms nothing, got {status} with {payload}"
    );
    assert_eq!(
        error_message(&payload),
        "User cannot be confirmed. Current status is CONFIRMED",
        "the refusal names the status the user is in, got {payload}"
    );
}

/// RULE: only a sign-up still awaiting confirmation is confirmed; a user in any
/// other status is refused with its own status named, and is left in it. The
/// rows are every `UserStatusType` value
/// (`smithy_query(query="shape",shape="UserStatusType")`).
#[tokio::test]
async fn a_user_in_any_other_status_is_refused_with_that_status_named() {
    let engine = engine();
    let setup = router(&engine, false);
    let emulator = router(&engine, true);
    let user_pool_id = pool(&setup, "admin-confirm-statuses", json!({})).await;
    let client_id = app_client(&setup, &user_pool_id, "open").await;

    let rows = [
        ("UNCONFIRMED", state::UserStatus::Unconfirmed, true),
        ("CONFIRMED", state::UserStatus::Confirmed, false),
        ("ARCHIVED", state::UserStatus::Archived, false),
        ("COMPROMISED", state::UserStatus::Compromised, false),
        ("UNKNOWN", state::UserStatus::Unknown, false),
        ("RESET_REQUIRED", state::UserStatus::ResetRequired, false),
        (
            "FORCE_CHANGE_PASSWORD",
            state::UserStatus::ForceChangePassword,
            false,
        ),
        (
            "EXTERNAL_PROVIDER",
            state::UserStatus::ExternalProvider,
            false,
        ),
    ];

    for (case, status, served) in rows {
        let username = format!("user-{}", case.to_lowercase());
        sign_up(&emulator, &client_id, &username).await;
        set_status(&engine, &user_pool_id, &username, status);

        let (http_status, payload) = dispatch(
            &setup,
            Addressing::Signed,
            "AdminConfirmSignUp",
            &json!({ "UserPoolId": user_pool_id, "Username": username }),
        )
        .await;

        if served {
            assert_eq!(
                http_status, 200,
                "case {case:?}: an awaiting sign-up is confirmed, got {http_status} with {payload}"
            );
            let status_now = stored_status(&setup, &user_pool_id, &username).await;
            assert_eq!(
                status_now, "CONFIRMED",
                "case {case:?}: expected CONFIRMED, got {status_now}"
            );
            continue;
        }

        assert_eq!(
            (http_status, error_code(&payload)),
            (403, "NotAuthorizedException"),
            "case {case:?}: expected a 403 NotAuthorizedException, got {http_status} with \
             {payload}"
        );
        let message = error_message(&payload);
        let expected = format!("User cannot be confirmed. Current status is {case}");
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
        // A refused confirmation leaves the user exactly as it was.
        let status_now = stored_status(&setup, &user_pool_id, &username).await;
        assert_eq!(
            status_now, case,
            "case {case:?}: a refused confirmation changes nothing, got {status_now}"
        );
    }
}

/// RULE: the confirmation resolves the user against the pool the request names,
/// so an absent pool and an absent user are each refused with that refusal's own
/// status, error code and message, and persist nothing. The usernames span what
/// `UsernameType` admits (length 1..128, pattern
/// `^[\p{L}\p{M}\p{S}\p{N}\p{P}]+$`, from
/// `smithy_query(query="shape",shape="UsernameType")`).
#[tokio::test]
async fn an_absent_pool_or_user_is_refused_and_confirms_nothing() {
    let engine = engine();
    let setup = router(&engine, false);
    let emulator = router(&engine, true);
    let user_pool_id = pool(&setup, "admin-confirm-absent", json!({})).await;
    let client_id = app_client(&setup, &user_pool_id, "open").await;
    // A user that DOES exist, so each refusal below is about the name it was
    // given rather than about an empty pool.
    sign_up(&emulator, &client_id, "present").await;

    let longest = "u".repeat(128);
    let usernames = [
        ("a plain name", "absent-user".to_string()),
        ("the 128-character length maximum", longest),
        ("the one-character length minimum", "z".to_string()),
        ("a non-ASCII name", "ユーザー-不在①".to_string()),
        ("an email-shaped name", "absent@example.test".to_string()),
        ("the stored name re-cased", "PRESENT".to_string()),
    ];

    for (name_case, username) in &usernames {
        let case = format!("an absent user: {name_case}");
        let (status, payload) = dispatch(
            &setup,
            Addressing::Signed,
            "AdminConfirmSignUp",
            &json!({ "UserPoolId": user_pool_id, "Username": username }),
        )
        .await;
        assert_eq!(
            (status, error_code(&payload)),
            (404, "UserNotFoundException"),
            "case {case:?}: expected a 404 UserNotFoundException, got {status} with {payload}"
        );
        let message = error_message(&payload);
        assert_eq!(
            message, USER_NOT_FOUND_MESSAGE,
            "case {case:?}: expected {USER_NOT_FOUND_MESSAGE:?}, got {message:?}"
        );

        // The same name in a pool that does not exist: the pool is what is
        // reported missing.
        let case = format!("an absent pool: {name_case}");
        let (status, payload) = dispatch(
            &setup,
            Addressing::Signed,
            "AdminConfirmSignUp",
            &json!({ "UserPoolId": ABSENT_POOL_ID, "Username": username }),
        )
        .await;
        assert_eq!(
            (status, error_code(&payload)),
            (404, "ResourceNotFoundException"),
            "case {case:?}: expected a 404 ResourceNotFoundException, got {status} with {payload}"
        );
        let message = error_message(&payload);
        let expected = format!("User pool {ABSENT_POOL_ID} does not exist.");
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
    }

    // Nothing above confirmed the user that does exist.
    let status_now = stored_status(&setup, &user_pool_id, "present").await;
    assert_eq!(
        status_now, "UNCONFIRMED",
        "a refused confirmation confirms no other user, got {status_now}"
    );
}
