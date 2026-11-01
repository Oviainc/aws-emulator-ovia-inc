// SPDX-License-Identifier: CC0-1.0

//! The `ApiDestinationState` every API-destination reader reports, end to end
//! over the wire (`awsJson1_1`, `POST /` with
//! `X-Amz-Target: AWSEvents.<Operation>`) through the real
//! `MultiServiceRouter`.
//!
//! The RULE asserted here: a destination's `ApiDestinationState` reports whether
//! the connection its `ConnectionArn` names holds the authorization the
//! invocation needs ("the destination endpoint must support the authorization
//! type specified for the connection",
//! `smithy_query(query="operation",operation="CreateApiDestination")`), and the
//! member is modelled `ACTIVE | INACTIVE` with no value for "settling" — so it
//! reports the authorization the connection COMES TO REST on, not the transient
//! it is passing through. A connection on its way to holding authorization
//! leaves the destination `ACTIVE`; only a connection that ends up holding none
//! — never obtained, stripped by a deauthorize, or on its way out — makes it
//! `INACTIVE`. `CreateApiDestination`, `DescribeApiDestination`,
//! `UpdateApiDestination` and `ListApiDestinations` all read that one derivation,
//! so none of them can disagree, and a read taken after the connection's clock
//! has settled it reports the same value as the one taken before.
//!
//! The table spans the class rather than one recorded case: its rows differ along
//! the connection's authorization type, whether its credentials ARE the
//! authorization or have to be exchanged for a token, which call put the
//! connection in flight (create, update, deauthorize, delete) and whether the
//! reader observes it in flight or settled.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_eventbridge_awsevents::state;
use service_com_amazonaws_eventbridge_awsevents::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below is signed for.
const REGION: &str = "ap-southeast-2";

/// The authorization endpoint of the OAuth rows: an `https` URL this emulator
/// serves nothing at, so the token exchange obtains nothing.
const UNREACHABLE_ENDPOINT: &str = "https://authorization.example.com/oauth/token";

/// How long a test waits before reading a resource whose next state the clock
/// owes it: the span itself plus enough slack that a loaded machine does not make
/// the wait the flaky part of the assertion.
fn past_the_transition() -> Duration {
    state::CONNECTION_TRANSITION + Duration::from_millis(250)
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

fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// The `AuthParameters` of each `ConnectionAuthorizationType`, as
/// `smithy_query(query="shape",shape="CreateConnectionAuthRequestParameters")`
/// names the member each type's credentials go in.
fn auth_parameters(authorization_type: &str) -> Value {
    match authorization_type {
        "API_KEY" => json!({
            "ApiKeyAuthParameters": { "ApiKeyName": "x-api-key", "ApiKeyValue": "sekrit-key" },
        }),
        "BASIC" => json!({
            "BasicAuthParameters": { "Username": "the-user", "Password": "sekrit-password" },
        }),
        _ => json!({
            "OAuthParameters": {
                "ClientParameters": { "ClientID": "the-client", "ClientSecret": "sekrit-secret" },
                "AuthorizationEndpoint": UNREACHABLE_ENDPOINT,
                "HttpMethod": "POST",
            },
        }),
    }
}

/// What a row does to its connection after creating it, which is what puts the
/// connection in the state the row is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ThenOnTheConnection {
    /// Nothing: the connection is as `CreateConnection` left it.
    Nothing,
    /// `UpdateConnection`, re-stating the same credentials.
    Updated,
    /// `DeauthorizeConnection`, which strips the authorization parameters.
    Deauthorized,
    /// `DeleteConnection`, which removes the connection.
    Deleted,
}

