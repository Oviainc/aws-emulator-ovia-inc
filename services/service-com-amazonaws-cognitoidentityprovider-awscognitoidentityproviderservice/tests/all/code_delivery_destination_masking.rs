// SPDX-License-Identifier: CC0-1.0

//! The `CodeDeliveryDetailsType.Destination` every operation that sends a code
//! reports, over the wire, driven exactly as an SDK drives it (`POST /` with
//! `X-Amz-Target: AWSCognitoIdentityProviderService.<Operation>`).
//!
//! The rule under test: `Destination` is "the email address or phone number
//! destination where Amazon Cognito sent the code"
//! (`smithy_query(query="doc",shape="CodeDeliveryDetailsType",member="Destination")`)
//! reported MASKED, so a caller learns which of the user's addresses was used
//! without being told an address it does not already hold. An EMAIL destination
//! keeps only the FIRST character of its local part and the FIRST character of
//! its domain, each followed by a fixed `***` run and joined by `@` — nothing of
//! the domain past that first character is disclosed, not its labels and not its
//! suffix. A destination that is not an address of that form — a phone number —
//! keeps a leading `+` and its last four characters instead, which is what
//! identifies the number to the user who gave it.
//!
//! The same projection answers every operation that reports a sent code, so the
//! table drives `SignUp`, `ResendConfirmationCode`, `ForgotPassword`,
//! `GetUserAttributeVerificationCode` and `UpdateUserAttributes` with each row.
//!
//! The second rule under test: a reply reports delivery details only for
//! messages that were actually sent, so an `UpdateUserAttributes` that triggers
//! no verification answers the EMPTY object and carries no
//! `CodeDeliveryDetailsList` member at all.
//!
//! The pools, app clients, users and access tokens are all real: they come from
//! `CreateUserPool`, `CreateUserPoolClient`, `SignUp`, `AdminCreateUser`,
//! `AdminSetUserPassword` and `InitiateAuth`.
//!
//! Each table row names the case it asserts.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
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
const EMULATOR_HOST: &str = "127.0.0.1:9311";

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
/// address, which is how a self-service operation is called.
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

/// Call the unsigned `operation`, requiring success, and return its response
/// body.
async fn ok_anonymous(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, payload) = dispatch(router, operation, body, false).await;
    assert_eq!(
        status, 200,
        "{operation} must succeed, got {status} with {payload}"
    );
    payload
}

/// Create a pool that auto-verifies `auto_verified` and return its minted
/// `UserPoolId`.
async fn pool(router: &MultiServiceRouter, name: &str, auto_verified: &[&str]) -> String {
    let mut body = json!({ "PoolName": name });
    if !auto_verified.is_empty() {
        body["AutoVerifiedAttributes"] = json!(auto_verified);
    }
    // A pool that auto-verifies `phone_number` sends that verification over SMS,
    // so it needs the Amazon SNS caller SMS is sent through: `CreateUserPool`
    // refuses the pairing without it.
    if auto_verified.contains(&"phone_number") {
        body["SmsConfiguration"] = json!({
            "SnsCallerArn": "arn:aws:iam::000000000000:role/sms-caller"
        });
    }
    let created = ok(router, "CreateUserPool", &body).await;
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
            "ClientName": "masking-client",
            "ExplicitAuthFlows": ["ALLOW_USER_PASSWORD_AUTH", "ALLOW_REFRESH_TOKEN_AUTH"],
        }),
    )
    .await;
    created["UserPoolClient"]["ClientId"]
        .as_str()
        .expect("a minted client id")
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

/// Create `username` holding `attributes`, with a PERMANENT password so the user
/// is `CONFIRMED` and can sign in, and return its access token.
async fn confirmed_user_token(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    client_id: &str,
    username: &str,
    attributes: &[(&str, &str)],
) -> String {
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
    let signed_in = ok_anonymous(
        router,
        "InitiateAuth",
        &json!({
            "ClientId": client_id,
            "AuthFlow": "USER_PASSWORD_AUTH",
            "AuthParameters": { "USERNAME": username, "PASSWORD": GOOD_PASSWORD },
        }),
    )
    .await;
    signed_in["AuthenticationResult"]["AccessToken"]
        .as_str()
        .expect("an access token")
        .to_string()
}

