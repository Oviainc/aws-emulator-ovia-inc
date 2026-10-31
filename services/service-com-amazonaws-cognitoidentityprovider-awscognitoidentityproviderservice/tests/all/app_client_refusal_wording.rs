// SPDX-License-Identifier: CC0-1.0

//! How the app-client resource words the refusals that quote nothing.
//!
//! The rules under test, each one sentence of fixed text:
//!
//! * An operation on the app-client resource itself — `DescribeUserPoolClient` reading it,
//!   `UpdateUserPoolClient` writing it — refuses a `ClientId` that names NO app client at
//!   all with `ResourceNotFoundException` and ONE sentence that quotes neither the id
//!   asked for nor the pool, so an id no create minted, one a delete took away and one
//!   that is a near miss of a live id are all reported identically. A `ClientId` that
//!   does name an app client, of ANOTHER pool, is the other way that lookup fails and is
//!   reported with both halves of what was asked for: the id and the pool it is not in.
//! * `DeleteUserPoolClientSecret` refuses to take an app client's ONLY secret away
//!   ("you cannot delete the last remaining secret for an app client", that operation in
//!   the service model) with `InvalidParameterException` and one sentence that quotes
//!   neither the app client nor the `ClientSecretId` the request named.
//!
//! The rows differ along what those rules generalise over: for the lookup, the FORM and
//! LENGTH of the `ClientId` asked for (`ClientIdType`, length 1..128, pattern `^[\w+]+$`,
//! from `smithy_query(query="shape",shape="ClientIdType")`) and HOW it came to be absent;
//! for the secret, how the app client came to hold exactly one secret (the create
//! generated it, the create supplied it, an add generated it, an add supplied a value on
//! the `ClientSecretType` maximum, or a second secret was deleted again).
//!
//! Every refusal is followed by a read of the state it left: the pool's app clients and
//! the client's secrets are unchanged, so these are refusals and not mutations.
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

/// The one sentence an app-client lookup that resolves NO app client answers: fixed text,
/// quoting neither the `ClientId` asked for nor the pool it was looked for in.
const APP_CLIENT_NOT_FOUND_MESSAGE: &str = "User pool client does not exist.";

/// The one sentence the refusal to take an app client's only secret away carries: fixed
/// text, quoting neither the app client nor the `ClientSecretId` the request named.
const LAST_SECRET_MESSAGE: &str = "Cannot delete the only client secret.";

/// A `ClientSecretType`-shaped value (pattern `^[\w+]+$`) of the modelled MAXIMUM
/// length, 64 (`smithy_query(query="shape",shape="ClientSecretType")`).
const LONGEST_SECRET: &str = "f123456789_123456789_123456789_123456789_123456789_123456789_12+";

/// A `ClientSecretType`-shaped value of the modelled MINIMUM length, 24.
const SHORTEST_SECRET: &str = "eeeeeeeeeeeeeeeeeeeeeeee";

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

/// Create a pool named `pool_name` and return the id it was minted under.
async fn pool(router: &MultiServiceRouter, case: &str, pool_name: &str) -> String {
    let created = ok(router, case, "CreateUserPool", &json!({ "PoolName": pool_name })).await;
    created["UserPool"]["Id"]
        .as_str()
        .expect("a minted pool id")
        .to_string()
}

/// Create one app client of `user_pool_id` and return the whole stored client.
async fn client(
    router: &MultiServiceRouter,
    case: &str,
    user_pool_id: &str,
    client_name: &str,
    secret: Option<&str>,
    generate: bool,
) -> Value {
    let mut body = Map::new();
    body.insert("UserPoolId".to_string(), json!(user_pool_id));
    body.insert("ClientName".to_string(), json!(client_name));
    if let Some(value) = secret {
        body.insert("ClientSecret".to_string(), json!(value));
    }
    if generate {
        body.insert("GenerateSecret".to_string(), json!(true));
    }
    let created = ok(router, case, "CreateUserPoolClient", &Value::Object(body)).await;
    created["UserPoolClient"].clone()
}

