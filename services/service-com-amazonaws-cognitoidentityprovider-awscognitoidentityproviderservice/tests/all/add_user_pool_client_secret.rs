// SPDX-License-Identifier: CC0-1.0

//! `AddUserPoolClientSecret` over the wire: a further client secret of an app client.
//!
//! The rule under test. `AddUserPoolClientSecret` "creates a new client secret for an
//! existing confidential user pool app client. Supports up to 2 active secrets per app
//! client for zero-downtime credential rotation workflows" (this operation in the service
//! model). So one call stores ONE more secret on the app client the request names — "the
//! client secret value you want to use. If you don't provide this parameter, Amazon
//! Cognito generates a secure secret for you" (`ClientSecret`) — every secret the client
//! holds authenticates it while it is held, and a client already holding the two secrets
//! an app client may hold at once is refused rather than grown.
//!
//! The response is the STORED secret's descriptor: its `ClientSecretId`, which "follows
//! the format <client-id>--<epoch-create-time>", its `ClientSecretCreateDate`, and a
//! `ClientSecretValue` that "is only returned when creating a new secret and only if
//! Amazon Cognito generated the secret. For custom secrets that you provide, this field
//! is not included" (`ClientSecretDescriptorType` in the service model).
//!
//! The rows below differ along what that rule generalises over: what the app client
//! already holds (a generated secret, a secret the create supplied, or none at all),
//! whether the add supplies a value or lets the service generate one, the length of a
//! supplied value across the published bounds of `ClientSecretType` (24..64), and how many
//! secrets the client already holds when the next add arrives.
//!
//! Each added secret is read back through two implemented operations: `GetClientToken`,
//! whose grant "must configure the app client with a client secret" and is authorized by
//! the stored secret, and `DescribeUserPoolClient`, which renders the app client's own
//! `ClientSecret`. `ListUserPoolClientSecrets` and `DeleteUserPoolClientSecret` are still
//! scaffolded stubs with issues of their own, so the listing of a client's secrets is not
//! asserted here.
//!
//! Each table row names the case it asserts.

use bytes::Bytes;
use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice as cognito_idp;

/// This service's SigV4 signing name, the 4th field of a credential scope.
const SIGNING_NAME: &str = "cognito-idp";

/// The local name of this service's Smithy shape, which is the `X-Amz-Target` prefix a
/// client writes.
const TARGET_PREFIX: &str = "AWSCognitoIdentityProviderService";

/// This service's real regional endpoint host.
const ENDPOINT_HOST: &str = "cognito-idp.us-east-1.amazonaws.com";

/// The region the suite's calls are signed for.
const DEFAULT_REGION: &str = "us-east-1";

/// The operation under test.
const ADD: &str = "AddUserPoolClientSecret";

/// The `ExplicitAuthFlows` value that configures an app client for machine-to-machine
/// authorization, which is the grant that reads a stored client secret back.
const CLIENT_TOKEN_AUTH_FLOW: &str = "ALLOW_CLIENT_TOKEN_AUTH";

/// How many secrets one app client holds at a time: "supports up to 2 active secrets per
/// app client" (this operation in the service model).
const MAX_CLIENT_SECRETS: usize = 2;

/// A `ClientSecretType`-shaped value (pattern `^[\w+]+$`) of the modelled MINIMUM length,
/// 24 (`smithy_query(query="shape",shape="ClientSecretType")`).
const SHORTEST_SECRET: &str = "aaaaaaaaaaaaaaaaaaaaaaaa";

/// A `ClientSecretType`-shaped value of the modelled MAXIMUM length, 64.
const LONGEST_SECRET: &str =
    "b123456789_123456789_123456789_123456789_123456789_123456789_12+";

/// A `ClientSecretType`-shaped value no app client of the suite holds.
const UNHELD_SECRET: &str = "nosuchclientsecret000000000";

/// A `UserPoolIdType`-shaped id (pattern `^[\w-]+_[0-9a-zA-Z]+$`) no create minted.
const UNKNOWN_POOL_ID: &str = "us-east-1_nosuchpool";

/// A `ClientIdType`-shaped id (pattern `^[\w+]+$`) no create minted.
const UNKNOWN_CLIENT_ID: &str = "nosuchappclient0000";

