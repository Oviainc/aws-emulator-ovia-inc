// SPDX-License-Identifier: CC0-1.0

//! Two rules of this service's messaging surface, over the wire.
//!
//! RULE A (addressing): a request that asks for a message to be delivered over a
//! delivery medium is refused `InvalidParameterException` / HTTP 400, writing nothing,
//! unless an address for that medium is held — the medium names the attribute that
//! addresses it (`EMAIL` is the `email` address, `SMS` the `phone_number`, the two
//! values of `DeliveryMediumType` from
//! `smithy_query(query="shape",shape="DeliveryMediumType,VerifiedAttributeType")`) —
//! so the refusal comes at request validation, before any send is attempted.
//!
//! RULE B (liveness): an operation that consumes a verification code compares the
//! presented value only against a LIVE code. Where none is live — none was ever
//! issued, the one that was has been spent, or its validity window has closed —
//! there is nothing to compare against and the answer is `ExpiredCodeException`,
//! whose remediation is to ask for a code; only a value that differs from a code
//! that IS live earns `CodeMismatchException`, whose remediation is to present the
//! right code.
//!
//! RULE C (lookup scope): a `UserNotFoundException` of an operation that resolves the
//! user from the `Username` and the `ClientId` together reports the PAIR as not
//! found, while one that resolves the user inside a pool the caller names reports the
//! user as not existing.
//!
//! The code itself is delivered out of band, so no response carries it: a test learns
//! the delivered code the way a mailbox would — from the emulator's own state.
//!
//! Each table row names the case it asserts.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

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

/// Another password satisfying it, for a reset to settle on.
const RESET_PASSWORD: &str = "Res3tPassw0rd!";

/// A `ConfirmationCodeType` value (len 1..2048, pattern `^[\S]+$`) that no send of
/// these tests ever delivered.
const UNISSUED_CODE: &str = "000000";

/// What a check against no live code says: there is none to present, so one has to be
/// requested.
const EXPIRED_CODE_MESSAGE: &str = "Invalid code provided, please request a code again.";

/// What a value that is not the live code says: a code is there, so the right one has
/// to be presented.
const CODE_MISMATCH_MESSAGE: &str = "Invalid verification code provided, please try again.";

/// The one sentence a `UserNotFoundException` says for an operation that resolves the
/// user from the `Username` and the `ClientId` of the request together.
const USERNAME_CLIENT_NOT_FOUND_MESSAGE: &str = "Username/client id combination not found.";

/// The one sentence it says for an operation that resolves the user inside the pool
/// the request names.
const USER_NOT_FOUND_MESSAGE: &str = "User does not exist.";

/// What a request asking for delivery over `medium` says when no `attribute`
/// addresses it.
fn no_address_message(medium: &str, attribute: &str) -> String {
    format!("Cannot send the message by {medium}. No registered {attribute}")
}

/// Whether a request carries a SigV4 credential scope.
#[derive(Clone, Copy, Debug)]
enum Addressing {
    /// A signed call, as the SDK sends every credentialed operation of this service.
    Signed,
    /// An unsigned call to the emulator's own endpoint: how the self-service
    /// operations are called.
    Anonymous,
}

/// Which operation one row of the liveness table presents its code to.
#[derive(Clone, Copy, Debug)]
enum Consumer {
    /// The sign-up confirmation code, answered by `ConfirmSignUp`.
    ConfirmSignUp,
    /// The password-reset code, answered by `ConfirmForgotPassword`.
    ConfirmForgotPassword,
    /// An attribute's verification code, answered by `VerifyUserAttribute`.
    VerifyUserAttribute,
}

