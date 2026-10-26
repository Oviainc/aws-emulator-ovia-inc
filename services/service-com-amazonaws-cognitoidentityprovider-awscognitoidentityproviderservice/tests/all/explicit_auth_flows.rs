// SPDX-License-Identifier: CC0-1.0

//! `ExplicitAuthFlows` on `CreateUserPoolClient` and `UpdateUserPoolClient`, over
//! the wire through the router exactly as an SDK drives it (`POST /` with
//! `X-Amz-Target: AWSCognitoIdentityProviderService.<Op>`), read back through
//! `DescribeUserPoolClient`.
//!
//! The rules under test:
//!
//! * EVERY value `ExplicitAuthFlowsType` declares is accepted by the two
//!   operations that configure an app client, and the stored list round-trips
//!   unchanged into the response and into a later read. The table below is that
//!   whole value set, one row per member, so no member is refused as unknown.
//! * The one value the model documents as exclusive — `ALLOW_CLIENT_TOKEN_AUTH`,
//!   which "is mutually exclusive with user authentication flows. It must be the
//!   only authentication flow that you configure for the app client"
//!   (`GetClientToken` in this service's model) — is refused with
//!   `InvalidParameterException` when a request names it beside any other flow,
//!   and a refused update leaves the stored app client as it was.
//! * A value the enum does NOT declare is still refused, with the accepted set in
//!   the message.
//!
//! Each row names the case it asserts and every assertion reports the actual
//! value.

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// A request's SigV4 credential scope is what routes it to this service. The
/// signature is never checked here: the suite turns request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/cognito-idp/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0000";

/// Every value `ExplicitAuthFlowsType` declares, in the order the model lists
/// them (`smithy_query(query="shape", shape="ExplicitAuthFlowsType")` on
/// `smithy-model/com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice.json`).
const MODELLED_AUTH_FLOWS: &[&str] = &[
    "ADMIN_NO_SRP_AUTH",
    "CUSTOM_AUTH_FLOW_ONLY",
    "USER_PASSWORD_AUTH",
    "ALLOW_ADMIN_USER_PASSWORD_AUTH",
    "ALLOW_CUSTOM_AUTH",
    "ALLOW_USER_PASSWORD_AUTH",
    "ALLOW_USER_SRP_AUTH",
    "ALLOW_REFRESH_TOKEN_AUTH",
    "ALLOW_USER_AUTH",
    "ALLOW_CLIENT_TOKEN_AUTH",
];

/// The flow the model documents as the only one an app client that uses it may
/// hold (`GetClientToken` in this service's model).
const EXCLUSIVE_FLOW: &str = "ALLOW_CLIENT_TOKEN_AUTH";

/// The message a configuration that breaks that documented exclusivity owes: the
/// rule as `GetClientToken` states it in the service model, word for word.
const EXCLUSIVITY_MESSAGE: &str = "The ALLOW_CLIENT_TOKEN_AUTH flow is mutually exclusive with \
     user authentication flows. It must be the only authentication flow that you configure for \
     the app client.";

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
        .or_else(|| payload.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
}

/// A created user pool's id.
async fn created_pool(router: &MultiServiceRouter) -> String {
    ok(
        router,
        "CreateUserPool",
        &json!({ "PoolName": "explicit-auth-flows-pool" }),
    )
    .await["UserPool"]["Id"]
        .as_str()
        .expect("a minted pool id")
        .to_string()
}

/// The app client `client_id` names, as `DescribeUserPoolClient` renders it.
async fn described_client(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    client_id: &str,
) -> Value {
    ok(
        router,
        "DescribeUserPoolClient",
        &json!({ "UserPoolId": user_pool_id, "ClientId": client_id }),
    )
    .await["UserPoolClient"]
        .clone()
}

/// A `CreateUserPoolClient` request naming `flows`, with a generated secret so a
/// machine-to-machine configuration is reachable on every row.
fn create_request(user_pool_id: &str, client_name: &str, flows: &[&str]) -> Value {
    json!({
        "UserPoolId": user_pool_id,
        "ClientName": client_name,
        "GenerateSecret": true,
        "ExplicitAuthFlows": flows,
    })
}