/// One emulator with just this service mounted.
fn router() -> MultiServiceRouter {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(cognito_idp::EmulatedService::new(engine));
    router
}

/// Dispatch `operation` with `body` and return the status and JSON response body.
async fn dispatch(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", ENDPOINT_HOST)
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{DEFAULT_REGION}/{SIGNING_NAME}/aws4_request, \
                 SignedHeaders=host;x-amz-date, Signature=0000"
            ),
        );
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

/// Call `operation`, requiring success, and return its response body.
async fn ok(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let (status, payload) = dispatch(router, operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: {operation} must succeed, got {status} with {payload}"
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

/// The `message` of an error body.
fn error_message(payload: &Value) -> &str {
    payload
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
}

/// Call `operation` expecting a refusal, assert its status and wire error code, and hand
/// the body back so a caller can read the message too.
async fn refused(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    body: &Value,
    expected_status: u16,
    expected_code: &str,
) -> Value {
    let (status, payload) = dispatch(router, operation, body).await;
    assert_eq!(
        (status, error_code(&payload)),
        (expected_status, expected_code),
        "case {case:?}: expected ({expected_status}, {expected_code:?}), \
         got ({status}, {:?}) with {payload}",
        error_code(&payload)
    );
    payload
}

/// What an app client holds when the add arrives, which is the other half of the "up to 2
/// active secrets" rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CreatedWith {
    /// `GenerateSecret: true`, so the create minted the client's first secret.
    Generated,
    /// The create supplied this `ClientSecret` value.
    Supplied(&'static str),
    /// Neither, so the app client is public and holds no secret at all.
    Public,
}

impl CreatedWith {
    /// How many secrets the created app client holds.
    fn held(self) -> usize {
        match self {
            Self::Public => 0,
            Self::Generated | Self::Supplied(_) => 1,
        }
    }
}

/// A created pool and the app client of it a secret is added to.
struct Fixture {
    user_pool_id: String,
    client_id: String,
    /// The secret the create stored, for an app client created with one.
    created_secret: Option<String>,
}

/// Create a pool named `pool_name` and one app client of it, configured for the
/// machine-to-machine grant so a stored secret can be read back through it.
async fn fixture(
    router: &MultiServiceRouter,
    case: &str,
    pool_name: &str,
    created_with: CreatedWith,
) -> Fixture {
    let created = ok(router, case, "CreateUserPool", &json!({ "PoolName": pool_name })).await;
    let user_pool_id = created["UserPool"]["Id"]
        .as_str()
        .expect("a minted pool id")
        .to_string();

    let mut body = Map::new();
    body.insert("UserPoolId".to_string(), json!(user_pool_id));
    body.insert("ClientName".to_string(), json!("rotating-client"));
    body.insert(
        "ExplicitAuthFlows".to_string(),
        json!([CLIENT_TOKEN_AUTH_FLOW]),
    );
    match created_with {
        CreatedWith::Generated => {
            body.insert("GenerateSecret".to_string(), json!(true));
        }
        CreatedWith::Supplied(secret) => {
            body.insert("ClientSecret".to_string(), json!(secret));
        }
        CreatedWith::Public => {}
    }
    let client = ok(router, case, "CreateUserPoolClient", &Value::Object(body)).await;
    let created_secret = client["UserPoolClient"]["ClientSecret"]
        .as_str()
        .map(str::to_string);
    assert_eq!(
        created_secret.is_some(),
        created_with.held() == 1,
        "case {case:?}: {created_with:?} must store {} secret(s), the create answered {created_secret:?}",
        created_with.held()
    );

    Fixture {
        user_pool_id,
        client_id: client["UserPoolClient"]["ClientId"]
            .as_str()
            .expect("a minted client id")
            .to_string(),
        created_secret,
    }
}

/// The add's request body: `ClientSecret` only when the row supplies a value.
fn add_body(fixture: &Fixture, supplied: Option<&str>) -> Value {
    let mut body = Map::new();
    body.insert("UserPoolId".to_string(), json!(fixture.user_pool_id));
    body.insert("ClientId".to_string(), json!(fixture.client_id));
    if let Some(secret) = supplied {
        body.insert("ClientSecret".to_string(), json!(secret));
    }
    Value::Object(body)
}

/// Assert the descriptor an add answered, and return the value of the secret it stored:
/// the one the request supplied, or the generated one the response carries.
fn stored_secret(case: &str, client_id: &str, supplied: Option<&str>, payload: &Value) -> String {
    let descriptor = payload
        .get("ClientSecretDescriptor")
        .unwrap_or_else(|| panic!("case {case:?}: the add must answer a ClientSecretDescriptor, got {payload}"));

    // "This identifier follows the format <client-id>--<epoch-create-time>"
    // (`ClientSecretDescriptorType.ClientSecretId`).
    let id = descriptor
        .get("ClientSecretId")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: the descriptor must carry a ClientSecretId, got {descriptor}"));
    let suffix = id.strip_prefix(&format!("{client_id}--")).unwrap_or_else(|| {
        panic!("case {case:?}: expected a ClientSecretId of the form {client_id}--<epoch-create-time>, got {id:?}")
    });
    assert!(
        !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit()),
        "case {case:?}: the ClientSecretId's create time must be an epoch time, got {id:?}"
    );
    assert!(
        id.len() <= 128,
        "case {case:?}: a ClientSecretId is at most 128 characters, got {} in {id:?}",
        id.len()
    );

    let create_date = descriptor
        .get("ClientSecretCreateDate")
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("case {case:?}: the descriptor must carry a ClientSecretCreateDate, got {descriptor}"));
    assert!(
        create_date > 0.0,
        "case {case:?}: the ClientSecretCreateDate must be an epoch timestamp, got {create_date}"
    );

    // "The actual secret value ... is only returned when creating a new secret and only if
    // Amazon Cognito generated the secret. For custom secrets that you provide, this field
    // is not included in the response" (`ClientSecretDescriptorType.ClientSecretValue`).
    let value = descriptor.get("ClientSecretValue").and_then(Value::as_str);
    match supplied {
        Some(supplied) => {
            assert!(
                value.is_none(),
                "case {case:?}: a supplied secret is not echoed in ClientSecretValue, got {value:?}"
            );
            supplied.to_string()
        }
        None => {
            let value = value.unwrap_or_else(|| {
                panic!("case {case:?}: a generated secret must come back in ClientSecretValue, got {descriptor}")
            });
            // The modelled `ClientSecretType`: length 24..64, pattern `^[\w+]+$`.
            assert!(
                (24..=64).contains(&value.len()),
                "case {case:?}: a generated secret is 24..64 characters, got {} in {value:?}",
                value.len()
            );
            assert!(
                value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'+')),
                "case {case:?}: a generated secret matches ^[\\w+]+$, got {value:?}"
            );
            value.to_string()
        }
    }
}

