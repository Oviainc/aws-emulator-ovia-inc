// SPDX-License-Identifier: CC0-1.0

//! The custom email sender Lambda trigger of a user pool: the function every code-bearing
//! message of a pool that configures one is handed to, instead of being mailed.
//!
//! The rule under test: on a pool whose `LambdaConfig` carries a `CustomEmailSender` — "this
//! trigger routes all email notifications from a user pool to a Lambda function"
//! (`LambdaConfigType$CustomEmailSender`) — the operation that ISSUES a code invokes that
//! `LambdaArn` with the custom-email-sender event of the stage that issued it: its
//! `triggerSource` names the trigger and that stage, its `request` is the
//! `customEmailSenderRequestV1` document, and its `request.code` is the issued code
//! encrypted under `LambdaConfig.KMSKeyID` — "the ARN of an KMS key. Amazon Cognito uses
//! the key to encrypt codes and temporary passwords sent to CustomEmailSender and
//! CustomSMSSender" (`LambdaConfigType$KMSKeyID`). The code the function receives is the
//! code the operation that consumes it accepts; a pool that configures no custom sender
//! invokes nothing.
//!
//! Three suites, each pinning one half of that:
//!
//! 1. the whole round trip through a function Lambda really runs: the message it received is
//!    read out of the queue it forwarded it to, decrypted under the pool's own key, and then
//!    answered — a wrong code is refused and the delivered one verifies the attribute;
//! 2. every code-issuing stage issues the invocation, pinned through the refusal a pool
//!    whose trigger names a function Lambda does not hold earns: that refusal can only be
//!    built from an answer Lambda gave;
//! 3. a pool that configures no custom sender invokes nothing, so the same calls are
//!    answered by Cognito alone.
//!
//! Nothing here asserts a value baked into the service: the key ARN, the queue URL, the
//! function ARN, the pool id, the username and the code itself all come back from the state
//! the setup calls created.

use std::io::Write as _;
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine as _;
use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice as cognito_idp;

use cognito_idp::state::lambda_triggers::esdk;

/// The Region every request below is signed for.
const REGION: &str = "us-east-1";

/// The local name of Cognito's Smithy shape, which is the `X-Amz-Target` prefix.
const COGNITO_TARGET_PREFIX: &str = "AWSCognitoIdentityProviderService";

/// The `X-Amz-Target` prefix of the key service.
const KMS_TARGET_PREFIX: &str = "TrentService";

/// The `X-Amz-Target` prefix of Amazon SQS.
const SQS_TARGET_PREFIX: &str = "AmazonSQS";

/// Where Lambda's `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// A well-formed execution role ARN, satisfying Lambda's `Role` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// The module and attribute the trigger function's package carries its handler as.
const HANDLER: &str = "lambda_function.lambda_handler";

/// A password satisfying a default pool's password policy.
const GOOD_PASSWORD: &str = "Passw0rd!2024";

/// How long this suite waits for a message the trigger function forwarded.
const DELIVERY_WAIT: Duration = Duration::from_secs(20);

/// How long a poll of the queue waits for a message to appear.
const RECEIVE_WAIT_SECONDS: u64 = 1;

/// Cognito, the key service, Lambda and Amazon SQS over ONE shared engine, as
/// `core/router/src/lib.rs` mounts them: Cognito's trigger dispatch wraps the code with a
/// key the key service holds and hands it to a function Lambda holds, which writes what it
/// received to a queue.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(cognito_idp::EmulatedService::new(Arc::clone(&engine)));
    router.register_service(service_com_amazonaws_kms_trentservice::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router
}

/// A SigV4 `Authorization` whose credential scope names `service`, which is what routes the
/// request. The signature is never checked here.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=emulator-test-key/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    )
}