#[tokio::test]
async fn every_modelled_auth_flow_is_accepted_and_round_trips() {
    let router = router();
    let user_pool_id = created_pool(&router).await;

    // One row per declared value, plus rows that vary the list's length and the
    // order of its entries — what the rule generalises over.
    let mut rows: Vec<(String, Vec<&str>)> = MODELLED_AUTH_FLOWS
        .iter()
        .map(|flow| (format!("the single flow {flow}"), vec![*flow]))
        .collect();
    rows.push((
        "no ExplicitAuthFlows member at all: the documented defaults".to_string(),
        Vec::new(),
    ));
    rows.push((
        "the exclusive flow repeated: the list still names no other flow".to_string(),
        vec![EXCLUSIVE_FLOW, EXCLUSIVE_FLOW],
    ));
    rows.push((
        "three user flows together".to_string(),
        vec![
            "ALLOW_USER_SRP_AUTH",
            "ALLOW_REFRESH_TOKEN_AUTH",
            "ALLOW_CUSTOM_AUTH",
        ],
    ));
    rows.push((
        "the same three in another order".to_string(),
        vec![
            "ALLOW_CUSTOM_AUTH",
            "ALLOW_USER_SRP_AUTH",
            "ALLOW_REFRESH_TOKEN_AUTH",
        ],
    ));

    for (index, (case, flows)) in rows.iter().enumerate() {
        let client_name = format!("flow-client-{index}");
        // An empty row stands for "the member is absent", which is a different
        // request from one carrying an empty list.
        let expected: Value = if flows.is_empty() {
            json!([
                "ALLOW_REFRESH_TOKEN_AUTH",
                "ALLOW_USER_SRP_AUTH",
                "ALLOW_CUSTOM_AUTH"
            ])
        } else {
            json!(flows)
        };
        let request = if flows.is_empty() {
            json!({
                "UserPoolId": user_pool_id,
                "ClientName": client_name,
                "GenerateSecret": true,
            })
        } else {
            create_request(&user_pool_id, &client_name, flows)
        };

        let (status, payload) = call(&router, "CreateUserPoolClient", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateUserPoolClient must answer 200, got {status} with {payload}"
        );
        let created = payload["UserPoolClient"].clone();
        assert_eq!(
            created.get("ExplicitAuthFlows"),
            Some(&expected),
            "case {case:?}: the created client must carry ExplicitAuthFlows {expected}, got {:?}",
            created.get("ExplicitAuthFlows")
        );
        let client_id = created["ClientId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: a created client carries a ClientId: {created}"))
            .to_string();
        let secret = created.get("ClientSecret").and_then(Value::as_str);
        assert!(
            secret.is_some_and(|secret| !secret.is_empty()),
            "case {case:?}: GenerateSecret=true must mint a ClientSecret, got {secret:?}"
        );

        // The response is the STORED client, so a later read renders the same list.
        let described = described_client(&router, &user_pool_id, &client_id).await;
        assert_eq!(
            described.get("ExplicitAuthFlows"),
            Some(&expected),
            "case {case:?}: DescribeUserPoolClient must render ExplicitAuthFlows {expected}, got {:?}",
            described.get("ExplicitAuthFlows")
        );
        assert_eq!(
            described.get("ClientSecret").and_then(Value::as_str),
            secret,
            "case {case:?}: the stored ClientSecret must survive the read, got {:?}",
            described.get("ClientSecret")
        );

        // The same value set is what an update may store, member for member. The
        // absent-member row has no update leg: what an update carrying no
        // `ExplicitAuthFlows` stores is the default rule, which
        // `tests/all/update_user_pool_client.rs` already pins, and the model says
        // nothing about an explicitly EMPTY list.
        if !flows.is_empty() {
            let updated = ok(
                &router,
                "UpdateUserPoolClient",
                &json!({
                    "UserPoolId": user_pool_id,
                    "ClientId": client_id,
                    "ExplicitAuthFlows": json!(flows),
                }),
            )
            .await["UserPoolClient"]
                .clone();
            assert_eq!(
                updated.get("ExplicitAuthFlows"),
                Some(&expected),
                "case {case:?}: UpdateUserPoolClient must store ExplicitAuthFlows {expected}, got {:?}",
                updated.get("ExplicitAuthFlows")
            );
        }
    }
}

