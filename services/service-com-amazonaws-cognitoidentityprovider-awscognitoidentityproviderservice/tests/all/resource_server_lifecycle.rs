// SPDX-License-Identifier: CC0-1.0

//! OAuth 2.0 resource servers over the wire: `CreateResourceServer` stores a
//! resource server in the pool it names and every later `DescribeResourceServer`
//! renders the STORED resource server, driven through the router exactly as an
//! SDK drives it (`POST /` with
//! `X-Amz-Target: AWSCognitoIdentityProviderService.<Op>`).
//!
//! The rule under test: a created resource server is keyed by (`UserPoolId`,
//! `Identifier`), it renders `UserPoolId`, `Identifier`, `Name` and the custom
//! scopes exactly as submitted (an empty list when the request set none), a
//! second resource server under an identifier the pool already holds is refused
//! and leaves the stored one untouched, and an absent pool or identifier is a
//! 404. Each table row names the case it asserts.

use bytes::Bytes;
use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// A request's SigV4 credential scope is what routes it to this service. The
/// signature is never checked here: the suite turns request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/cognito-idp/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0000";

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

/// Create a pool and return its minted `UserPoolId`.
async fn pool(router: &MultiServiceRouter, name: &str) -> String {
    let created = ok(router, "CreateUserPool", &json!({ "PoolName": name })).await;
    created["UserPool"]["Id"]
        .as_str()
        .expect("a minted pool id")
        .to_string()
}

/// One row of the `CreateResourceServer` table: an identifier, a friendly name
/// and the scopes the request sets. The rows differ along what the rule
/// generalises over — the form and length of the identifier and the name, and
/// the count, order and content of the scope list.
struct ResourceServerCase {
    case: &'static str,
    identifier: String,
    name: String,
    /// `None` leaves `Scopes` OUT of the request; `Some(list)` sends exactly it.
    scopes: Option<Vec<(String, String)>>,
}

impl ResourceServerCase {
    /// This case's `CreateResourceServer` request for `user_pool_id`: a `Scopes`
    /// the case leaves unset is left OUT of the request, not sent as null.
    fn request(&self, user_pool_id: &str) -> Value {
        let mut body = Map::new();
        body.insert("UserPoolId".to_string(), json!(user_pool_id));
        body.insert("Identifier".to_string(), json!(self.identifier));
        body.insert("Name".to_string(), json!(self.name));
        if let Some(scopes) = &self.scopes {
            body.insert("Scopes".to_string(), self.scopes_json(scopes));
        }
        Value::Object(body)
    }

    /// The `Scopes` list as the wire carries it.
    fn scopes_json(&self, scopes: &[(String, String)]) -> Value {
        Value::Array(
            scopes
                .iter()
                .map(|(scope_name, scope_description)| {
                    json!({ "ScopeName": scope_name, "ScopeDescription": scope_description })
                })
                .collect(),
        )
    }

    /// What a stored resource server must render for `Scopes`: the submitted
    /// list, in order, and an empty list when the request set none.
    fn expected_scopes(&self) -> Value {
        match &self.scopes {
            Some(scopes) => self.scopes_json(scopes),
            None => json!([]),
        }
    }
}

/// The table. `Identifier` spans its modelled length 1..256 and pattern
/// `^[\x21\x23-\x5B\x5D-\x7E]+$` (an API friendly name, an API URL, punctuation),
/// `Name` its length 1..256 and pattern `^[\w\s+=,.@-]+$` (spaces and the
/// permitted symbols), and `Scopes` its modelled length 0..100 (unset, empty,
/// one, several out of alphabetical order, and the maximum 100) — every
/// constraint from
/// `smithy_query(query="operation",operation="CreateResourceServer")`.
fn resource_server_cases() -> Vec<ResourceServerCase> {
    vec![
        ResourceServerCase {
            case: "an API friendly identifier with no Scopes member at all",
            identifier: "solar-system-data".to_string(),
            name: "Solar system data".to_string(),
            scopes: None,
        },
        ResourceServerCase {
            case: "an explicitly empty scope list, the modelled length minimum 0",
            identifier: "https://solar-system-data-api.example.com".to_string(),
            name: "Solar system API".to_string(),
            scopes: Some(Vec::new()),
        },
        ResourceServerCase {
            case: "one scope",
            identifier: "weather".to_string(),
            name: "weather+data,service@edge-1".to_string(),
            scopes: Some(vec![(
                "forecast.read".to_string(),
                "Read the forecast".to_string(),
            )]),
        },
        ResourceServerCase {
            case: "several scopes, submitted out of alphabetical order",
            identifier: "tele!metry#1[2]~".to_string(),
            name: "Telemetry = ingest".to_string(),
            scopes: Some(vec![
                ("write".to_string(), "Write telemetry".to_string()),
                ("read".to_string(), "Read telemetry".to_string()),
                ("admin".to_string(), "Administer telemetry".to_string()),
            ]),
        },
        ResourceServerCase {
            case: "identifier and name at the length maximum 256, 100 scopes (the list maximum)",
            identifier: "i".repeat(256),
            name: "n".repeat(256),
            scopes: Some(
                (0..100)
                    .map(|index| (format!("scope{index}"), format!("scope number {index}")))
                    .collect(),
            ),
        },
    ]
}