/// Whether the machine-to-machine grant of `client_id` accepts `secret`, which is how an
/// added secret is read back: the grant "must configure the app client with a client
/// secret" and is authorized by the STORED one (`GetClientToken` in the service model).
async fn grant_accepts(router: &MultiServiceRouter, client_id: &str, secret: &str) -> bool {
    let (status, payload) = dispatch(
        router,
        "GetClientToken",
        &json!({ "ClientId": client_id, "Secret": secret }),
    )
    .await;
    if status == 200 {
        assert!(
            payload["ClientAuthenticationResult"]["AccessToken"]
                .as_str()
                .is_some_and(|token| !token.is_empty()),
            "an accepted grant mints an access token, got {payload}"
        );
        return true;
    }
    assert_eq!(
        (status, error_code(&payload)),
        (403, "NotAuthorizedException"),
        "a refused grant is a 403 NotAuthorizedException, got ({status}, {:?}) with {payload}",
        error_code(&payload)
    );
    false
}

/// The app client as `DescribeUserPoolClient` renders it.
async fn described(router: &MultiServiceRouter, case: &str, fixture: &Fixture) -> Value {
    ok(
        router,
        case,
        "DescribeUserPoolClient",
        &json!({ "UserPoolId": fixture.user_pool_id, "ClientId": fixture.client_id }),
    )
    .await["UserPoolClient"]
        .clone()
}

/// One row of the add table: what the app client was created with and what the add
/// supplies.
struct AddCase {
    case: &'static str,
    created_with: CreatedWith,
    /// The `ClientSecret` the add supplies, or `None` to let the service generate one.
    supplied: Option<&'static str>,
}

