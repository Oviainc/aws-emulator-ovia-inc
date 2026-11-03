// SPDX-License-Identifier: CC0-1.0

//! `InitiateAuth` over the wire: the sign-in entry point a client calls with NO SigV4
//! credentials (the model marks the operation `@optionalAuth`), driven through the
//! router exactly as an SDK drives it (`POST /` with
//! `X-Amz-Target: AWSCognitoIdentityProviderService.<Op>`).
//!
//! Two rules are under test.
//!
//! * ROUTING. The framework resolves a request's routing key from the SigV4 credential
//!   scope first and from `X-Amz-Target` only when there is no scope, and those two
//!   carry different names — the signing name (`cognito-idp`) and the model's service
//!   shape name (`AWSCognitoIdentityProviderService`). An unsigned request that names
//!   this service's shape must therefore reach this service, and one that names another
//!   service's must not. The suite addresses its unsigned calls to a host that is an
//!   ADDRESS rather than a service name, so the target prefix is the only signal left,
//!   and runs one of them through an AUTHENTICATING router, which is how the emulator
//!   serves traffic for real.
//! * THE OPERATION. Every `AuthFlowType` member reaches the operation and gets the
//!   answer its own rule owes: the two username-password and refresh-token flows sign
//!   in from STORED state, the two admin flows are refused as not valid for
//!   `InitiateAuth`, and the flows that would need the SRP protocol or a Lambda trigger
//!   answer 501 rather than a sign-in nothing performed.
//!
//! Each table row names the case it asserts.

use bytes::Bytes;
use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice as cognito_idp;
use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice::state;

/// This service's SigV4 signing name, the 4th field of a credential scope.
const SIGNING_NAME: &str = "cognito-idp";

/// The local name of this service's Smithy shape, which is the `X-Amz-Target` prefix a
/// client writes (`smithy_query(query="service_info")`:
/// `com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService`).
const TARGET_PREFIX: &str = "AWSCognitoIdentityProviderService";

/// The shape name of the SEPARATE Amazon Cognito Identity service
/// (`com.amazonaws.cognitoidentity#AWSCognitoIdentityService`): a target this service
/// must NOT claim.
const ANOTHER_TARGET_PREFIX: &str = "AWSCognitoIdentityService";

/// The emulator's own endpoint, which is what an unsigned client is pointed at: an
/// address, so the `Host` header names no service and cannot route the request.
const EMULATOR_HOST: &str = "127.0.0.1:9297";

/// This service's real regional endpoint host.
const ENDPOINT_HOST: &str = "cognito-idp.us-east-1.amazonaws.com";

/// A password satisfying a default pool's password policy.
const GOOD_PASSWORD: &str = "Passw0rd!2024";

/// A second, different password satisfying the same policy.
const TEMPORARY_PASSWORD: &str = "Temp0rary!2024";

/// The shared engine every router in one test mounts, so state written through one is
/// read through the other.
fn engine() -> Arc<engine::InMemoryEngine> {
    Arc::new(engine::InMemoryEngine::new())
}

/// One emulator with just this service mounted over `engine`.
///
/// `authenticating` turns request authentication ON, as the emulator runs it: a signed
/// request would then need a real signature, while an operation whose model lists no
/// auth scheme is served unsigned. The setup calls use a non-authenticating router, so
/// they do not have to carry real signatures.
fn router(engine: &Arc<engine::InMemoryEngine>, authenticating: bool) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.register_service(cognito_idp::EmulatedService::new(Arc::clone(engine)));
    router.set_request_authentication(authenticating);
    router
}

/// How a request is addressed — the three signals the claim rule reads.
#[derive(Clone, Copy, Debug)]
struct Addressing {
    /// The service field of the SigV4 credential scope, or `None` for an unsigned
    /// (anonymous) request, which is how `InitiateAuth` is called.
    scope: Option<&'static str>,
    /// The `X-Amz-Target` service prefix, or `None` to send no target at all.
    target_prefix: Option<&'static str>,
    host: &'static str,
}

/// A signed call, as the SDK sends every credentialed operation of this service.
const SIGNED: Addressing = Addressing {
    scope: Some(SIGNING_NAME),
    target_prefix: Some(TARGET_PREFIX),
    host: ENDPOINT_HOST,
};

/// An unsigned call to the emulator's own endpoint: no credential scope, and a host
/// that names no service, so `X-Amz-Target` is the only routing signal.
const ANONYMOUS: Addressing = Addressing {
    scope: None,
    target_prefix: Some(TARGET_PREFIX),
    host: EMULATOR_HOST,
};

