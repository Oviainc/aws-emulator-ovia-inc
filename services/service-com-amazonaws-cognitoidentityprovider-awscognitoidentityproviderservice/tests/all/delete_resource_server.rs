// SPDX-License-Identifier: CC0-1.0

//! `DeleteResourceServer` over the wire, driven through the router exactly as an
//! SDK drives it (`POST /` with
//! `X-Amz-Target: AWSCognitoIdentityProviderService.<Op>`).
//!
//! The rule under test: `DeleteResourceServer` drops the one resource server the
//! pair (`UserPoolId`, `Identifier`) names from the pool's stored resource
//! servers, so every later read of that pair is a 400 and no other stored
//! resource server, pool or app client is touched; a pair that names no stored
//! resource server — an absent pool, an identifier the pool does not hold, an
//! identifier of another pool, or one a previous call already deleted — is a 400
//! `ResourceNotFoundException` that stores nothing — worded, as every reader of
//! the same pair words it, by naming the identifier, what is wrong with it, and
//! the pool it was looked for in. Because a pool defines a
//! custom scope only through a resource server it still holds, "after you delete
//! a resource server, users can no longer generate access tokens with scopes
//! that are associate with that resource server" (`DeleteResourceServer` in this
//! service's model) follows from that one write.
//!
//! The rows differ along what the rule generalises over: the form and length of
//! the identifier, the friendly name, and the count and order of the custom
//! scopes — the dimensions `CreateResourceServer`'s own constraints span. Each
//! assertion names the case it asserts.

use bytes::Bytes;
use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// A request's SigV4 credential scope is what routes it to this service. The
/// signature is never checked here: the suite turns request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/cognito-idp/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0000";

/// A `ClientSecretType`-shaped secret (pattern `^[\w+]+$`, length 24..64) the
/// machine-to-machine app client below is created with.
const CLIENT_SECRET: &str = "m2mdeletesecret0000000000001";

/// One emulator with just this service mounted.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(
        service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice::EmulatedService::new(engine),
    );
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

/// The `message` of an error body.
fn error_message(payload: &Value) -> &str {
    payload
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
}

/// Create a pool and return its minted `UserPoolId`.
async fn pool(router: &MultiServiceRouter, name: &str) -> String {
    let created = ok(router, "CreateUserPool", &json!({ "PoolName": name })).await;
    created["UserPool"]["Id"]
        .as_str()
        .expect("a minted pool id")
        .to_string()
}

/// One row of the table: an identifier, a friendly name and the custom scopes
/// the resource server is created with.
struct DeleteCase {
    case: &'static str,
    identifier: String,
    name: String,
    /// `None` leaves `Scopes` OUT of the create request; `Some(list)` sends it.
    scopes: Option<Vec<(String, String)>>,
}

impl DeleteCase {
    /// This case's `CreateResourceServer` request for `user_pool_id`.
    fn create_request(&self, user_pool_id: &str) -> Value {
        let mut body = Map::new();
        body.insert("UserPoolId".to_string(), json!(user_pool_id));
        body.insert("Identifier".to_string(), json!(self.identifier));
        body.insert("Name".to_string(), json!(self.name));
        if let Some(scopes) = &self.scopes {
            body.insert(
                "Scopes".to_string(),
                Value::Array(
                    scopes
                        .iter()
                        .map(|(scope_name, description)| {
                            json!({ "ScopeName": scope_name, "ScopeDescription": description })
                        })
                        .collect(),
                ),
            );
        }
        Value::Object(body)
    }
}

/// The table. `Identifier` spans its modelled length 1..256 and pattern
/// `^[\x21\x23-\x5B\x5D-\x7E]+$` (an API friendly name, an API URL, punctuation,
/// the length maximum), `Name` its length 1..256 and pattern `^[\w\s+=,.@-]+$`,
/// and `Scopes` its modelled length 0..100 (unset, empty, one, several out of
/// alphabetical order) — the constraints
/// `smithy_query(query="operation",operation="CreateResourceServer")` publishes
/// for the resource server a delete removes.
fn delete_cases() -> Vec<DeleteCase> {
    vec![
        DeleteCase {
            case: "an API friendly identifier with no Scopes member at all",
            identifier: "solar-system-data".to_string(),
            name: "Solar system data".to_string(),
            scopes: None,
        },
        DeleteCase {
            case: "an API URL identifier and an explicitly empty scope list",
            identifier: "https://solar-system-data-api.example.com".to_string(),
            name: "Solar system API".to_string(),
            scopes: Some(Vec::new()),
        },
        DeleteCase {
            case: "one scope",
            identifier: "weather".to_string(),
            name: "weather+data,service@edge-1".to_string(),
            scopes: Some(vec![(
                "forecast.read".to_string(),
                "Read the forecast".to_string(),
            )]),
        },
        DeleteCase {
            case: "several scopes, created out of alphabetical order",
            identifier: "tele!metry#1[2]~".to_string(),
            name: "Telemetry = ingest".to_string(),
            scopes: Some(vec![
                ("write".to_string(), "Write telemetry".to_string()),
                ("read".to_string(), "Read telemetry".to_string()),
                ("admin".to_string(), "Administer telemetry".to_string()),
            ]),
        },
        DeleteCase {
            case: "identifier and name at the length maximum 256",
            identifier: "i".repeat(256),
            name: "n".repeat(256),
            scopes: Some(vec![("scope0".to_string(), "scope number 0".to_string())]),
        },
    ]
}

