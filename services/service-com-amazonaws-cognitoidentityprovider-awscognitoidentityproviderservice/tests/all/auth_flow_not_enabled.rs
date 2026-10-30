// SPDX-License-Identifier: CC0-1.0

//! The refusal a sign-in gets when the app client's stored `ExplicitAuthFlows`
//! do not enable the `AuthFlow` the request asked for, over the wire through the
//! router exactly as an SDK drives it (`POST /` with
//! `X-Amz-Target: AWSCognitoIdentityProviderService.<Op>`).
//!
//! The rule under test: a sign-in whose `AuthFlow` the app client's stored
//! `ExplicitAuthFlows` — "the authentication flows that you want your user pool
//! client to support" (`CreateUserPoolClient.ExplicitAuthFlows` in this
//! service's model) — do not enable is refused with a 400
//! `InvalidParameterException` carrying the sentence the ASKING API words that
//! refusal with. The administrative sign-in answers ONE fixed sentence that
//! names no flow at all; the client-side sign-in answers a sentence whose only
//! variable part is the `AuthFlowType` member as SUBMITTED, so an alias is
//! echoed as the caller wrote it. Neither names the app client id, which matters
//! because an unauthenticated caller can provoke this refusal.
//!
//! The rows span what the rule generalises over: the operation that performs the
//! sign-in (the client-side one and the admin one), the flow asked for and the
//! alias of one of them, and the stored configuration that fails to enable it
//! (the documented default set, a single other flow, several other flows). A
//! second table is the control: on an app client that DOES enable the flow —
//! including through the legacy `ExplicitAuthFlows` value for it — the same
//! request is never answered with this sentence.
//!
//! Each row names the case it asserts and every assertion reports what the call
//! actually answered.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice as cognito_idp;

/// A request's SigV4 credential scope is what routes it to this service; the
/// signature itself is never checked here.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/cognito-idp/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0000";

/// A password satisfying a default pool's password policy.
const GOOD_PASSWORD: &str = "Passw0rd!2024";

/// The user every row signs in as.
const USERNAME: &str = "alice";

/// A `REFRESH_TOKEN` value this emulator never issued, so a refresh row that
/// gets PAST the flow check still fails on the token rather than signing in.
const UNISSUED_REFRESH_TOKEN: &str = "not-a-token-this-emulator-issued";

/// The one sentence the administrative sign-in owes, whatever flow was asked
/// for.
const ADMIN_MESSAGE: &str = "Auth flow not enabled for this client";

/// The sentence the client-side sign-in owes for `auth_flow`.
fn client_message(auth_flow: &str) -> String {
    format!("{auth_flow} flow not enabled for this client")
}

/// The sentence `operation` owes for `auth_flow`.
fn expected_message(operation: &str, auth_flow: &str) -> String {
    if operation == "AdminInitiateAuth" {
        ADMIN_MESSAGE.to_string()
    } else {
        client_message(auth_flow)
    }
}

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

/// The `message` member of an error body.
fn error_message(payload: &Value) -> &str {
    payload
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
}

/// A pool with one confirmed user that every row signs in as.
async fn pool_with_user(router: &MultiServiceRouter, pool_name: &str) -> String {
    let created = ok(router, "CreateUserPool", &json!({ "PoolName": pool_name })).await;
    let user_pool_id = created["UserPool"]["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateUserPool must mint an id, got {created}"))
        .to_string();
    ok(
        router,
        "AdminCreateUser",
        &json!({ "UserPoolId": user_pool_id, "Username": USERNAME }),
    )
    .await;
    ok(
        router,
        "AdminSetUserPassword",
        &json!({
            "UserPoolId": user_pool_id,
            "Username": USERNAME,
            "Password": GOOD_PASSWORD,
            "Permanent": true,
        }),
    )
    .await;
    user_pool_id
}

/// Create an app client holding `flows`, or — for an EMPTY slice — one that
/// names no `ExplicitAuthFlows` at all and so holds the documented default set.
async fn create_client(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    client_name: &str,
    flows: &[&str],
) -> String {
    let mut body = Map::new();
    body.insert("UserPoolId".to_string(), json!(user_pool_id));
    body.insert("ClientName".to_string(), json!(client_name));
    if !flows.is_empty() {
        body.insert("ExplicitAuthFlows".to_string(), json!(flows));
    }
    let created = ok(router, "CreateUserPoolClient", &Value::Object(body)).await;
    created["UserPoolClient"]["ClientId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateUserPoolClient must mint a client id, got {created}"))
        .to_string()
}