/// Dispatch `operation` with `body`, addressed as `addressing` says, and return the
/// status and JSON response body.
async fn dispatch(
    router: &MultiServiceRouter,
    addressing: Addressing,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let mut request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", addressing.host)
        .header("content-type", "application/x-amz-json-1.1");
    if let Some(prefix) = addressing.target_prefix {
        request = request.header("x-amz-target", format!("{prefix}.{operation}"));
    }
    if let Some(scope) = addressing.scope {
        request = request.header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-east-1/{scope}/aws4_request, \
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

/// Call `operation` signed, requiring success, and return its response body.
async fn ok(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, payload) = dispatch(router, SIGNED, operation, body).await;
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

/// What a call must answer: the HTTP status, and the wire error code — `None` where the
/// call must SUCCEED. One type for every table below, so each row states its answer the
/// same way.
#[derive(Clone, Copy, Debug)]
struct Answer {
    status: u16,
    code: Option<&'static str>,
}

/// The answer of a call that signs in (or otherwise answers 200).
const SERVED: Answer = Answer {
    status: 200,
    code: None,
};

/// The answer of a call refused with `status` and the wire error code `code`.
const fn refused(status: u16, code: &'static str) -> Answer {
    Answer {
        status,
        code: Some(code),
    }
}

/// Assert that the call described by `case` answered `expected`.
///
/// The error code is read only where one is expected, so a successful row is compared on
/// its status alone; the message always carries the case and what the call actually
/// answered.
fn assert_answer(case: &str, expected: Answer, status: u16, payload: &Value) {
    let actual_code = expected.code.map(|_| error_code(payload).to_string());
    assert_eq!(
        (status, actual_code.as_deref()),
        (expected.status, expected.code),
        "case {case:?}: expected ({}, {:?}), got ({status}, {:?}) with {payload}",
        expected.status,
        expected.code,
        error_code(payload)
    );
}

/// A created pool and one of its app clients.
struct Fixture {
    user_pool_id: String,
    client_id: String,
}

/// Create a pool named `pool_name` and return its minted id.
async fn create_pool(router: &MultiServiceRouter, pool_name: &str) -> String {
    let created = ok(router, "CreateUserPool", &json!({ "PoolName": pool_name })).await;
    created["UserPool"]["Id"]
        .as_str()
        .expect("a minted pool id")
        .to_string()
}

/// Create an app client in `user_pool_id` with `explicit_auth_flows`, and the
/// `PreventUserExistenceErrors` mode `prevent` when one is given; return its minted id.
async fn create_client(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    client_name: &str,
    explicit_auth_flows: &[&str],
    prevent: Option<&str>,
) -> String {
    let mut body = Map::new();
    body.insert("UserPoolId".to_string(), json!(user_pool_id));
    body.insert("ClientName".to_string(), json!(client_name));
    body.insert("ExplicitAuthFlows".to_string(), json!(explicit_auth_flows));
    if let Some(prevent) = prevent {
        body.insert("PreventUserExistenceErrors".to_string(), json!(prevent));
    }
    let created = ok(router, "CreateUserPoolClient", &Value::Object(body)).await;
    created["UserPoolClient"]["ClientId"]
        .as_str()
        .expect("a minted client id")
        .to_string()
}

/// A pool with one app client that allows the two flows this operation serves.
async fn pool_with_sign_in_client(router: &MultiServiceRouter, name: &str) -> Fixture {
    let user_pool_id = create_pool(router, name).await;
    let client_id = create_client(
        router,
        &user_pool_id,
        "sign-in-client",
        &["ALLOW_USER_PASSWORD_AUTH", "ALLOW_REFRESH_TOKEN_AUTH"],
        None,
    )
    .await;
    Fixture {
        user_pool_id,
        client_id,
    }
}

/// Create `username` in `user_pool_id` and give it a PERMANENT password, so the user is
/// `CONFIRMED` and can sign in.
async fn create_confirmed_user(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    username: &str,
    password: &str,
) {
    ok(
        router,
        "AdminCreateUser",
        &json!({ "UserPoolId": user_pool_id, "Username": username }),
    )
    .await;
    ok(
        router,
        "AdminSetUserPassword",
        &json!({
            "UserPoolId": user_pool_id,
            "Username": username,
            "Password": password,
            "Permanent": true,
        }),
    )
    .await;
}

/// An `InitiateAuth` request body.
fn initiate_auth(client_id: &str, auth_flow: &str, auth_parameters: Value) -> Value {
    json!({
        "ClientId": client_id,
        "AuthFlow": auth_flow,
        "AuthParameters": auth_parameters,
    })
}

/// Sign `username` in with `USER_PASSWORD_AUTH` through an unsigned call, requiring
/// success, and return the `AuthenticationResult`.
async fn sign_in(
    router: &MultiServiceRouter,
    client_id: &str,
    username: &str,
    password: &str,
) -> Value {
    let (status, payload) = dispatch(
        router,
        ANONYMOUS,
        "InitiateAuth",
        &initiate_auth(
            client_id,
            "USER_PASSWORD_AUTH",
            json!({ "USERNAME": username, "PASSWORD": password }),
        ),
    )
    .await;
    assert_eq!(
        status, 200,
        "signing {username} in must succeed, got {status} with {payload}"
    );
    payload["AuthenticationResult"].clone()
}

/// An unsigned `InitiateAuth` reaches this service and signs the stored user in, with
/// request authentication ON — the way the emulator serves traffic. The caller holds no
/// credentials at this point in the flow, so nothing but `X-Amz-Target` says which
/// service the request is for.
#[tokio::test]
async fn an_unsigned_initiate_auth_signs_in_against_the_emulated_user_pool() {
    let engine = engine();
    let setup = router(&engine, false);
    let fixture = pool_with_sign_in_client(&setup, "unsigned-pool").await;
    create_confirmed_user(&setup, &fixture.user_pool_id, "alice", GOOD_PASSWORD).await;

    let serving = router(&engine, true);
    let (status, payload) = dispatch(
        &serving,
        ANONYMOUS,
        "InitiateAuth",
        &initiate_auth(
            &fixture.client_id,
            "USER_PASSWORD_AUTH",
            json!({ "USERNAME": "alice", "PASSWORD": GOOD_PASSWORD }),
        ),
    )
    .await;

    assert_eq!(
        status, 200,
        "an unsigned InitiateAuth must be served, got {status} with code {:?}: {payload}",
        error_code(&payload)
    );
    let result = &payload["AuthenticationResult"];
    for member in ["AccessToken", "IdToken", "RefreshToken"] {
        assert!(
            result[member].as_str().is_some_and(|t| !t.is_empty()),
            "case {member:?}: a completed sign-in returns it, got {payload}"
        );
    }
    assert_eq!(
        result["TokenType"], "Bearer",
        "the token type is Bearer, got {payload}"
    );
    assert!(
        result["ExpiresIn"].as_i64().unwrap_or_default() > 0,
        "ExpiresIn is the access token's lifetime in seconds, got {payload}"
    );
    assert!(
        payload.get("ChallengeName").is_none(),
        "a completed sign-in carries no challenge, got {payload}"
    );

    // Every token the sign-in returned is recorded against the STORED user, so a later
    // operation resolves it back to them.
    let service_state = state::service_state(&*engine).expect("this service's state");
    let guard = service_state.lock();
    for member in ["AccessToken", "IdToken", "RefreshToken"] {
        let token = result[member].as_str().expect("the token");
        let record = guard
            .token(token)
            .unwrap_or_else(|| panic!("case {member:?}: the token must be recorded, got None"));
        assert_eq!(
            (record.username.as_str(), record.client_id.as_str()),
            ("alice", fixture.client_id.as_str()),
            "case {member:?}: the record must name the stored user and app client, \
             got {record:?}"
        );
    }
}

/// Which requests this service claims: the key resolves from the credential scope when
/// there is one and from the target prefix otherwise, so an unsigned request naming this
/// service's shape reaches it, and one naming another service — or signed for another
/// service — does not.
#[tokio::test]
async fn an_unsigned_request_is_claimed_only_when_its_target_names_this_service() {
    let engine = engine();
    let setup = router(&engine, false);
    let fixture = pool_with_sign_in_client(&setup, "routing-pool").await;
    create_confirmed_user(&setup, &fixture.user_pool_id, "alice", GOOD_PASSWORD).await;
    let serving = router(&engine, false);

    /// One row: how the request is addressed, and the answer that addressing owes.
    struct Row {
        case: &'static str,
        addressing: Addressing,
        expected: Answer,
    }

    let rows = [
        Row {
            case: "unsigned, its own target, on the emulator's own address",
            addressing: ANONYMOUS,
            expected: SERVED,
        },
        Row {
            case: "unsigned, its own target, on its endpoint host",
            addressing: Addressing {
                scope: None,
                target_prefix: Some(TARGET_PREFIX),
                host: ENDPOINT_HOST,
            },
            expected: SERVED,
        },
        Row {
            case: "unsigned, no target, on its endpoint host: the host still names it",
            addressing: Addressing {
                scope: None,
                target_prefix: None,
                host: ENDPOINT_HOST,
            },
            // Claimed by this service, whose awsJson protocol then refuses a request
            // that names no operation — a refusal of THIS service, never the router's
            // `UnknownService`.
            expected: refused(400, "SerializationException"),
        },
        Row {
            case: "signed for it, its own target",
            addressing: SIGNED,
            expected: SERVED,
        },
        Row {
            case: "unsigned, another service's target",
            addressing: Addressing {
                scope: None,
                target_prefix: Some(ANOTHER_TARGET_PREFIX),
                host: EMULATOR_HOST,
            },
            expected: refused(404, "UnknownService"),
        },
        Row {
            case: "signed for another service, its own target",
            addressing: Addressing {
                scope: Some("lambda"),
                target_prefix: Some(TARGET_PREFIX),
                host: EMULATOR_HOST,
            },
            expected: refused(404, "UnknownService"),
        },
    ];

    for row in rows {
        let (status, payload) = dispatch(
            &serving,
            row.addressing,
            "InitiateAuth",
            &initiate_auth(
                &fixture.client_id,
                "USER_PASSWORD_AUTH",
                json!({ "USERNAME": "alice", "PASSWORD": GOOD_PASSWORD }),
            ),
        )
        .await;
        assert_answer(row.case, row.expected, status, &payload);
    }
}

/// Every `AuthFlowType` member reaches the operation unsigned and gets the answer its
/// own rule owes. The rows span the whole enumeration
/// (`smithy_query(query="shape",shape="AuthFlowType")`): the flows this emulator signs
/// in, the two the model says are not valid for `InitiateAuth`, and the three that need
/// machinery nothing here runs.
#[tokio::test]
async fn every_modeled_auth_flow_reaches_the_operation_unsigned() {
    let engine = engine();
    let setup = router(&engine, false);
    let fixture = pool_with_sign_in_client(&setup, "flow-pool").await;
    create_confirmed_user(&setup, &fixture.user_pool_id, "alice", GOOD_PASSWORD).await;
    let serving = router(&engine, false);
    let refresh_token = sign_in(&serving, &fixture.client_id, "alice", GOOD_PASSWORD)
        .await["RefreshToken"]
        .as_str()
        .expect("the sign-in returns a refresh token")
        .to_string();

    let password_parameters = json!({ "USERNAME": "alice", "PASSWORD": GOOD_PASSWORD });
    let refresh_parameters = json!({ "REFRESH_TOKEN": refresh_token });
    /// One row: the `AuthFlow` the request names, the `AuthParameters` that flow takes,
    /// and the answer the flow owes.
    struct Row<'a> {
        case: &'static str,
        auth_flow: &'static str,
        auth_parameters: &'a Value,
        expected: Answer,
    }

    let rows = [
        Row {
            case: "the client-side username-password flow",
            auth_flow: "USER_PASSWORD_AUTH",
            auth_parameters: &password_parameters,
            expected: SERVED,
        },
        Row {
            case: "the refresh-token flow",
            auth_flow: "REFRESH_TOKEN_AUTH",
            auth_parameters: &refresh_parameters,
            expected: SERVED,
        },
        Row {
            case: "the alias the same enum carries for it",
            auth_flow: "REFRESH_TOKEN",
            auth_parameters: &refresh_parameters,
            expected: SERVED,
        },
        Row {
            case: "ADMIN_USER_PASSWORD_AUTH, a flow type of AdminInitiateAuth",
            auth_flow: "ADMIN_USER_PASSWORD_AUTH",
            auth_parameters: &password_parameters,
            expected: refused(400, "InvalidParameterException"),
        },
        Row {
            case: "ADMIN_NO_SRP_AUTH, the legacy server-side flow",
            auth_flow: "ADMIN_NO_SRP_AUTH",
            auth_parameters: &password_parameters,
            expected: refused(400, "InvalidParameterException"),
        },
        Row {
            case: "USER_SRP_AUTH, which needs the SRP protocol",
            auth_flow: "USER_SRP_AUTH",
            auth_parameters: &password_parameters,
            expected: refused(501, "NotImplemented"),
        },
        Row {
            case: "CUSTOM_AUTH, which needs Lambda triggers",
            auth_flow: "CUSTOM_AUTH",
            auth_parameters: &password_parameters,
            expected: refused(501, "NotImplemented"),
        },
        Row {
            case: "USER_AUTH, choice-based sign-in",
            auth_flow: "USER_AUTH",
            auth_parameters: &password_parameters,
            expected: refused(501, "NotImplemented"),
        },
    ];

    for row in rows {
        let (status, payload) = dispatch(
            &serving,
            ANONYMOUS,
            "InitiateAuth",
            &initiate_auth(
                &fixture.client_id,
                row.auth_flow,
                row.auth_parameters.clone(),
            ),
        )
        .await;
        assert_ne!(
            error_code(&payload),
            "UnknownService",
            "case {:?}: the request must reach this service, got {payload}",
            row.case
        );
        assert_answer(row.case, row.expected, status, &payload);
        if row.expected.code.is_none() {
            assert!(
                payload["AuthenticationResult"]["AccessToken"]
                    .as_str()
                    .is_some_and(|token| !token.is_empty()),
                "case {:?}: a served flow returns an access token, got {payload}",
                row.case
            );
        }
    }
}

/// `USER_PASSWORD_AUTH` answers from the STORED app client and the STORED user: the
/// password it holds, whether the client enables the flow, and whether the client hides
/// user existence.
#[tokio::test]
async fn user_password_auth_answers_from_the_stored_client_and_user() {
    let engine = engine();
    let setup = router(&engine, false);
    let user_pool_id = create_pool(&setup, "state-pool").await;
    let enabled_client = create_client(
        &setup,
        &user_pool_id,
        "password-client",
        &["ALLOW_USER_PASSWORD_AUTH"],
        None,
    )
    .await;
    // The same flow on a client that does not enable it, and on one that suppresses
    // "no such user".
    let admin_only_client = create_client(
        &setup,
        &user_pool_id,
        "admin-only-client",
        &["ALLOW_ADMIN_USER_PASSWORD_AUTH"],
        None,
    )
    .await;
    let discreet_client = create_client(
        &setup,
        &user_pool_id,
        "discreet-client",
        &["ALLOW_USER_PASSWORD_AUTH"],
        Some("ENABLED"),
    )
    .await;
    create_confirmed_user(&setup, &user_pool_id, "alice", GOOD_PASSWORD).await;
    let serving = router(&engine, false);

    /// One row: the app client and user the request names, the password it presents
    /// (`None` sends no `PASSWORD` at all), and the answer the stored state owes.
    struct Row<'a> {
        case: &'static str,
        client_id: &'a str,
        username: &'static str,
        password: Option<&'static str>,
        expected: Answer,
    }

    let rows = [
        Row {
            case: "the stored password",
            client_id: &enabled_client,
            username: "alice",
            password: Some(GOOD_PASSWORD),
            expected: SERVED,
        },
        Row {
            case: "a password the user does not have",
            client_id: &enabled_client,
            username: "alice",
            password: Some("Wr0ngPassword!"),
            expected: refused(400, "NotAuthorizedException"),
        },
        Row {
            case: "the stored password, in a different case: the stored one is exact",
            client_id: &enabled_client,
            username: "alice",
            password: Some("passw0rd!2024"),
            expected: refused(400, "NotAuthorizedException"),
        },
        Row {
            case: "no PASSWORD parameter for a flow that requires one",
            client_id: &enabled_client,
            username: "alice",
            password: None,
            expected: refused(400, "InvalidParameterException"),
        },
        Row {
            case: "a user the pool does not hold",
            client_id: &enabled_client,
            username: "nobody",
            password: Some(GOOD_PASSWORD),
            expected: refused(400, "UserNotFoundException"),
        },
        Row {
            case: "a user the pool does not hold, on a client that hides user existence",
            client_id: &discreet_client,
            username: "nobody",
            password: Some(GOOD_PASSWORD),
            expected: refused(400, "NotAuthorizedException"),
        },
        Row {
            case: "a client that does not enable this flow",
            client_id: &admin_only_client,
            username: "alice",
            password: Some(GOOD_PASSWORD),
            expected: refused(400, "InvalidParameterException"),
        },
    ];

    for row in rows {
        let mut parameters = Map::new();
        parameters.insert("USERNAME".to_string(), json!(row.username));
        if let Some(password) = row.password {
            parameters.insert("PASSWORD".to_string(), json!(password));
        }
        let (status, payload) = dispatch(
            &serving,
            ANONYMOUS,
            "InitiateAuth",
            &initiate_auth(
                row.client_id,
                "USER_PASSWORD_AUTH",
                Value::Object(parameters),
            ),
        )
        .await;
        assert_answer(row.case, row.expected, status, &payload);
    }

    // A `ClientId` no pool holds: the app client is what names the pool here, so an
    // unknown one is a refusal and not a sign-in.
    let (status, payload) = dispatch(
        &serving,
        ANONYMOUS,
        "InitiateAuth",
        &initiate_auth(
            "nosuchclient",
            "USER_PASSWORD_AUTH",
            json!({ "USERNAME": "alice", "PASSWORD": GOOD_PASSWORD }),
        ),
    )
    .await;
    assert_eq!(
        (status, error_code(&payload)),
        (400, "ResourceNotFoundException"),
        "an unknown ClientId must be a 400 ResourceNotFoundException, got {payload}"
    );
}

