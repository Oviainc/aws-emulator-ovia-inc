// SPDX-License-Identifier: CC0-1.0

//! `AdminDisableUser` and `AdminEnableUser` over the wire, driven exactly as an
//! SDK drives them (`POST /` with
//! `X-Amz-Target: AWSCognitoIdentityProviderService.<Op>`).
//!
//! The rule under test: an administrative deactivation STORES `Enabled=false` on
//! the stored user the request names and an activation stores `Enabled=true`, so
//! the flag a later read reports is the one the LAST such call stored, whatever
//! the order or the repetition; a deactivation also revokes every token already
//! issued to that user, so none of them authorizes a later request and the user
//! cannot sign in while it stands. The call lands on one user of one pool, the
//! user resolving by its username or by its `sub`, and a request naming a pool or
//! a user that is not stored is refused and stores nothing.
//!
//! Each table row names the case it asserts.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice as cognito_idp;

/// A request's SigV4 credential scope is what routes it to this service. The
/// signature is never checked here: the suite turns request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/cognito-idp/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0000";

/// A password satisfying a default pool's password policy, so a user can sign in
/// and the token the sign-in issues can be presented afterwards.
const GOOD_PASSWORD: &str = "Passw0rd!2024";

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

/// Create a pool and return its minted `UserPoolId`.
async fn pool(router: &MultiServiceRouter, name: &str) -> String {
    let created = ok(router, "CreateUserPool", &json!({ "PoolName": name })).await;
    created["UserPool"]["Id"]
        .as_str()
        .expect("a minted pool id")
        .to_string()
}

/// Create `username` in `user_pool_id` and return its generated `sub`.
async fn user(router: &MultiServiceRouter, user_pool_id: &str, username: &str) -> String {
    let created = ok(
        router,
        "AdminCreateUser",
        &json!({ "UserPoolId": user_pool_id, "Username": username }),
    )
    .await;
    created["User"]["Attributes"]
        .as_array()
        .expect("the stored attributes")
        .iter()
        .find(|attribute| attribute["Name"] == "sub")
        .and_then(|attribute| attribute["Value"].as_str())
        .expect("a generated sub")
        .to_string()
}

/// Apply `operation` (a deactivation or an activation) to `username` and return
/// its status and body.
async fn toggle(
    router: &MultiServiceRouter,
    operation: &str,
    user_pool_id: &str,
    username: &str,
) -> (u16, Value) {
    call(
        router,
        operation,
        &json!({ "UserPoolId": user_pool_id, "Username": username }),
    )
    .await
}

/// The `Enabled` flag `AdminGetUser` reports for `username`.
async fn enabled(router: &MultiServiceRouter, user_pool_id: &str, username: &str) -> bool {
    let payload = ok(
        router,
        "AdminGetUser",
        &json!({ "UserPoolId": user_pool_id, "Username": username }),
    )
    .await;
    payload["Enabled"]
        .as_bool()
        .unwrap_or_else(|| panic!("AdminGetUser must report Enabled, got {payload}"))
}

/// One row of the toggle table: the calls applied in request order and the flag
/// a later read must then report.
struct ToggleCase {
    case: &'static str,
    calls: Vec<&'static str>,
    expected: bool,
}

/// The rows differ along the COUNT of calls, their ORDER, whether the same call
/// is REPEATED on a user already in that state, and which call is last.
fn toggle_cases() -> Vec<ToggleCase> {
    vec![
        ToggleCase {
            case: "no call at all: a created user is active",
            calls: vec![],
            expected: true,
        },
        ToggleCase {
            case: "one deactivation",
            calls: vec!["AdminDisableUser"],
            expected: false,
        },
        ToggleCase {
            case: "one activation of a user that was never deactivated",
            calls: vec!["AdminEnableUser"],
            expected: true,
        },
        ToggleCase {
            case: "the same deactivation three times",
            calls: vec![
                "AdminDisableUser",
                "AdminDisableUser",
                "AdminDisableUser",
            ],
            expected: false,
        },
        ToggleCase {
            case: "a deactivation then an activation",
            calls: vec!["AdminDisableUser", "AdminEnableUser"],
            expected: true,
        },
        ToggleCase {
            case: "an activation then a deactivation",
            calls: vec!["AdminEnableUser", "AdminDisableUser"],
            expected: false,
        },
        ToggleCase {
            case: "three alternating calls, ending deactivated",
            calls: vec!["AdminDisableUser", "AdminEnableUser", "AdminDisableUser"],
            expected: false,
        },
        ToggleCase {
            case: "four alternating calls, ending active",
            calls: vec![
                "AdminDisableUser",
                "AdminEnableUser",
                "AdminDisableUser",
                "AdminEnableUser",
            ],
            expected: true,
        },
    ]
}