/// Assert that `details` reports a delivery of `attribute` over `medium` to the
/// masked `destination`.
fn assert_delivery(case: &str, details: &Value, attribute: &str, medium: &str, destination: &str) {
    assert_eq!(
        details["Destination"],
        json!(destination),
        "case {case:?}: expected Destination {destination:?}, got {details}"
    );
    assert_eq!(
        details["DeliveryMedium"],
        json!(medium),
        "case {case:?}: expected DeliveryMedium {medium:?}, got {details}"
    );
    assert_eq!(
        details["AttributeName"],
        json!(attribute),
        "case {case:?}: expected AttributeName {attribute:?}, got {details}"
    );
}

/// One row of the email-masking table.
#[derive(Clone, Copy, Debug)]
struct EmailCase {
    /// What this row asserts.
    case: &'static str,
    /// The `email` attribute value a code is addressed to.
    email: &'static str,
    /// The `Destination` every reply reporting that delivery must carry.
    masked: &'static str,
}

/// The rows differ along the local part's length and leading character, the
/// domain's label count, the length of its suffix and whether it has one at all,
/// and the case of the characters that stay readable — so what a row asserts is
/// the masking RULE rather than one rendering of it. None of these addresses is
/// the one the issue cited.
const EMAIL_CASES: &[EmailCase] = &[
    EmailCase {
        case: "a two-label domain",
        email: "sam@example.com",
        masked: "s***@e***",
    },
    EmailCase {
        case: "a long dotted local part and a four-label domain with a two-part suffix",
        email: "a.very.long.local.part@mail.example.co.uk",
        masked: "a***@m***",
    },
    EmailCase {
        case: "a single-character local part and a two-character suffix",
        email: "x@q.io",
        masked: "x***@q***",
    },
    EmailCase {
        case: "a domain carrying no suffix at all",
        email: "root@localhost",
        masked: "r***@l***",
    },
    EmailCase {
        case: "a digit-leading local part and a long suffix",
        email: "7eleven@store.example.technology",
        masked: "7***@s***",
    },
    EmailCase {
        case: "upper-case initials, which stay as stored",
        email: "Zoe.Smith@Example.ORG",
        masked: "Z***@E***",
    },
];

/// RULE: every operation that reports a sent code reports an EMAIL destination
/// with only the first character of the local part and the first character of
/// the domain readable.
#[tokio::test]
async fn an_email_code_destination_is_masked_past_the_first_character_of_each_part() {
    let router = emulator();
    for (index, row) in EMAIL_CASES.iter().enumerate() {
        let EmailCase {
            case,
            email,
            masked,
        } = *row;
        let user_pool_id = pool(&router, &format!("masking-pool-{index}"), &["email"]).await;
        let client_id = client(&router, &user_pool_id).await;

        // SignUp sends the confirmation code to the address the registration
        // submitted.
        let signed_up = ok_anonymous(
            &router,
            "SignUp",
            &json!({
                "ClientId": client_id,
                "Username": format!("registrant-{index}"),
                "Password": GOOD_PASSWORD,
                "UserAttributes": attribute_list(&[("email", email)]),
            }),
        )
        .await;
        assert_delivery(
            case,
            &signed_up["CodeDeliveryDetails"],
            "email",
            "EMAIL",
            masked,
        );

        // ResendConfirmationCode supersedes that code, to the same address.
        let resent = ok_anonymous(
            &router,
            "ResendConfirmationCode",
            &json!({
                "ClientId": client_id,
                "Username": format!("registrant-{index}"),
            }),
        )
        .await;
        assert_delivery(
            case,
            &resent["CodeDeliveryDetails"],
            "email",
            "EMAIL",
            masked,
        );

        // ForgotPassword sends to the pool's recovery destination, which is the
        // user's VERIFIED email address.
        let token = confirmed_user_token(
            &router,
            &user_pool_id,
            &client_id,
            &format!("member-{index}"),
            &[("email", email), ("email_verified", "true")],
        )
        .await;
        let reset = ok_anonymous(
            &router,
            "ForgotPassword",
            &json!({
                "ClientId": client_id,
                "Username": format!("member-{index}"),
            }),
        )
        .await;
        assert_delivery(
            case,
            &reset["CodeDeliveryDetails"],
            "email",
            "EMAIL",
            masked,
        );

        // GetUserAttributeVerificationCode sends to the stored value of the
        // attribute the caller named.
        let requested = ok_anonymous(
            &router,
            "GetUserAttributeVerificationCode",
            &json!({ "AccessToken": token, "AttributeName": "email" }),
        )
        .await;
        assert_delivery(
            case,
            &requested["CodeDeliveryDetails"],
            "email",
            "EMAIL",
            masked,
        );

        // UpdateUserAttributes reports the same projection, in a LIST, for the
        // new value a pool that auto-verifies the attribute sends a code to.
        let updater_token = confirmed_user_token(
            &router,
            &user_pool_id,
            &client_id,
            &format!("updater-{index}"),
            &[("name", "Updater")],
        )
        .await;
        let updated = ok_anonymous(
            &router,
            "UpdateUserAttributes",
            &json!({
                "AccessToken": updater_token,
                "UserAttributes": attribute_list(&[("email", email)]),
            }),
        )
        .await;
        let deliveries = updated["CodeDeliveryDetailsList"]
            .as_array()
            .unwrap_or_else(|| {
                panic!("case {case:?}: the update sent a code, so it reports one: {updated}")
            });
        assert_eq!(
            deliveries.len(),
            1,
            "case {case:?}: expected one delivery, got {updated}"
        );
        assert_delivery(case, &deliveries[0], "email", "EMAIL", masked);
    }
}

