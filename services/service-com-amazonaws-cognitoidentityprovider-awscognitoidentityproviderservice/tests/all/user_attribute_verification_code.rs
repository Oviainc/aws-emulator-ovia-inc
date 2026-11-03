// SPDX-License-Identifier: CC0-1.0

//! `GetUserAttributeVerificationCode` over the wire: the operation a signed-in user
//! calls to have a verification code sent to one of its own attributes.
//!
//! The rule under test: the access token names the user (the operation models no
//! `UserPoolId` and no `Username`), only an attribute that IS a destination a message
//! can be addressed to carries a code — `email` by mail, `phone_number` by text
//! message — and the destination reported is the attribute's value as STORED for that
//! user, masked as this service reports a delivery destination, so a user that holds
//! no value for the attribute is refused.
//!
//! Each table row names the case it asserts.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice as cognito_idp;

use cognito_idp::ops::types::{DELIVERY_MEDIUM_TYPE_VALUES, VERIFIED_ATTRIBUTE_TYPE_VALUES};
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

/// Whether a request carries a SigV4 credential scope.
#[derive(Clone, Copy, Debug)]
enum Addressing {
    /// A signed call, as the SDK sends every credentialed operation of this service.
    Signed,
    /// An unsigned call to the emulator's own endpoint: how the self-service
    /// operations, this one among them, are called.
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

/// Create a pool and an app client that admits the admin password flow, and return
/// both minted ids.
async fn pool_with_client(router: &MultiServiceRouter, pool_name: &str) -> (String, String) {
    let created = ok(router, "CreateUserPool", &json!({ "PoolName": pool_name })).await;
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
            "ExplicitAuthFlows": ["ALLOW_ADMIN_USER_PASSWORD_AUTH"],
        }),
    )
    .await;
    let client_id = client["UserPoolClient"]["ClientId"]
        .as_str()
        .expect("a minted client id")
        .to_string();
    (user_pool_id, client_id)
}

/// Create `username` with `attributes`, give it a permanent password, sign it in, and
/// return the access, id and refresh tokens of that sign-in.
async fn signed_in_user(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    client_id: &str,
    username: &str,
    attributes: &[(&str, &str)],
) -> (String, String, String) {
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
    let result = &signed_in["AuthenticationResult"];
    (
        result["AccessToken"]
            .as_str()
            .expect("a sign-in issues an access token")
            .to_string(),
        result["IdToken"]
            .as_str()
            .expect("a sign-in issues an id token")
            .to_string(),
        result["RefreshToken"]
            .as_str()
            .expect("a sign-in issues a refresh token")
            .to_string(),
    )
}

