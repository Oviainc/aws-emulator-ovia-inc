// SPDX-License-Identifier: CC0-1.0

//! `UpdateResourceServer` over the wire, driven through the router exactly as an
//! SDK drives it (`POST /` with
//! `X-Amz-Target: AWSCognitoIdentityProviderService.<Op>`).
//!
//! The rule under test: `UpdateResourceServer` REPLACES the two writable members
//! of the stored resource server its (`UserPoolId`, `Identifier`) names — `Name`
//! and the custom scope list — with exactly what the request carries, setting a
//! member the request omits to its default (no custom scopes), leaves every
//! read-only field and every other resource untouched, and answers with the
//! stored record, so a later `DescribeResourceServer` renders the same thing. An
//! unknown pool or identifier is a 400 `ResourceNotFoundException` that stores
//! nothing. Each table row names the case it asserts.

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

/// The resource server `identifier` names in `user_pool_id`, as
/// `DescribeResourceServer` renders it.
async fn described(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    identifier: &str,
) -> Value {
    ok(
        router,
        "DescribeResourceServer",
        &json!({ "UserPoolId": user_pool_id, "Identifier": identifier }),
    )
    .await["ResourceServer"]
        .clone()
}

/// A scope list as the wire carries it.
fn scopes_json(scopes: &[(String, String)]) -> Value {
    Value::Array(
        scopes
            .iter()
            .map(|(scope_name, scope_description)| {
                json!({ "ScopeName": scope_name, "ScopeDescription": scope_description })
            })
            .collect(),
    )
}

/// One row of the `UpdateResourceServer` table: the `Name` the request sets and
/// the `Scopes` it carries. The rows differ along what the rule generalises
/// over — the form and length of the name, and the presence, count, order and
/// content of the scope list.
struct UpdateCase {
    case: &'static str,
    name: String,
    /// `None` leaves `Scopes` OUT of the request; `Some(list)` sends exactly it.
    scopes: Option<Vec<(String, String)>>,
}

impl UpdateCase {
    /// This case's `UpdateResourceServer` request: a `Scopes` the case leaves
    /// unset is left OUT of the request, not sent as null.
    fn request(&self, user_pool_id: &str, identifier: &str) -> Value {
        let mut body = Map::new();
        body.insert("UserPoolId".to_string(), json!(user_pool_id));
        body.insert("Identifier".to_string(), json!(identifier));
        body.insert("Name".to_string(), json!(self.name));
        if let Some(scopes) = &self.scopes {
            body.insert("Scopes".to_string(), scopes_json(scopes));
        }
        Value::Object(body)
    }

    /// What the updated resource server must render for `Scopes`: the submitted
    /// list, in order; and the default — an empty list — when the request
    /// carried none, because "if you don't provide a value for an attribute, it
    /// is set to the default value" (`UpdateResourceServer`).
    fn expected_scopes(&self) -> Value {
        match &self.scopes {
            Some(scopes) => scopes_json(scopes),
            None => json!([]),
        }
    }
}

/// A scope list of `count` entries, named so each case's content differs.
fn generated_scopes(count: usize, prefix: &str) -> Vec<(String, String)> {
    (0..count)
        .map(|index| {
            (
                format!("{prefix}{index}"),
                format!("{prefix} scope number {index}"),
            )
        })
        .collect()
}