#[tokio::test]
async fn a_machine_to_machine_app_client_is_created_with_its_secret_and_scopes() {
    let router = router();
    let user_pool_id = created_pool(&router).await;
    let resource_server = "https://api.example.com";
    ok(
        &router,
        "CreateResourceServer",
        &json!({
            "UserPoolId": user_pool_id,
            "Identifier": resource_server,
            "Name": "example api",
            "Scopes": [{ "ScopeName": "read", "ScopeDescription": "read access" }],
        }),
    )
    .await;
    let scope = format!("{resource_server}/read");

    let created = ok(
        &router,
        "CreateUserPoolClient",
        &json!({
            "UserPoolId": user_pool_id,
            "ClientName": "m2m",
            "GenerateSecret": true,
            "ExplicitAuthFlows": [EXCLUSIVE_FLOW],
            "AllowedOAuthFlows": ["client_credentials"],
            "AllowedOAuthFlowsUserPoolClient": true,
            "AllowedOAuthScopes": [scope],
        }),
    )
    .await["UserPoolClient"]
        .clone();

    let case = "a client-credentials app client";
    let client_id = created["ClientId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: a created client carries a ClientId: {created}"))
        .to_string();
    let described = described_client(&router, &user_pool_id, &client_id).await;
    for (where_from, client) in [("the create response", &created), ("the read", &described)] {
        assert_eq!(
            client.get("ExplicitAuthFlows"),
            Some(&json!([EXCLUSIVE_FLOW])),
            "case {case:?}: {where_from} must carry the exclusive flow, got {:?}",
            client.get("ExplicitAuthFlows")
        );
        assert_eq!(
            client.get("AllowedOAuthFlows"),
            Some(&json!(["client_credentials"])),
            "case {case:?}: {where_from} must carry the client_credentials grant, got {:?}",
            client.get("AllowedOAuthFlows")
        );
        assert_eq!(
            client.get("AllowedOAuthScopes"),
            Some(&json!([scope])),
            "case {case:?}: {where_from} must carry the resource-server scope, got {:?}",
            client.get("AllowedOAuthScopes")
        );
        let secret = client.get("ClientSecret").and_then(Value::as_str);
        assert!(
            secret.is_some_and(|secret| !secret.is_empty()),
            "case {case:?}: {where_from} must carry a ClientSecret, got {secret:?}"
        );
    }
}