#[tokio::test]
async fn a_created_resource_server_round_trips_through_stored_state() {
    let router = router();
    let user_pool_id = pool(&router, "resource-server-pool").await;

    for case in resource_server_cases() {
        let label = case.case;
        let created = ok(&router, "CreateResourceServer", &case.request(&user_pool_id)).await;
        let resource_server = &created["ResourceServer"];

        assert_eq!(
            resource_server["UserPoolId"],
            json!(user_pool_id),
            "case {label:?}: the resource server must name the pool it was created in, got {resource_server}"
        );
        assert_eq!(
            resource_server["Identifier"],
            json!(case.identifier),
            "case {label:?}: the stored identifier must be the created one, got {resource_server}"
        );
        assert_eq!(
            resource_server["Name"],
            json!(case.name),
            "case {label:?}: the stored name must be the created one, got {resource_server}"
        );
        assert_eq!(
            resource_server["Scopes"],
            case.expected_scopes(),
            "case {label:?}: the stored scopes must be the submitted ones, in order, got {resource_server}"
        );

        // The round trip: the reader renders the STORED resource server, member
        // for member.
        let read = ok(
            &router,
            "DescribeResourceServer",
            &json!({ "UserPoolId": user_pool_id, "Identifier": case.identifier }),
        )
        .await;
        assert_eq!(
            read["ResourceServer"], created["ResourceServer"],
            "case {label:?}: DescribeResourceServer must render the stored resource server, got {read}"
        );

        // And reading it again changes nothing a later caller sees.
        let again = ok(
            &router,
            "DescribeResourceServer",
            &json!({ "UserPoolId": user_pool_id, "Identifier": case.identifier }),
        )
        .await;
        assert_eq!(
            again["ResourceServer"], read["ResourceServer"],
            "case {label:?}: a read must not change what the next read sees, got {again}"
        );
    }
}

#[tokio::test]
async fn an_identifier_the_pool_already_holds_is_refused_and_the_stored_resource_server_survives() {
    let router = router();
    let user_pool_id = pool(&router, "duplicate-identifier-pool").await;

    for case in resource_server_cases() {
        let label = case.case;
        let created = ok(&router, "CreateResourceServer", &case.request(&user_pool_id)).await;

        // A second resource server under the same identifier, with a DIFFERENT
        // name and scopes: refused, and the stored one is left exactly as it was.
        let (status, payload) = call(
            &router,
            "CreateResourceServer",
            &json!({
                "UserPoolId": user_pool_id,
                "Identifier": case.identifier,
                "Name": "a second resource server under a taken identifier",
                "Scopes": [{ "ScopeName": "other", "ScopeDescription": "another scope" }],
            }),
        )
        .await;
        assert_eq!(
            (status, error_code(&payload)),
            (400, "InvalidParameterException"),
            "case {label:?}: an identifier already taken is a 400 InvalidParameterException, got {payload}"
        );

        let read = ok(
            &router,
            "DescribeResourceServer",
            &json!({ "UserPoolId": user_pool_id, "Identifier": case.identifier }),
        )
        .await;
        assert_eq!(
            read["ResourceServer"], created["ResourceServer"],
            "case {label:?}: a refused create must not have touched the stored resource server, got {read}"
        );
    }
}

#[tokio::test]
async fn a_resource_server_is_keyed_by_its_pool_and_its_exact_identifier() {
    let router = router();
    let first_pool = pool(&router, "first-rs-pool").await;
    let second_pool = pool(&router, "second-rs-pool").await;

    // The same identifier in two pools is two resource servers, each naming its
    // own pool and keeping its own members.
    for (label, user_pool_id, name) in [
        ("the first pool", &first_pool, "first pool API"),
        ("the second pool", &second_pool, "second pool API"),
    ] {
        let created = ok(
            &router,
            "CreateResourceServer",
            &json!({
                "UserPoolId": user_pool_id,
                "Identifier": "shared-identifier",
                "Name": name,
            }),
        )
        .await;
        assert_eq!(
            created["ResourceServer"]["UserPoolId"],
            json!(user_pool_id),
            "case {label:?}: the resource server must name the pool it was created in, got {created}"
        );
    }
    for (label, user_pool_id, name) in [
        ("the first pool", &first_pool, "first pool API"),
        ("the second pool", &second_pool, "second pool API"),
    ] {
        let read = ok(
            &router,
            "DescribeResourceServer",
            &json!({ "UserPoolId": user_pool_id, "Identifier": "shared-identifier" }),
        )
        .await;
        assert_eq!(
            read["ResourceServer"]["Name"],
            json!(name),
            "case {label:?}: each pool's resource server keeps its own members, got {read}"
        );
    }

    // `ResourceServerIdentifierType` carries no case-insensitivity trait, so
    // identifiers differing only in case are different resource servers.
    let created = ok(
        &router,
        "CreateResourceServer",
        &json!({
            "UserPoolId": first_pool,
            "Identifier": "Shared-Identifier",
            "Name": "capitalised",
        }),
    )
    .await;
    assert_eq!(
        created["ResourceServer"]["Identifier"], "Shared-Identifier",
        "an identifier differing only in case is a different resource server, got {created}"
    );
    let read = ok(
        &router,
        "DescribeResourceServer",
        &json!({ "UserPoolId": first_pool, "Identifier": "shared-identifier" }),
    )
    .await;
    assert_eq!(
        read["ResourceServer"]["Name"],
        json!("first pool API"),
        "a resource server is matched by its exact identifier, got {read}"
    );
}