/// What state the code the row presents is in.
#[derive(Clone, Copy, Debug)]
enum CodeState {
    /// No code was ever issued for that user and purpose.
    NeverIssued,
    /// A code was issued and already accepted, so it is spent.
    Spent,
    /// A code is outstanding but its validity window has closed.
    PastWindow,
    /// A code is live and the row presents a different value.
    LiveButWrong,
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

/// Create a pool with `extra_pool` merged into its configuration, plus an app client
/// that admits the flows these tests sign in with, and return both minted ids.
async fn pool_with_client(
    router: &MultiServiceRouter,
    pool_name: &str,
    extra_pool: &Value,
) -> (String, String) {
    let mut body = json!({ "PoolName": pool_name });
    if let Value::Object(members) = extra_pool.clone() {
        for (member, value) in members {
            body[member] = value;
        }
    }
    let created = ok(router, "CreateUserPool", &body).await;
    let user_pool_id = created["UserPool"]["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateUserPool must mint an id, got {created}"))
        .to_string();
    let client = ok(
        router,
        "CreateUserPoolClient",
        &json!({
            "UserPoolId": user_pool_id,
            "ClientName": "app",
            "ExplicitAuthFlows": [
                "ALLOW_ADMIN_USER_PASSWORD_AUTH",
                "ALLOW_USER_PASSWORD_AUTH",
                "ALLOW_REFRESH_TOKEN_AUTH",
            ],
        }),
    )
    .await;
    let client_id = client["UserPoolClient"]["ClientId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateUserPoolClient must mint an id, got {client}"))
        .to_string();
    (user_pool_id, client_id)
}