#[tokio::test]
async fn an_added_client_secret_is_stored_and_authenticates_the_app_client() {
    // The two supplied values below sit on the published bounds of `ClientSecretType`
    // (length 24..64), so a typo in either is a failure of this suite rather than a
    // surprise refusal by the framework's constraint checks.
    assert_eq!(
        (SHORTEST_SECRET.len(), LONGEST_SECRET.len()),
        (24, 64),
        "the bound rows must sit on the modelled bounds, got ({}, {})",
        SHORTEST_SECRET.len(),
        LONGEST_SECRET.len()
    );

    let rows = [
        AddCase {
            case: "a generated secret added to a client that generated its first",
            created_with: CreatedWith::Generated,
            supplied: None,
        },
        AddCase {
            case: "a supplied secret of the shortest modelled length",
            created_with: CreatedWith::Generated,
            supplied: Some(SHORTEST_SECRET),
        },
        AddCase {
            case: "a supplied secret of the longest modelled length",
            created_with: CreatedWith::Supplied("createsecret000000000000001"),
            supplied: Some(LONGEST_SECRET),
        },
        AddCase {
            case: "a generated secret added beside a supplied one",
            created_with: CreatedWith::Supplied("createsecret000000000000002"),
            supplied: None,
        },
        AddCase {
            case: "the first secret of an app client that holds none",
            created_with: CreatedWith::Public,
            supplied: None,
        },
        AddCase {
            case: "a supplied first secret of an app client that holds none",
            created_with: CreatedWith::Public,
            supplied: Some("suppliedfirstsecret00000000"),
        },
    ];

    for (at, row) in rows.iter().enumerate() {
        let router = router();
        let case = row.case;
        let fixture = fixture(
            &router,
            case,
            &format!("rotation-pool-{at}"),
            row.created_with,
        )
        .await;

        // A client that holds no secret has nothing a grant can prove it knows.
        if row.created_with == CreatedWith::Public {
            assert!(
                !grant_accepts(&router, &fixture.client_id, UNHELD_SECRET).await,
                "case {case:?}: an app client that holds no secret authorizes no grant"
            );
        }

        let payload = ok(
            &router,
            case,
            ADD,
            &add_body(&fixture, row.supplied),
        )
        .await;
        let added = stored_secret(case, &fixture.client_id, row.supplied, &payload);

        // The STORED secret is what a later call authenticates with.
        assert!(
            grant_accepts(&router, &fixture.client_id, &added).await,
            "case {case:?}: the added secret {added:?} must authenticate the app client"
        );
        // Every secret the client holds stays active, which is what makes the rotation
        // zero-downtime.
        if let Some(created) = fixture.created_secret.as_deref() {
            assert!(
                grant_accepts(&router, &fixture.client_id, created).await,
                "case {case:?}: the secret the create stored must stay active beside the added one"
            );
        }
        assert!(
            !grant_accepts(&router, &fixture.client_id, UNHELD_SECRET).await,
            "case {case:?}: a value the app client does not hold authorizes no grant"
        );

        // `UserPoolClientType.ClientSecret` is "the app client secret": the one the client
        // already had, and the added one for a client that had none.
        let read_back = described(&router, case, &fixture).await;
        let expected = fixture.created_secret.clone().unwrap_or(added.clone());
        assert_eq!(
            read_back.get("ClientSecret").and_then(Value::as_str),
            Some(expected.as_str()),
            "case {case:?}: DescribeUserPoolClient must render {expected:?}, got {:?}",
            read_back.get("ClientSecret")
        );
    }
}