#[tokio::test]
async fn the_last_call_decides_the_stored_enabled_flag() {
    let router = router();
    let user_pool_id = pool(&router, "toggle-pool").await;

    for (index, case) in toggle_cases().into_iter().enumerate() {
        let label = case.case;
        // One user per row, so each row starts from a freshly created user.
        let username = format!("user-{index}");
        user(&router, &user_pool_id, &username).await;

        for operation in &case.calls {
            let (status, payload) = toggle(&router, operation, &user_pool_id, &username).await;
            assert_eq!(
                status, 200,
                "case {label:?}: {operation} must succeed, got {status} with {payload}"
            );
            assert_eq!(
                payload,
                Value::Object(serde_json::Map::new()),
                "case {label:?}: {operation} answers an empty body, got {payload}"
            );
        }

        let actual = enabled(&router, &user_pool_id, &username).await;
        assert_eq!(
            actual, case.expected,
            "case {label:?}: expected Enabled {}, got {actual:?}",
            case.expected
        );

        // The flag is STORED, not an artefact of the first read: a second read
        // answers the same.
        let again = enabled(&router, &user_pool_id, &username).await;
        assert_eq!(
            again, actual,
            "case {label:?}: a second read answers the same stored flag, got {again:?}"
        );
    }
}

#[tokio::test]
async fn every_username_form_is_deactivated_and_reactivated() {
    let router = router();
    let user_pool_id = pool(&router, "username-form-pool").await;

    // Every username here satisfies `UsernameType` (length 1..128, pattern
    // `^[\p{L}\p{M}\p{S}\p{N}\p{P}]+$`) from
    // `smithy_query(query="operation",operation="AdminDisableUser")`.
    for (label, username) in [
        ("an ASCII username", "alice".to_string()),
        ("a username holding punctuation", "a.b-c_d+e".to_string()),
        ("a non-ASCII username", "ユーザー①".to_string()),
        (
            "a username at the modelled length maximum 128",
            "u".repeat(128),
        ),
        ("a username at the modelled length minimum 1", "u".to_string()),
    ] {
        user(&router, &user_pool_id, &username).await;

        let (status, payload) =
            toggle(&router, "AdminDisableUser", &user_pool_id, &username).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the deactivation must succeed, got {status} with {payload}"
        );
        let actual = enabled(&router, &user_pool_id, &username).await;
        assert!(
            !actual,
            "case {label:?}: expected Enabled false after the deactivation, got {actual:?}"
        );

        let (status, payload) = toggle(&router, "AdminEnableUser", &user_pool_id, &username).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the activation must succeed, got {status} with {payload}"
        );
        let actual = enabled(&router, &user_pool_id, &username).await;
        assert!(
            actual,
            "case {label:?}: expected Enabled true after the activation, got {actual:?}"
        );
    }
}

#[tokio::test]
async fn a_user_is_resolved_by_username_or_by_sub() {
    let router = router();
    let user_pool_id = pool(&router, "resolve-pool").await;
    let sub = user(&router, &user_pool_id, "carol").await;

    // `Username` "must be the sub of a local user or the username" — both name
    // the same stored user, so a call under either identifier lands on it.
    for (label, identifier, operation, expected) in [
        ("deactivated by username", "carol".to_string(), "AdminDisableUser", false),
        ("reactivated by sub", sub.clone(), "AdminEnableUser", true),
        ("deactivated by sub", sub, "AdminDisableUser", false),
        ("reactivated by username", "carol".to_string(), "AdminEnableUser", true),
    ] {
        let (status, payload) = toggle(&router, operation, &user_pool_id, &identifier).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the user must resolve, got {status} with {payload}"
        );
        let actual = enabled(&router, &user_pool_id, "carol").await;
        assert_eq!(
            actual, expected,
            "case {label:?}: expected Enabled {expected}, got {actual:?}"
        );
    }
}