/// The refusals a stored user's OWN state owes a password sign-in, over the states the
/// API cannot yet put a user into — `AdminDisableUser`, `AdminResetUserPassword` and
/// `SignUp` are still scaffolded — so each row sets that state directly in the store and
/// then signs in over the wire with the RIGHT password: what is asserted is that the
/// stored state, not the credential, decides the answer.
#[tokio::test]
async fn the_stored_user_state_decides_the_refusal() {
    use std::time::{Duration, SystemTime};

    let engine = engine();
    let setup = router(&engine, false);
    let fixture = pool_with_sign_in_client(&setup, "user-state-pool").await;
    let serving = router(&engine, false);

    /// One row: the user it signs in as, what it makes true of that STORED user, the
    /// password the request presents, and the answer that state owes.
    struct Row {
        case: &'static str,
        username: &'static str,
        store: fn(&mut state::User),
        password: &'static str,
        expected: Answer,
    }

    /// `PasswordPolicyType.TemporaryPasswordValidityDays` defaults to 7 days, so a
    /// temporary password set 8 days ago is past it.
    const BEYOND_TEMPORARY_PASSWORD_VALIDITY: Duration = Duration::from_secs(8 * 24 * 60 * 60);

    let rows = [
        Row {
            case: "a confirmed, enabled user: the control row",
            username: "untouched",
            store: |_user| {},
            password: GOOD_PASSWORD,
            expected: SERVED,
        },
        Row {
            case: "a disabled user",
            username: "disabled",
            store: |user| user.enabled = false,
            password: GOOD_PASSWORD,
            expected: refused(400, "NotAuthorizedException"),
        },
        Row {
            case: "a user Amazon Cognito put in RESET_REQUIRED",
            username: "resetrequired",
            store: |user| user.status = state::UserStatus::ResetRequired,
            password: GOOD_PASSWORD,
            expected: refused(400, "PasswordResetRequiredException"),
        },
        Row {
            case: "a user that never confirmed its sign-up",
            username: "unconfirmed",
            store: |user| user.status = state::UserStatus::Unconfirmed,
            password: GOOD_PASSWORD,
            expected: refused(400, "UserNotConfirmedException"),
        },
        Row {
            case: "a user whose temporary password has expired",
            username: "expiredtemporary",
            store: |user| {
                user.password_permanent = false;
                user.password_set_at =
                    Some(SystemTime::now() - BEYOND_TEMPORARY_PASSWORD_VALIDITY);
            },
            password: GOOD_PASSWORD,
            expected: refused(400, "NotAuthorizedException"),
        },
        // The stored user's own state is read BEFORE the credential, so a caller that
        // does not hold the password gets the refusal that state owes rather than the
        // wrong-password one: the error code therefore says nothing about whether the
        // password presented was right.
        Row {
            case: "the wrong password for an unconfirmed user",
            username: "unconfirmedwrongpassword",
            store: |user| user.status = state::UserStatus::Unconfirmed,
            password: "NotTheP4ssword!",
            expected: refused(400, "UserNotConfirmedException"),
        },
        Row {
            case: "the wrong password for a user in RESET_REQUIRED",
            username: "resetrequiredwrongpassword",
            store: |user| user.status = state::UserStatus::ResetRequired,
            password: "NotTheP4ssword!",
            expected: refused(400, "PasswordResetRequiredException"),
        },
        Row {
            case: "the wrong password for a disabled user",
            username: "disabledwrongpassword",
            store: |user| user.enabled = false,
            password: "NotTheP4ssword!",
            expected: refused(400, "NotAuthorizedException"),
        },
    ];

    for row in rows {
        create_confirmed_user(&setup, &fixture.user_pool_id, row.username, GOOD_PASSWORD).await;
        {
            let service_state = state::service_state(&*engine).expect("this service's state");
            let mut guard = service_state.lock();
            let pool = guard
                .pool_mut(&fixture.user_pool_id)
                .expect("the stored pool");
            let user = pool.user_mut(row.username).expect("the stored user");
            (row.store)(user);
        }
        let (status, payload) = dispatch(
            &serving,
            ANONYMOUS,
            "InitiateAuth",
            &initiate_auth(
                &fixture.client_id,
                "USER_PASSWORD_AUTH",
                json!({ "USERNAME": row.username, "PASSWORD": row.password }),
            ),
        )
        .await;
        assert_answer(row.case, row.expected, status, &payload);
    }
}