/// The table, applied in order to ONE resource server so each row also asserts
/// that an update replaces what the row before it stored. `Name` spans its
/// modelled length 1..256 and pattern `^[\w\s+=,.@-]+$`, and `Scopes` its
/// modelled length 0..100 and its member's pattern
/// `^[\x21\x23-\x2E\x30-\x5B\x5D-\x7E]+$` — the constraints from
/// `smithy_query(query="operation",operation="UpdateResourceServer")` and
/// `smithy_query(query="shape",shape="ResourceServerScopeType")`.
fn update_cases() -> Vec<UpdateCase> {
    vec![
        UpdateCase {
            case: "a new name and one scope",
            name: "Solar system data v2".to_string(),
            scopes: Some(vec![(
                "asteroid.read".to_string(),
                "Read asteroid data".to_string(),
            )]),
        },
        UpdateCase {
            case: "several scopes, submitted out of alphabetical order",
            name: "solar+system,data@v3".to_string(),
            scopes: Some(vec![
                ("write".to_string(), "Write everything".to_string()),
                ("read".to_string(), "Read everything".to_string()),
                ("admin".to_string(), "Administer everything".to_string()),
            ]),
        },
        UpdateCase {
            case: "the same scope names with new descriptions only",
            name: "solar+system,data@v3".to_string(),
            scopes: Some(vec![
                ("write".to_string(), "Write, but revised".to_string()),
                ("read".to_string(), "Read, but revised".to_string()),
                ("admin".to_string(), "Administer, but revised".to_string()),
            ]),
        },
        UpdateCase {
            case: "an explicitly empty scope list, the modelled length minimum 0",
            name: "emptied".to_string(),
            scopes: Some(Vec::new()),
        },
        UpdateCase {
            case: "scopes carrying the punctuation its pattern admits",
            name: "punctuated = scopes".to_string(),
            scopes: Some(vec![
                ("tele!metry#1[2]~".to_string(), "Punctuated".to_string()),
                ("a".repeat(256), "A scope name at the length maximum".to_string()),
            ]),
        },
        UpdateCase {
            case: "100 scopes, the list maximum",
            name: "n".repeat(256),
            scopes: Some(generated_scopes(100, "bulk")),
        },
        UpdateCase {
            case: "no Scopes member at all, which resets them to the default",
            name: "N".to_string(),
            scopes: None,
        },
        UpdateCase {
            case: "scopes again, from the reset default",
            name: "restored".to_string(),
            scopes: Some(generated_scopes(3, "restored")),
        },
    ]
}

#[tokio::test]
async fn an_update_replaces_the_writable_members_and_round_trips_through_stored_state() {
    let router = router();
    let user_pool_id = pool(&router, "update-resource-server-pool").await;
    let identifier = "https://solar-system-data-api.example.com";
    let created = ok(
        &router,
        "CreateResourceServer",
        &json!({
            "UserPoolId": user_pool_id,
            "Identifier": identifier,
            "Name": "Solar system data",
            "Scopes": [{ "ScopeName": "planets.read", "ScopeDescription": "Read planet data" }],
        }),
    )
    .await;
    assert_eq!(
        created["ResourceServer"]["Name"],
        json!("Solar system data"),
        "the created resource server is the starting point, got {created}"
    );

    for case in update_cases() {
        let label = case.case;
        let updated = ok(
            &router,
            "UpdateResourceServer",
            &case.request(&user_pool_id, identifier),
        )
        .await;
        let resource_server = &updated["ResourceServer"];

        assert_eq!(
            resource_server["Name"],
            json!(case.name),
            "case {label:?}: the updated name must be the submitted one, got {resource_server}"
        );
        assert_eq!(
            resource_server["Scopes"],
            case.expected_scopes(),
            "case {label:?}: the scopes must be replaced by the submitted list, in order, got {resource_server}"
        );
        // The read-only fields: identity never moves.
        assert_eq!(
            resource_server["UserPoolId"],
            json!(user_pool_id),
            "case {label:?}: the resource server must still name its pool, got {resource_server}"
        );
        assert_eq!(
            resource_server["Identifier"],
            json!(identifier),
            "case {label:?}: the identifier is read-only, got {resource_server}"
        );

        // The round trip: the reader renders the STORED record, member for
        // member, and reading it twice changes nothing.
        let read = described(&router, &user_pool_id, identifier).await;
        assert_eq!(
            read, *resource_server,
            "case {label:?}: DescribeResourceServer must render what the update stored, got {read}"
        );
        let again = described(&router, &user_pool_id, identifier).await;
        assert_eq!(
            again, read,
            "case {label:?}: a read must not change what the next read sees, got {again}"
        );
    }
}