/// One row of the phone-masking table.
#[derive(Clone, Copy, Debug)]
struct PhoneCase {
    /// What this row asserts.
    case: &'static str,
    /// The `phone_number` attribute value a code is addressed to.
    phone: &'static str,
    /// The `Destination` the reply must carry.
    masked: &'static str,
}

/// A destination that is not an email address keeps a leading `+` and its last
/// four characters. The rows differ along the number's length, so what they
/// assert is how much of it is masked rather than one rendering.
const PHONE_CASES: &[PhoneCase] = &[
    PhoneCase {
        case: "a North American number",
        phone: "+15551230001",
        masked: "+*******0001",
    },
    PhoneCase {
        case: "a shorter number",
        phone: "+15550199",
        masked: "+****0199",
    },
    PhoneCase {
        case: "a longer international number",
        phone: "+442071838750",
        masked: "+********8750",
    },
];

/// RULE: a destination that is not an email address is masked by its own rule —
/// a leading `+` and the last four characters stay readable — so the email rule
/// does not reach it.
#[tokio::test]
async fn a_phone_number_destination_keeps_its_last_four_characters() {
    let router = emulator();
    for (index, row) in PHONE_CASES.iter().enumerate() {
        let PhoneCase {
            case,
            phone,
            masked,
        } = *row;
        let user_pool_id = pool(&router, &format!("sms-pool-{index}"), &["phone_number"]).await;
        let client_id = client(&router, &user_pool_id).await;
        let token = confirmed_user_token(
            &router,
            &user_pool_id,
            &client_id,
            &format!("texter-{index}"),
            &[("phone_number", phone)],
        )
        .await;
        let requested = ok_anonymous(
            &router,
            "GetUserAttributeVerificationCode",
            &json!({ "AccessToken": token, "AttributeName": "phone_number" }),
        )
        .await;
        assert_delivery(
            case,
            &requested["CodeDeliveryDetails"],
            "phone_number",
            "SMS",
            masked,
        );
    }
}

/// RULE: delivery details report messages that were SENT, so an update that
/// sends none carries no delivery member at all.
#[tokio::test]
async fn an_update_that_sends_no_message_reports_no_delivery_member() {
    let router = emulator();

    // (case, what the pool auto-verifies, the attributes the update submits)
    type Row<'a> = (&'a str, &'a [&'a str], Vec<(&'a str, &'a str)>);
    let rows: Vec<Row<'_>> = vec![
        (
            "a pool that verifies nothing, updating one plain attribute",
            &[],
            vec![("name", "Ada Lovelace")],
        ),
        (
            "a pool that verifies nothing, updating an email address",
            &[],
            vec![("email", "ada@example.com")],
        ),
        (
            "a pool that auto-verifies email, updating attributes it does not verify",
            &["email"],
            vec![("given_name", "Ada"), ("family_name", "Lovelace")],
        ),
        (
            "a pool that auto-verifies email, deleting the stored address",
            &["email"],
            vec![("email", "")],
        ),
    ];

    for (index, (case, auto_verified, submitted)) in rows.iter().enumerate() {
        let user_pool_id = pool(&router, &format!("silent-pool-{index}"), auto_verified).await;
        let client_id = client(&router, &user_pool_id).await;
        let token = confirmed_user_token(
            &router,
            &user_pool_id,
            &client_id,
            &format!("quiet-{index}"),
            &[("email", "stored@example.com")],
        )
        .await;
        let (status, payload) = dispatch(
            &router,
            "UpdateUserAttributes",
            &json!({
                "AccessToken": token,
                "UserAttributes": attribute_list(submitted),
            }),
            false,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the update must succeed, got {status} with {payload}"
        );
        assert_eq!(
            payload,
            json!({}),
            "case {case:?}: an update that sent nothing answers the empty object, got {payload}"
        );
    }
}
