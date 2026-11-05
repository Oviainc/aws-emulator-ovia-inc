// SPDX-License-Identifier: CC0-1.0

//! The post-confirmation user-pool Lambda trigger across the two services that own the
//! two halves: Cognito holds the pool whose `LambdaConfig` names the function, Lambda
//! holds the function.
//!
//! The rule pinned here: when an operation completes a confirmation of a user in a pool
//! whose `LambdaConfig` names a `PostConfirmation` function, Cognito INVOKES that function
//! — "Amazon Cognito invokes triggers at several possible stages of user pool operations"
//! (`LambdaConfigType`) — so a pool whose trigger names a function Lambda does not hold
//! refuses the confirmation with the operation's modelled `UnexpectedLambdaException`
//! naming what Lambda answered, while a pool naming one Lambda does hold, and a pool
//! naming none, are answered by Cognito alone.
//!
//! That refusal is the observable the invocation leaves: it can only be built from an
//! answer Lambda gave, so a row that earns it proves the invocation was issued.
//!
//! Rows span what the rule generalises over: whether the function exists, which Region its
//! ARN names, a pool that configures no trigger at all, and each of the three operations
//! that complete a confirmation. Every row also reads the user back: the trigger "can take
//! custom actions after a user confirms their user account", so the confirmation is
//! already settled when the trigger runs and stands whatever the trigger answered.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice as cognito_idp;

/// The Region every request here is signed for.
const REGION: &str = "us-east-1";

/// The account id every emulated ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// A role ARN inside Lambda's `Role` pattern.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/service-role/trigger-exec";

/// A deployment package as `Code.ZipFile` carries it: base64 in a `restJson1` JSON body.
/// The content is irrelevant here — only that Lambda accepts it and stores a function.
const ZIP_FILE: &str = "UEsDBAoAAAAAAAAAAAA=";

/// The local name of Cognito's Smithy shape, which is the `X-Amz-Target` prefix.
const COGNITO_TARGET_PREFIX: &str = "AWSCognitoIdentityProviderService";

/// A password satisfying a default pool's password policy.
const GOOD_PASSWORD: &str = "Passw0rd!2024";

/// The password a reset row sets, distinct from the one the sign-up used.
const RESET_PASSWORD: &str = "Res3tPassw0rd!";

/// Cognito and Lambda over ONE shared engine, as `router/src/lib.rs` mounts them, so
/// Cognito's in-process callback reaches this very Lambda.
fn router() -> (Arc<engine::InMemoryEngine>, MultiServiceRouter) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(cognito_idp::EmulatedService::new(Arc::clone(&engine)));
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(&engine)),
    );
    (engine, router)
}

/// One REST call to the service whose signing name is `signing_name`.
async fn rest(
    router: &MultiServiceRouter,
    signing_name: &str,
    method: &str,
    uri: &str,
    body: Value,
) -> (u16, Value) {
    let payload = if body.is_null() {
        Bytes::new()
    } else {
        Bytes::from(body.to_string())
    };
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/aws4_request, \
                 SignedHeaders=host, Signature=abc"
            ),
        )
        .body(payload)
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    answer(response)
}

/// One Cognito call: `awsJson1_1`, addressed by `X-Amz-Target`.
async fn cognito(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("cognito-idp.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "x-amz-target",
            format!("{COGNITO_TARGET_PREFIX}.{operation}"),
        )
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/cognito-idp/aws4_request, \
                 SignedHeaders=host, Signature=abc"
            ),
        )
        .body(Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    answer(response)
}

