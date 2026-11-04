// SPDX-License-Identifier: CC0-1.0

//! What becomes of an API destination when the connection it invokes through is
//! DELETED from under it, end to end over the wire (`awsJson1_1`, `POST /` with
//! `X-Amz-Target: AWSEvents.<Operation>`) through the real `MultiServiceRouter`.
//!
//! The RULE asserted here: a destination's `ApiDestinationState` is the
//! authorization of the connection it invokes through ("the destination endpoint
//! must support the authorization type specified for the connection",
//! `smithy_query(query="operation",operation="CreateApiDestination")`), taken
//! when the destination is written and carried forward by that connection's own
//! transitions — so a `DeauthorizeConnection` reaches the destinations invoking
//! through it and a later reauthorization reaches them again, while DELETING the
//! connection reaches none of them: deleting a connection an API destination
//! still names is permitted and leaves the destination exactly as it was, state
//! and all, with a now-dangling `ConnectionArn`. A destination is never revoked
//! by the disappearance of its connection, and every reader of it agrees.
//!
//! The table spans the class rather than one recorded case: its rows differ
//! along the authorization type of the connection, the state the destination
//! holds when the deletion happens (`ACTIVE` on an authorized connection,
//! `INACTIVE` on one whose authorization was stripped), and whether the read is
//! taken while the deletion is still in flight or after it has settled.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_eventbridge_awsevents::state;
use service_com_amazonaws_eventbridge_awsevents::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below is signed for.
const REGION: &str = "us-west-2";

/// Where every destination below points: a host this emulator serves nothing
/// at, which these rows are not about.
const ENDPOINT: &str = "https://hooks.example.com/orders";

/// How long a test waits before reading a resource whose next state the clock
/// owes it: the span itself plus enough slack that a loaded machine does not
/// make the wait the flaky part of the assertion.
fn past_the_transition() -> Duration {
    state::CONNECTION_TRANSITION + Duration::from_millis(250)
}

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

/// The `AuthParameters` of each `ConnectionAuthorizationType`, as
/// `smithy_query(query="shape",shape="CreateConnectionAuthRequestParameters")`
/// names the member each type's credentials go in.
fn auth_parameters(authorization_type: &str) -> Value {
    match authorization_type {
        "API_KEY" => json!({
            "ApiKeyAuthParameters": { "ApiKeyName": "x-api-key", "ApiKeyValue": "sekrit-key" },
        }),
        _ => json!({
            "BasicAuthParameters": { "Username": "the-user", "Password": "sekrit-password" },
        }),
    }
}