#[tokio::test]
async fn the_exclusive_flow_is_refused_beside_any_other_flow() {
    let router = router();
    let user_pool_id = created_pool(&router).await;

    // One row per other declared value, beside rows that vary the position of the
    // exclusive flow in the list and how many other flows the list names.
    let mut rows: Vec<(String, Vec<&str>)> = MODELLED_AUTH_FLOWS
        .iter()
        .filter(|flow| **flow != EXCLUSIVE_FLOW)
        .map(|flow| {
            (
                format!("the exclusive flow before {flow}"),
                vec![EXCLUSIVE_FLOW, *flow],
            )
        })
        .collect();
    rows.push((
        "the exclusive flow last".to_string(),
        vec!["ALLOW_USER_SRP_AUTH", EXCLUSIVE_FLOW],
    ));
    rows.push((
        "the exclusive flow in the middle of three".to_string(),
        vec!["ALLOW_REFRESH_TOKEN_AUTH", EXCLUSIVE_FLOW, "ALLOW_USER_AUTH"],
    ));
    rows.push((
        "the exclusive flow twice beside another".to_string(),
        vec![EXCLUSIVE_FLOW, EXCLUSIVE_FLOW, "ALLOW_CUSTOM_AUTH"],
    ));

    // One app client already configured for machine-to-machine authorization, so
    // every refused update can be shown to have stored nothing.
    let stored = ok(
        &router,
        "CreateUserPoolClient",
        &create_request(&user_pool_id, "stored-m2m", &[EXCLUSIVE_FLOW]),
    )
    .await["UserPoolClient"]
        .clone();
    let stored_client_id = stored["ClientId"]
        .as_str()
        .expect("a minted client id")
        .to_string();

    for (index, (case, flows)) in rows.iter().enumerate() {
        let create = create_request(&user_pool_id, &format!("mixed-{index}"), flows);
        let (status, payload) = call(&router, "CreateUserPoolClient", &create).await;
        assert_eq!(
            status, 400,
            "case {case:?}: CreateUserPoolClient must answer 400, got {status} with {payload}"
        );
        assert!(
            error_code(&payload).ends_with("InvalidParameterException"),
            "case {case:?}: expected InvalidParameterException, got {:?} ({payload})",
            error_code(&payload)
        );
        assert_eq!(
            error_message(&payload),
            EXCLUSIVITY_MESSAGE,
            "case {case:?}: expected the documented exclusivity message, got {:?}",
            error_message(&payload)
        );

        let (status, payload) = call(
            &router,
            "UpdateUserPoolClient",
            &json!({
                "UserPoolId": user_pool_id,
                "ClientId": stored_client_id,
                "ExplicitAuthFlows": flows,
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: UpdateUserPoolClient must answer 400, got {status} with {payload}"
        );
        assert_eq!(
            error_message(&payload),
            EXCLUSIVITY_MESSAGE,
            "case {case:?}: expected the documented exclusivity message, got {:?}",
            error_message(&payload)
        );

        // A refused update stored nothing: the app client still holds the
        // configuration it was created with.
        let described = described_client(&router, &user_pool_id, &stored_client_id).await;
        assert_eq!(
            described.get("ExplicitAuthFlows"),
            Some(&json!([EXCLUSIVE_FLOW])),
            "case {case:?}: the refused update must leave the stored flows alone, got {:?}",
            described.get("ExplicitAuthFlows")
        );
    }

    // The precedence these two paths chose, pinned rather than implied: a request
    // whose own parameters contradict each other is refused as a parameter error
    // even when it also names a resource that does not exist, the way the
    // framework's modelled constraint checks refuse before a handler runs at all.
    let conflicting = json!([EXCLUSIVE_FLOW, "ALLOW_USER_SRP_AUTH"]);
    for (case, operation, request) in [
        (
            "an update naming an app client that does not exist",
            "UpdateUserPoolClient",
            json!({
                "UserPoolId": user_pool_id,
                "ClientId": "nosuchclient",
                "ExplicitAuthFlows": conflicting,
            }),
        ),
        (
            "a create naming a user pool that does not exist",
            "CreateUserPoolClient",
            json!({
                "UserPoolId": "us-east-1_nosuchpool",
                "ClientName": "absent-pool-client",
                "ExplicitAuthFlows": conflicting,
            }),
        ),
    ] {
        let (status, payload) = call(&router, operation, &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: {operation} must answer 400, got {status} with {payload}"
        );
        assert!(
            error_code(&payload).ends_with("InvalidParameterException"),
            "case {case:?}: expected InvalidParameterException, got {:?} ({payload})",
            error_code(&payload)
        );
        assert_eq!(
            error_message(&payload),
            EXCLUSIVITY_MESSAGE,
            "case {case:?}: expected the documented exclusivity message, got {:?}",
            error_message(&payload)
        );
    }
}

#[tokio::test]
async fn a_flow_the_model_does_not_declare_is_still_refused() {
    let router = router();
    let user_pool_id = created_pool(&router).await;

    // Rows differ along the form of the value the enum does not declare: a
    // near-miss of a declared member, the wrong case, an empty string, and a
    // declared member beside one that is not.
    let rows: &[(&str, Vec<&str>)] = &[
        ("a near miss of the exclusive flow", vec!["ALLOW_CLIENT_TOKEN"]),
        ("the exclusive flow lower-cased", vec!["allow_client_token_auth"]),
        ("an empty value", vec![""]),
        (
            "a declared member beside an undeclared one",
            vec!["ALLOW_USER_SRP_AUTH", "ALLOW_EVERYTHING"],
        ),
    ];

    for (index, (case, flows)) in rows.iter().enumerate() {
        let request = create_request(&user_pool_id, &format!("unknown-flow-{index}"), flows);
        let (status, payload) = call(&router, "CreateUserPoolClient", &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: CreateUserPoolClient must answer 400, got {status} with {payload}"
        );
        assert!(
            error_code(&payload).ends_with("InvalidParameterException"),
            "case {case:?}: expected InvalidParameterException, got {:?} ({payload})",
            error_code(&payload)
        );
        let message = error_message(&payload);
        assert!(
            message.contains("Member must satisfy enum value set"),
            "case {case:?}: expected the enum-constraint message, got {message:?}"
        );
        // The accepted set the refusal reports is the model's, which now names
        // the machine-to-machine flow.
        assert!(
            message.contains(EXCLUSIVE_FLOW),
            "case {case:?}: the accepted value set must name {EXCLUSIVE_FLOW}, got {message:?}"
        );
    }
}