#[tokio::test]
async fn an_update_touches_only_the_resource_server_its_pool_and_identifier_name() {
    let router = router();
    let first_pool = pool(&router, "update-rs-first-pool").await;
    let second_pool = pool(&router, "update-rs-second-pool").await;

    // The same identifier in two pools, plus a sibling in the first pool: three
    // records, and an update must move exactly one of them.
    for (user_pool_id, identifier) in [
        (&first_pool, "shared-identifier"),
        (&first_pool, "sibling-identifier"),
        (&second_pool, "shared-identifier"),
    ] {
        ok(
            &router,
            "CreateResourceServer",
            &json!({
                "UserPoolId": user_pool_id,
                "Identifier": identifier,
                "Name": "before the update",
                "Scopes": [{ "ScopeName": "untouched", "ScopeDescription": "Untouched" }],
            }),
        )
        .await;
    }

    ok(
        &router,
        "UpdateResourceServer",
        &json!({
            "UserPoolId": first_pool,
            "Identifier": "shared-identifier",
            "Name": "after the update",
            "Scopes": [{ "ScopeName": "touched", "ScopeDescription": "Touched" }],
        }),
    )
    .await;

    for (label, user_pool_id, identifier, expected_name, expected_scope) in [
        (
            "the updated resource server",
            &first_pool,
            "shared-identifier",
            "after the update",
            "touched",
        ),
        (
            "its sibling in the same pool",
            &first_pool,
            "sibling-identifier",
            "before the update",
            "untouched",
        ),
        (
            "the same identifier in another pool",
            &second_pool,
            "shared-identifier",
            "before the update",
            "untouched",
        ),
    ] {
        let read = described(&router, user_pool_id, identifier).await;
        assert_eq!(
            read["Name"],
            json!(expected_name),
            "case {label:?}: expected the name {expected_name:?}, got {read}"
        );
        assert_eq!(
            read["Scopes"][0]["ScopeName"],
            json!(expected_scope),
            "case {label:?}: expected the scope {expected_scope:?}, got {read}"
        );
    }
}

