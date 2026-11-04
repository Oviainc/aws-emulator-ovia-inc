// SPDX-License-Identifier: CC0-1.0

//! Two rules of the connection resource, end to end over the wire
//! (`awsJson1_1`, `POST /` with `X-Amz-Target: AWSEvents.<Operation>`) through
//! the real `MultiServiceRouter`.
//!
//! The RULES asserted here:
//!
//! * a listing refuses a request that names more than one of its mutually
//!   exclusive filters, and the refusal NAMES those members of the operation's
//!   own filter set — for `ListConnections` the two members that narrow it,
//!   `ConnectionState` and `NamePrefix`
//!   (`smithy_query(query="operation",operation="ListConnections")`) — after the
//!   clause naming the call that failed, so a caller is told WHICH parameters
//!   conflicted rather than that some pair of them did. A member that is not a
//!   filter (`Limit`, `NextToken`) is not part of that count, and either filter
//!   on its own still narrows the listing;
//! * `AuthorizationType` is what SELECTS which member of `AuthParameters`
//!   carries a connection's credentials, so an `UpdateConnection` that supplies
//!   `AuthParameters` without it names no type for them and is refused with a
//!   400 client error — whichever block it carried, the block of the type the
//!   connection already has included — and the connection is left exactly as it
//!   was, the authorization state it had included. The same request that names
//!   the type is accepted and its credentials land.
//!
//! Each table spans the class rather than one case: the filter rows differ along
//! which filters are named, how many, and whether a non-filter member is named
//! beside them; the update rows differ along the block of credentials submitted,
//! the type the connection already holds, and whether that connection holds
//! authorization or has had it stripped.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_eventbridge_awsevents::state;
use service_com_amazonaws_eventbridge_awsevents::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below is signed for.
const REGION: &str = "eu-west-3";

/// The authorization endpoint of the OAuth rows: an `https` URL this emulator
/// serves nothing at, which is not what these rows are about.
const OAUTH_ENDPOINT: &str = "https://authorization.example.com/oauth/token";

/// How long a test waits before reading a connection whose next state the clock
/// owes it: the span itself plus enough slack that a loaded machine does not
/// make the wait the flaky part of the assertion.
fn past_the_transition() -> Duration {
    state::CONNECTION_TRANSITION + Duration::from_millis(250)
}

/// The emulator: one router carrying only this service over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// One `awsJson1_1` call to EventBridge over the router.
async fn events(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("events.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("AWSEvents.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTTESTTEST/20260101/{REGION}/events/\
                 aws4_request, SignedHeaders=host, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed = if payload.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice::<Value>(&payload).unwrap_or_else(|error| {
            panic!(
                "{operation} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&payload)
            )
        })
    };
    (status, parsed)
}

/// The wire error code (`__type`), without its namespace; the empty string on a
/// success.
fn error_code(body: &Value) -> &str {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
}

/// The `message` an error body carries.
fn message(body: &Value) -> &str {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
}

/// What the rule says a refusal of a connection call reads in full: the clause
/// naming the call that failed, then the reason — composed here from the call's
/// own verb rather than imported from the implementation.
fn expected_refusal(verb: &str, reason: &str) -> String {
    format!("Failed to {verb} the connection(s). {reason}")
}

/// The `AuthParameters` block each `ConnectionAuthorizationType` carries its
/// credentials in, as
/// `smithy_query(query="shape",shape="CreateConnectionAuthRequestParameters")`
/// names the member of each type.
fn auth_parameters(authorization_type: &str, value: &str) -> Value {
    match authorization_type {
        "API_KEY" => json!({
            "ApiKeyAuthParameters": { "ApiKeyName": value, "ApiKeyValue": "sekrit-key" },
        }),
        "BASIC" => json!({
            "BasicAuthParameters": { "Username": value, "Password": "sekrit-password" },
        }),
        _ => json!({
            "OAuthParameters": {
                "ClientParameters": { "ClientID": value, "ClientSecret": "sekrit-secret" },
                "AuthorizationEndpoint": OAUTH_ENDPOINT,
                "HttpMethod": "POST",
            },
        }),
    }
}