/// The `ApiDestinationState` the listing carries for the entry called `name`,
/// beside the `ConnectionArn` it reports for it.
async fn listed(router: &MultiServiceRouter, case: &str, name: &str) -> (Value, Value) {
    let (status, body) = events(router, "ListApiDestinations", &json!({})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListApiDestinations answered {status} {body}"
    );
    let entry = body["ApiDestinations"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no ApiDestinations in {body}"))
        .iter()
        .find(|entry| entry["Name"] == json!(name))
        .unwrap_or_else(|| panic!("case {case:?}: no entry called {name:?} in {body}"))
        .clone();
    (
        entry["ApiDestinationState"].clone(),
        entry["ConnectionArn"].clone(),
    )
}

/// One row: the case, the connection's `AuthorizationType`, whether its
/// authorization is stripped before the destination is created, and the
/// `ApiDestinationState` the destination then holds — which the deletion of its
/// connection must not change.
type Row = (&'static str, &'static str, bool, &'static str);

/// Deleting a connection an API destination still names leaves that destination
/// exactly as it was.
#[tokio::test]
async fn deleting_a_connection_does_not_revoke_the_destinations_that_name_it() {
    let rows: [Row; 3] = [
        (
            "an api-key connection that holds its authorization",
            "API_KEY",
            false,
            state::API_DESTINATION_STATE_ACTIVE,
        ),
        (
            "a basic connection that holds its authorization",
            "BASIC",
            false,
            state::API_DESTINATION_STATE_ACTIVE,
        ),
        (
            // The destination is already INACTIVE when the connection goes: the
            // deletion must not move it in EITHER direction.
            "an api-key connection whose authorization was stripped",
            "API_KEY",
            true,
            state::API_DESTINATION_STATE_INACTIVE,
        ),
    ];

    for (case, authorization_type, deauthorized, expected) in rows {
        let router = emulator();
        let connection_name = "the-connection";
        let (status, created) = events(
            &router,
            "CreateConnection",
            &json!({
                "Name": connection_name,
                "AuthorizationType": authorization_type,
                "AuthParameters": auth_parameters(authorization_type),
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateConnection answered {status} {created}"
        );
        let connection_arn = created["ConnectionArn"].clone();
        if deauthorized {
            let (status, body) = events(
                &router,
                "DeauthorizeConnection",
                &json!({ "Name": connection_name }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: DeauthorizeConnection answered {status} {body}"
            );
        }
        tokio::time::sleep(past_the_transition()).await;

        let name = "the-destination";
        let (status, body) = events(
            &router,
            "CreateApiDestination",
            &json!({
                "Name": name,
                "ConnectionArn": connection_arn,
                "InvocationEndpoint": ENDPOINT,
                "HttpMethod": "POST",
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateApiDestination answered {status} {body}"
        );
        assert_eq!(
            body["ApiDestinationState"],
            json!(expected),
            "case {case:?}: the destination starts {expected}, got {body}"
        );
        let before = {
            let (status, body) =
                events(&router, "DescribeApiDestination", &json!({ "Name": name })).await;
            assert_eq!(
                status, 200,
                "case {case:?}: DescribeApiDestination answered {status} {body}"
            );
            body
        };

        // The connection goes away from under the destination, which AWS
        // permits.
        let (status, deleted) = events(
            &router,
            "DeleteConnection",
            &json!({ "Name": connection_name }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: DeleteConnection must be permitted, got {status} {deleted}"
        );

        // While the deletion is in flight, and once it has settled, the
        // destination is the one the read above reported — state and dangling
        // connection reference included.
        for when in ["while the deletion is in flight", "once it has settled"] {
            if when == "once it has settled" {
                tokio::time::sleep(past_the_transition()).await;
                // The connection itself really is gone by now.
                let (status, gone) = events(
                    &router,
                    "DescribeConnection",
                    &json!({ "Name": connection_name }),
                )
                .await;
                assert_eq!(
                    status, 400,
                    "case {case:?}: the deleted connection must be gone, got {status} {gone}"
                );
            }
            let (status, after) =
                events(&router, "DescribeApiDestination", &json!({ "Name": name })).await;
            assert_eq!(
                status, 200,
                "case {case:?} {when}: DescribeApiDestination answered {status} {after}"
            );
            assert_eq!(
                after, before,
                "case {case:?} {when}: the destination must be untouched by the deletion of its \
                 connection, got {after}"
            );
            assert_eq!(
                after["ApiDestinationState"],
                json!(expected),
                "case {case:?} {when}: expected {expected}, got {after}"
            );
            let (listed_state, listed_connection) = listed(&router, case, name).await;
            assert_eq!(
                listed_state,
                json!(expected),
                "case {case:?} {when}: the listing must carry {expected}"
            );
            assert_eq!(
                listed_connection, before["ConnectionArn"],
                "case {case:?} {when}: the listing keeps the dangling connection reference"
            );
        }
    }
}

/// The counterpart the rule rests on: a connection that is DEAUTHORIZED, and
/// then authorized again, does carry its destinations with it — so the
/// invariance above is about the deletion and not about a destination that
/// ignores its connection.
#[tokio::test]
async fn deauthorizing_and_reauthorizing_a_connection_moves_its_destinations() {
    let router = emulator();
    let connection_name = "the-connection";
    let (status, created) = events(
        &router,
        "CreateConnection",
        &json!({
            "Name": connection_name,
            "AuthorizationType": "API_KEY",
            "AuthParameters": auth_parameters("API_KEY"),
        }),
    )
    .await;
    assert_eq!(status, 200, "CreateConnection answered {status} {created}");
    let connection_arn = created["ConnectionArn"].clone();
    tokio::time::sleep(past_the_transition()).await;

    let name = "the-destination";
    let (status, body) = events(
        &router,
        "CreateApiDestination",
        &json!({
            "Name": name,
            "ConnectionArn": connection_arn,
            "InvocationEndpoint": ENDPOINT,
            "HttpMethod": "PUT",
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateApiDestination answered {status} {body}"
    );

    // (case, the call made on the connection, the request it carries, the state
    //  the destination comes to rest in because of it)
    let steps: [(&str, &str, Value, &str); 2] = [
        (
            "the authorization is stripped",
            "DeauthorizeConnection",
            json!({ "Name": connection_name }),
            state::API_DESTINATION_STATE_INACTIVE,
        ),
        (
            "the credentials are stated again",
            "UpdateConnection",
            json!({
                "Name": connection_name,
                "AuthorizationType": "API_KEY",
                "AuthParameters": auth_parameters("API_KEY"),
            }),
            state::API_DESTINATION_STATE_ACTIVE,
        ),
    ];
    for (case, operation, request, expected) in steps {
        let (status, body) = events(&router, operation, &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: {operation} answered {status} {body}"
        );
        tokio::time::sleep(past_the_transition()).await;
        let (status, after) =
            events(&router, "DescribeApiDestination", &json!({ "Name": name })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeApiDestination answered {status} {after}"
        );
        assert_eq!(
            after["ApiDestinationState"],
            json!(expected),
            "case {case:?}: the destination must come to rest {expected}, got {after}"
        );
    }
}