#[tokio::test]
async fn an_app_client_holds_at_most_two_client_secrets() {
    let rows = [
        CreatedWith::Generated,
        CreatedWith::Supplied("createsecret000000000000003"),
        CreatedWith::Public,
    ];

    for (at, created_with) in rows.iter().enumerate() {
        let router = router();
        let case = format!("{created_with:?} at the two-secret limit");
        let fixture = fixture(&router, &case, &format!("limit-pool-{at}"), *created_with).await;

        let mut held: Vec<String> = fixture.created_secret.clone().into_iter().collect();
        let mut ids = Vec::new();
        while held.len() < MAX_CLIENT_SECRETS {
            let payload = ok(&router, &case, ADD, &add_body(&fixture, None)).await;
            ids.push(
                payload["ClientSecretDescriptor"]["ClientSecretId"]
                    .as_str()
                    .expect("a minted ClientSecretId")
                    .to_string(),
            );
            held.push(stored_secret(&case, &fixture.client_id, None, &payload));
        }
        assert_eq!(
            held.len(),
            MAX_CLIENT_SECRETS,
            "case {case:?}: the client must now hold {MAX_CLIENT_SECRETS} secrets, holds {}",
            held.len()
        );

        // One more is refused: "supports up to 2 active secrets per app client".
        let payload = refused(
            &router,
            &case,
            ADD,
            &add_body(&fixture, None),
            400,
            "LimitExceededException",
        )
        .await;
        let expected_message =
            format!("Client secrets cannot exceed limit of {MAX_CLIENT_SECRETS} secrets.");
        assert_eq!(
            error_message(&payload),
            expected_message,
            "case {case:?}: expected {expected_message:?}, got {:?}",
            error_message(&payload)
        );

        // A supplied value is refused the same way: the limit is on the app client, not on
        // where the value came from.
        refused(
            &router,
            &case,
            ADD,
            &add_body(&fixture, Some(UNHELD_SECRET)),
            400,
            "LimitExceededException",
        )
        .await;

        // The refusal stored nothing: the secrets the client held are exactly the secrets
        // it holds, and the refused value is not one of them.
        for secret in &held {
            assert!(
                grant_accepts(&router, &fixture.client_id, secret).await,
                "case {case:?}: the held secret {secret:?} must still authenticate the app client"
            );
        }
        assert!(
            !grant_accepts(&router, &fixture.client_id, UNHELD_SECRET).await,
            "case {case:?}: the refused value must not have been stored"
        );
        if ids.len() == MAX_CLIENT_SECRETS {
            assert_ne!(
                ids[0], ids[1],
                "case {case:?}: each secret of an app client carries its own id, got {ids:?}"
            );
        }
    }
}

/// Which part of an add's request names nothing the state holds.
#[derive(Debug, Clone, Copy)]
enum Absent {
    /// A `UserPoolId` no create minted.
    UserPool,
    /// A `ClientId` no create minted.
    AppClient,
    /// A `ClientId` that names an app client of ANOTHER user pool, which the named
    /// pool holds no more than one no create minted.
    AppClientOfAnotherPool,
}

#[tokio::test]
async fn an_add_for_a_resource_no_pool_holds_is_refused() {
    for (at, absent) in [Absent::UserPool, Absent::AppClient, Absent::AppClientOfAnotherPool]
        .iter()
        .enumerate()
    {
        let router = router();
        let case = format!("{absent:?} named by the add");
        let named = fixture(
            &router,
            &case,
            &format!("not-found-pool-{at}"),
            CreatedWith::Generated,
        )
        .await;
        let elsewhere = fixture(
            &router,
            &case,
            &format!("other-pool-{at}"),
            CreatedWith::Generated,
        )
        .await;

        let (user_pool_id, client_id, expected) = match absent {
            Absent::UserPool => (
                UNKNOWN_POOL_ID.to_string(),
                named.client_id.clone(),
                format!("User pool {UNKNOWN_POOL_ID} does not exist."),
            ),
            Absent::AppClient => (
                named.user_pool_id.clone(),
                UNKNOWN_CLIENT_ID.to_string(),
                format!("User pool client {UNKNOWN_CLIENT_ID} does not exist."),
            ),
            Absent::AppClientOfAnotherPool => (
                named.user_pool_id.clone(),
                elsewhere.client_id.clone(),
                format!("User pool client {} does not exist.", elsewhere.client_id),
            ),
        };
        let body = json!({ "UserPoolId": user_pool_id, "ClientId": client_id });
        let payload = refused(&router, &case, ADD, &body, 404, "ResourceNotFoundException").await;
        assert_eq!(
            error_message(&payload),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            error_message(&payload)
        );

        // The refusal stored nothing: both app clients still hold just the secret their
        // create stored.
        for client in [&named, &elsewhere] {
            let read_back = described(&router, &case, client).await;
            assert_eq!(
                read_back.get("ClientSecret").and_then(Value::as_str),
                client.created_secret.as_deref(),
                "case {case:?}: a refused add leaves an app client's secret alone, got {:?}",
                read_back.get("ClientSecret")
            );
        }
    }
}