/// The member path a read reports the non-secret half of each type's
/// credentials at.
fn reported_path(authorization_type: &str) -> Vec<&'static str> {
    match authorization_type {
        "API_KEY" => vec!["ApiKeyAuthParameters", "ApiKeyName"],
        "BASIC" => vec!["BasicAuthParameters", "Username"],
        _ => vec!["OAuthParameters", "ClientParameters", "ClientID"],
    }
}

/// The value at `path` under a read's `AuthParameters`.
fn reported_credential(read: &Value, path: &[&str]) -> Value {
    let mut cursor = &read["AuthParameters"];
    for step in path {
        cursor = &cursor[*step];
    }
    cursor.clone()
}

/// A connection of `authorization_type`, settled, whose credentials report
/// `value` back.
async fn given_connection(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    authorization_type: &str,
    value: &str,
) {
    let (status, body) = events(
        router,
        "CreateConnection",
        &json!({
            "Name": name,
            "AuthorizationType": authorization_type,
            "AuthParameters": auth_parameters(authorization_type, value),
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateConnection answered {status} {body}"
    );
    tokio::time::sleep(past_the_transition()).await;
}

/// `DescribeConnection`, which must answer.
async fn read_connection(router: &MultiServiceRouter, case: &str, name: &str) -> Value {
    let (status, body) = events(router, "DescribeConnection", &json!({ "Name": name })).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeConnection answered {status} {body}"
    );
    body
}

/// One filter row: the case, the request, and the names the listing must answer
/// — or `None` where the rule refuses the request instead.
type FilterRow = (&'static str, Value, Option<Vec<&'static str>>);

/// `ListConnections` takes at most one filter, and refuses a request naming both
/// with a sentence that names them.
#[tokio::test]
async fn a_listing_refuses_more_than_one_filter_and_names_the_members() {
    let router = emulator();
    // Two connections whose names differ in their first segment, so a prefix
    // selects one of them, and whose settled states differ, so a state filter
    // selects the other.
    given_connection(&router, "fixture", "alpha-connection", "API_KEY", "x-api-key").await;
    given_connection(
        &router,
        "fixture",
        "beta-connection",
        "OAUTH_CLIENT_CREDENTIALS",
        "the-client",
    )
    .await;

    let rows: Vec<FilterRow> = vec![
        (
            "no filter at all",
            json!({}),
            Some(vec!["alpha-connection", "beta-connection"]),
        ),
        (
            "a name prefix alone",
            json!({ "NamePrefix": "alpha" }),
            Some(vec!["alpha-connection"]),
        ),
        (
            "a connection state alone",
            json!({ "ConnectionState": state::CONNECTION_STATE_AUTHORIZED }),
            Some(vec!["alpha-connection"]),
        ),
        (
            "the other connection state alone",
            json!({ "ConnectionState": state::CONNECTION_STATE_DEAUTHORIZED }),
            Some(vec!["beta-connection"]),
        ),
        (
            "a name prefix beside a member that is not a filter",
            json!({ "NamePrefix": "beta", "Limit": 10 }),
            Some(vec!["beta-connection"]),
        ),
        (
            "both filters",
            json!({
                "NamePrefix": "alpha",
                "ConnectionState": state::CONNECTION_STATE_AUTHORIZED,
            }),
            None,
        ),
        (
            // The same combination the other way round, and with a non-filter
            // member beside it: what is refused is naming both filters, not the
            // order they arrived in or the company they keep.
            "both filters beside a member that is not a filter",
            json!({
                "ConnectionState": state::CONNECTION_STATE_DEAUTHORIZED,
                "NamePrefix": "beta",
                "Limit": 1,
            }),
            None,
        ),
        (
            // Both filters, where neither selects anything: the verdict is on
            // how many filters were named, not on what they would have matched.
            "both filters, neither matching anything",
            json!({ "NamePrefix": "gamma", "ConnectionState": state::CONNECTION_STATE_CREATING }),
            None,
        ),
    ];

    let expected_message = expected_refusal(
        "list",
        "Request failed because only one filter is supported per request, either ConnectionState \
         or NamePrefix.",
    );
    for (case, request, expected) in rows {
        let (status, body) = events(&router, "ListConnections", &request).await;
        match expected {
            Some(names) => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: one filter at most is still one filter, got {status} {body}"
                );
                let listed: Vec<&str> = body["Connections"]
                    .as_array()
                    .unwrap_or_else(|| panic!("case {case:?}: no Connections in {body}"))
                    .iter()
                    .filter_map(|entry| entry["Name"].as_str())
                    .collect();
                assert_eq!(
                    listed, names,
                    "case {case:?}: expected the listing {names:?}, got {listed:?}"
                );
            }
            None => {
                assert_eq!(
                    status, 400,
                    "case {case:?}: a combined filter is refused, got {status} {body}"
                );
                assert_eq!(
                    error_code(&body),
                    "ValidationException",
                    "case {case:?}: expected ValidationException, got {body}"
                );
                assert_eq!(
                    message(&body),
                    expected_message,
                    "case {case:?}: expected {expected_message:?}, got {:?}",
                    message(&body)
                );
                for member in ["ConnectionState", "NamePrefix"] {
                    assert!(
                        message(&body).contains(member),
                        "case {case:?}: the refusal must name {member}, got {:?}",
                        message(&body)
                    );
                }
            }
        }
    }
}