/// RULE: a code is sent only for an attribute that is a destination a message can be
/// addressed to, over the medium that attribute is reached by, and to the value the
/// NAMED USER stores for it — over a table whose rows differ along the attribute, the
/// values the user stores, and the user the token names.
#[tokio::test]
async fn a_verification_code_is_sent_to_the_stored_value_of_the_attribute_it_names() {
    let engine = engine();
    let setup = router(&engine, false);
    let emulator = router(&engine, true);

    let (user_pool_id, client_id) = pool_with_client(&setup, "attribute-verification").await;

    // Three users, each storing a different set of the verifiable attributes, so what
    // is reported has to come from the user the token names rather than from the
    // request — which carries no destination at all.
    let (both_token, both_id_token, both_refresh_token) = signed_in_user(
        &setup,
        &user_pool_id,
        &client_id,
        "both",
        &[
            ("email", "both@example.com"),
            ("phone_number", "+15550100001"),
            ("name", "Both"),
        ],
    )
    .await;
    let (mail_only_token, _, _) = signed_in_user(
        &setup,
        &user_pool_id,
        &client_id,
        "mailonly",
        &[("email", "mail.only@example.net")],
    )
    .await;
    let (phone_only_token, _, _) = signed_in_user(
        &setup,
        &user_pool_id,
        &client_id,
        "phoneonly",
        &[("phone_number", "+15550100002")],
    )
    .await;

    // (case, access token, attribute, expected status, expected medium / error code,
    //  expected destination)
    type Row = (
        &'static str,
        String,
        &'static str,
        u16,
        &'static str,
        Option<&'static str>,
    );
    let rows: Vec<Row> = vec![
        (
            "email, which the user stores",
            both_token.clone(),
            "email",
            200,
            "EMAIL",
            Some("b***@e***"),
        ),
        (
            "phone_number, which the same user also stores",
            both_token.clone(),
            "phone_number",
            200,
            "SMS",
            Some("+*******0001"),
        ),
        (
            "email of ANOTHER user, whose stored address differs",
            mail_only_token.clone(),
            "email",
            200,
            "EMAIL",
            Some("m***@e***"),
        ),
        (
            "phone_number of a user that stores only that",
            phone_only_token.clone(),
            "phone_number",
            200,
            "SMS",
            Some("+*******0002"),
        ),
        (
            "phone_number of a user that stores none",
            mail_only_token.clone(),
            "phone_number",
            400,
            "InvalidParameterException",
            None,
        ),
        (
            "email of a user that stores none",
            phone_only_token.clone(),
            "email",
            400,
            "InvalidParameterException",
            None,
        ),
        (
            "name, a stored attribute no message can be addressed to",
            both_token.clone(),
            "name",
            400,
            "InvalidParameterException",
            None,
        ),
        (
            "an attribute the pool's schema does not declare",
            both_token.clone(),
            "custom:pager",
            400,
            "InvalidParameterException",
            None,
        ),
        (
            "the id token of the same sign-in",
            both_id_token.clone(),
            "email",
            400,
            "NotAuthorizedException",
            None,
        ),
        (
            "the refresh token of the same sign-in",
            both_refresh_token.clone(),
            "email",
            400,
            "NotAuthorizedException",
            None,
        ),
        (
            "a token this emulator never issued",
            "not.a.token".to_string(),
            "email",
            400,
            "NotAuthorizedException",
            None,
        ),
    ];

    for (case, access_token, attribute, expected_status, expected, destination) in &rows {
        let (status, payload) = dispatch(
            &emulator,
            Addressing::Anonymous,
            "GetUserAttributeVerificationCode",
            &json!({ "AccessToken": access_token, "AttributeName": attribute }),
        )
        .await;
        assert_eq!(
            status, *expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with {payload}"
        );
        match destination {
            Some(destination) => {
                let details = &payload["CodeDeliveryDetails"];
                assert_eq!(
                    details["DeliveryMedium"],
                    json!(expected),
                    "case {case:?}: expected DeliveryMedium {expected}, got {payload}"
                );
                assert_eq!(
                    details["Destination"],
                    json!(destination),
                    "case {case:?}: expected the stored value reported as \
                     {destination}, got {payload}"
                );
                assert_eq!(
                    details["AttributeName"],
                    json!(attribute),
                    "case {case:?}: expected AttributeName {attribute}, got {payload}"
                );
            }
            None => assert_eq!(
                error_code(&payload),
                *expected,
                "case {case:?}: expected error code {expected}, got {payload}"
            ),
        }
    }

    // Asking again answers the same delivery — a second code goes to the same stored
    // destination — and nothing the operation wrote changed the user's own profile:
    // a code was SENT, not accepted, so the attribute is no more verified than before.
    let again = dispatch(
        &emulator,
        Addressing::Anonymous,
        "GetUserAttributeVerificationCode",
        &json!({ "AccessToken": both_token, "AttributeName": "email" }),
    )
    .await;
    assert_eq!(
        again.1["CodeDeliveryDetails"]["Destination"],
        json!("b***@e***"),
        "a repeated request reports the same stored destination, got {}",
        again.1
    );

    let profile = dispatch(
        &emulator,
        Addressing::Anonymous,
        "GetUser",
        &json!({ "AccessToken": both_token }),
    )
    .await;
    let attributes: Vec<(String, String)> = profile.1["UserAttributes"]
        .as_array()
        .expect("GetUser reports the stored attributes")
        .iter()
        .map(|attribute| {
            (
                attribute["Name"].as_str().unwrap_or_default().to_string(),
                attribute["Value"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    assert!(
        attributes.contains(&("email".to_string(), "both@example.com".to_string())),
        "the stored email is unchanged by sending a code to it, got {attributes:?}"
    );
    assert!(
        !attributes.iter().any(|(name, value)| {
            (name == "email_verified" || name == "phone_number_verified") && value == "true"
        }),
        "sending a code verifies nothing, so no verified flag is confirmed, got \
         {attributes:?}"
    );

    // The code itself is never reported: the real service delivers it out of band.
    assert!(
        again.1["CodeDeliveryDetails"].get("Code").is_none()
            && again.1.get("Code").is_none()
            && !again.1.to_string().contains("\"Code\""),
        "no response member carries the code, got {}",
        again.1
    );
}

/// RULE: the attributes a code can be sent for are exactly the model's
/// `VerifiedAttributeType`, and each is paired with one of its `DeliveryMediumType`
/// values — asserted over the generated value lists rather than one cited name.
#[test]
fn every_verifiable_attribute_has_a_modelled_delivery_medium() {
    for attribute in VERIFIED_ATTRIBUTE_TYPE_VALUES {
        let medium = state::delivery_medium(attribute);
        assert!(
            medium.is_some_and(|medium| DELIVERY_MEDIUM_TYPE_VALUES.contains(&medium)),
            "case {attribute:?}: expected one of {DELIVERY_MEDIUM_TYPE_VALUES:?}, \
             got {medium:?}"
        );
    }

    // Everything else is no destination at all, including an attribute carrying an
    // address-shaped value and one the pool's schema never declared.
    for attribute in ["name", "preferred_username", "address", "custom:pager", ""] {
        let medium = state::delivery_medium(attribute);
        assert!(
            medium.is_none(),
            "case {attribute:?}: expected no delivery medium, got {medium:?}"
        );
    }
}

/// RULE: a minted verification code is six decimal digits, which is inside
/// `ConfirmationCodeType`'s modelled length 1..2048 and satisfies its pattern
/// `^[\S]+$` — over enough draws that one lucky draw cannot carry the assertion.
#[test]
fn a_minted_verification_code_is_six_digits() {
    for draw in 0..64 {
        let code = state::new_verification_code();
        assert!(
            code.len() == 6 && code.chars().all(|c| c.is_ascii_digit()),
            "case draw {draw}: expected six decimal digits, got {code:?}"
        );
    }
}
