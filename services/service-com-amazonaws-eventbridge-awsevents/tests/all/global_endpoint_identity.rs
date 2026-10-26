// SPDX-License-Identifier: CC0-1.0

//! The identity `CreateEndpoint` mints for a global endpoint, over the wire
//! (`awsJson1_1`, `POST /` with `X-Amz-Target: AWSEvents.<Operation>`) through
//! the real `MultiServiceRouter`.
//!
//! The RULE asserted here: the subdomain a created endpoint is published at is
//! its OWN — "The URL subdomain of the endpoint. For example, if the URL for
//! Endpoint is `https://abcde.veo.endpoints.event.amazonaws.com`, then the
//! EndpointId is `abcde.veo`" (`Endpoint$EndpointId`) — so every endpoint a
//! create stores carries an `EndpointId` no other endpoint of the same account
//! carries, in the modelled form (length 1..50, two dot-separated segments of
//! `[A-Za-z0-9\-]`), it is the subdomain of the endpoint's own `EndpointUrl`,
//! and a read reports it unchanged.
//!
//! A URL is how events reach an endpoint, so two endpoints sharing one makes the
//! pair unroutable: the ids have to differ whatever else about the two requests
//! is identical. The table therefore spans endpoints created back-to-back within
//! one transition span and endpoints created with every other member identical —
//! the cases where an id derived from the clock alone collides — as well as names
//! of different forms and lengths, and asserts the ids are distinct across ALL
//! of them, not only within a row.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_eventbridge_awsevents::state;
use service_com_amazonaws_eventbridge_awsevents::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request below is signed for.
const REGION: &str = "eu-central-1";

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

/// The ARN of the bus `name` in `region`, in the `NonPartnerEventBusArn` form the
/// model gives `EndpointEventBus$EventBusArn`.
fn bus_arn(region: &str, name: &str) -> String {
    format!(
        "arn:aws:events:{region}:{}:event-bus/{name}",
        engine::emulator_account_id()
    )
}

/// A `CreateEndpoint` request for `name`, with a bus pair and a routing policy
/// that differ only where a row says they do.
fn create_request(name: &str, bus: &str, secondary: &str, health_check: &str) -> Value {
    json!({
        "Name": name,
        "RoutingConfig": {
            "FailoverConfig": {
                "Primary": { "HealthCheck": format!("arn:aws:route53:::healthcheck/{health_check}") },
                "Secondary": { "Route": secondary },
            },
        },
        "EventBuses": [
            { "EventBusArn": bus_arn(REGION, bus) },
            { "EventBusArn": bus_arn(secondary, bus) },
        ],
        "ReplicationConfig": { "State": state::REPLICATION_STATE_DISABLED },
    })
}

/// One endpoint the table creates.
struct IdentityCase {
    case: &'static str,
    name: &'static str,
    bus: &'static str,
    secondary: &'static str,
    health_check: &'static str,
}

/// `EndpointId`'s modelled constraints: `length 1..50` and
/// `pattern ^[A-Za-z0-9\-]+[\.][A-Za-z0-9\-]+$`
/// (`smithy_query(query="shape",shape="com.amazonaws.eventbridge#EndpointId")`).
fn endpoint_id_violation(endpoint_id: &str) -> Option<String> {
    if endpoint_id.is_empty() || endpoint_id.chars().count() > 50 {
        return Some(format!(
            "expected an EndpointId of 1..50 characters, got {} characters",
            endpoint_id.chars().count()
        ));
    }
    let Some((subdomain, suffix)) = endpoint_id.split_once('.') else {
        return Some("expected two dot-separated segments, got no '.'".to_string());
    };
    let segment_ok = |segment: &str| {
        !segment.is_empty()
            && segment
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || character == '-')
    };
    if !segment_ok(subdomain) || !segment_ok(suffix) {
        return Some(format!(
            "expected both segments to be non-empty [A-Za-z0-9-], got {subdomain:?} and {suffix:?}"
        ));
    }
    None
}

