// SPDX-License-Identifier: CC0-1.0

//! The self-service half of the user-pools API over the wire: the operations a user
//! calls with NO SigV4 credentials, because it has none until it has signed in. Driven
//! through the same `MultiServiceRouter` the production router builds, addressed the way
//! an SDK addresses them (`POST /` with
//! `X-Amz-Target: AWSCognitoIdentityProviderService.<Op>` and no credential scope).
//!
//! Two rules are under test.
//!
//! * `SignUp` registers a user in the pool THE APP CLIENT belongs to: the `ClientId`
//!   resolves the pool (the operation models no `UserPoolId` at all), the proposed
//!   password is checked against that pool's password policy and the attributes against
//!   its schema, and what is stored is an `UNCONFIRMED` user carrying a freshly minted
//!   `sub` — the `UserSub` the response reports and the one an `AdminGetUser` reads
//!   back.
//! * `GetUser` reports the profile of the user its `AccessToken` identifies, read from
//!   stored state: only an access token this emulator issued and has not expired names a
//!   user, and every other token is `NotAuthorizedException`.
//!
//! Each table row names the case it asserts.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice as cognito_idp;

/// This service's SigV4 signing name, the 4th field of a credential scope.
const SIGNING_NAME: &str = "cognito-idp";

/// The local name of this service's Smithy shape, which is the `X-Amz-Target` prefix a
/// client writes (`smithy_query(query="service_info")`:
/// `com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService`).
const TARGET_PREFIX: &str = "AWSCognitoIdentityProviderService";

/// The emulator's own endpoint, which is what an unsigned client is pointed at: an
/// address, so the `Host` header names no service and `X-Amz-Target` is the only routing
/// signal left.
const EMULATOR_HOST: &str = "127.0.0.1:9297";

/// This service's real regional endpoint host.
const ENDPOINT_HOST: &str = "cognito-idp.us-east-1.amazonaws.com";

/// A password satisfying a default pool's password policy.
const GOOD_PASSWORD: &str = "Passw0rd!2024";

/// A password that misses the default policy's minimum length.
const SHORT_PASSWORD: &str = "Ab1!";