#[tokio::test]
async fn updating_a_resource_server_leaves_every_app_clients_allowed_oauth_scopes_alone() {
    let router = router();
    let user_pool_id = pool(&router, "update-rs-client-pool").await;
    let identifier = "solar-system-data";
    ok(
        &router,
        "CreateResourceServer",
        &json!({
            "UserPoolId": user_pool_id,
            "Identifier": identifier,
            "Name": "Solar system data",
            "Scopes": [
                { "ScopeName": "planets.read", "ScopeDescription": "Read planet data" },
                { "ScopeName": "planets.write", "ScopeDescription": "Write planet data" },
            ],
        }),
    )
    .await;

    // An app client's `AllowedOAuthScopes` is that client's own configuration,
    // so each of these survives every shape of resource-server update below.
    let mut clients = Vec::new();
    for (label, requested_scopes) in [
        (
            "a client allowing both custom scopes",
            json!([
                format!("{identifier}/planets.read"),
                format!("{identifier}/planets.write"),
            ]),
        ),
        (
            "a client allowing one custom scope beside a standard one",
            json!(["openid", format!("{identifier}/planets.read")]),
        ),
        (
            "a client allowing only standard scopes",
            json!(["openid", "email"]),
        ),
    ] {
        let created = ok(
            &router,
            "CreateUserPoolClient",
            &json!({
                "UserPoolId": user_pool_id,
                "ClientName": label,
                "AllowedOAuthFlows": ["client_credentials"],
                "AllowedOAuthFlowsUserPoolClient": true,
                "AllowedOAuthScopes": requested_scopes,
            }),
        )
        .await;
        let client_id = created["UserPoolClient"]["ClientId"]
            .as_str()
            .expect("a minted client id")
            .to_string();
        clients.push((label, client_id, requested_scopes));
    }

    // Rename the resource server, drop a scope, empty the list, then restore it:
    // no shape of update is allowed to cascade into an app client.
    for (label, body) in [
        (
            "a rename that drops one scope",
            json!({
                "UserPoolId": user_pool_id,
                "Identifier": identifier,
                "Name": "Solar system data, renamed",
                "Scopes": [{ "ScopeName": "planets.read", "ScopeDescription": "Read planet data" }],
            }),
        ),
        (
            "an update that empties the scope list",
            json!({
                "UserPoolId": user_pool_id,
                "Identifier": identifier,
                "Name": "Solar system data, emptied",
                "Scopes": [],
            }),
        ),
        (
            "an update carrying no Scopes member at all",
            json!({
                "UserPoolId": user_pool_id,
                "Identifier": identifier,
                "Name": "Solar system data, defaulted",
            }),
        ),
        (
            "an update that restores a differently named scope",
            json!({
                "UserPoolId": user_pool_id,
                "Identifier": identifier,
                "Name": "Solar system data, restored",
                "Scopes": [{ "ScopeName": "moons.read", "ScopeDescription": "Read moon data" }],
            }),
        ),
    ] {
        ok(&router, "UpdateResourceServer", &body).await;

        for (client_label, client_id, requested_scopes) in &clients {
            let read = ok(
                &router,
                "DescribeUserPoolClient",
                &json!({ "UserPoolId": user_pool_id, "ClientId": client_id }),
            )
            .await;
            assert_eq!(
                read["UserPoolClient"]["AllowedOAuthScopes"], *requested_scopes,
                "case {label:?} / {client_label:?}: a resource-server update must not change an app client's AllowedOAuthScopes, got {read}"
            );
        }
    }
}

#[tokio::test]
async fn an_absent_pool_or_identifier_is_a_400_and_stores_nothing() {
    let router = router();
    let user_pool_id = pool(&router, "update-rs-notfound-pool").await;
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

    for (label, body, expected_message) in [
        (
            "an absent pool",
            json!({
                "UserPoolId": "us-east-1_absent",
                "Identifier": "solar-system-data",
                "Name": "Solar system data",
            }),
            "User pool us-east-1_absent does not exist.".to_string(),
        ),
        (
            "an identifier the pool does not hold",
            json!({
                "UserPoolId": user_pool_id,
                "Identifier": "absent-identifier",
                "Name": "Absent identifier",
            }),
            format!("absent-identifier does not exist in user pool {user_pool_id}."),
        ),
        (
            "an identifier differing only in case from a stored one",
            json!({
                "UserPoolId": user_pool_id,
                "Identifier": "Solar-System-Data",
                "Name": "Capitalised",
            }),
            format!("Solar-System-Data does not exist in user pool {user_pool_id}."),
        ),
    ] {
        let (status, payload) = call(&router, "UpdateResourceServer", &body).await;
        assert_eq!(
            (status, error_code(&payload)),
            (400, "ResourceNotFoundException"),
            "case {label:?}: must be a 400 ResourceNotFoundException, got {payload}"
        );
        // The message is built from the request and the store, never freely
        // worded: the thing that was not found, then the pool it was looked for
        // in.
        assert_eq!(
            error_message(&payload),
            expected_message,
            "case {label:?}: expected the message {expected_message:?}, got {:?}",
            error_message(&payload)
        );
    }

    // A refused update created nothing and moved nothing.
    let read = described(&router, &user_pool_id, "solar-system-data").await;
    assert_eq!(
        read["Name"],
        json!("Solar system data"),
        "a refused update must leave the stored resource server exactly as it was, got {read}"
    );
    let (status, payload) = call(
        &router,
        "DescribeResourceServer",
        &json!({ "UserPoolId": user_pool_id, "Identifier": "absent-identifier" }),
    )
    .await;
    assert_eq!(
        (status, error_code(&payload)),
        (400, "ResourceNotFoundException"),
        "a refused update must not have created a resource server, got {payload}"
    );
}