/// The `AuthParameters` each flow requires before the app client's configuration
/// is even looked at, so every row is refused on the flow and not on a missing
/// parameter (`InitiateAuth.AuthParameters` /
/// `AdminInitiateAuth.AuthParameters` in this service's model).
fn auth_parameters(auth_flow: &str) -> Value {
    match auth_flow {
        "REFRESH_TOKEN_AUTH" | "REFRESH_TOKEN" => {
            json!({ "REFRESH_TOKEN": UNISSUED_REFRESH_TOKEN })
        }
        "USER_AUTH" => json!({ "USERNAME": USERNAME, "PREFERRED_CHALLENGE": "EMAIL_OTP" }),
        _ => json!({ "USERNAME": USERNAME, "PASSWORD": GOOD_PASSWORD }),
    }
}

/// A sign-in request for `operation`: the admin one names the pool, the
/// client-side one is named by its app client alone.
fn sign_in_request(operation: &str, user_pool_id: &str, client_id: &str, auth_flow: &str) -> Value {
    let mut body = Map::new();
    if operation == "AdminInitiateAuth" {
        body.insert("UserPoolId".to_string(), json!(user_pool_id));
    }
    body.insert("ClientId".to_string(), json!(client_id));
    body.insert("AuthFlow".to_string(), json!(auth_flow));
    body.insert("AuthParameters".to_string(), auth_parameters(auth_flow));
    Value::Object(body)
}

/// One row: the sign-in operation, the flow it asks for, and the
/// `ExplicitAuthFlows` the app client is created with.
struct Row {
    case: &'static str,
    operation: &'static str,
    auth_flow: &'static str,
    flows: &'static [&'static str],
}