/// One `awsJson` call to the service whose signing name is `service`, addressed by
/// `X-Amz-Target`.
async fn json_call(
    router: &MultiServiceRouter,
    service: &str,
    content_type: &str,
    target: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{service}.{REGION}.amazonaws.com"))
        .header("content-type", content_type)
        .header("x-amz-target", target)
        .header("authorization", authorization(service))
        .body(Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// One Cognito call, as its status and parsed body.
async fn cognito(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    json_call(
        router,
        "cognito-idp",
        "application/x-amz-json-1.1",
        &format!("{COGNITO_TARGET_PREFIX}.{operation}"),
        body,
    )
    .await
}

/// A Cognito call that must answer 200, with its response body.
async fn ok(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, payload) = cognito(router, operation, body).await;
    assert_eq!(
        status, 200,
        "{operation} must succeed, got {status} with {payload}"
    );
    payload
}

/// One key-service call that must answer 200.
async fn kms(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, payload) = json_call(
        router,
        "kms",
        "application/x-amz-json-1.1",
        &format!("{KMS_TARGET_PREFIX}.{operation}"),
        body,
    )
    .await;
    assert_eq!(
        status, 200,
        "KMS {operation} must succeed, got {status} with {payload}"
    );
    payload
}

/// One Amazon SQS call that must answer 200.
async fn sqs(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, payload) = json_call(
        router,
        "sqs",
        "application/x-amz-json-1.0",
        &format!("{SQS_TARGET_PREFIX}.{operation}"),
        body,
    )
    .await;
    assert_eq!(
        status, 200,
        "SQS {operation} must succeed, got {status} with {payload}"
    );
    payload
}

/// The wire error code of an error body (`__type`), without its namespace.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The `message` member of a body.
fn message(body: &Value) -> String {
    body.get("message")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// A Lambda function ARN that names a function nothing created.
fn absent_function(name: &str) -> String {
    format!("arn:aws:lambda:{REGION}:000000000000:function:{name}")
}

/// Create a symmetric key and return the ARN the key service minted for it.
async fn create_key(router: &MultiServiceRouter) -> String {
    let created = kms(router, "CreateKey", &json!({ "Description": "custom sender" })).await;
    created["KeyMetadata"]["Arn"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateKey must mint a key ARN, got {created}"))
        .to_string()
}

/// A .zip deployment package holding `source` as the module the handler names.
fn package(source: &str) -> Vec<u8> {
    let mut buffer = std::io::Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut buffer);
        let options: zip::write::FileOptions<'_, '_, ()> = zip::write::FileOptions::default();
        writer
            .start_file("lambda_function.py", options)
            .expect("the entry starts");
        writer
            .write_all(source.as_bytes())
            .expect("the entry's bytes");
        writer.finish().expect("a finished archive");
    }
    buffer.into_inner()
}

/// Create a Lambda function running `source`, and return the ARN Lambda minted for it.
///
/// The function is created and then waited for: an invocation only reaches a function
/// Lambda reports as active.
async fn create_function(router: &MultiServiceRouter, name: &str, source: &str) -> String {
    let request = http::Request::builder()
        .method("POST")
        .uri(FUNCTIONS_URI)
        .header("host", format!("lambda.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header("authorization", authorization("lambda"))
        .body(Bytes::from(
            serde_json::to_vec(&json!({
                "FunctionName": name,
                "Role": ROLE,
                "Runtime": "python3.12",
                "Handler": HANDLER,
                "Timeout": 10,
                "Code": {
                    "ZipFile": base64::engine::general_purpose::STANDARD.encode(package(source)),
                },
            }))
            .expect("the create body serializes"),
        ))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let created: Value = serde_json::from_slice(&response.into_body()).unwrap_or(Value::Null);
    assert_eq!(
        status, 201,
        "CreateFunction must answer 201, got {status} with {created}"
    );
    tokio::time::sleep(
        service_com_amazonaws_lambda_awsgirapiservice::state::ACTIVATION_DELAY
            + Duration::from_millis(200),
    )
    .await;
    created["FunctionArn"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateFunction must mint an ARN, got {created}"))
        .to_string()
}

/// Create a pool whose `LambdaConfig` is `lambda_config`, and an app client of it that
/// allows the administrative password sign-in this suite takes its access tokens from.
async fn pool_with_client(
    router: &MultiServiceRouter,
    name: &str,
    lambda_config: Value,
) -> (String, String) {
    let mut body = json!({ "PoolName": name, "AutoVerifiedAttributes": ["email"] });
    if !lambda_config.is_null() {
        body["LambdaConfig"] = lambda_config;
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
            "ClientName": format!("{name}-app"),
            "ExplicitAuthFlows": ["ALLOW_ADMIN_USER_PASSWORD_AUTH", "ALLOW_REFRESH_TOKEN_AUTH"],
        }),
    )
    .await;
    let client_id = client["UserPoolClient"]["ClientId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateUserPoolClient must mint a client id, got {client}"))
        .to_string();
    (user_pool_id, client_id)
}