/// Every endpoint a create stores is published at its OWN subdomain: the
/// `EndpointId` is in the modelled form, is the subdomain of the endpoint's own
/// `EndpointUrl`, is reported unchanged by a later read, and is carried by no
/// other endpoint of the account.
#[tokio::test]
async fn every_created_endpoint_is_published_at_its_own_subdomain() {
    let cases = [
        IdentityCase {
            case: "the first endpoint",
            name: "orders",
            bus: "orders",
            secondary: "us-west-2",
            health_check: "abcdef0123456789",
        },
        IdentityCase {
            case: "a second endpoint whose request differs only in its name",
            name: "orders-2",
            bus: "orders",
            secondary: "us-west-2",
            health_check: "abcdef0123456789",
        },
        IdentityCase {
            case: "a dotted name, another bus and another secondary Region",
            name: "shipments.v2",
            bus: "shipments",
            secondary: "us-east-1",
            health_check: "0123456789abcdef",
        },
        IdentityCase {
            case: "an underscored name and a hyphenated health check",
            name: "custom_bus_A_endpoint",
            bus: "bus-with-hyphens",
            secondary: "ap-southeast-1",
            health_check: "health-check-7",
        },
        IdentityCase {
            case: "the shortest name EndpointName allows (length 1..64)",
            name: "e",
            bus: "payments",
            secondary: "eu-west-1",
            health_check: "deadbeef00000000",
        },
        IdentityCase {
            case: "the longest name EndpointName allows (length 1..64)",
            name: "a-64-character-endpoint-name-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            bus: "payments",
            secondary: "eu-west-1",
            health_check: "deadbeef00000000",
        },
    ];

    let router = emulator();
    // Every id seen so far, against the endpoint that carries it, so a collision
    // names BOTH endpoints.
    let mut seen: BTreeMap<String, String> = BTreeMap::new();

    for IdentityCase {
        case,
        name,
        bus,
        secondary,
        health_check,
    } in cases
    {
        let request = create_request(name, bus, secondary, health_check);
        let (status, created) = events(&router, "CreateEndpoint", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateEndpoint answered {status} {created}"
        );

        let (status, read) = events(&router, "DescribeEndpoint", &json!({ "Name": name })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeEndpoint answered {status} {read}"
        );
        let endpoint_id = read["EndpointId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected an EndpointId, got {read}"))
            .to_string();

        assert!(
            endpoint_id_violation(&endpoint_id).is_none(),
            "case {case:?}: {}, got {endpoint_id:?}",
            endpoint_id_violation(&endpoint_id).unwrap_or_default()
        );
        assert_eq!(
            read["EndpointUrl"],
            json!(format!(
                "https://{endpoint_id}.{}",
                state::ENDPOINT_URL_DOMAIN
            )),
            "case {case:?}: the URL is the EndpointId under the published domain, got {read}"
        );
        // `CreateEndpointResponse` models no `EndpointId` member (its members are
        // Name, Arn, RoutingConfig, ReplicationConfig, EventBuses, RoleArn and
        // State), so the subdomain is read back rather than asserted on the
        // create; what the create must report is the identity it stored.
        assert_eq!(
            created["Name"], read["Name"],
            "case {case:?}: the create reports the name a read reports, got {created}"
        );
        assert_eq!(
            created["Arn"], read["Arn"],
            "case {case:?}: the create reports the ARN a read reports, got {created}"
        );
        let (_, again) = events(&router, "DescribeEndpoint", &json!({ "Name": name })).await;
        assert_eq!(
            again["EndpointId"], read["EndpointId"],
            "case {case:?}: the subdomain must not change because the endpoint was read, \
             got {again}"
        );

        if let Some(other) = seen.insert(endpoint_id.clone(), name.to_string()) {
            panic!(
                "case {case:?}: expected endpoint {name:?} to be published at a subdomain of \
                 its own, got {endpoint_id:?}, which endpoint {other:?} already carries"
            );
        }
    }
}

/// The same rule over a COUNT rather than a handful of rows: a run of endpoints
/// created one after another, with other mints of the same store interleaved,
/// each reads back a modelled subdomain of its own.
#[tokio::test]
async fn a_run_of_creates_hands_out_no_subdomain_twice() {
    let router = emulator();
    let mut seen: BTreeMap<String, String> = BTreeMap::new();

    for index in 0..12u32 {
        let name = format!("run-{index}");
        let request = create_request(&name, "orders", "us-west-2", "abcdef0123456789");
        let (status, created) = events(&router, "CreateEndpoint", &request).await;
        assert_eq!(
            status, 200,
            "case {name:?}: CreateEndpoint answered {status} {created}"
        );

        // Events are minted from the SAME counter as an endpoint's subdomain, so
        // a run with deliveries between the creates is the case where a
        // subdomain built from part of that counter could repeat.
        let (status, published) = events(
            &router,
            "PutEvents",
            &json!({
                "Entries": [{
                    "Source": "identity.test",
                    "DetailType": "endpoint-identity",
                    "Detail": format!("{{\"index\":{index}}}"),
                }],
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?}: PutEvents answered {status} {published}"
        );

        let (status, read) = events(&router, "DescribeEndpoint", &json!({ "Name": name })).await;
        assert_eq!(
            status, 200,
            "case {name:?}: DescribeEndpoint answered {status} {read}"
        );
        let endpoint_id = read["EndpointId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {name:?}: expected an EndpointId, got {read}"))
            .to_string();
        assert!(
            endpoint_id_violation(&endpoint_id).is_none(),
            "case {name:?}: {}, got {endpoint_id:?}",
            endpoint_id_violation(&endpoint_id).unwrap_or_default()
        );
        if let Some(other) = seen.insert(endpoint_id.clone(), name.clone()) {
            panic!(
                "case {name:?}: expected endpoint {name:?} to be published at a subdomain of \
                 its own, got {endpoint_id:?}, which endpoint {other:?} already carries"
            );
        }
    }
}