#[tokio::test]
async fn a_deactivation_touches_one_user_of_one_pool() {
    let router = router();
    let first_pool = pool(&router, "first-pool").await;
    let second_pool = pool(&router, "second-pool").await;

    // The same usernames exist in both pools; only the first pool's `alice` is
    // deactivated.
    for user_pool_id in [&first_pool, &second_pool] {
        for username in ["alice", "bob"] {
            user(&router, user_pool_id, username).await;
        }
    }
    let (status, payload) = toggle(&router, "AdminDisableUser", &first_pool, "alice").await;
    assert_eq!(
        status, 200,
        "the deactivation must succeed, got {status} with {payload}"
    );

    for (label, user_pool_id, username, expected) in [
        (
            "the deactivated user of the pool it was deactivated in",
            &first_pool,
            "alice",
            false,
        ),
        ("another user of the same pool", &first_pool, "bob", true),
        (
            "the same username in another pool",
            &second_pool,
            "alice",
            true,
        ),
    ] {
        let actual = enabled(&router, user_pool_id, username).await;
        assert_eq!(
            actual, expected,
            "case {label:?}: expected Enabled {expected}, got {actual:?}"
        );
    }
}

#[tokio::test]
async fn an_absent_pool_or_user_is_refused_and_stores_nothing() {
    let router = router();
    let user_pool_id = pool(&router, "refusal-pool").await;
    user(&router, &user_pool_id, "dave").await;

    for operation in ["AdminDisableUser", "AdminEnableUser"] {
        for (label, body, expected) in [
            (
                "an absent user pool",
                json!({ "UserPoolId": "us-east-1_absent", "Username": "dave" }),
                (404, "ResourceNotFoundException"),
            ),
            (
                "a user the pool does not hold",
                json!({ "UserPoolId": user_pool_id, "Username": "absent-user" }),
                (404, "UserNotFoundException"),
            ),
        ] {
            let (status, payload) = call(&router, operation, &body).await;
            assert_eq!(
                (status, error_code(&payload)),
                expected,
                "case {label:?} on {operation}: expected {expected:?}, got {status} with {payload}"
            );
        }
    }

    // Nothing above was stored: the created user is still active.
    let actual = enabled(&router, &user_pool_id, "dave").await;
    assert!(
        actual,
        "a refused call stores nothing, so the user stays active, got {actual:?}"
    );
}

#[tokio::test]
async fn a_request_outside_a_modelled_constraint_is_refused() {
    let router = router();
    let user_pool_id = pool(&router, "constraint-pool").await;
    user(&router, &user_pool_id, "dave").await;

    // One value past each published bound of the input
    // (`smithy_query(query="operation",operation="AdminDisableUser")`):
    // `Username` length 1..128 and pattern `^[\p{L}\p{M}\p{S}\p{N}\p{P}]+$`, and
    // `UserPoolId` length 1..55 and pattern `^[\w-]+_[0-9a-zA-Z]+$`.
    for operation in ["AdminDisableUser", "AdminEnableUser"] {
        for (label, body) in [
            (
                "an empty username, below the length minimum 1",
                json!({ "UserPoolId": user_pool_id, "Username": "" }),
            ),
            (
                "a username of 129 characters, past the length maximum 128",
                json!({ "UserPoolId": user_pool_id, "Username": "u".repeat(129) }),
            ),
            (
                "a username holding a space, which the pattern excludes",
                json!({ "UserPoolId": user_pool_id, "Username": "two words" }),
            ),
            (
                "a user pool id the pattern excludes",
                json!({ "UserPoolId": "no-underscore", "Username": "dave" }),
            ),
            (
                "a user pool id of 56 characters, past the length maximum 55",
                json!({
                    "UserPoolId": format!("us-east-1_{}", "a".repeat(46)),
                    "Username": "dave",
                }),
            ),
            (
                "a request missing the required Username",
                json!({ "UserPoolId": user_pool_id }),
            ),
        ] {
            let (status, payload) = call(&router, operation, &body).await;
            assert_eq!(
                status, 400,
                "case {label:?} on {operation}: a request outside a modelled constraint is a 400, got {status} with {payload}"
            );
            assert_ne!(
                error_code(&payload),
                "NotImplemented",
                "case {label:?} on {operation}: a bad request is never answered with the scaffold's 501, got {payload}"
            );
        }
    }
}

