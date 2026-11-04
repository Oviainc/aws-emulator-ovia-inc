// SPDX-License-Identifier: CC0-1.0

//! What an api-destination invocation CARRIES: the credentials the connection
//! holds and the parameters the connection and the rule's target each
//! contribute.
//!
//! The connection is written over the wire (`awsJson1_1`, `POST /` with
//! `X-Amz-Target: AWSEvents.CreateConnection`) through the real
//! `MultiServiceRouter`, so what the invocation is built from is what a client's
//! own `CreateConnection` stored — and the read of that connection is asserted
//! to report none of it.
//!
//! The RULES asserted here:
//!
//! * a connection whose credentials ARE the authorization sends them with every
//!   invocation: an `API_KEY` connection under the `ApiKeyName` header it names,
//!   a `BASIC` connection as `Authorization: Basic <base64(user:password)>`
//!   (RFC 7617);
//! * the connection's `InvocationHttpParameters` and the target's
//!   `Target$HttpParameters` are BOTH sent — headers as headers, query-string
//!   parameters on the query string, the connection's body parameters as members
//!   of the JSON body, the target's `PathParameterValues` filling the `*`
//!   wildcards of the destination's `InvocationEndpoint` in order — and the
//!   CONNECTION's value wins wherever both name one key;
//! * a value the connection flagged secret is sent to the endpoint and is still
//!   withheld from every read of the connection.
//!
//! The table spans the class rather than one recorded case: its rows differ
//! along which side contributes a parameter (neither, the target, the
//! connection, both), the kind of parameter (header, query string, body, path),
//! whether the two sides collide, how many wildcards the endpoint has against
//! how many values the target supplies, whether the endpoint already carries a
//! query, and whether the body is JSON at all.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_eventbridge_awsevents::ops::types::HttpParameters;
use service_com_amazonaws_eventbridge_awsevents::state::invocation::{
    api_destination_invocation, ConnectionSecret, Invocation,
};
use service_com_amazonaws_eventbridge_awsevents::state::ComAmazonawsEventbridgeAwseventsState;
use service_com_amazonaws_eventbridge_awsevents::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below is signed for.
const REGION: &str = "eu-central-1";

/// The content type and user agent EventBridge makes an api-destination request
/// with, which the merge writes as the defaults a parameter of either side may
/// replace.
const CONTENT_TYPE: &str = "application/json; charset=utf-8";
const USER_AGENT: &str = "Amazon/EventBridge/ApiDestinations";

/// The invocation of `endpoint`, made with the content type and user agent
/// above — the two every api-destination request of this service carries.
fn invocation_of(
    endpoint: &str,
    secret: &ConnectionSecret,
    target: Option<&HttpParameters>,
    body: &str,
) -> Invocation {
    api_destination_invocation(endpoint, secret, target, body, CONTENT_TYPE, USER_AGENT)
}

/// The event envelope every invocation below carries as its body — the shape
/// `PutEvents` hands a target.
fn envelope() -> String {
    json!({
        "version": "0",
        "id": "2f8a1b4c-0000-4000-8000-00000000abcd",
        "detail-type": "Hook Fired",
        "source": "com.demo.webhooks",
        "detail": { "order": "A-1" },
    })
    .to_string()
}

/// The emulator: one router carrying only this service over one shared engine,
/// and the engine itself, because what the invocation is built from is the
/// STORED connection.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

fn emulator() -> Emulator {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    Emulator { router, engine }
}