/// The `ClientId`s `ListUserPoolClients` reports for `user_pool_id`.
async fn listed_clients(router: &MultiServiceRouter, case: &str, user_pool_id: &str) -> Vec<String> {
    let payload = ok(
        router,
        case,
        "ListUserPoolClients",
        &json!({ "UserPoolId": user_pool_id }),
    )
    .await;
    payload
        .get("UserPoolClients")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: a UserPoolClients list, got {payload}"))
        .iter()
        .map(|described| {
            described["ClientId"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a listed client id, got {described}"))
                .to_string()
        })
        .collect()
}

/// The `ClientSecretId`s `ListUserPoolClientSecrets` reports for one app client.
async fn listed_secrets(
    router: &MultiServiceRouter,
    case: &str,
    user_pool_id: &str,
    client_id: &str,
) -> Vec<String> {
    let payload = ok(
        router,
        case,
        "ListUserPoolClientSecrets",
        &json!({ "UserPoolId": user_pool_id, "ClientId": client_id }),
    )
    .await;
    payload
        .get("ClientSecrets")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: a ClientSecrets list, got {payload}"))
        .iter()
        .map(|descriptor| {
            descriptor["ClientSecretId"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a listed secret id, got {descriptor}"))
                .to_string()
        })
        .collect()
}

/// How the `ClientId` the request asks for came to name no app client of the pool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Absent {
    /// A `ClientIdType`-shaped id no create ever minted.
    NeverMinted(&'static str),
    /// An id no create minted, of the modelled MAXIMUM length, 128.
    OfMaximumLength,
    /// An id a create minted and `DeleteUserPoolClient` then took away.
    Deleted,
    /// An id that names a live app client of ANOTHER pool.
    OfAnotherPool,
    /// The live app client's own id with one further `^[\w+]+$` character on it.
    NearMiss,
}

#[tokio::test]
async fn an_unresolved_app_client_is_refused_by_the_way_the_lookup_failed() {
    // The rows differ along the form and length of the `ClientId` asked for, and along
    // how it came to be absent. The three NeverMinted forms span the pattern
    // `^[\w+]+$` and the published length bounds 1 and 128.
    let rows: &[(&str, Absent)] = &[
        (
            "a lower-case id no create minted",
            Absent::NeverMinted("1h57kf5cpq17m0eml12eukdi1o"),
        ),
        (
            "an id of the minimum modelled length, 1",
            Absent::NeverMinted("a"),
        ),
        (
            "an id of the maximum modelled length, 128",
            Absent::OfMaximumLength,
        ),
        ("an app client an earlier delete removed", Absent::Deleted),
        ("a live app client of another pool", Absent::OfAnotherPool),
        (
            "the live app client's id with one more character",
            Absent::NearMiss,
        ),
    ];

    for (case, absent) in rows {
        let router = router();
        let user_pool_id = pool(&router, case, "app-client-refusal-pool").await;
        let live = client(&router, case, &user_pool_id, "live-client", None, true).await;
        let live_id = live["ClientId"].as_str().expect("a minted client id").to_string();

        let asked = match absent {
            Absent::NeverMinted(id) => (*id).to_string(),
            // `ClientIdType` is length 1..128, so 128 `^[\w+]$` characters is the
            // longest id a request may carry at all.
            Absent::OfMaximumLength => "z".repeat(128),
            Absent::Deleted => {
                let doomed = client(&router, case, &user_pool_id, "doomed-client", None, true).await;
                let doomed_id = doomed["ClientId"].as_str().expect("a minted client id").to_string();
                ok(
                    &router,
                    case,
                    "DeleteUserPoolClient",
                    &json!({ "UserPoolId": user_pool_id, "ClientId": doomed_id }),
                )
                .await;
                doomed_id
            }
            Absent::OfAnotherPool => {
                let other_pool = pool(&router, case, "app-client-refusal-other-pool").await;
                let elsewhere =
                    client(&router, case, &other_pool, "elsewhere-client", None, true).await;
                elsewhere["ClientId"]
                    .as_str()
                    .expect("a minted client id")
                    .to_string()
            }
            Absent::NearMiss => format!("{live_id}+"),
        };

        // Which sentence the lookup owes: an id that names an app client of ANOTHER pool
        // is reported with the id and the pool it is not in, while an id that names no
        // app client at all carries this operation's fixed sentence.
        let cross_pool = *absent == Absent::OfAnotherPool;
        let expected = if cross_pool {
            format!("User pool client {asked} does not exist in user pool {user_pool_id}.")
        } else {
            APP_CLIENT_NOT_FOUND_MESSAGE.to_string()
        };

        // The same unresolved lookup, reached by the read and by the write: one answer.
        for (operation, request) in [
            (
                "DescribeUserPoolClient",
                json!({ "UserPoolId": user_pool_id, "ClientId": asked }),
            ),
            (
                "UpdateUserPoolClient",
                json!({
                    "UserPoolId": user_pool_id,
                    "ClientId": asked,
                    "ClientName": "renamed-by-a-refused-update",
                }),
            ),
        ] {
            let (status, payload) = dispatch(&router, operation, &request).await;
            assert_eq!(
                (status, error_code(&payload)),
                (400, "ResourceNotFoundException"),
                "case {case:?}: {operation} must answer a 400 ResourceNotFoundException, \
                 got ({status}, {:?}) with {payload}",
                error_code(&payload)
            );
            let message = error_message(&payload);
            assert_eq!(
                message, expected,
                "case {case:?}: {operation} must answer {expected:?}, got {message:?}"
            );
            // Nothing of the request is in the sentence for an id that names no app
            // client, so no two callers in that condition can be told apart by its text
            // — and the `@sensitive` ClientId never reaches it. A one-character id is a
            // substring of ordinary prose, so the quoting assertion is made on the ids
            // long enough to mean something.
            if !cross_pool {
                if asked.len() >= 4 {
                    assert!(
                        !message.contains(asked.as_str()),
                        "case {case:?}: {operation} must not quote the id asked for, \
                         got {message:?}"
                    );
                }
                assert!(
                    !message.contains(user_pool_id.as_str()),
                    "case {case:?}: {operation} must not quote the pool, got {message:?}"
                );
            }
        }

        // The refusals changed nothing: the pool still holds exactly the app client that
        // was never deleted, and it still reads back.
        assert_eq!(
            listed_clients(&router, case, &user_pool_id).await,
            vec![live_id.clone()],
            "case {case:?}: the refused calls must leave the pool's app clients alone"
        );
        let described = ok(
            &router,
            case,
            "DescribeUserPoolClient",
            &json!({ "UserPoolId": user_pool_id, "ClientId": live_id }),
        )
        .await;
        assert_eq!(
            described["UserPoolClient"]["ClientName"],
            json!("live-client"),
            "case {case:?}: the live app client keeps the name it was created with, \
             got {described}"
        );
    }
}

/// How the app client came to hold exactly one client secret.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OnlySecret {
    /// `GenerateSecret: true` on the create minted it.
    CreateGenerated,
    /// The create supplied this value.
    CreateSupplied(&'static str),
    /// The client was created public and one add stored a secret on it — a generated
    /// value for `None`, the supplied one otherwise.
    Added(Option<&'static str>),
    /// The client was created with a secret, an add stored a second, and a delete took
    /// that second one away again.
    AddedThenDeleted,
}

#[tokio::test]
async fn the_only_secret_of_an_app_client_is_refused_with_one_fixed_sentence() {
    let rows: &[(&str, OnlySecret)] = &[
        ("the secret the create generated", OnlySecret::CreateGenerated),
        (
            "the secret the create supplied, at the minimum modelled length",
            OnlySecret::CreateSupplied(SHORTEST_SECRET),
        ),
        ("a generated secret one add stored", OnlySecret::Added(None)),
        (
            "a supplied secret at the maximum modelled length",
            OnlySecret::Added(Some(LONGEST_SECRET)),
        ),
        (
            "the one secret left after a second was deleted",
            OnlySecret::AddedThenDeleted,
        ),
    ];

    for (case, holding) in rows {
        let router = router();
        let user_pool_id = pool(&router, case, "last-secret-refusal-pool").await;
        let created = match holding {
            OnlySecret::CreateGenerated | OnlySecret::AddedThenDeleted => {
                client(&router, case, &user_pool_id, "secret-holder", None, true).await
            }
            OnlySecret::CreateSupplied(value) => {
                client(&router, case, &user_pool_id, "secret-holder", Some(value), false).await
            }
            OnlySecret::Added(_) => {
                client(&router, case, &user_pool_id, "secret-holder", None, false).await
            }
        };
        let client_id = created["ClientId"].as_str().expect("a minted client id").to_string();

        // What the add that stores the client's further secret supplies, when the row
        // calls for an add at all: a value, or nothing so Amazon Cognito generates one.
        let adds = match holding {
            OnlySecret::Added(value) => Some(*value),
            OnlySecret::AddedThenDeleted => Some(None),
            OnlySecret::CreateGenerated | OnlySecret::CreateSupplied(_) => None,
        };
        if let Some(supplied) = adds {
            let mut body = Map::new();
            body.insert("UserPoolId".to_string(), json!(user_pool_id));
            body.insert("ClientId".to_string(), json!(client_id));
            if let Some(value) = supplied {
                body.insert("ClientSecret".to_string(), json!(value));
            }
            let added = ok(
                &router,
                case,
                "AddUserPoolClientSecret",
                &Value::Object(body),
            )
            .await;
            if *holding == OnlySecret::AddedThenDeleted {
                let added_id = added["ClientSecretDescriptor"]["ClientSecretId"]
                    .as_str()
                    .expect("the add reports the id it minted")
                    .to_string();
                ok(
                    &router,
                    case,
                    "DeleteUserPoolClientSecret",
                    &json!({
                        "UserPoolId": user_pool_id,
                        "ClientId": client_id,
                        "ClientSecretId": added_id,
                    }),
                )
                .await;
            }
        }

        let held = listed_secrets(&router, case, &user_pool_id, &client_id).await;
        assert_eq!(
            held.len(),
            1,
            "case {case:?}: the row must leave the app client holding one secret, got {held:?}"
        );

        let (status, payload) = dispatch(
            &router,
            "DeleteUserPoolClientSecret",
            &json!({
                "UserPoolId": user_pool_id,
                "ClientId": client_id,
                "ClientSecretId": held[0],
            }),
        )
        .await;
        assert_eq!(
            (status, error_code(&payload)),
            (400, "InvalidParameterException"),
            "case {case:?}: the last secret must be refused with a 400 \
             InvalidParameterException, got ({status}, {:?}) with {payload}",
            error_code(&payload)
        );
        let message = error_message(&payload);
        assert_eq!(
            message, LAST_SECRET_MESSAGE,
            "case {case:?}: expected {LAST_SECRET_MESSAGE:?}, got {message:?}"
        );
        assert!(
            !message.contains(client_id.as_str()) && !message.contains(held[0].as_str()),
            "case {case:?}: the refusal must quote neither the app client nor the secret id, \
             got {message:?}"
        );

        // The refusal stored nothing and removed nothing: the one secret is still there.
        assert_eq!(
            listed_secrets(&router, case, &user_pool_id, &client_id).await,
            held,
            "case {case:?}: the refused delete must leave the stored secret alone"
        );
    }
}