#[tokio::test]
async fn a_deactivated_user_cannot_sign_in_and_its_tokens_are_revoked() {
    let router = router();
    let user_pool_id = pool(&router, "sign-in-pool").await;
    let client = ok(
        &router,
        "CreateUserPoolClient",
        &json!({
            "UserPoolId": user_pool_id,
            "ClientName": "sign-in-pool-client",
            "ExplicitAuthFlows": ["ALLOW_ADMIN_USER_PASSWORD_AUTH"],
        }),
    )
    .await;
    let client_id = client["UserPoolClient"]["ClientId"]
        .as_str()
        .expect("a minted client id")
        .to_string();
    user(&router, &user_pool_id, "erin").await;
    ok(
        &router,
        "AdminSetUserPassword",
        &json!({
            "UserPoolId": user_pool_id,
            "Username": "erin",
            "Password": GOOD_PASSWORD,
            "Permanent": true,
        }),
    )
    .await;

    // A token held before the deactivation, so the revocation can be observed on
    // an operation that identifies its caller by that token.
    let access_token = sign_in(&router, &user_pool_id, &client_id, "erin").await;
    let (status, payload) = call(
        &router,
        "ListDevices",
        &json!({ "AccessToken": access_token }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"before the deactivation\": the held token authorizes the request, got {status} with {payload}"
    );

    let (status, payload) = toggle(&router, "AdminDisableUser", &user_pool_id, "erin").await;
    assert_eq!(
        status, 200,
        "the deactivation must succeed, got {status} with {payload}"
    );

    for (label, operation, body) in [
        (
            "a sign-in of a deactivated user",
            "AdminInitiateAuth",
            json!({
                "UserPoolId": user_pool_id,
                "ClientId": client_id,
                "AuthFlow": "ADMIN_USER_PASSWORD_AUTH",
                "AuthParameters": { "USERNAME": "erin", "PASSWORD": GOOD_PASSWORD },
            }),
        ),
        (
            "a request carrying a token issued before the deactivation",
            "ListDevices",
            json!({ "AccessToken": access_token }),
        ),
    ] {
        let (status, payload) = call(&router, operation, &body).await;
        assert_eq!(
            (status, error_code(&payload)),
            (403, "NotAuthorizedException"),
            "case {label:?}: expected a 403 NotAuthorizedException, got {status} with {payload}"
        );
    }

    // The activation restores sign-in, and the token it issues is a fresh one
    // that authorizes again.
    let (status, payload) = toggle(&router, "AdminEnableUser", &user_pool_id, "erin").await;
    assert_eq!(
        status, 200,
        "the activation must succeed, got {status} with {payload}"
    );
    let reissued = sign_in(&router, &user_pool_id, &client_id, "erin").await;
    assert_ne!(
        reissued, access_token,
        "the reactivated user signs in with a freshly issued token"
    );
    let (status, payload) = call(&router, "ListDevices", &json!({ "AccessToken": reissued })).await;
    assert_eq!(
        status, 200,
        "case \"after the activation\": the reissued token authorizes the request, got {status} with {payload}"
    );

    // The token the deactivation revoked stays revoked: an activation activates
    // sign-in, it does not reinstate a credential already revoked.
    let (status, payload) = call(
        &router,
        "ListDevices",
        &json!({ "AccessToken": access_token }),
    )
    .await;
    assert_eq!(
        (status, error_code(&payload)),
        (403, "NotAuthorizedException"),
        "case \"a revoked token after the activation\": expected a 403 NotAuthorizedException, got {status} with {payload}"
    );
}

/// Sign `username` in and return the access token the sign-in issues.
async fn sign_in(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    client_id: &str,
    username: &str,
) -> String {
    let result = ok(
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
    result["AuthenticationResult"]["AccessToken"]
        .as_str()
        .unwrap_or_else(|| panic!("an access token must be issued, got {result}"))
        .to_string()
}