/// Every `Identifier` a pool's listing serves, in the order it served them.
async fn listed_identifiers(router: &MultiServiceRouter, user_pool_id: &str) -> Vec<String> {
    let listed = ok(
        router,
        "ListResourceServers",
        // `MaxResults`' published maximum is 50, and the table holds far fewer
        // rows than that, so one page serves the whole listing.
        &json!({ "UserPoolId": user_pool_id, "MaxResults": 50 }),
    )
    .await;
    listed["ResourceServers"]
        .as_array()
        .unwrap_or_else(|| panic!("ResourceServers must be a present list, got {listed}"))
        .iter()
        .map(|entry| {
            entry["Identifier"]
                .as_str()
                .unwrap_or_else(|| panic!("every entry carries an Identifier, got {entry}"))
                .to_string()
        })
        .collect()
}

#[tokio::test]
async fn a_delete_removes_exactly_the_resource_server_it_names() {
    let router = router();
    let user_pool_id = pool(&router, "delete-resource-server-pool").await;

    // Every row's resource server is stored in the one pool, so each delete has
    // siblings that must survive it.
    let cases = delete_cases();
    let mut created = Vec::new();
    for case in &cases {
        let response = ok(
            &router,
            "CreateResourceServer",
            &case.create_request(&user_pool_id),
        )
        .await;
        created.push(response["ResourceServer"].clone());
    }

    for (at, case) in cases.iter().enumerate() {
        let label = case.case;
        let (status, payload) = call(
            &router,
            "DeleteResourceServer",
            &json!({ "UserPoolId": user_pool_id, "Identifier": case.identifier }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: a delete of a stored resource server succeeds, got {status} with {payload}"
        );

        // The named resource server is gone from every reader.
        let (read_status, read) = call(
            &router,
            "DescribeResourceServer",
            &json!({ "UserPoolId": user_pool_id, "Identifier": case.identifier }),
        )
        .await;
        assert_eq!(
            (read_status, error_code(&read)),
            (400, "ResourceNotFoundException"),
            "case {label:?}: a deleted resource server is gone from DescribeResourceServer, got {read}"
        );
        // Every reader of a pair naming no stored resource server words the miss
        // the same way: the identifier, what is wrong with it, and the pool it
        // was looked for in.
        assert_eq!(
            error_message(&read),
            format!(
                "{} does not exist in user pool {user_pool_id}.",
                case.identifier
            ),
            "case {label:?}: the read's refusal names the identifier and the pool it was looked for in, got {read}"
        );

        let served = listed_identifiers(&router, &user_pool_id).await;
        let expected: Vec<String> = cases[at + 1..]
            .iter()
            .map(|remaining| remaining.identifier.clone())
            .collect();
        let mut sorted_expected = expected.clone();
        sorted_expected.sort();
        let mut sorted_served = served.clone();
        sorted_served.sort();
        assert_eq!(
            sorted_served, sorted_expected,
            "case {label:?}: the listing serves exactly the resource servers not yet deleted, got {served:?}"
        );

        // And every sibling still renders exactly what its create stored.
        for sibling in at + 1..cases.len() {
            let other = &cases[sibling];
            let read = ok(
                &router,
                "DescribeResourceServer",
                &json!({ "UserPoolId": user_pool_id, "Identifier": other.identifier }),
            )
            .await;
            assert_eq!(
                read["ResourceServer"], created[sibling],
                "case {label:?}: deleting it must not touch the stored resource server {:?}, got {read}",
                other.case
            );
        }
    }

    let served = listed_identifiers(&router, &user_pool_id).await;
    assert!(
        served.is_empty(),
        "once every resource server is deleted the pool holds none, got {served:?}"
    );
}

#[tokio::test]
async fn a_pair_that_names_no_stored_resource_server_is_a_400_that_stores_nothing() {
    let router = router();
    let user_pool_id = pool(&router, "delete-rs-notfound-pool").await;
    let other_pool = pool(&router, "delete-rs-other-pool").await;

    for case in delete_cases() {
        let label = case.case;
        ok(
            &router,
            "CreateResourceServer",
            &case.create_request(&user_pool_id),
        )
        .await;

        // An identifier differing only in case is a different resource server
        // (`ResourceServerIdentifierType` carries no case-insensitivity trait),
        // and so is the same identifier in another pool.
        let upper = case.identifier.to_uppercase();
        let mut rows = vec![
            (
                "an absent pool".to_string(),
                json!({ "UserPoolId": "us-east-1_absent", "Identifier": case.identifier }),
                "User pool us-east-1_absent does not exist.".to_string(),
            ),
            (
                "an identifier the pool does not hold".to_string(),
                json!({ "UserPoolId": user_pool_id, "Identifier": "absent-identifier" }),
                format!("absent-identifier does not exist in user pool {user_pool_id}."),
            ),
            (
                "the identifier of another pool".to_string(),
                json!({ "UserPoolId": other_pool, "Identifier": case.identifier }),
                format!(
                    "{} does not exist in user pool {other_pool}.",
                    case.identifier
                ),
            ),
        ];
        if upper != case.identifier {
            rows.push((
                "an identifier differing only in case".to_string(),
                json!({ "UserPoolId": user_pool_id, "Identifier": upper.clone() }),
                format!("{upper} does not exist in user pool {user_pool_id}."),
            ));
        }

        for (row, body, expected_message) in rows {
            let (status, payload) = call(&router, "DeleteResourceServer", &body).await;
            assert_eq!(
                (status, error_code(&payload)),
                (400, "ResourceNotFoundException"),
                "case {label:?} / {row:?}: a pair naming no stored resource server is a 400 ResourceNotFoundException, got {payload}"
            );
            assert_eq!(
                error_message(&payload),
                expected_message,
                "case {label:?} / {row:?}: the refusal names the identifier and the pool it was looked for in, got {payload}"
            );
        }

        // The refused calls stored nothing: the resource server is still there,
        // and is deleted exactly once.
        ok(
            &router,
            "DescribeResourceServer",
            &json!({ "UserPoolId": user_pool_id, "Identifier": case.identifier }),
        )
        .await;
        ok(
            &router,
            "DeleteResourceServer",
            &json!({ "UserPoolId": user_pool_id, "Identifier": case.identifier }),
        )
        .await;

        // A second delete of the same pair names no stored resource server, so
        // it is the same 400 as any other miss.
        let (status, payload) = call(
            &router,
            "DeleteResourceServer",
            &json!({ "UserPoolId": user_pool_id, "Identifier": case.identifier }),
        )
        .await;
        assert_eq!(
            (status, error_code(&payload)),
            (400, "ResourceNotFoundException"),
            "case {label:?}: a repeated delete is a 400 ResourceNotFoundException, got {payload}"
        );
        assert_eq!(
            error_message(&payload),
            format!(
                "{} does not exist in user pool {user_pool_id}.",
                case.identifier
            ),
            "case {label:?}: the repeated delete's refusal names the identifier and the pool, got {payload}"
        );
    }
}

#[tokio::test]
async fn a_delete_is_keyed_by_the_pool_and_frees_the_identifier() {
    let router = router();
    let first_pool = pool(&router, "delete-rs-first-pool").await;
    let second_pool = pool(&router, "delete-rs-second-pool").await;

    for case in delete_cases() {
        let label = case.case;
        for user_pool_id in [&first_pool, &second_pool] {
            ok(
                &router,
                "CreateResourceServer",
                &case.create_request(user_pool_id),
            )
            .await;
        }

        ok(
            &router,
            "DeleteResourceServer",
            &json!({ "UserPoolId": first_pool, "Identifier": case.identifier }),
        )
        .await;

        // The other pool's resource server under the same identifier survives.
        let read = ok(
            &router,
            "DescribeResourceServer",
            &json!({ "UserPoolId": second_pool, "Identifier": case.identifier }),
        )
        .await;
        assert_eq!(
            read["ResourceServer"]["UserPoolId"],
            json!(second_pool),
            "case {label:?}: a delete only removes the resource server of the pool it names, got {read}"
        );

        // The deleted record is really gone, so the identifier is free again:
        // a create under it succeeds and stores the NEW name and scopes.
        let recreated = ok(
            &router,
            "CreateResourceServer",
            &json!({
                "UserPoolId": first_pool,
                "Identifier": case.identifier,
                "Name": "recreated after a delete",
                "Scopes": [{ "ScopeName": "fresh", "ScopeDescription": "a fresh scope" }],
            }),
        )
        .await;
        let read = ok(
            &router,
            "DescribeResourceServer",
            &json!({ "UserPoolId": first_pool, "Identifier": case.identifier }),
        )
        .await;
        assert_eq!(
            read["ResourceServer"], recreated["ResourceServer"],
            "case {label:?}: a resource server created after the delete is the one that is stored, got {read}"
        );
        assert_eq!(
            read["ResourceServer"]["Scopes"],
            json!([{ "ScopeName": "fresh", "ScopeDescription": "a fresh scope" }]),
            "case {label:?}: the re-created resource server holds its own scopes, not the deleted one's, got {read}"
        );
    }
}

#[tokio::test]
async fn after_a_delete_the_deleted_servers_scopes_no_longer_authorize_a_client_token() {
    let router = router();
    let user_pool_id = pool(&router, "delete-rs-scope-pool").await;

    // Two resource servers, so a delete of one leaves the other's scope
    // authorizable: the consequence is scoped to the deleted record.
    for (identifier, scope) in [
        ("https://orders.example.test", "place"),
        ("urn:billing", "invoice.read"),
    ] {
        ok(
            &router,
            "CreateResourceServer",
            &json!({
                "UserPoolId": user_pool_id,
                "Identifier": identifier,
                "Name": format!("resource server {scope}"),
                "Scopes": [{ "ScopeName": scope, "ScopeDescription": format!("the {scope} scope") }],
            }),
        )
        .await;
    }

    let allowed_scopes = json!(["https://orders.example.test/place", "urn:billing/invoice.read"]);
    let client = ok(
        &router,
        "CreateUserPoolClient",
        &json!({
            "UserPoolId": user_pool_id,
            "ClientName": "m2m-client",
            "ClientSecret": CLIENT_SECRET,
            "ExplicitAuthFlows": ["ALLOW_CLIENT_TOKEN_AUTH"],
            "AllowedOAuthFlows": ["client_credentials"],
            "AllowedOAuthFlowsUserPoolClient": true,
            "AllowedOAuthScopes": allowed_scopes,
        }),
    )
    .await;
    let client_id = client["UserPoolClient"]["ClientId"]
        .as_str()
        .expect("a minted client id")
        .to_string();

    // Before the delete both scopes are defined by a resource server of the
    // pool, so the grant serves them.
    for scope in [
        "https://orders.example.test/place",
        "urn:billing/invoice.read",
    ] {
        ok(
            &router,
            "GetClientToken",
            &json!({ "ClientId": client_id, "Secret": CLIENT_SECRET, "Scopes": [scope] }),
        )
        .await;
    }

    ok(
        &router,
        "DeleteResourceServer",
        &json!({ "UserPoolId": user_pool_id, "Identifier": "https://orders.example.test" }),
    )
    .await;

    // The deleted resource server's scope is no longer one the pool defines.
    let (status, payload) = call(
        &router,
        "GetClientToken",
        &json!({
            "ClientId": client_id,
            "Secret": CLIENT_SECRET,
            "Scopes": ["https://orders.example.test/place"],
        }),
    )
    .await;
    assert_eq!(
        (status, error_code(&payload)),
        (400, "NotAuthorizedException"),
        "a deleted resource server's scope can no longer be authorized, got {status} with {payload}"
    );

    // The surviving resource server's scope still is.
    ok(
        &router,
        "GetClientToken",
        &json!({
            "ClientId": client_id,
            "Secret": CLIENT_SECRET,
            "Scopes": ["urn:billing/invoice.read"],
        }),
    )
    .await;

    // And the delete wrote nothing outside the resource server record: the app
    // client keeps the `AllowedOAuthScopes` it was created with.
    let described = ok(
        &router,
        "DescribeUserPoolClient",
        &json!({ "UserPoolId": user_pool_id, "ClientId": client_id }),
    )
    .await;
    assert_eq!(
        described["UserPoolClient"]["AllowedOAuthScopes"], allowed_scopes,
        "a delete must not touch an app client's own configuration, got {described}"
    );
}