/// Create `username` with `attributes` and a permanent password, which is what makes
/// it a `CONFIRMED` user an access token can be issued for.
async fn admin_user(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    username: &str,
    attributes: &[(&str, &str)],
) {
    let attributes: Vec<Value> = attributes
        .iter()
        .map(|(name, value)| json!({ "Name": name, "Value": value }))
        .collect();
    ok(
        router,
        "AdminCreateUser",
        &json!({
            "UserPoolId": user_pool_id,
            "Username": username,
            "UserAttributes": attributes,
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

/// Sign `username` in through the admin password flow and return the access token of
/// that sign-in.
async fn access_token(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    client_id: &str,
    username: &str,
) -> String {
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
        .unwrap_or_else(|| panic!("a sign-in must issue an access token, got {signed_in}"))
        .to_string()
}

/// The code outstanding for `purpose`, read out of the emulator's own state: the real
/// service delivers a code out of band, so no response carries it.
fn outstanding_code(
    engine: &Arc<engine::InMemoryEngine>,
    user_pool_id: &str,
    username: &str,
    purpose: &state::CodePurpose,
) -> Option<String> {
    let service_state = state::service_state(&**engine).expect("this service's state");
    let guard = service_state.lock();
    guard
        .pool(user_pool_id)
        .and_then(|pool| pool.user(username))
        .and_then(|user| user.outstanding_code(purpose))
        .map(|code| code.code.clone())
}

/// Backdate the code outstanding for `purpose` by `age`, so the validity window is
/// read against the clock rather than waited out.
fn backdate_code(
    engine: &Arc<engine::InMemoryEngine>,
    user_pool_id: &str,
    username: &str,
    purpose: &state::CodePurpose,
    age: Duration,
) {
    let service_state = state::service_state(&**engine).expect("this service's state");
    let mut guard = service_state.lock();
    let code = guard
        .pool_mut(user_pool_id)
        .and_then(|pool| pool.user_mut(username))
        .and_then(|user| user.outstanding_codes.get_mut(purpose))
        .expect("a code is outstanding for this purpose");
    code.issued_at = SystemTime::now() - age;
}

/// A code of the same shape the service did NOT send: every digit of `code` shifted
/// by one, so no character of it matches.
fn wrong_code(code: &str) -> String {
    code.chars()
        .map(|digit| {
            char::from_digit((digit.to_digit(10).unwrap_or(0) + 1) % 10, 10).unwrap_or('0')
        })
        .collect()
}

/// The `UserStatus` `AdminGetUser` reports for `username`.
async fn stored_status(router: &MultiServiceRouter, user_pool_id: &str, username: &str) -> String {
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

/// The attributes `AdminGetUser` reports for `username`, as `(name, value)` pairs.
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
        .expect("AdminGetUser reports the stored attributes")
        .iter()
        .map(|attribute| {
            (
                attribute["Name"].as_str().unwrap_or_default().to_string(),
                attribute["Value"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// RULE A over `AdminCreateUser`: the welcome message's delivery mediums are
/// cross-validated against the attributes that address them, before any user is
/// written. Rows differ along which mediums are requested and which addressing
/// attributes the request carries.
#[tokio::test]
async fn a_requested_delivery_medium_needs_the_attribute_that_addresses_it() {
    let engine = engine();
    let setup = router(&engine, false);
    let (user_pool_id, _client_id) = pool_with_client(
        &setup,
        "delivery-mediums",
        &json!({ "AutoVerifiedAttributes": ["email"] }),
    )
    .await;

    // (case, DesiredDeliveryMediums as the request carries it, attributes submitted,
    //  the (medium, attribute) the refusal must name, or None where the request is
    //  deliverable)
    type Row<'a> = (
        &'a str,
        Option<Vec<&'a str>>,
        Vec<(&'a str, &'a str)>,
        Option<(&'a str, &'a str)>,
    );
    let rows: Vec<Row> = vec![
        (
            "SMS alone, for a user carrying only an email",
            Some(vec!["SMS"]),
            vec![("email", "sms-only@example.com")],
            Some(("SMS", "phone_number")),
        ),
        (
            "EMAIL alone, for a user carrying only an email",
            Some(vec!["EMAIL"]),
            vec![("email", "mail-only@example.com")],
            None,
        ),
        (
            "EMAIL alone, for a user carrying only a phone number",
            Some(vec!["EMAIL"]),
            vec![("phone_number", "+15550100001")],
            Some(("EMAIL", "email")),
        ),
        (
            "SMS alone, for a user carrying only a phone number",
            Some(vec!["SMS"]),
            vec![("phone_number", "+15550100002")],
            None,
        ),
        (
            "both mediums, for a user carrying both attributes",
            Some(vec!["EMAIL", "SMS"]),
            vec![
                ("email", "both@example.com"),
                ("phone_number", "+15550100003"),
            ],
            None,
        ),
        (
            "both mediums, for a user carrying only an email",
            Some(vec!["EMAIL", "SMS"]),
            vec![("email", "half@example.com")],
            Some(("SMS", "phone_number")),
        ),
        (
            "both mediums, for a user carrying neither attribute",
            Some(vec!["SMS", "EMAIL"]),
            vec![("name", "Nameless")],
            // The first requested medium is the one the refusal reports.
            Some(("SMS", "phone_number")),
        ),
        (
            "SMS alone, for a user whose phone number is submitted empty",
            Some(vec!["SMS"]),
            vec![("email", "empty@example.com"), ("phone_number", "")],
            Some(("SMS", "phone_number")),
        ),
        (
            "no medium requested at all, for a user carrying no address",
            None,
            vec![("name", "Unaddressed")],
            None,
        ),
        (
            "an empty medium list, for a user carrying no address",
            Some(vec![]),
            vec![("name", "Unrequested")],
            None,
        ),
    ];

    for (index, (case, mediums, attributes, refusal)) in rows.iter().enumerate() {
        let username = format!("created{index}");
        let submitted: Vec<Value> = attributes
            .iter()
            .map(|(name, value)| json!({ "Name": name, "Value": value }))
            .collect();
        let mut body = json!({
            "UserPoolId": user_pool_id,
            "Username": username,
            "UserAttributes": submitted,
        });
        if let Some(mediums) = mediums {
            body["DesiredDeliveryMediums"] = json!(mediums);
        }
        let (status, payload) = dispatch(&setup, Addressing::Signed, "AdminCreateUser", &body).await;

        match refusal {
            Some((medium, attribute)) => {
                assert_eq!(
                    (status, error_code(&payload)),
                    (400, "InvalidParameterException"),
                    "case {case:?}: expected a 400 InvalidParameterException, got {status} \
                     with {payload}"
                );
                let expected = no_address_message(medium, attribute);
                let message = error_message(&payload);
                assert_eq!(
                    message, expected,
                    "case {case:?}: expected {expected:?}, got {message:?}"
                );
                // The refusal came at request validation, so NO user was written.
                let (read_status, read_back) = dispatch(
                    &setup,
                    Addressing::Signed,
                    "AdminGetUser",
                    &json!({ "UserPoolId": user_pool_id, "Username": username }),
                )
                .await;
                assert_eq!(
                    (read_status, error_code(&read_back)),
                    (404, "UserNotFoundException"),
                    "case {case:?}: a refused create writes no user, got {read_status} \
                     with {read_back}"
                );
            }
            None => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected HTTP 200, got {status} with {payload}"
                );
                // ROUND TRIP: the created user is the one a later read reports.
                let stored = stored_attributes(&setup, &user_pool_id, &username).await;
                for (name, value) in attributes {
                    if value.is_empty() {
                        continue;
                    }
                    assert!(
                        stored.contains(&((*name).to_string(), (*value).to_string())),
                        "case {case:?}: expected the stored user to carry {name}={value:?}, \
                         got {stored:?}"
                    );
                }
            }
        }
    }
}

/// RULE A over the token path: `GetUserAttributeVerificationCode` sends a code only
/// where the attribute it names holds an address. Rows differ along which attribute
/// is asked for and which the signed-in user holds.
#[tokio::test]
async fn an_attribute_with_no_stored_address_cannot_be_sent_a_code() {
    let engine = engine();
    let setup = router(&engine, false);
    let emulator = router(&engine, true);
    let (user_pool_id, client_id) =
        pool_with_client(&setup, "delivery-token-path", &json!({})).await;

    admin_user(
        &setup,
        &user_pool_id,
        "mailonly",
        &[("email", "mailonly@example.com")],
    )
    .await;
    admin_user(
        &setup,
        &user_pool_id,
        "phoneonly",
        &[("phone_number", "+15550100010")],
    )
    .await;
    let mail_token = access_token(&setup, &user_pool_id, &client_id, "mailonly").await;
    let phone_token = access_token(&setup, &user_pool_id, &client_id, "phoneonly").await;

    // (case, access token of the signed-in user, attribute asked for, the
    //  `DeliveryMediumType` that attribute's value is reached over, whether a code
    //  can be addressed)
    let rows: Vec<(&str, &String, &str, &str, bool)> = vec![
        (
            "email, which that user holds",
            &mail_token,
            "email",
            "EMAIL",
            true,
        ),
        (
            "phone_number, which that user does not hold",
            &mail_token,
            "phone_number",
            "SMS",
            false,
        ),
        (
            "phone_number, which the other user holds",
            &phone_token,
            "phone_number",
            "SMS",
            true,
        ),
        (
            "email, which the other user does not hold",
            &phone_token,
            "email",
            "EMAIL",
            false,
        ),
    ];

    for (case, token, attribute, medium, deliverable) in &rows {
        let (status, payload) = dispatch(
            &emulator,
            Addressing::Anonymous,
            "GetUserAttributeVerificationCode",
            &json!({ "AccessToken": token, "AttributeName": attribute }),
        )
        .await;
        if *deliverable {
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with {payload}"
            );
            assert_eq!(
                payload["CodeDeliveryDetails"]["DeliveryMedium"],
                json!(medium),
                "case {case:?}: expected DeliveryMedium {medium}, got {payload}"
            );
        } else {
            assert_eq!(
                (status, error_code(&payload)),
                (400, "InvalidParameterException"),
                "case {case:?}: expected a 400 InvalidParameterException, got {status} \
                 with {payload}"
            );
            // The one wording the rule has, wherever a delivery has no address.
            let expected = no_address_message(medium, attribute);
            let message = error_message(&payload);
            assert_eq!(
                message, expected,
                "case {case:?}: expected {expected:?}, got {message:?}"
            );
        }
    }
}

/// RULE B: a presented code is compared only against a LIVE one — rows differ along
/// the operation the code is presented to and the state the code is in.
#[tokio::test]
async fn a_check_made_where_no_live_code_exists_is_refused_as_expired() {
    let engine = engine();
    let setup = router(&engine, false);
    let emulator = router(&engine, true);
    // The pool that auto-verifies an attribute is the one whose sign-up sends a code;
    // the plain pool auto-verifies nothing, so a sign-up on it delivers none.
    let (verifying_pool, verifying_client) = pool_with_client(
        &setup,
        "live-codes",
        &json!({ "AutoVerifiedAttributes": ["email"] }),
    )
    .await;
    let (plain_pool, plain_client) = pool_with_client(&setup, "no-codes", &json!({})).await;
    let window = state::VERIFICATION_CODE_VALIDITY;

    // (case, operation the code is presented to, the state the code is in, expected
    //  error code, expected message)
    let rows: Vec<(&str, Consumer, CodeState, &str, &str)> = vec![
        (
            "ConfirmSignUp where the sign-up delivered no code at all",
            Consumer::ConfirmSignUp,
            CodeState::NeverIssued,
            "ExpiredCodeException",
            EXPIRED_CODE_MESSAGE,
        ),
        (
            "ConfirmSignUp against a live code",
            Consumer::ConfirmSignUp,
            CodeState::LiveButWrong,
            "CodeMismatchException",
            CODE_MISMATCH_MESSAGE,
        ),
        (
            "ConfirmSignUp against a code past its window",
            Consumer::ConfirmSignUp,
            CodeState::PastWindow,
            "ExpiredCodeException",
            EXPIRED_CODE_MESSAGE,
        ),
        (
            "ConfirmForgotPassword where no reset was ever asked for",
            Consumer::ConfirmForgotPassword,
            CodeState::NeverIssued,
            "ExpiredCodeException",
            EXPIRED_CODE_MESSAGE,
        ),
        (
            "ConfirmForgotPassword with the code the reset already spent",
            Consumer::ConfirmForgotPassword,
            CodeState::Spent,
            "ExpiredCodeException",
            EXPIRED_CODE_MESSAGE,
        ),
        (
            "ConfirmForgotPassword against a live code",
            Consumer::ConfirmForgotPassword,
            CodeState::LiveButWrong,
            "CodeMismatchException",
            CODE_MISMATCH_MESSAGE,
        ),
        (
            "VerifyUserAttribute where no code was ever sent",
            Consumer::VerifyUserAttribute,
            CodeState::NeverIssued,
            "ExpiredCodeException",
            EXPIRED_CODE_MESSAGE,
        ),
        (
            "VerifyUserAttribute with the code the verification already spent",
            Consumer::VerifyUserAttribute,
            CodeState::Spent,
            "ExpiredCodeException",
            EXPIRED_CODE_MESSAGE,
        ),
        (
            "VerifyUserAttribute against a code past its window",
            Consumer::VerifyUserAttribute,
            CodeState::PastWindow,
            "ExpiredCodeException",
            EXPIRED_CODE_MESSAGE,
        ),
        (
            "VerifyUserAttribute against a live code",
            Consumer::VerifyUserAttribute,
            CodeState::LiveButWrong,
            "CodeMismatchException",
            CODE_MISMATCH_MESSAGE,
        ),
    ];

    for (index, (case, consumer, code_state, expected_error, expected_message)) in
        rows.iter().enumerate()
    {
        let username = format!("live{index}");
        let address = format!("{username}@example.com");
        let (status, payload) = match consumer {
            Consumer::ConfirmSignUp => {
                let purpose = state::CodePurpose::SignUpConfirmation;
                // A sign-up on the plain pool has nothing to auto-verify, so it
                // delivers no code; one on the verifying pool delivers to the email.
                let (pool_id, client_id, attributes) = match code_state {
                    CodeState::NeverIssued => (&plain_pool, &plain_client, json!([])),
                    _ => (
                        &verifying_pool,
                        &verifying_client,
                        json!([{ "Name": "email", "Value": address }]),
                    ),
                };
                let signed_up = ok_anonymous(
                    &emulator,
                    "SignUp",
                    &json!({
                        "ClientId": client_id,
                        "Username": username,
                        "Password": GOOD_PASSWORD,
                        "UserAttributes": attributes,
                    }),
                )
                .await;
                let delivered = outstanding_code(&engine, pool_id, &username, &purpose);
                assert_eq!(
                    delivered.is_some(),
                    !matches!(code_state, CodeState::NeverIssued),
                    "case {case:?}: the sign-up must leave a code outstanding exactly when \
                     it reported a delivery, got {signed_up}"
                );
                let presented = match code_state {
                    CodeState::NeverIssued => UNISSUED_CODE.to_string(),
                    CodeState::LiveButWrong => {
                        wrong_code(delivered.as_deref().unwrap_or_default())
                    }
                    CodeState::PastWindow => {
                        backdate_code(&engine, pool_id, &username, &purpose, window);
                        delivered.clone().unwrap_or_default()
                    }
                    // Spending a sign-up code is what CONFIRMS the account, so this
                    // operation has no spent-code case to present: the status check
                    // answers first. The other two consumers carry that row.
                    CodeState::Spent => {
                        panic!("case {case:?}: ConfirmSignUp carries no spent-code row")
                    }
                };
                let answer = dispatch(
                    &emulator,
                    Addressing::Anonymous,
                    "ConfirmSignUp",
                    &json!({
                        "ClientId": client_id,
                        "Username": username,
                        "ConfirmationCode": presented,
                    }),
                )
                .await;
                // A refused confirmation confirms nothing, whichever refusal it is.
                let status_now = stored_status(&setup, pool_id, &username).await;
                assert_eq!(
                    status_now, "UNCONFIRMED",
                    "case {case:?}: a refused confirmation leaves the sign-up unconfirmed, \
                     got {status_now}"
                );
                answer
            }
            Consumer::ConfirmForgotPassword => {
                let purpose = state::CodePurpose::PasswordReset;
                // A reset message goes to a VERIFIED address, so the user holds one.
                admin_user(
                    &setup,
                    &verifying_pool,
                    &username,
                    &[("email", &address), ("email_verified", "true")],
                )
                .await;
                let presented = match code_state {
                    CodeState::NeverIssued => UNISSUED_CODE.to_string(),
                    _ => {
                        ok_anonymous(
                            &emulator,
                            "ForgotPassword",
                            &json!({ "ClientId": verifying_client, "Username": username }),
                        )
                        .await;
                        let live =
                            outstanding_code(&engine, &verifying_pool, &username, &purpose)
                                .unwrap_or_else(|| {
                                    panic!("case {case:?}: the request must send a code")
                                });
                        match code_state {
                            CodeState::LiveButWrong => wrong_code(&live),
                            CodeState::PastWindow => {
                                backdate_code(
                                    &engine,
                                    &verifying_pool,
                                    &username,
                                    &purpose,
                                    window,
                                );
                                live
                            }
                            // Accepting the code is what spends it, which leaves none
                            // live for the same code to be presented against again.
                            _ => {
                                ok_anonymous(
                                    &emulator,
                                    "ConfirmForgotPassword",
                                    &json!({
                                        "ClientId": verifying_client,
                                        "Username": username,
                                        "ConfirmationCode": live,
                                        "Password": RESET_PASSWORD,
                                    }),
                                )
                                .await;
                                live
                            }
                        }
                    }
                };
                dispatch(
                    &emulator,
                    Addressing::Anonymous,
                    "ConfirmForgotPassword",
                    &json!({
                        "ClientId": verifying_client,
                        "Username": username,
                        "ConfirmationCode": presented,
                        "Password": RESET_PASSWORD,
                    }),
                )
                .await
            }
            Consumer::VerifyUserAttribute => {
                let purpose = state::CodePurpose::AttributeVerification("email".to_string());
                admin_user(&setup, &verifying_pool, &username, &[("email", &address)]).await;
                let token =
                    access_token(&setup, &verifying_pool, &verifying_client, &username).await;
                let presented = match code_state {
                    CodeState::NeverIssued => UNISSUED_CODE.to_string(),
                    _ => {
                        ok_anonymous(
                            &emulator,
                            "GetUserAttributeVerificationCode",
                            &json!({ "AccessToken": token, "AttributeName": "email" }),
                        )
                        .await;
                        let live =
                            outstanding_code(&engine, &verifying_pool, &username, &purpose)
                                .unwrap_or_else(|| {
                                    panic!("case {case:?}: the send must leave a code")
                                });
                        match code_state {
                            CodeState::LiveButWrong => wrong_code(&live),
                            CodeState::PastWindow => {
                                backdate_code(
                                    &engine,
                                    &verifying_pool,
                                    &username,
                                    &purpose,
                                    window,
                                );
                                live
                            }
                            _ => {
                                ok_anonymous(
                                    &emulator,
                                    "VerifyUserAttribute",
                                    &json!({
                                        "AccessToken": token,
                                        "AttributeName": "email",
                                        "Code": live,
                                    }),
                                )
                                .await;
                                live
                            }
                        }
                    }
                };
                dispatch(
                    &emulator,
                    Addressing::Anonymous,
                    "VerifyUserAttribute",
                    &json!({
                        "AccessToken": token,
                        "AttributeName": "email",
                        "Code": presented,
                    }),
                )
                .await
            }
        };

        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {payload}"
        );
        assert_eq!(
            error_code(&payload),
            *expected_error,
            "case {case:?}: expected {expected_error}, got {payload}"
        );
        let message = error_message(&payload);
        assert_eq!(
            message, *expected_message,
            "case {case:?}: expected {expected_message:?}, got {message:?}"
        );
    }
}

/// RULE C: the client-id-scoped lookup reports the PAIR as not found, while the
/// pool-scoped admin lookup reports the user as not existing — over the same absent
/// usernames.
#[tokio::test]
async fn an_absent_user_is_reported_by_the_scope_the_lookup_used() {
    let engine = engine();
    let setup = router(&engine, false);
    let emulator = router(&engine, true);
    let (user_pool_id, client_id) = pool_with_client(
        &setup,
        "lookup-scope",
        &json!({ "AutoVerifiedAttributes": ["email"] }),
    )
    .await;

    // Names spanning what `UsernameType` admits (len 1..128, pattern
    // `^[\p{L}\p{M}\p{S}\p{N}\p{P}]+$`, from
    // `smithy_query(query="shape",shape="UsernameType")`).
    let usernames = [
        ("a plain name", "no-such-user-xyz".to_string()),
        ("the one-character minimum", "q".to_string()),
        ("the 128-character maximum", "n".repeat(128)),
        ("an email-shaped name", "absent@example.test".to_string()),
    ];

    for (case, username) in &usernames {
        let (status, payload) = dispatch(
            &emulator,
            Addressing::Anonymous,
            "ConfirmSignUp",
            &json!({
                "ClientId": client_id,
                "Username": username,
                "ConfirmationCode": UNISSUED_CODE,
            }),
        )
        .await;
        assert_eq!(
            (status, error_code(&payload)),
            (404, "UserNotFoundException"),
            "case {case:?} (ConfirmSignUp): expected a 404 UserNotFoundException, got \
             {status} with {payload}"
        );
        let message = error_message(&payload);
        assert_eq!(
            message, USERNAME_CLIENT_NOT_FOUND_MESSAGE,
            "case {case:?} (ConfirmSignUp): expected {USERNAME_CLIENT_NOT_FOUND_MESSAGE:?}, \
             got {message:?}"
        );

        let (status, payload) = dispatch(
            &setup,
            Addressing::Signed,
            "AdminGetUser",
            &json!({ "UserPoolId": user_pool_id, "Username": username }),
        )
        .await;
        assert_eq!(
            (status, error_code(&payload)),
            (404, "UserNotFoundException"),
            "case {case:?} (AdminGetUser): expected a 404 UserNotFoundException, got \
             {status} with {payload}"
        );
        let message = error_message(&payload);
        assert_eq!(
            message, USER_NOT_FOUND_MESSAGE,
            "case {case:?} (AdminGetUser): expected {USER_NOT_FOUND_MESSAGE:?}, got \
             {message:?}"
        );
    }
}