/// One `awsJson1_1` call to EventBridge over the router.
async fn events(emulator: &Emulator, operation: &str, body: &Value) -> (u16, Value) {
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
    let response = emulator.router.dispatch(request).await;
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

/// Create the connection `name` with `auth_parameters`, and answer the
/// credential half the STORE holds for it — which is what an invocation through
/// it carries.
async fn given_connection(
    emulator: &Emulator,
    case: &str,
    name: &str,
    authorization_type: &str,
    auth_parameters: &Value,
) -> ConnectionSecret {
    let (status, body) = events(
        emulator,
        "CreateConnection",
        &json!({
            "Name": name,
            "AuthorizationType": authorization_type,
            "AuthParameters": auth_parameters,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateConnection answered {status} {body}"
    );
    // A read of the connection reports no credential at all, whatever the
    // invocation carries.
    let (status, read) = events(emulator, "DescribeConnection", &json!({ "Name": name })).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeConnection answered {status} {read}"
    );
    let rendered = read.to_string();
    for secret in ["sekrit-key", "sekrit-password", "sekrit-header"] {
        assert!(
            !rendered.contains(secret),
            "case {case:?}: a read must report no credential, but {secret:?} is in {rendered}"
        );
    }
    emulator
        .engine
        .state::<ComAmazonawsEventbridgeAwseventsState>()
        .lock()
        .connection(name)
        .unwrap_or_else(|| panic!("case {case:?}: the created connection is in the store"))
        .secret
        .clone()
}

/// The target's `HttpParameters`, built from the pieces a row supplies.
fn target_parameters(
    path: &[&str],
    headers: &[(&str, &str)],
    query: &[(&str, &str)],
) -> HttpParameters {
    HttpParameters {
        path_parameter_values: (!path.is_empty())
            .then(|| path.iter().map(|value| (*value).to_string()).collect()),
        header_parameters: (!headers.is_empty()).then(|| {
            headers
                .iter()
                .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
                .collect()
        }),
        query_string_parameters: (!query.is_empty()).then(|| {
            query
                .iter()
                .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
                .collect()
        }),
    }
}

/// The header value `headers` carries for `name`, or `None`.
fn header<'a>(invocation: &'a Invocation, name: &str) -> Option<&'a str> {
    invocation
        .headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

/// The api-key connection's `AuthParameters`: the key, plus one parameter of
/// each kind the connection may contribute — the header value flagged SECRET,
/// so the rule that a secret value is sent and never reported is exercised.
fn api_key_parameters() -> Value {
    json!({
        "ApiKeyAuthParameters": { "ApiKeyName": "x-api-key", "ApiKeyValue": "sekrit-key" },
        "InvocationHttpParameters": {
            "HeaderParameters": [
                { "Key": "x-connection", "Value": "sekrit-header", "IsValueSecret": true },
                { "Key": "x-shared", "Value": "from-the-connection", "IsValueSecret": false },
            ],
            "QueryStringParameters": [
                { "Key": "shared", "Value": "from the connection", "IsValueSecret": false },
            ],
            "BodyParameters": [
                { "Key": "scope", "Value": "orders/write", "IsValueSecret": false },
            ],
        },
    })
}

/// An api-key connection sends its key under the header it names, and the
/// connection's parameters beat the target's wherever both name one key.
#[tokio::test]
async fn an_api_key_connection_sends_its_key_and_wins_the_shared_parameters() {
    let emulator = emulator();
    let case = "an api key with parameters of every kind";
    let secret = given_connection(
        &emulator,
        case,
        "the-connection",
        "API_KEY",
        &api_key_parameters(),
    )
    .await;

    let target = target_parameters(
        &["orders"],
        &[
            ("X-Shared", "from-the-target"),
            ("x-target", "target-only"),
        ],
        &[("shared", "from-the-target"), ("target", "target-only")],
    );
    let invocation = invocation_of(
        "https://hooks.example.com/v1/*/events",
        &secret,
        Some(&target),
        &envelope(),
    );

    assert_eq!(
        header(&invocation, "x-api-key"),
        Some("sekrit-key"),
        "case {case:?}: the key goes in the header the connection names, got {invocation:?}"
    );
    assert_eq!(
        header(&invocation, "x-connection"),
        Some("sekrit-header"),
        "case {case:?}: a secret header value is SENT to the endpoint, got {invocation:?}"
    );
    assert_eq!(
        header(&invocation, "x-target"),
        Some("target-only"),
        "case {case:?}: the target's own header is sent too, got {invocation:?}"
    );
    assert_eq!(
        header(&invocation, "x-shared"),
        Some("from-the-connection"),
        "case {case:?}: the connection wins the shared header, whichever case either side \
         spelled it in, got {invocation:?}"
    );
    assert_eq!(
        invocation
            .headers
            .iter()
            .filter(|(name, _)| name.eq_ignore_ascii_case("x-shared"))
            .count(),
        1,
        "case {case:?}: one header name is one header, got {invocation:?}"
    );
    assert_eq!(
        header(&invocation, "content-type"),
        Some(CONTENT_TYPE),
        "case {case:?}: the request still carries the content type AWS sends, got {invocation:?}"
    );
    assert_eq!(
        header(&invocation, "user-agent"),
        Some(USER_AGENT),
        "case {case:?}: the request still carries the user agent AWS sends, got {invocation:?}"
    );
    assert_eq!(
        invocation.uri,
        "https://hooks.example.com/v1/orders/events?shared=from%20the%20connection&target=target-only",
        "case {case:?}: the wildcard is filled and the connection wins the shared query parameter"
    );
    let body: Value = serde_json::from_str(&invocation.body)
        .unwrap_or_else(|error| panic!("case {case:?}: the body is not JSON ({error})"));
    assert_eq!(
        body["scope"],
        json!("orders/write"),
        "case {case:?}: a body parameter is a member of the delivered document, got {body}"
    );
    assert_eq!(
        body["id"],
        json!("2f8a1b4c-0000-4000-8000-00000000abcd"),
        "case {case:?}: the event itself is still the body, got {body}"
    );
}

/// A basic connection sends the standard `Authorization: Basic` header, built
/// from the credentials it stored.
#[tokio::test]
async fn a_basic_connection_sends_the_basic_authorization_header() {
    let emulator = emulator();
    // (case, the user name, the password, the header value RFC 7617 defines for
    //  them — base64 of `<user>:<password>`)
    let rows: [(&str, &str, &str); 3] = [
        ("ascii credentials", "the-user", "sekrit-password"),
        ("a one-character user", "u", "p"),
        (
            // Neither member is modelled with a colon in it; a space is
            // admitted, and the header is still one base64 value.
            "credentials carrying a space",
            "the user",
            "pass word",
        ),
    ];

    for (index, (case, username, password)) in rows.iter().enumerate() {
        let name = format!("connection-{index}");
        let secret = given_connection(
            &emulator,
            case,
            &name,
            "BASIC",
            &json!({
                "BasicAuthParameters": { "Username": username, "Password": password },
            }),
        )
        .await;
        let invocation = invocation_of(
            "https://hooks.example.com/events",
            &secret,
            None,
            &envelope(),
        );
        // The expected value is computed the way RFC 7617 defines it rather
        // than copied, so the row states the rule and not one encoding.
        let expected = {
            use base64::Engine as _;
            format!(
                "Basic {}",
                base64::engine::general_purpose::STANDARD
                    .encode(format!("{username}:{password}"))
            )
        };
        assert_eq!(
            header(&invocation, "authorization"),
            Some(expected.as_str()),
            "case {case:?}: expected {expected:?}, got {invocation:?}"
        );
        assert_eq!(
            invocation.uri, "https://hooks.example.com/events",
            "case {case:?}: a connection carrying no parameters changes no URI"
        );
        assert_eq!(
            invocation.body,
            envelope(),
            "case {case:?}: a connection carrying no body parameters delivers the bare envelope"
        );
    }
}

/// One row of the endpoint table: the case, the endpoint as the destination
/// stored it, the `PathParameterValues` the target supplies, the
/// query-string parameters it supplies, and the URI the invocation is made
/// against.
type EndpointRow = (
    &'static str,
    &'static str,
    &'static [&'static str],
    &'static [(&'static str, &'static str)],
    &'static str,
);