/// Whether a request carries a SigV4 credential scope.
#[derive(Clone, Copy, Debug)]
enum Addressing {
    /// A signed call, as the SDK sends every credentialed operation of this service.
    Signed,
    /// An unsigned call to the emulator's own endpoint: how the self-service operations
    /// are called.
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

/// Create a pool with `extra` members beside its name, and return its minted id.
async fn create_pool(router: &MultiServiceRouter, pool_name: &str, extra: Value) -> String {
    let mut body = Map::new();
    body.insert("PoolName".to_string(), json!(pool_name));
    for (name, value) in extra.as_object().cloned().unwrap_or_default() {
        body.insert(name, value);
    }
    let created = ok(router, "CreateUserPool", &Value::Object(body)).await;
    created["UserPool"]["Id"]
        .as_str()
        .expect("a minted pool id")
        .to_string()
}

/// Create an app client in `user_pool_id` with `explicit_auth_flows`, and return its
/// minted id.
async fn create_client(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    client_name: &str,
    explicit_auth_flows: &[&str],
) -> String {
    let created = ok(
        router,
        "CreateUserPoolClient",
        &json!({
            "UserPoolId": user_pool_id,
            "ClientName": client_name,
            "ExplicitAuthFlows": explicit_auth_flows,
        }),
    )
    .await;
    created["UserPoolClient"]["ClientId"]
        .as_str()
        .expect("a minted client id")
        .to_string()
}

/// A `SignUp` request body: the members a self-service registration carries.
fn sign_up_body(
    client_id: &str,
    username: &str,
    password: Option<&str>,
    attributes: &[(&str, &str)],
) -> Value {
    let mut body = Map::new();
    body.insert("ClientId".to_string(), json!(client_id));
    body.insert("Username".to_string(), json!(username));
    if let Some(password) = password {
        body.insert("Password".to_string(), json!(password));
    }
    if !attributes.is_empty() {
        body.insert(
            "UserAttributes".to_string(),
            Value::Array(
                attributes
                    .iter()
                    .map(|(name, value)| json!({ "Name": name, "Value": value }))
                    .collect(),
            ),
        );
    }
    Value::Object(body)
}

/// RULE: an unsigned `SignUp` registers the user in the pool its app client belongs to,
/// and refuses exactly what that pool's policy and schema refuse — over a table whose
/// rows differ along the password, the attributes and the client the request names.
#[tokio::test]
async fn sign_up_registers_a_user_in_the_pool_of_the_app_client_it_names() {
    let engine = engine();
    let setup = router(&engine, false);
    let emulator = router(&engine, true);

    // A pool with the default schema, and one whose schema marks `email` required: the
    // same request is served by the first and refused by the second, so the refusal
    // comes from the POOL's configuration rather than from the request alone.
    let open_pool = create_pool(&setup, "self-service-open", json!({})).await;
    let open_client = create_client(&setup, &open_pool, "app", &[]).await;
    let strict_pool = create_pool(
        &setup,
        "self-service-strict",
        json!({
            "Schema": [
                {
                    "Name": "email",
                    "AttributeDataType": "String",
                    "Required": true,
                    "Mutable": true,
                },
            ],
        }),
    )
    .await;
    let strict_client = create_client(&setup, &strict_pool, "app", &[]).await;

    // A well-formed `ClientId` (modelled `^[\w+]+$`) no pool ever minted.
    let absent_client = "absentclient0000000000000".to_string();

    // (case, client id, username, password, attributes, expected status, expected code)
    type Row = (
        &'static str,
        String,
        &'static str,
        Option<&'static str>,
        Vec<(&'static str, &'static str)>,
        u16,
        Option<&'static str>,
    );
    let rows: Vec<Row> = vec![
        (
            "a username and a conforming password",
            open_client.clone(),
            "alice",
            Some(GOOD_PASSWORD),
            vec![],
            200,
            None,
        ),
        (
            "attributes beside the password",
            open_client.clone(),
            "bob",
            Some(GOOD_PASSWORD),
            vec![("email", "bob@example.com"), ("name", "Bob")],
            200,
            None,
        ),
        (
            "no password at all, which the member allows",
            open_client.clone(),
            "carol",
            None,
            vec![],
            200,
            None,
        ),
        (
            "a username already registered in this pool",
            open_client.clone(),
            "alice",
            Some(GOOD_PASSWORD),
            vec![],
            400,
            Some("UsernameExistsException"),
        ),
        (
            "the same username in ANOTHER pool, which is a different user",
            strict_client.clone(),
            "alice",
            Some(GOOD_PASSWORD),
            vec![("email", "alice@example.com")],
            200,
            None,
        ),
        (
            "a password the pool's policy refuses",
            open_client.clone(),
            "dave",
            Some(SHORT_PASSWORD),
            vec![],
            400,
            Some("InvalidPasswordException"),
        ),
        (
            "an attribute the pool's schema does not declare",
            open_client.clone(),
            "erin",
            Some(GOOD_PASSWORD),
            vec![("custom:unschemed", "x")],
            400,
            Some("InvalidParameterException"),
        ),
        (
            "an attribute the pool's schema marks required, left out",
            strict_client.clone(),
            "frank",
            Some(GOOD_PASSWORD),
            vec![],
            400,
            Some("InvalidParameterException"),
        ),
        (
            "an app client no pool holds",
            absent_client,
            "grace",
            Some(GOOD_PASSWORD),
            vec![],
            404,
            Some("ResourceNotFoundException"),
        ),
    ];

    for (case, client_id, username, password, attributes, expected_status, expected_code) in &rows {
        let (status, payload) = dispatch(
            &emulator,
            Addressing::Anonymous,
            "SignUp",
            &sign_up_body(client_id, username, *password, attributes),
        )
        .await;
        assert_eq!(
            status, *expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with {payload}"
        );
        match expected_code {
            Some(code) => assert_eq!(
                error_code(&payload),
                *code,
                "case {case:?}: expected error code {code}, got {payload}"
            ),
            None => {
                // A registered user is UNCONFIRMED until something confirms it, and the
                // response carries the id the pool minted for it.
                assert_eq!(
                    payload["UserConfirmed"],
                    json!(false),
                    "case {case:?}: a registration nothing auto-confirmed reports \
                     UserConfirmed false, got {payload}"
                );
                let user_sub = payload["UserSub"].as_str().unwrap_or_default();
                assert!(
                    !user_sub.is_empty(),
                    "case {case:?}: expected a minted UserSub, got {payload}"
                );
            }
        }
    }

    // ROUND TRIP: the user the first row registered is READ BACK through an implemented
    // reader, in the pool its app client belongs to and with the attributes submitted.
    let read_back = ok(
        &setup,
        "AdminGetUser",
        &json!({ "UserPoolId": open_pool, "Username": "bob" }),
    )
    .await;
    assert_eq!(
        read_back["Username"], json!("bob"),
        "AdminGetUser must report the registered username, got {read_back}"
    );
    assert_eq!(
        read_back["UserStatus"], json!("UNCONFIRMED"),
        "a registered user is UNCONFIRMED, got {read_back}"
    );
    let attributes: Vec<(String, String)> = read_back["UserAttributes"]
        .as_array()
        .expect("AdminGetUser reports the stored attributes")
        .iter()
        .map(|attribute| {
            (
                attribute["Name"].as_str().unwrap_or_default().to_string(),
                attribute["Value"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    assert!(
        attributes.contains(&("email".to_string(), "bob@example.com".to_string())),
        "the stored user carries the submitted email, got {attributes:?}"
    );
    assert!(
        attributes
            .iter()
            .any(|(name, value)| name == "sub" && !value.is_empty()),
        "the stored user carries the sub the pool minted, got {attributes:?}"
    );

    // The same username in the other pool is a DIFFERENT user: each pool holds its own,
    // with its own minted `sub`.
    let other = ok(
        &setup,
        "AdminGetUser",
        &json!({ "UserPoolId": strict_pool, "Username": "alice" }),
    )
    .await;
    let one = ok(
        &setup,
        "AdminGetUser",
        &json!({ "UserPoolId": open_pool, "Username": "alice" }),
    )
    .await;
    let sub_of = |user: &Value| -> String {
        user["UserAttributes"]
            .as_array()
            .expect("the stored attributes")
            .iter()
            .find(|attribute| attribute["Name"] == json!("sub"))
            .and_then(|attribute| attribute["Value"].as_str())
            .expect("a stored sub")
            .to_string()
    };
    assert_ne!(
        sub_of(&one),
        sub_of(&other),
        "one username registered in two pools is two users, each with its own sub"
    );
}

/// RULE: `GetUser` answers the profile of the user its access token identifies, and
/// nothing else names a user — over a table spanning the token kinds a sign-in hands
/// out, a token this emulator never issued, and two users whose tokens must not cross.
#[tokio::test]
async fn get_user_answers_the_profile_the_access_token_identifies() {
    let engine = engine();
    let setup = router(&engine, false);
    let emulator = router(&engine, true);

    let user_pool_id = create_pool(&setup, "profile-reads", json!({})).await;
    let client_id = create_client(
        &setup,
        &user_pool_id,
        "admin-app",
        &["ALLOW_ADMIN_USER_PASSWORD_AUTH"],
    )
    .await;

    // Two users, so a token of one cannot answer for the other.
    let mut tokens = Vec::new();
    for (username, email) in [("alice", "alice@example.com"), ("bob", "bob@example.com")] {
        ok(
            &setup,
            "AdminCreateUser",
            &json!({
                "UserPoolId": user_pool_id,
                "Username": username,
                "UserAttributes": [{ "Name": "email", "Value": email }],
            }),
        )
        .await;
        ok(
            &setup,
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
            &setup,
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
        tokens.push((
            username,
            email,
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
        ));
    }

    // Each user's own access token answers with that user's own stored profile.
    for (username, email, access_token, _, _) in &tokens {
        let (status, payload) = dispatch(
            &emulator,
            Addressing::Anonymous,
            "GetUser",
            &json!({ "AccessToken": access_token }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {username:?}: an access token must name its user, got {status} with {payload}"
        );
        assert_eq!(
            payload["Username"], json!(username),
            "case {username:?}: expected the token's own user, got {payload}"
        );
        let attributes: Vec<(String, String)> = payload["UserAttributes"]
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
            attributes.contains(&("email".to_string(), (*email).to_string())),
            "case {username:?}: expected the stored email {email}, got {attributes:?}"
        );
    }

    // (case, token, expected status, expected code)
    let (username, _, access_token, id_token, refresh_token) = &tokens[0];
    let rows: Vec<(&str, String, u16, Option<&str>)> = vec![
        ("the user's own access token", access_token.clone(), 200, None),
        (
            "the id token of the same sign-in",
            id_token.clone(),
            403,
            Some("NotAuthorizedException"),
        ),
        (
            "the refresh token of the same sign-in",
            refresh_token.clone(),
            403,
            Some("NotAuthorizedException"),
        ),
        (
            "a token this emulator never issued",
            "not.a.token".to_string(),
            403,
            Some("NotAuthorizedException"),
        ),
        (
            "an access token with its last character changed",
            {
                let mut tampered = access_token.clone();
                let last = tampered.pop().expect("a non-empty token");
                tampered.push(if last == 'A' { 'B' } else { 'A' });
                tampered
            },
            403,
            Some("NotAuthorizedException"),
        ),
    ];

    for (case, token, expected_status, expected_code) in &rows {
        let (status, payload) = dispatch(
            &emulator,
            Addressing::Anonymous,
            "GetUser",
            &json!({ "AccessToken": token }),
        )
        .await;
        assert_eq!(
            status, *expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with {payload}"
        );
        match expected_code {
            Some(code) => assert_eq!(
                error_code(&payload),
                *code,
                "case {case:?}: expected error code {code}, got {payload}"
            ),
            None => assert_eq!(
                payload["Username"], json!(username),
                "case {case:?}: expected the token's own user, got {payload}"
            ),
        }
    }
}