#[tokio::test]
async fn a_flow_the_app_client_does_not_enable_is_refused_with_one_sentence() {
    let router = router();
    let user_pool_id = pool_with_user(&router, "flow-not-enabled-pool").await;

    let rows = [
        Row {
            case: "USER_PASSWORD_AUTH on a client holding the documented default flows",
            operation: "InitiateAuth",
            auth_flow: "USER_PASSWORD_AUTH",
            flows: &[],
        },
        Row {
            case: "USER_PASSWORD_AUTH on a client that enables only the admin flow",
            operation: "InitiateAuth",
            auth_flow: "USER_PASSWORD_AUTH",
            flows: &["ALLOW_ADMIN_USER_PASSWORD_AUTH"],
        },
        Row {
            case: "USER_AUTH on a client that enables only the user-password flow",
            operation: "InitiateAuth",
            auth_flow: "USER_AUTH",
            flows: &["ALLOW_USER_PASSWORD_AUTH"],
        },
        Row {
            case: "REFRESH_TOKEN_AUTH on a client that enables only the user-password flow",
            operation: "InitiateAuth",
            auth_flow: "REFRESH_TOKEN_AUTH",
            flows: &["ALLOW_USER_PASSWORD_AUTH"],
        },
        Row {
            case: "the REFRESH_TOKEN alias on a client that enables two other flows",
            operation: "InitiateAuth",
            auth_flow: "REFRESH_TOKEN",
            flows: &["ALLOW_USER_PASSWORD_AUTH", "ALLOW_USER_AUTH"],
        },
        Row {
            case: "ADMIN_USER_PASSWORD_AUTH on a client holding the documented default flows",
            operation: "AdminInitiateAuth",
            auth_flow: "ADMIN_USER_PASSWORD_AUTH",
            flows: &[],
        },
        Row {
            case: "ADMIN_NO_SRP_AUTH on a client that enables only the client-side flow",
            operation: "AdminInitiateAuth",
            auth_flow: "ADMIN_NO_SRP_AUTH",
            flows: &["ALLOW_USER_PASSWORD_AUTH"],
        },
        Row {
            case: "ADMIN_USER_PASSWORD_AUTH on a client that enables three other flows",
            operation: "AdminInitiateAuth",
            auth_flow: "ADMIN_USER_PASSWORD_AUTH",
            flows: &[
                "ALLOW_USER_SRP_AUTH",
                "ALLOW_CUSTOM_AUTH",
                "ALLOW_USER_AUTH",
            ],
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let client_id = create_client(
            &router,
            &user_pool_id,
            &format!("refused-client-{index}"),
            row.flows,
        )
        .await;
        let request = sign_in_request(row.operation, &user_pool_id, &client_id, row.auth_flow);
        let (status, payload) = call(&router, row.operation, &request).await;

        assert_eq!(
            (status, error_code(&payload)),
            (400, "InvalidParameterException"),
            "case {case:?}: expected a 400 InvalidParameterException, got {status} with {payload}"
        );
        let message = error_message(&payload);
        let expected = expected_message(row.operation, row.auth_flow);
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
        // The administrative sentence names no flow; the client-side one names
        // the flow asked for and nothing else.
        assert_eq!(
            message.contains(row.auth_flow),
            row.operation != "AdminInitiateAuth",
            "case {case:?}: only the client-side sentence names the flow asked for, got \
             {message:?}"
        );
        assert!(
            !message.contains(client_id.as_str()),
            "case {case:?}: the refusal must not name the app client, got {message:?}"
        );
        assert!(
            !message.contains(user_pool_id.as_str()),
            "case {case:?}: the refusal must not name the user pool, got {message:?}"
        );
    }
}

#[tokio::test]
async fn a_client_that_enables_the_flow_is_never_answered_that_sentence() {
    let router = router();
    let user_pool_id = pool_with_user(&router, "flow-enabled-pool").await;

    // Rows differ along the operation, the flow, and whether the stored value
    // that enables it is the current `ALLOW_*` one or the legacy one the same
    // member documents. `signs_in` is false where the flow is enabled but the
    // request still cannot complete — the refresh row presents a token nothing
    // issued, and the choice-based row asks for a first authentication factor
    // this pool's sign-in policy does not admit — which is exactly the point:
    // the answer is some OTHER refusal, never this sentence.
    let rows: &[(Row, bool)] = &[
        (
            Row {
                case: "USER_PASSWORD_AUTH on a client that enables it",
                operation: "InitiateAuth",
                auth_flow: "USER_PASSWORD_AUTH",
                flows: &["ALLOW_USER_PASSWORD_AUTH"],
            },
            true,
        ),
        (
            Row {
                case: "USER_PASSWORD_AUTH enabled through the legacy value",
                operation: "InitiateAuth",
                auth_flow: "USER_PASSWORD_AUTH",
                flows: &["USER_PASSWORD_AUTH"],
            },
            true,
        ),
        (
            Row {
                case: "ADMIN_USER_PASSWORD_AUTH on a client that enables it",
                operation: "AdminInitiateAuth",
                auth_flow: "ADMIN_USER_PASSWORD_AUTH",
                flows: &["ALLOW_ADMIN_USER_PASSWORD_AUTH"],
            },
            true,
        ),
        (
            Row {
                case: "ADMIN_NO_SRP_AUTH enabled through the legacy value",
                operation: "AdminInitiateAuth",
                auth_flow: "ADMIN_NO_SRP_AUTH",
                flows: &["ADMIN_NO_SRP_AUTH"],
            },
            true,
        ),
        (
            Row {
                case: "REFRESH_TOKEN_AUTH on a client that enables it",
                operation: "InitiateAuth",
                auth_flow: "REFRESH_TOKEN_AUTH",
                flows: &["ALLOW_REFRESH_TOKEN_AUTH"],
            },
            false,
        ),
        (
            Row {
                case: "USER_AUTH on a client that enables it",
                operation: "InitiateAuth",
                auth_flow: "USER_AUTH",
                flows: &["ALLOW_USER_AUTH"],
            },
            false,
        ),
    ];

    for (index, (row, signs_in)) in rows.iter().enumerate() {
        let case = row.case;
        let client_id = create_client(
            &router,
            &user_pool_id,
            &format!("enabled-client-{index}"),
            row.flows,
        )
        .await;
        let request = sign_in_request(row.operation, &user_pool_id, &client_id, row.auth_flow);
        let (status, payload) = call(&router, row.operation, &request).await;

        let message = error_message(&payload);
        assert_ne!(
            message,
            expected_message(row.operation, row.auth_flow).as_str(),
            "case {case:?}: an enabled flow must not be refused as not enabled, got {status} \
             with {payload}"
        );
        if *signs_in {
            assert_eq!(
                status, 200,
                "case {case:?}: the stored user must be signed in, got {status} with {payload}"
            );
            let result = &payload["AuthenticationResult"];
            for member in ["AccessToken", "IdToken", "RefreshToken"] {
                assert!(
                    result[member].as_str().is_some_and(|token| !token.is_empty()),
                    "case {case:?}: a completed sign-in returns {member}, got {payload}"
                );
            }
        } else {
            assert_ne!(
                status, 200,
                "case {case:?}: this row cannot complete its sign-in, got {payload}"
            );
        }
    }
}