/// Where the invocation GOES: the wildcards of the stored endpoint filled by
/// the target's path values, in order, with the merged query string on it.
#[tokio::test]
async fn the_invocation_goes_to_the_endpoint_with_its_wildcards_filled() {
    let rows: [EndpointRow; 8] = [
        (
            "no wildcard and no parameter",
            "https://hooks.example.com/orders",
            &[],
            &[],
            "https://hooks.example.com/orders",
        ),
        (
            "one wildcard, one value",
            "https://hooks.example.com/v1/*/events",
            &["orders"],
            &[],
            "https://hooks.example.com/v1/orders/events",
        ),
        (
            "two wildcards, two values, filled in order",
            "https://hooks.example.com/*/items/*",
            &["orders", "17"],
            &[],
            "https://hooks.example.com/orders/items/17",
        ),
        (
            "a trailing wildcard",
            "https://hooks.example.com/orders/*",
            &["17"],
            &[],
            "https://hooks.example.com/orders/17",
        ),
        (
            // Nothing says what the second wildcard stands for, so it is left
            // as the destination stored it.
            "two wildcards, one value",
            "https://hooks.example.com/*/items/*",
            &["orders"],
            &[],
            "https://hooks.example.com/orders/items/*",
        ),
        (
            // A value with no wildcard left to fill is not appended anywhere.
            "one wildcard, two values",
            "https://hooks.example.com/*/events",
            &["orders", "spare"],
            &[],
            "https://hooks.example.com/orders/events",
        ),
        (
            "a query the endpoint already carries",
            "https://hooks.example.com/orders?stage=prod",
            &[],
            &[("page", "2")],
            "https://hooks.example.com/orders?stage=prod&page=2",
        ),
        (
            // The `*` of a query string is not a path wildcard.
            "a wildcard in the query, which is not a path wildcard",
            "https://hooks.example.com/orders?glob=*",
            &["unused"],
            &[],
            "https://hooks.example.com/orders?glob=*",
        ),
    ];

    for (case, endpoint, path, query, expected) in rows {
        let target = target_parameters(path, &[], query);
        let invocation = invocation_of(
            endpoint,
            &ConnectionSecret::default(),
            Some(&target),
            &envelope(),
        );
        assert_eq!(
            invocation.uri, expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            invocation.uri
        );
        assert_eq!(
            invocation.headers,
            vec![
                ("content-type".to_string(), CONTENT_TYPE.to_string()),
                ("user-agent".to_string(), USER_AGENT.to_string()),
            ],
            "case {case:?}: a connection with no credential and no parameter adds no header of \
             its own, got {invocation:?}"
        );
    }
}