/// Create `username` with a verified email address and a permanent password, and return the
/// access token an administrative sign-in issues for it.
///
/// None of these calls issues a code: an administrator that vouches for the address with
/// `email_verified` "skip[s] the verification message"
/// (`AdminUpdateUserAttributes$UserAttributes`), so the pool's custom sender is not reached
/// by the setup.
async fn signed_in_user(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    client_id: &str,
    username: &str,
) -> String {
    ok(
        router,
        "AdminCreateUser",
        &json!({
            "UserPoolId": user_pool_id,
            "Username": username,
            "MessageAction": "SUPPRESS",
            "UserAttributes": [
                { "Name": "email", "Value": format!("{username}@example.test") },
                { "Name": "email_verified", "Value": "true" },
            ],
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
        .unwrap_or_else(|| panic!("the sign-in must issue an access token, got {signed_in}"))
        .to_string()
}

/// The handler the trigger function runs: it forwards what the custom sender event carried
/// to the queue the source names, which is how a test observes what the function received.
fn handler_source(queue_url: &str) -> String {
    format!(
        r#"
import json

import boto3

QUEUE_URL = "{queue_url}"


def lambda_handler(event, context):
    client = boto3.client("sqs")
    client.send_message(
        QueueUrl=QUEUE_URL,
        MessageBody=json.dumps(
            {{
                "triggerSource": event["triggerSource"],
                "userName": event["userName"],
                "userPoolId": event["userPoolId"],
                "type": event["request"].get("type"),
                "code": event["request"].get("code"),
                "hasResponse": "response" in event,
            }}
        ),
    )
    return {{}}
"#
    )
}

/// One message the trigger function forwarded, waited for until [`DELIVERY_WAIT`] is spent.
async fn next_delivery(router: &MultiServiceRouter, case: &str, queue_url: &str) -> Value {
    let deadline = Instant::now() + DELIVERY_WAIT;
    while Instant::now() < deadline {
        let received = sqs(
            router,
            "ReceiveMessage",
            &json!({
                "QueueUrl": queue_url,
                "MaxNumberOfMessages": 1,
                "WaitTimeSeconds": RECEIVE_WAIT_SECONDS,
            }),
        )
        .await;
        if let Some(body) = received["Messages"]
            .as_array()
            .and_then(|messages| messages.first())
            .and_then(|first| first["Body"].as_str())
        {
            return serde_json::from_str(body).unwrap_or_else(|error| {
                panic!("case {case:?}: the delivery is not JSON ({error}): {body}")
            });
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("case {case:?}: no custom sender delivery reached the queue within {DELIVERY_WAIT:?}");
}

/// The code a delivery carried, decrypted under the pool's own key.
///
/// Every step is the one a decrypting client performs: the member is base64, the bytes are
/// an encryption-SDK message, the key it names is asked to unwrap the data key, and the
/// message opens under the data key that comes back.
async fn decrypted_code(
    router: &MultiServiceRouter,
    case: &str,
    delivery: &Value,
    key_arn: &str,
) -> String {
    let carried = delivery["code"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the delivery carries no code: {delivery}"));
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(carried)
        .unwrap_or_else(|error| {
            panic!("case {case:?}: the code is not base64 ({error}): {carried}")
        });
    let parsed = esdk::parse(&bytes)
        .unwrap_or_else(|fault| panic!("case {case:?}: the code is not a message: {fault}"));
    assert_eq!(
        parsed.key_provider_id, esdk::KEY_PROVIDER_ID,
        "case {case:?}: expected the key service as the key provider, got {:?}",
        parsed.key_provider_id
    );
    assert_eq!(
        parsed.key_provider_info, key_arn,
        "case {case:?}: expected the pool's own key, got {:?}",
        parsed.key_provider_info
    );
    let unwrapped = kms(
        router,
        "Decrypt",
        &json!({
            "CiphertextBlob": base64::engine::general_purpose::STANDARD
                .encode(&parsed.encrypted_data_key),
        }),
    )
    .await;
    let data_key = base64::engine::general_purpose::STANDARD
        .decode(
            unwrapped["Plaintext"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: Decrypt answered no plaintext")),
        )
        .expect("the key service answers base64");
    let opened = parsed
        .open(&data_key)
        .unwrap_or_else(|fault| panic!("case {case:?}: the message does not open: {fault}"));
    String::from_utf8(opened)
        .unwrap_or_else(|error| panic!("case {case:?}: the code is not text: {error}"))
}

/// The value of `attribute` `AdminGetUser` reports for `username`.
async fn stored_attribute(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    username: &str,
    attribute: &str,
) -> String {
    let read = ok(
        router,
        "AdminGetUser",
        &json!({ "UserPoolId": user_pool_id, "Username": username }),
    )
    .await;
    read["UserAttributes"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|held| held["Name"].as_str() == Some(attribute))
        .and_then(|held| held["Value"].as_str())
        .unwrap_or("<absent>")
        .to_string()
}

#[tokio::test]
async fn the_custom_sender_receives_the_code_the_attribute_verification_accepts() {
    let router = router();
    let key_arn = create_key(&router).await;
    let queue = sqs(&router, "CreateQueue", &json!({ "QueueName": "sender-sink" })).await;
    let queue_url = queue["QueueUrl"]
        .as_str()
        .expect("CreateQueue must mint a queue URL")
        .to_string();
    let function_arn =
        create_function(&router, "custom-email-sender", &handler_source(&queue_url)).await;
    let (user_pool_id, client_id) = pool_with_client(
        &router,
        "routed",
        json!({
            "CustomEmailSender": { "LambdaVersion": "V1_0", "LambdaArn": function_arn },
            "KMSKeyID": key_arn,
        }),
    )
    .await;
    let username = "routeduser";
    let access_token = signed_in_user(&router, &user_pool_id, &client_id, username).await;

    // An update of a verifiable attribute issues a code, and on this pool that code is
    // delivered by invoking the configured function.
    let changed = "routed-changed@example.test";
    let updated = ok(
        &router,
        "UpdateUserAttributes",
        &json!({
            "AccessToken": access_token,
            "UserAttributes": [{ "Name": "email", "Value": changed }],
            "ClientMetadata": { "tenant": "acme" },
        }),
    )
    .await;
    assert!(
        updated["CodeDeliveryDetailsList"]
            .as_array()
            .is_some_and(|sent| !sent.is_empty()),
        "the update must report the message it sent, got {updated}"
    );

    let case = "UpdateUserAttributes on a verifiable attribute";
    let delivery = next_delivery(&router, case, &queue_url).await;
    assert_eq!(
        delivery["triggerSource"].as_str(),
        Some("CustomEmailSender_UpdateUserAttribute"),
        "case {case:?}: expected the update's own trigger source, got {delivery}"
    );
    assert_eq!(
        (
            delivery["userName"].as_str(),
            delivery["userPoolId"].as_str(),
        ),
        (Some(username), Some(user_pool_id.as_str())),
        "case {case:?}: expected the pool and user the code was issued for, got {delivery}"
    );
    assert_eq!(
        delivery["type"].as_str(),
        Some("customEmailSenderRequestV1"),
        "case {case:?}: expected the custom sender request type, got {delivery}"
    );
    assert_eq!(
        delivery["hasResponse"].as_bool(),
        Some(false),
        "case {case:?}: a sender amends nothing, so the event carries no response half, got \
         {delivery}"
    );
    let code = decrypted_code(&router, case, &delivery, &key_arn).await;
    assert!(
        code.len() == 6 && code.chars().all(|digit| digit.is_ascii_digit()),
        "case {case:?}: expected a six-digit code, got {code:?}"
    );

    // A value that is not the delivered code is refused, in the one wording this API
    // refuses a wrong code with.
    let wrong = format!("{}", code.parse::<u32>().unwrap_or(0) ^ 1);
    let (status, refused) = cognito(
        &router,
        "VerifyUserAttribute",
        &json!({
            "AccessToken": access_token,
            "AttributeName": "email",
            "Code": wrong,
        }),
    )
    .await;
    assert_eq!(
        (status, error_code(&refused).as_str(), message(&refused).as_str()),
        (
            400,
            "CodeMismatchException",
            "Invalid verification code provided, please try again."
        ),
        "case \"a code that is not the delivered one\": expected the mismatch refusal, got \
         {status} ({refused})"
    );

    // The delivered code verifies the attribute, which is what makes the trigger the real
    // delivery channel: nothing else ever reported this code.
    ok(
        &router,
        "VerifyUserAttribute",
        &json!({
            "AccessToken": access_token,
            "AttributeName": "email",
            "Code": code,
        }),
    )
    .await;
    assert_eq!(
        stored_attribute(&router, &user_pool_id, username, "email_verified").await,
        "true",
        "case \"the delivered code\": expected the attribute to read back verified"
    );
    assert_eq!(
        stored_attribute(&router, &user_pool_id, username, "email").await,
        changed,
        "case \"the delivered code\": expected the updated address to read back"
    );

    // A resend goes to the same function, under its own stage's source, with a FRESH code:
    // a later code for the same attribute supersedes the one before it.
    let case = "GetUserAttributeVerificationCode";
    ok(
        &router,
        "GetUserAttributeVerificationCode",
        &json!({ "AccessToken": access_token, "AttributeName": "email" }),
    )
    .await;
    let resent = next_delivery(&router, case, &queue_url).await;
    assert_eq!(
        resent["triggerSource"].as_str(),
        Some("CustomEmailSender_VerifyUserAttribute"),
        "case {case:?}: expected the verification stage's own trigger source, got {resent}"
    );
    let resent_code = decrypted_code(&router, case, &resent, &key_arn).await;
    assert_ne!(
        resent_code, code,
        "case {case:?}: expected a fresh code, got the one the update delivered"
    );
    ok(
        &router,
        "VerifyUserAttribute",
        &json!({
            "AccessToken": access_token,
            "AttributeName": "email",
            "Code": resent_code,
        }),
    )
    .await;
}

/// Which code-issuing stage a row drives.
#[derive(Clone, Copy, Debug)]
enum Stage {
    /// `SignUp`, which sends a new user's confirmation code.
    SignUp,
    /// `ResendConfirmationCode`, which sends it again.
    ResendCode,
    /// `ForgotPassword`, which sends a password-reset code.
    ForgotPassword,
    /// `AdminResetUserPassword`, which sends the same reset code as an administrator.
    AdminResetPassword,
    /// `UpdateUserAttributes`, which sends the code of an attribute it changed.
    UpdateAttributes,
    /// `AdminUpdateUserAttributes`, which sends that code as an administrator.
    AdminUpdateAttributes,
    /// `GetUserAttributeVerificationCode`, which sends one attribute's code.
    VerificationCode,
}

impl Stage {
    /// The operation a row calls last, which is the one that issues the code.
    fn operation(self) -> &'static str {
        match self {
            Self::SignUp => "SignUp",
            Self::ResendCode => "ResendConfirmationCode",
            Self::ForgotPassword => "ForgotPassword",
            Self::AdminResetPassword => "AdminResetUserPassword",
            Self::UpdateAttributes => "UpdateUserAttributes",
            Self::AdminUpdateAttributes => "AdminUpdateUserAttributes",
            Self::VerificationCode => "GetUserAttributeVerificationCode",
        }
    }
}

/// Register `username` through `client_id` with an email address, and return what the
/// registration answered.
async fn sign_up(
    router: &MultiServiceRouter,
    client_id: &str,
    username: &str,
) -> (u16, Value) {
    cognito(
        router,
        "SignUp",
        &json!({
            "ClientId": client_id,
            "Username": username,
            "Password": GOOD_PASSWORD,
            "UserAttributes": [{ "Name": "email", "Value": format!("{username}@example.test") }],
        }),
    )
    .await
}

/// Drive `stage` against `user_pool_id`, each row on a user of its own, and return what the
/// code-issuing operation answered.
async fn drive(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    client_id: &str,
    username: &str,
    stage: Stage,
) -> (u16, Value) {
    let changed = format!("{username}-changed@example.test");
    match stage {
        Stage::SignUp => sign_up(router, client_id, username).await,
        Stage::ResendCode => {
            // The sign-up that registers the user is the SignUp row's own subject; what it
            // answered says nothing about this row, and the user it created is there either
            // way.
            let _ = sign_up(router, client_id, username).await;
            cognito(
                router,
                stage.operation(),
                &json!({ "ClientId": client_id, "Username": username }),
            )
            .await
        }
        Stage::ForgotPassword => {
            signed_in_user(router, user_pool_id, client_id, username).await;
            cognito(
                router,
                stage.operation(),
                &json!({ "ClientId": client_id, "Username": username }),
            )
            .await
        }
        Stage::AdminResetPassword => {
            signed_in_user(router, user_pool_id, client_id, username).await;
            cognito(
                router,
                stage.operation(),
                &json!({ "UserPoolId": user_pool_id, "Username": username }),
            )
            .await
        }
        Stage::UpdateAttributes => {
            let token = signed_in_user(router, user_pool_id, client_id, username).await;
            cognito(
                router,
                stage.operation(),
                &json!({
                    "AccessToken": token,
                    "UserAttributes": [{ "Name": "email", "Value": changed }],
                }),
            )
            .await
        }
        Stage::AdminUpdateAttributes => {
            signed_in_user(router, user_pool_id, client_id, username).await;
            cognito(
                router,
                stage.operation(),
                &json!({
                    "UserPoolId": user_pool_id,
                    "Username": username,
                    "UserAttributes": [{ "Name": "email", "Value": changed }],
                }),
            )
            .await
        }
        Stage::VerificationCode => {
            let token = signed_in_user(router, user_pool_id, client_id, username).await;
            cognito(
                router,
                stage.operation(),
                &json!({ "AccessToken": token, "AttributeName": "email" }),
            )
            .await
        }
    }
}

/// Every stage a row drives, which is every stage this service issues a code at.
const EVERY_STAGE: [Stage; 7] = [
    Stage::SignUp,
    Stage::ResendCode,
    Stage::ForgotPassword,
    Stage::AdminResetPassword,
    Stage::UpdateAttributes,
    Stage::AdminUpdateAttributes,
    Stage::VerificationCode,
];

#[tokio::test]
async fn every_code_issuing_stage_invokes_the_pools_custom_sender() {
    // The pool's trigger names a function Lambda does not hold, so the refusal each row
    // earns can only have been built from an answer Lambda gave: that is what says the
    // invocation was issued at that stage.
    let router = router();
    let key_arn = create_key(&router).await;
    let absent = absent_function("never-created-sender");
    let (user_pool_id, client_id) = pool_with_client(
        &router,
        "absent-sender",
        json!({
            "CustomEmailSender": { "LambdaVersion": "V1_0", "LambdaArn": absent },
            "KMSKeyID": key_arn,
        }),
    )
    .await;

    for (index, stage) in EVERY_STAGE.into_iter().enumerate() {
        let case = stage.operation();
        let username = format!("stage{index}");
        let (status, answer) = drive(&router, &user_pool_id, &client_id, &username, stage).await;
        assert_eq!(
            (status, error_code(&answer).as_str()),
            (400, "UnexpectedLambdaException"),
            "case {case:?}: expected the trigger's own failure, got {status} ({answer})"
        );
        assert_eq!(
            message(&answer),
            format!("CustomEmailSender failed with error Function not found: {absent}:$LATEST."),
            "case {case:?}: expected the failure to name the trigger and what Lambda answered, \
             got {answer}"
        );
    }
}

#[tokio::test]
async fn a_pool_that_configures_no_custom_sender_invokes_nothing() {
    // (case, the pool's LambdaConfig) — a pool that configures none at all, and one that
    // configures another trigger and a key, so neither the key nor another member is what
    // routes a message to a function.
    let rows: [(&str, Value); 2] = [
        ("a pool with no LambdaConfig at all", Value::Null),
        (
            "a pool configuring another trigger and a key",
            json!({
                "PostConfirmation": absent_function("post-confirm"),
                "KMSKeyID": "arn:aws:kms:us-east-1:000000000000:key/00000000-0000-0000-0000-000000000000",
            }),
        ),
    ];

    for (index, (case, lambda_config)) in rows.into_iter().enumerate() {
        let router = router();
        let (user_pool_id, client_id) =
            pool_with_client(&router, &format!("unrouted{index}"), lambda_config).await;
        for (stage_index, stage) in EVERY_STAGE.into_iter().enumerate() {
            let username = format!("stage{index}x{stage_index}");
            let (status, answer) =
                drive(&router, &user_pool_id, &client_id, &username, stage).await;
            assert_eq!(
                status,
                200,
                "case {case:?} ({}): expected Cognito's own answer, got {status} ({answer})",
                stage.operation()
            );
        }
    }
}