/// A user that still has to change its password gets the `NEW_PASSWORD_REQUIRED`
/// challenge and a `Session` this emulator STORED, not tokens.
#[tokio::test]
async fn a_user_that_must_change_its_password_gets_the_challenge_and_a_stored_session() {
    let engine = engine();
    let setup = router(&engine, false);
    let fixture = pool_with_sign_in_client(&setup, "challenge-pool").await;
    ok(
        &setup,
        "AdminCreateUser",
        &json!({
            "UserPoolId": fixture.user_pool_id,
            "Username": "carol",
            "TemporaryPassword": TEMPORARY_PASSWORD,
        }),
    )
    .await;
    let serving = router(&engine, false);

    let (status, payload) = dispatch(
        &serving,
        ANONYMOUS,
        "InitiateAuth",
        &initiate_auth(
            &fixture.client_id,
            "USER_PASSWORD_AUTH",
            json!({ "USERNAME": "carol", "PASSWORD": TEMPORARY_PASSWORD }),
        ),
    )
    .await;

    assert_eq!(status, 200, "the challenge is a 200, got {payload}");
    assert_eq!(
        payload["ChallengeName"], "NEW_PASSWORD_REQUIRED",
        "expected the NEW_PASSWORD_REQUIRED challenge, got {payload}"
    );
    assert!(
        payload.get("AuthenticationResult").is_none(),
        "a challenge carries no tokens, got {payload}"
    );
    assert_eq!(
        payload["ChallengeParameters"]["USER_ID_FOR_SRP"], "carol",
        "the challenge names the stored user, got {payload}"
    );
    let session = payload["Session"]
        .as_str()
        .unwrap_or_else(|| panic!("the challenge carries a Session, got {payload}"));
    // `SessionType` is modelled `length 20..4096`.
    assert!(
        (20..=4096).contains(&session.len()),
        "a Session is 20..4096 characters, got {} in {payload}",
        session.len()
    );

    let service_state = state::service_state(&*engine).expect("this service's state");
    let guard = service_state.lock();
    let stored = guard
        .auth_sessions
        .get(session)
        .unwrap_or_else(|| panic!("the Session must be stored, got {:?}", guard.auth_sessions));
    assert_eq!(
        (
            stored.user_pool_id.as_str(),
            stored.client_id.as_str(),
            stored.username.as_str(),
            stored.challenge_name.as_str(),
        ),
        (
            fixture.user_pool_id.as_str(),
            fixture.client_id.as_str(),
            "carol",
            "NEW_PASSWORD_REQUIRED",
        ),
        "the stored session must carry the challenge it was started for, got {stored:?}"
    );
}