/// The status and JSON body of a dispatched response.
fn answer(response: http::Response<Bytes>) -> (u16, Value) {
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// A Cognito call that must succeed, with its response body.
async fn ok(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, payload) = cognito(router, operation, body).await;
    assert_eq!(
        status, 200,
        "{operation} must succeed, got {status} with {payload}"
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

/// A Lambda function ARN in `region`.
fn function_arn(region: &str, name: &str) -> String {
    format!("arn:aws:lambda:{region}:{ACCOUNT}:function:{name}")
}

/// Create a Lambda function and return the ARN Lambda minted for it.
async fn create_function(router: &MultiServiceRouter, name: &str) -> String {
    let (status, created) = rest(
        router,
        "lambda",
        "POST",
        "/2015-03-31/functions",
        json!({
            "FunctionName": name,
            "Role": ROLE_ARN,
            "Runtime": "python3.12",
            "Handler": "index.handler",
            "Code": {"ZipFile": ZIP_FILE},
        }),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {name:?}: expected HTTP 201 from Lambda CreateFunction, got {status} ({created})"
    );
    created["FunctionArn"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateFunction must mint an ARN, got {created}"))
        .to_string()
}

/// Create a user pool with `extra` merged into its configuration, and an app client of
/// it. Every pool here auto-verifies the email address, which is what makes a sign-up
/// send a confirmation code and a reset message have a destination to go to.
async fn pool_with_client(
    router: &MultiServiceRouter,
    name: &str,
    extra: Value,
) -> (String, String) {
    let mut body = json!({ "PoolName": name, "AutoVerifiedAttributes": ["email"] });
    if let Value::Object(members) = extra {
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
        &json!({ "UserPoolId": user_pool_id, "ClientName": format!("{name}-app") }),
    )
    .await;
    let client_id = client["UserPoolClient"]["ClientId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateUserPoolClient must mint a client id, got {client}"))
        .to_string();
    (user_pool_id, client_id)
}

/// Sign `username` up through `client_id` with an email address.
async fn sign_up(router: &MultiServiceRouter, client_id: &str, username: &str) {
    ok(
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

/// The code outstanding for `purpose`, read out of Cognito's own state: the real service
/// delivers it out of band, so no response carries it.
fn outstanding_code(
    engine: &Arc<engine::InMemoryEngine>,
    user_pool_id: &str,
    username: &str,
    purpose: &cognito_idp::state::CodePurpose,
) -> String {
    let service_state =
        cognito_idp::state::service_state(&**engine).expect("Cognito's own state");
    let guard = service_state.lock();
    guard
        .pool(user_pool_id)
        .and_then(|pool| pool.user(username))
        .and_then(|user| user.outstanding_code(purpose))
        .map(|code| code.code.clone())
        .unwrap_or_else(|| panic!("a code must be outstanding for {username}"))
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

#[tokio::test]
async fn a_confirmation_invokes_the_pools_post_confirmation_function() {
    let (_engine, router) = router();
    let present = create_function(&router, "post-confirm").await;

    // (case, the pool's LambdaConfig, expected status, expected error code)
    //
    // Every row is the same sign-up and the same AdminConfirmSignUp; only what the pool's
    // trigger names differs, so the function Lambda holds is the only thing that can
    // decide the answer.
    let rows: [(&str, Value, u16, &str); 5] = [
        (
            "a trigger naming a function Lambda holds",
            json!({ "LambdaConfig": { "PostConfirmation": present } }),
            200,
            "",
        ),
        (
            "a trigger naming a function nothing created",
            json!({ "LambdaConfig": { "PostConfirmation": function_arn(REGION, "never-created") } }),
            400,
            "UnexpectedLambdaException",
        ),
        (
            // The ARN's Region chooses which Lambda endpoint is asked. This emulator's
            // Lambda holds one store rather than one per Region, so the same name is
            // found through either endpoint.
            "a trigger naming that same function through another Region's endpoint",
            json!({ "LambdaConfig": { "PostConfirmation": function_arn("eu-west-1", "post-confirm") } }),
            200,
            "",
        ),
        (
            "a trigger whose function name differs by one character",
            json!({ "LambdaConfig": { "PostConfirmation": function_arn(REGION, "post-confir") } }),
            400,
            "UnexpectedLambdaException",
        ),
        ("a pool that configures no trigger at all", json!({}), 200, ""),
    ];

    for (index, (case, lambda_config, expected_status, expected_code)) in
        rows.into_iter().enumerate()
    {
        let configured = lambda_config["LambdaConfig"]
            .get("PostConfirmation")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let (user_pool_id, client_id) =
            pool_with_client(&router, &format!("triggered{index}"), lambda_config).await;
        let username = format!("user{index}");
        sign_up(&router, &client_id, &username).await;

        let (status, answer) = cognito(
            &router,
            "AdminConfirmSignUp",
            &json!({ "UserPoolId": user_pool_id, "Username": username }),
        )
        .await;
        assert_eq!(
            (status, error_code(&answer).as_str()),
            (expected_status, expected_code),
            "case {case:?}: expected HTTP {expected_status} {expected_code:?}, got {status} {} \
             ({answer})",
            error_code(&answer)
        );
        if !expected_code.is_empty() {
            // The refusal states the trigger and the error Lambda itself reported for the
            // function the pool named — both taken from the request's own pool and from
            // what Lambda answered. An INVOCATION's not-found refusal names the ARN it
            // resolved, qualifier included, and the trigger names no qualifier, so the
            // unpublished version is the one it resolved
            // (`lambda::state::addressed_function_arn`).
            assert_eq!(
                message(&answer),
                format!(
                    "PostConfirmation failed with error Function not found: {configured}:\
                     $LATEST."
                ),
                "case {case:?}: expected the trigger failure to name the function Lambda could \
                 not find, got {answer}"
            );
        }

        // The confirmation itself is settled before the trigger runs, so the user is
        // CONFIRMED whatever the trigger answered.
        let status = stored_status(&router, &user_pool_id, &username).await;
        assert_eq!(
            status, "CONFIRMED",
            "case {case:?}: expected the confirmed user to read back CONFIRMED, got {status:?}"
        );
    }
}

#[tokio::test]
async fn every_confirming_operation_invokes_the_trigger() {
    let (engine, router) = router();
    // A pool whose trigger names a function Lambda does not hold: the refusal it earns is
    // what says the operation issued the invocation.
    let absent = function_arn(REGION, "absent-trigger");
    let (user_pool_id, client_id) = pool_with_client(
        &router,
        "fired-from-everywhere",
        json!({ "LambdaConfig": { "PostConfirmation": absent } }),
    )
    .await;
    // `ConfirmSignUp`: the self-service confirmation, with the code the sign-up sent.
    let username = "selfservice";
    sign_up(&router, &client_id, username).await;
    let code = outstanding_code(
        &engine,
        &user_pool_id,
        username,
        &cognito_idp::state::CodePurpose::SignUpConfirmation,
    );
    let (status, answer) = cognito(
        &router,
        "ConfirmSignUp",
        &json!({
            "ClientId": client_id,
            "Username": username,
            "ConfirmationCode": code,
        }),
    )
    .await;
    assert_eq!(
        (status, error_code(&answer).as_str()),
        (400, "UnexpectedLambdaException"),
        "case \"ConfirmSignUp with an absent trigger function\": expected the trigger failure, \
         got {status} ({answer})"
    );
    assert_eq!(
        message(&answer),
        format!("PostConfirmation failed with error Function not found: {absent}:$LATEST."),
        "case \"ConfirmSignUp with an absent trigger function\": expected the failure to name \
         the function, got {answer}"
    );
    // The confirmation it fired from is still a confirmation.
    let stored = stored_status(&router, &user_pool_id, username).await;
    assert_eq!(
        stored, "CONFIRMED",
        "case \"ConfirmSignUp with an absent trigger function\": expected the user to read back \
         CONFIRMED, got {stored:?}"
    );

    // `ConfirmForgotPassword`: the other confirmation the post-confirmation trigger has a
    // documented source for, which reports ITS own source.
    ok(
        &router,
        "AdminUpdateUserAttributes",
        &json!({
            "UserPoolId": user_pool_id,
            "Username": username,
            "UserAttributes": [{ "Name": "email_verified", "Value": "true" }],
        }),
    )
    .await;
    ok(
        &router,
        "ForgotPassword",
        &json!({ "ClientId": client_id, "Username": username }),
    )
    .await;
    let reset_code = outstanding_code(
        &engine,
        &user_pool_id,
        username,
        &cognito_idp::state::CodePurpose::PasswordReset,
    );
    let (status, answer) = cognito(
        &router,
        "ConfirmForgotPassword",
        &json!({
            "ClientId": client_id,
            "Username": username,
            "ConfirmationCode": reset_code,
            "Password": RESET_PASSWORD,
        }),
    )
    .await;
    assert_eq!(
        (status, error_code(&answer).as_str()),
        (400, "UnexpectedLambdaException"),
        "case \"ConfirmForgotPassword with an absent trigger function\": expected the trigger \
         failure, got {status} ({answer})"
    );
    assert_eq!(
        message(&answer),
        format!("PostConfirmation failed with error Function not found: {absent}:$LATEST."),
        "case \"ConfirmForgotPassword with an absent trigger function\": expected the failure to \
         name the function, got {answer}"
    );
    // The reset the trigger followed is settled too: its code is spent, so presenting it
    // again finds nothing outstanding.
    let (status, replay) = cognito(
        &router,
        "ConfirmForgotPassword",
        &json!({
            "ClientId": client_id,
            "Username": username,
            "ConfirmationCode": reset_code,
            "Password": GOOD_PASSWORD,
        }),
    )
    .await;
    assert_eq!(
        error_code(&replay),
        "ExpiredCodeException",
        "case \"a reset whose trigger failed\": expected the spent code to be refused, got \
         {status} ({replay})"
    );
}