/// One update row: the case, the type the connection is created with, whether it
/// is deauthorized before the update, and the type whose credentials the update
/// submits.
type UpdateRow = (&'static str, &'static str, bool, &'static str);

/// `UpdateConnection` refuses credentials that arrive without the
/// `AuthorizationType` selecting them, changes nothing, and accepts the same
/// credentials once the type is named.
#[tokio::test]
async fn an_update_supplying_credentials_must_name_the_type_that_selects_them() {
    let rows: Vec<UpdateRow> = vec![
        (
            "an api key for a connection that already holds one",
            "API_KEY",
            false,
            "API_KEY",
        ),
        (
            "basic credentials for a connection whose authorization was stripped",
            "BASIC",
            true,
            "BASIC",
        ),
        (
            "basic credentials for a connection that holds an api key",
            "API_KEY",
            false,
            "BASIC",
        ),
        (
            "oauth client credentials for a connection that holds basic ones",
            "BASIC",
            false,
            "OAUTH_CLIENT_CREDENTIALS",
        ),
        (
            "an api key for a connection whose authorization was stripped",
            "API_KEY",
            true,
            "API_KEY",
        ),
    ];

    let expected_message = expected_refusal(
        "update",
        "Parameter AuthParameters is not valid. Reason: AuthorizationType must be provided when \
         AuthParameters is provided.",
    );
    for (case, held_type, deauthorized, submitted_type) in rows {
        let router = emulator();
        let name = "the-connection";
        given_connection(&router, case, name, held_type, "the-stored-credential").await;
        if deauthorized {
            let (status, body) =
                events(&router, "DeauthorizeConnection", &json!({ "Name": name })).await;
            assert_eq!(
                status, 200,
                "case {case:?}: DeauthorizeConnection answered {status} {body}"
            );
            tokio::time::sleep(past_the_transition()).await;
        }
        let before = read_connection(&router, case, name).await;

        // The credentials, with no type to select them.
        let submitted = auth_parameters(submitted_type, "the-submitted-credential");
        let (status, body) = events(
            &router,
            "UpdateConnection",
            &json!({ "Name": name, "AuthParameters": submitted.clone() }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: credentials with no type to select them are refused, got {status} \
             {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body}"
        );
        assert_eq!(
            message(&body),
            expected_message,
            "case {case:?}: expected {expected_message:?}, got {:?}",
            message(&body)
        );

        // The refused update wrote nothing: the connection is the one the read
        // above reported, authorization state and credentials included.
        let after = read_connection(&router, case, name).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused update must leave the connection exactly as it was, \
             got {after}"
        );

        // The SAME credentials, with the type that selects them: accepted, and
        // what a read reports is what the update carried.
        let (status, body) = events(
            &router,
            "UpdateConnection",
            &json!({
                "Name": name,
                "AuthorizationType": submitted_type,
                "AuthParameters": submitted,
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the same credentials with their type are accepted, got {status} \
             {body}"
        );
        tokio::time::sleep(past_the_transition()).await;
        let updated = read_connection(&router, case, name).await;
        assert_eq!(
            updated["AuthorizationType"],
            json!(submitted_type),
            "case {case:?}: expected the submitted authorization type, got {updated}"
        );
        let path = reported_path(submitted_type);
        assert_eq!(
            reported_credential(&updated, &path),
            json!("the-submitted-credential"),
            "case {case:?}: expected AuthParameters{path:?} to carry the submitted credential, \
             got {updated}"
        );
    }
}