/// `REFRESH_TOKEN_AUTH` exchanges a refresh token this emulator issued for a fresh
/// access and id token — "Receive new ID and access tokens when you pass a
/// `REFRESH_TOKEN` parameter with a valid refresh token as the value"
/// (`InitiateAuth.AuthFlow`) — and refuses a token it cannot resolve to a live refresh
/// token of the app client the request names.
#[tokio::test]
async fn refresh_token_auth_renews_the_tokens_of_the_stored_refresh_token() {
    let engine = engine();
    let setup = router(&engine, false);
    let fixture = pool_with_sign_in_client(&setup, "refresh-pool").await;
    create_confirmed_user(&setup, &fixture.user_pool_id, "alice", GOOD_PASSWORD).await;
    // A second app client in the same pool, whose refresh token is not this one's.
    let other_client = create_client(
        &setup,
        &fixture.user_pool_id,
        "other-sign-in-client",
        &["ALLOW_USER_PASSWORD_AUTH", "ALLOW_REFRESH_TOKEN_AUTH"],
        None,
    )
    .await;
    // And one that does not enable the refresh flow at all.
    let no_refresh_client = create_client(
        &setup,
        &fixture.user_pool_id,
        "no-refresh-client",
        &["ALLOW_USER_PASSWORD_AUTH"],
        None,
    )
    .await;
    let serving = router(&engine, false);

    let first = sign_in(&serving, &fixture.client_id, "alice", GOOD_PASSWORD).await;
    let refresh_token = first["RefreshToken"].as_str().expect("a refresh token");
    let (status, payload) = dispatch(
        &serving,
        ANONYMOUS,
        "InitiateAuth",
        &initiate_auth(
            &fixture.client_id,
            "REFRESH_TOKEN_AUTH",
            json!({ "REFRESH_TOKEN": refresh_token }),
        ),
    )
    .await;

    assert_eq!(status, 200, "the refresh must succeed, got {payload}");
    let renewed = &payload["AuthenticationResult"];
    for member in ["AccessToken", "IdToken"] {
        let token = renewed[member]
            .as_str()
            .unwrap_or_else(|| panic!("case {member:?}: the refresh returns it, got {payload}"));
        assert_ne!(
            token,
            first[member].as_str().unwrap_or_default(),
            "case {member:?}: the refresh must issue a NEW token, got the first one back"
        );
    }
    assert!(
        renewed.get("RefreshToken").is_none(),
        "the refresh flow returns new ID and access tokens only, got {payload}"
    );

    {
        let service_state = state::service_state(&*engine).expect("this service's state");
        let guard = service_state.lock();
        let renewed_access = renewed["AccessToken"].as_str().expect("the access token");
        let record = guard
            .token(renewed_access)
            .expect("the renewed access token must be recorded");
        assert_eq!(
            (record.username.as_str(), record.kind),
            ("alice", state::TokenKind::Access),
            "the renewed token must be an access token of the stored user, got {record:?}"
        );
        assert!(
            guard.token(refresh_token).is_some(),
            "the refresh token the caller presented stays the one in force"
        );
    }

    let other_refresh_token = sign_in(&serving, &other_client, "alice", GOOD_PASSWORD).await
        ["RefreshToken"]
        .as_str()
        .expect("a refresh token")
        .to_string();
    let access_token = first["AccessToken"].as_str().expect("an access token");

    /// One row: the app client the request names, the `REFRESH_TOKEN` it presents
    /// (`None` sends none at all), and the refusal that presentation owes.
    struct Row<'a> {
        case: &'static str,
        client_id: &'a str,
        token: Option<&'a str>,
        expected: Answer,
    }

    let rows = [
        Row {
            case: "a token this emulator never issued",
            client_id: &fixture.client_id,
            token: Some("notatokenthisemulatorissued"),
            expected: refused(400, "NotAuthorizedException"),
        },
        Row {
            case: "an access token presented as a refresh token",
            client_id: &fixture.client_id,
            token: Some(access_token),
            expected: refused(400, "NotAuthorizedException"),
        },
        Row {
            case: "a refresh token issued for another app client",
            client_id: &fixture.client_id,
            token: Some(&other_refresh_token),
            expected: refused(400, "NotAuthorizedException"),
        },
        Row {
            case: "no REFRESH_TOKEN parameter",
            client_id: &fixture.client_id,
            token: None,
            expected: refused(400, "InvalidParameterException"),
        },
        Row {
            case: "a client that does not enable the refresh flow",
            client_id: &no_refresh_client,
            token: Some(refresh_token),
            expected: refused(400, "InvalidParameterException"),
        },
    ];

    for row in rows {
        let parameters = match row.token {
            Some(token) => json!({ "REFRESH_TOKEN": token }),
            None => json!({}),
        };
        let (status, payload) = dispatch(
            &serving,
            ANONYMOUS,
            "InitiateAuth",
            &initiate_auth(row.client_id, "REFRESH_TOKEN_AUTH", parameters),
        )
        .await;
        assert_answer(row.case, row.expected, status, &payload);
    }
}