/// One row of the body table: the case, the body the delivery built, and the
/// members the invocation must carry — `None` where that body has none to add
/// to and is delivered unchanged.
type BodyRow = (&'static str, String, Option<Vec<(&'static str, &'static str)>>);

/// A body parameter is a MEMBER of the delivered JSON document; a body that is
/// not a JSON object has no member to add and is delivered as it was built.
#[tokio::test]
async fn body_parameters_are_members_of_a_json_body_and_leave_any_other_body_alone() {
    let emulator = emulator();
    let case = "a connection carrying body parameters";
    let secret = given_connection(
        &emulator,
        case,
        "the-connection",
        "API_KEY",
        &json!({
            "ApiKeyAuthParameters": { "ApiKeyName": "x-api-key", "ApiKeyValue": "sekrit-key" },
            "InvocationHttpParameters": {
                "BodyParameters": [
                    { "Key": "scope", "Value": "orders/write", "IsValueSecret": false },
                    { "Key": "tenant", "Value": "acme", "IsValueSecret": false },
                ],
            },
        }),
    )
    .await;

    // (case, the body the delivery built, the members the invocation must
    //  carry, or `None` where the body is delivered unchanged)
    let rows: [BodyRow; 4] = [
        (
            "the event envelope",
            envelope(),
            Some(vec![("scope", "orders/write"), ("tenant", "acme")]),
        ),
        (
            "a transformed JSON object",
            json!({ "order": "A-1" }).to_string(),
            Some(vec![("scope", "orders/write"), ("tenant", "acme")]),
        ),
        (
            "a transformed TEXT payload, which has no members",
            "A-1 is in state SHIPPED".to_string(),
            None,
        ),
        (
            "a transformed JSON array, which has no members either",
            json!(["A-1"]).to_string(),
            None,
        ),
    ];

    for (row, body, expected) in rows {
        let invocation = invocation_of(
            "https://hooks.example.com/orders",
            &secret,
            None,
            &body,
        );
        match expected {
            None => assert_eq!(
                invocation.body, body,
                "case {row:?}: a body with no members is delivered as it was built"
            ),
            Some(members) => {
                let delivered: Value = serde_json::from_str(&invocation.body)
                    .unwrap_or_else(|error| panic!("case {row:?}: not JSON ({error})"));
                for (key, value) in members {
                    assert_eq!(
                        delivered[key],
                        json!(value),
                        "case {row:?}: expected the member {key}={value}, got {delivered}"
                    );
                }
                let original: Value = serde_json::from_str(&body).expect("the row's body is JSON");
                for (key, value) in original.as_object().into_iter().flatten() {
                    assert_eq!(
                        delivered[key], *value,
                        "case {row:?}: the body the delivery built is still there, got {delivered}"
                    );
                }
            }
        }
    }
}