#[tokio::test]
async fn an_absent_pool_or_identifier_is_a_404() {
    let router = router();
    let user_pool_id = pool(&router, "notfound-rs-pool").await;
    ok(
        &router,
        "CreateResourceServer",
        &json!({
            "UserPoolId": user_pool_id,
            "Identifier": "solar-system-data",
            "Name": "Solar system data",
        }),
    )
    .await;

    for (label, operation, body) in [
        (
            "CreateResourceServer on an absent pool",
            "CreateResourceServer",
            json!({
                "UserPoolId": "us-east-1_absent",
                "Identifier": "solar-system-data",
                "Name": "Solar system data",
            }),
        ),
        (
            "DescribeResourceServer on an absent pool",
            "DescribeResourceServer",
            json!({ "UserPoolId": "us-east-1_absent", "Identifier": "solar-system-data" }),
        ),
        (
            "DescribeResourceServer for an identifier the pool does not hold",
            "DescribeResourceServer",
            json!({ "UserPoolId": user_pool_id, "Identifier": "absent-identifier" }),
        ),
    ] {
        let (status, payload) = call(&router, operation, &body).await;
        assert_eq!(
            (status, error_code(&payload)),
            (404, "ResourceNotFoundException"),
            "case {label:?}: must be a 404 ResourceNotFoundException, got {payload}"
        );
    }
}

#[tokio::test]
async fn a_request_outside_a_modelled_constraint_is_refused() {
    let router = router();
    let user_pool_id = pool(&router, "rs-constraint-pool").await;

    // One value past each published bound of `CreateResourceServer`'s input
    // (`smithy_query(query="operation",operation="CreateResourceServer")`): the
    // identifier's length 1..256 and pattern, the name's length 1..256 and
    // pattern, and the scope list's length 0..100.
    let over_long_scopes: Vec<Value> = (0..101)
        .map(|index| json!({ "ScopeName": format!("scope{index}"), "ScopeDescription": "s" }))
        .collect();
    for (label, body) in [
        (
            "an empty identifier, below the length minimum 1",
            json!({ "UserPoolId": user_pool_id, "Identifier": "", "Name": "empty-identifier" }),
        ),
        (
            "an identifier of 257 characters, past the length maximum 256",
            json!({
                "UserPoolId": user_pool_id,
                "Identifier": "i".repeat(257),
                "Name": "over-long-identifier",
            }),
        ),
        (
            "an identifier holding a space, which the pattern excludes",
            json!({
                "UserPoolId": user_pool_id,
                "Identifier": "two words",
                "Name": "spaced-identifier",
            }),
        ),
        (
            "an empty name, below the length minimum 1",
            json!({ "UserPoolId": user_pool_id, "Identifier": "empty-name", "Name": "" }),
        ),
        (
            "a name of 257 characters, past the length maximum 256",
            json!({
                "UserPoolId": user_pool_id,
                "Identifier": "over-long-name",
                "Name": "n".repeat(257),
            }),
        ),
        (
            "a name holding a slash, which the pattern excludes",
            json!({
                "UserPoolId": user_pool_id,
                "Identifier": "slashed-name",
                "Name": "not/allowed",
            }),
        ),
        (
            "101 scopes, past the list maximum 100",
            json!({
                "UserPoolId": user_pool_id,
                "Identifier": "over-long-scopes",
                "Name": "over-long-scopes",
                "Scopes": over_long_scopes,
            }),
        ),
        (
            "a user pool id the pattern excludes",
            json!({
                "UserPoolId": "no-underscore",
                "Identifier": "solar-system-data",
                "Name": "Solar system data",
            }),
        ),
    ] {
        let (status, payload) = call(&router, "CreateResourceServer", &body).await;
        assert_eq!(
            status, 400,
            "case {label:?}: a request outside a modelled constraint is a 400, got {status} with {payload}"
        );
        assert_ne!(
            error_code(&payload),
            "NotImplemented",
            "case {label:?}: a bad request is never answered with the scaffold's 501, got {payload}"
        );
    }

    // None of the refused requests stored anything.
    for (label, identifier) in [
        ("over-long name", "over-long-name"),
        ("over-long scope list", "over-long-scopes"),
        ("slashed name", "slashed-name"),
    ] {
        let (status, payload) = call(
            &router,
            "DescribeResourceServer",
            &json!({ "UserPoolId": user_pool_id, "Identifier": identifier }),
        )
        .await;
        assert_eq!(
            (status, error_code(&payload)),
            (404, "ResourceNotFoundException"),
            "case {label:?}: a refused create stores no resource server, got {payload}"
        );
    }
}