#[tokio::test]
async fn a_request_outside_a_modelled_constraint_is_refused_and_changes_nothing() {
    let router = router();
    let user_pool_id = pool(&router, "update-rs-constraint-pool").await;
    let identifier = "solar-system-data";
    let created = ok(
        &router,
        "CreateResourceServer",
        &json!({
            "UserPoolId": user_pool_id,
            "Identifier": identifier,
            "Name": "Solar system data",
            "Scopes": [{ "ScopeName": "planets.read", "ScopeDescription": "Read planet data" }],
        }),
    )
    .await["ResourceServer"]
        .clone();

    // One value past each published bound of `UpdateResourceServer`'s input
    // (`smithy_query(query="operation",operation="UpdateResourceServer")`): the
    // name's length 1..256 and pattern, the identifier's length 1..256 and
    // pattern, the user pool id's pattern, and the scope list's length 0..100
    // with its member's own length bounds.
    let over_long_scopes: Vec<Value> = (0..101)
        .map(|index| json!({ "ScopeName": format!("scope{index}"), "ScopeDescription": "s" }))
        .collect();
    for (label, body) in [
        (
            "an empty name, below the length minimum 1",
            json!({ "UserPoolId": user_pool_id, "Identifier": identifier, "Name": "" }),
        ),
        (
            "a name of 257 characters, past the length maximum 256",
            json!({
                "UserPoolId": user_pool_id,
                "Identifier": identifier,
                "Name": "n".repeat(257),
            }),
        ),
        (
            "a name holding a slash, which the pattern excludes",
            json!({
                "UserPoolId": user_pool_id,
                "Identifier": identifier,
                "Name": "not/allowed",
            }),
        ),
        (
            "an empty identifier, below the length minimum 1",
            json!({ "UserPoolId": user_pool_id, "Identifier": "", "Name": "empty identifier" }),
        ),
        (
            "an identifier holding a space, which the pattern excludes",
            json!({
                "UserPoolId": user_pool_id,
                "Identifier": "two words",
                "Name": "spaced identifier",
            }),
        ),
        (
            "a user pool id the pattern excludes",
            json!({
                "UserPoolId": "no-underscore",
                "Identifier": identifier,
                "Name": "bad pool id",
            }),
        ),
        (
            "101 scopes, past the list maximum 100",
            json!({
                "UserPoolId": user_pool_id,
                "Identifier": identifier,
                "Name": "too many scopes",
                "Scopes": over_long_scopes,
            }),
        ),
        (
            "a scope with an empty description, below its length minimum 1",
            json!({
                "UserPoolId": user_pool_id,
                "Identifier": identifier,
                "Name": "empty scope description",
                "Scopes": [{ "ScopeName": "planets.read", "ScopeDescription": "" }],
            }),
        ),
        (
            "a scope name of 257 characters, past its length maximum 256",
            json!({
                "UserPoolId": user_pool_id,
                "Identifier": identifier,
                "Name": "over-long scope name",
                "Scopes": [{ "ScopeName": "s".repeat(257), "ScopeDescription": "s" }],
            }),
        ),
    ] {
        let (status, payload) = call(&router, "UpdateResourceServer", &body).await;
        assert_eq!(
            status, 400,
            "case {label:?}: a request outside a modelled constraint is a 400, got {status} with {payload}"
        );
        assert_ne!(
            error_code(&payload),
            "NotImplemented",
            "case {label:?}: a bad request is never answered with the scaffold's 501, got {payload}"
        );

        let read = described(&router, &user_pool_id, identifier).await;
        assert_eq!(
            read, created,
            "case {label:?}: a refused update must leave the stored resource server untouched, got {read}"
        );
    }
}