/// One row: the case, the connection's `AuthorizationType`, what is then done to
/// the connection, and the `ApiDestinationState` every reader of a destination on
/// that connection must report.
type Row = (&'static str, &'static str, ThenOnTheConnection, &'static str);

/// Every reader of an API destination reports the state its connection settles
/// on, both while the connection is in flight and once its clock has settled it.
#[tokio::test]
async fn every_api_destination_reader_reports_the_state_its_connection_settles_on() {
    let rows: Vec<Row> = vec![
        (
            "an api key, which IS the authorization",
            "API_KEY",
            ThenOnTheConnection::Nothing,
            state::API_DESTINATION_STATE_ACTIVE,
        ),
        (
            "basic credentials, which ARE the authorization",
            "BASIC",
            ThenOnTheConnection::Nothing,
            state::API_DESTINATION_STATE_ACTIVE,
        ),
        (
            "an api-key connection whose credentials are being re-stated",
            "API_KEY",
            ThenOnTheConnection::Updated,
            state::API_DESTINATION_STATE_ACTIVE,
        ),
        (
            "oauth client credentials whose authorization endpoint issues no token",
            "OAUTH_CLIENT_CREDENTIALS",
            ThenOnTheConnection::Nothing,
            state::API_DESTINATION_STATE_INACTIVE,
        ),
        (
            "an api-key connection whose authorization has been stripped",
            "API_KEY",
            ThenOnTheConnection::Deauthorized,
            state::API_DESTINATION_STATE_INACTIVE,
        ),
        (
            "a basic connection whose authorization has been stripped",
            "BASIC",
            ThenOnTheConnection::Deauthorized,
            state::API_DESTINATION_STATE_INACTIVE,
        ),
        (
            "an api-key connection that is on its way out",
            "API_KEY",
            ThenOnTheConnection::Deleted,
            state::API_DESTINATION_STATE_INACTIVE,
        ),
    ];

    for (case, authorization_type, then, expected) in rows {
        let router = emulator();
        let connection_name = "the-connection";
        let (status, connection) = events(
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
            "case {case:?}: CreateConnection answered {status} {connection}"
        );
        let connection_arn = connection["ConnectionArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no ConnectionArn in {connection}"))
            .to_string();

        match then {
            ThenOnTheConnection::Nothing => {}
            ThenOnTheConnection::Updated => {
                let (status, body) = events(
                    &router,
                    "UpdateConnection",
                    &json!({
                        "Name": connection_name,
                        "AuthorizationType": authorization_type,
                        "AuthParameters": auth_parameters(authorization_type),
                    }),
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: UpdateConnection answered {status} {body}"
                );
            }
            ThenOnTheConnection::Deauthorized => {
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
            ThenOnTheConnection::Deleted => {
                let (status, body) = events(
                    &router,
                    "DeleteConnection",
                    &json!({ "Name": connection_name }),
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: DeleteConnection answered {status} {body}"
                );
            }
        }

        // The destination is created while the connection is still in flight, so
        // the state each reader answers is the one the connection settles on
        // rather than the transient it is in at this instant.
        let name = "the-destination";
        let (status, created) = events(
            &router,
            "CreateApiDestination",
            &json!({
                "Name": name,
                "ConnectionArn": connection_arn,
                "InvocationEndpoint": "https://destination.example.com/events",
                "HttpMethod": "POST",
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateApiDestination answered {status} {created}"
        );
        assert_eq!(
            created["ApiDestinationState"],
            json!(expected),
            "case {case:?}: the create must answer {expected}, got {created}"
        );

        let (status, described) =
            events(&router, "DescribeApiDestination", &json!({ "Name": name })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeApiDestination answered {status} {described}"
        );
        assert_eq!(
            described["ApiDestinationState"],
            json!(expected),
            "case {case:?}: the read must answer {expected}, got {described}"
        );

        // An update changes what the destination invokes, not whether its
        // connection authorizes the invocation.
        let (status, updated) = events(
            &router,
            "UpdateApiDestination",
            &json!({
                "Name": name,
                "InvocationEndpoint": "https://destination.example.com/v2/events",
                "InvocationRateLimitPerSecond": 25,
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateApiDestination answered {status} {updated}"
        );
        assert_eq!(
            updated["ApiDestinationState"],
            json!(expected),
            "case {case:?}: the update must answer {expected}, got {updated}"
        );

        assert_eq!(
            listed_state(&router, case, name).await,
            json!(expected),
            "case {case:?}: the listing must carry {expected}"
        );

        // Once the clock has settled the connection, every reader still reports
        // the same state: the state was read off where the connection was
        // HEADED, so settling it changes nothing.
        tokio::time::sleep(past_the_transition()).await;
        let (status, settled) =
            events(&router, "DescribeApiDestination", &json!({ "Name": name })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeApiDestination answered {status} {settled}"
        );
        assert_eq!(
            settled["ApiDestinationState"],
            json!(expected),
            "case {case:?}: a read of the settled connection must still answer {expected}, \
             got {settled}"
        );
        assert_eq!(
            listed_state(&router, case, name).await,
            json!(expected),
            "case {case:?}: the listing of the settled connection must still carry {expected}"
        );
    }
}

/// A destination follows the connection it is REPOINTED at, so one update can
/// move it from `ACTIVE` to `INACTIVE` and the next one back.
#[tokio::test]
async fn repointing_a_destination_moves_its_state_to_the_new_connections() {
    let router = emulator();
    // (the connection's name, its authorization type, whether it is deauthorized
    //  after the create, the state a destination invoking through it reports)
    let connections = [
        (
            "authorizing",
            "API_KEY",
            false,
            state::API_DESTINATION_STATE_ACTIVE,
        ),
        (
            "stripped",
            "BASIC",
            true,
            state::API_DESTINATION_STATE_INACTIVE,
        ),
        (
            "settling",
            "BASIC",
            false,
            state::API_DESTINATION_STATE_ACTIVE,
        ),
    ];

    let mut arns: Vec<(String, &str)> = Vec::new();
    for (connection_name, authorization_type, deauthorized, expected) in connections {
        let (status, body) = events(
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
            "case {connection_name:?}: CreateConnection answered {status} {body}"
        );
        if deauthorized {
            let (status, body) = events(
                &router,
                "DeauthorizeConnection",
                &json!({ "Name": connection_name }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {connection_name:?}: DeauthorizeConnection answered {status} {body}"
            );
        }
        let arn = body["ConnectionArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {connection_name:?}: no ConnectionArn in {body}"))
            .to_string();
        arns.push((arn, expected));
    }

    let name = "repointed-destination";
    let (first_arn, first_expected) = &arns[0];
    let (status, created) = events(
        &router,
        "CreateApiDestination",
        &json!({
            "Name": name,
            "ConnectionArn": first_arn,
            "InvocationEndpoint": "https://destination.example.com/events",
            "HttpMethod": "PUT",
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateApiDestination answered {status} {created}"
    );
    assert_eq!(
        created["ApiDestinationState"],
        json!(first_expected),
        "a destination on the first connection is {first_expected}, got {created}"
    );
    let arn = created["ApiDestinationArn"].clone();

    for (connection_arn, expected) in &arns[1..] {
        let (status, updated) = events(
            &router,
            "UpdateApiDestination",
            &json!({ "Name": name, "ConnectionArn": connection_arn }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {connection_arn:?}: UpdateApiDestination answered {status} {updated}"
        );
        assert_eq!(
            updated["ApiDestinationState"],
            json!(expected),
            "case {connection_arn:?}: the repointed destination is {expected}, got {updated}"
        );
        assert_eq!(
            updated["ApiDestinationArn"], arn,
            "case {connection_arn:?}: the ARN must survive the update, got {updated}"
        );
        let (status, described) =
            events(&router, "DescribeApiDestination", &json!({ "Name": name })).await;
        assert_eq!(
            status, 200,
            "case {connection_arn:?}: DescribeApiDestination answered {status} {described}"
        );
        assert_eq!(
            described["ApiDestinationState"],
            json!(expected),
            "case {connection_arn:?}: a read agrees with the update, got {described}"
        );
    }
}

/// The `ApiDestinationState` the listing carries for the entry called `name`.
async fn listed_state(router: &MultiServiceRouter, case: &str, name: &str) -> Value {
    let (status, listed) = events(router, "ListApiDestinations", &json!({})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListApiDestinations answered {status} {listed}"
    );
    let entry = listed["ApiDestinations"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no ApiDestinations in {listed}"))
        .iter()
        .find(|entry| entry["Name"] == json!(name))
        .unwrap_or_else(|| panic!("case {case:?}: no entry called {name:?} in {listed}"))
        .clone();
    entry["ApiDestinationState"].clone()
}
