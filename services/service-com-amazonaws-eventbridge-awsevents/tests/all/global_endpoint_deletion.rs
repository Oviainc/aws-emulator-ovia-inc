// SPDX-License-Identifier: CC0-1.0

//! `DeleteEndpoint` end to end over the wire (`awsJson1_1`, `POST /` with
//! `X-Amz-Target: AWSEvents.<Operation>`) through the real `MultiServiceRouter`.
//!
//! The RULES asserted here:
//!
//! * a delete removes the global endpoint its `Name` names from this account's
//!   state, whatever state that endpoint was in, and answers no members at all
//!   (`DeleteEndpointResponse` models none) — after which every reader agrees it
//!   is gone: a read reports it missing with the message that names the endpoint
//!   and the Region searched, and no listing reports it;
//! * the delete is IDEMPOTENT in the name it names: a name this account holds no
//!   endpoint under is answered with the same success as one it does, so a
//!   repeated or late delete is accepted rather than refused;
//! * a delete reaches exactly the endpoint it names: every other endpoint is
//!   still reported, unchanged, in name order;
//! * the name of a removed endpoint is free again, and what is created under it
//!   is a NEW endpoint — the subdomain is minted with the endpoint, never derived
//!   from its name.
//!
//! The tables span the class rather than one recorded case: their rows differ
//! along the form and length of the endpoint's name (`EndpointName` is modelled
//! `length 1..64, pattern ^[\.\-_A-Za-z0-9]+$`), the endpoint's description, its
//! replication state and role, the bus name and the Regions of its pair, and
//! whether the delete arrives while the create's own transition is still
//! outstanding or after it settled.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_eventbridge_awsevents::state;
use service_com_amazonaws_eventbridge_awsevents::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below is signed for, which is the home Region of
/// every endpoint it creates.
const REGION: &str = "eu-central-1";

/// How long a test waits before reading a resource whose next state the clock
/// owes it: the span itself plus enough slack that a loaded machine does not make
/// the wait the flaky part of the assertion.
fn past_the_transition(span: Duration) -> Duration {
    span + Duration::from_millis(250)
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

/// The wire error code of a failure body (`__type` on awsJson), without its
/// namespace.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .map(|name| name.rsplit('#').next().unwrap_or(name).to_string())
        .unwrap_or_default()
}

/// The ARN of the bus `name` in `region`, in the `NonPartnerEventBusArn` form the
/// model gives `EndpointEventBus$EventBusArn`.
fn bus_arn(region: &str, name: &str) -> String {
    format!(
        "arn:aws:events:{region}:{}:event-bus/{name}",
        engine::emulator_account_id()
    )
}

/// The ARN of an IAM role, in the form `CreateEndpoint$RoleArn` is modelled with.
fn role_arn(name: &str) -> String {
    format!("arn:aws:iam::{}:role/{name}", engine::emulator_account_id())
}

/// The routing configuration of an endpoint whose secondary Region is `route`,
/// watching the Route 53 health check `health_check` (the form
/// `Primary$HealthCheck` is modelled with).
fn routing_config(health_check: &str, route: &str) -> Value {
    json!({
        "FailoverConfig": {
            "Primary": { "HealthCheck": format!("arn:aws:route53:::healthcheck/{health_check}") },
            "Secondary": { "Route": route },
        },
    })
}

/// The pair of event buses of an endpoint: ONE bus name in the home Region and in
/// `secondary`.
fn event_buses(bus: &str, secondary: &str) -> Value {
    json!([
        { "EventBusArn": bus_arn(REGION, bus) },
        { "EventBusArn": bus_arn(secondary, bus) },
    ])
}

/// The names an endpoint listing reports, in the order it reports them.
fn listed_names(listing: &Value) -> Vec<String> {
    listing["Endpoints"]
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .map(|entry| entry["Name"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// Whether a body carries no members at all, which is what an operation that
/// models no output answers.
fn carries_no_members(body: &Value) -> bool {
    body.is_null() || body.as_object().is_some_and(|members| members.is_empty())
}

/// One endpoint a table creates.
struct EndpointCase {
    case: &'static str,
    /// The endpoint's name, which is also what every refusal must name.
    name: &'static str,
    /// The bus name the pair carries, identical in both Regions.
    bus: &'static str,
    /// The secondary Region of the pair and of the routing policy.
    secondary: &'static str,
    /// The health check id the failover policy watches.
    health_check: &'static str,
    /// Extra members of the `CreateEndpoint` request.
    extra: Value,
    /// Whether the delete waits for the create's own transition to settle, so
    /// the endpoint is deleted out of `ACTIVE` rather than out of `CREATING`.
    settle_before_delete: bool,
}

/// The `CreateEndpoint` request of one table row.
fn create_request(endpoint: &EndpointCase) -> Value {
    let mut request = json!({
        "Name": endpoint.name,
        "RoutingConfig": routing_config(endpoint.health_check, endpoint.secondary),
        "EventBuses": event_buses(endpoint.bus, endpoint.secondary),
    });
    for (member, value) in endpoint
        .extra
        .as_object()
        .expect("the extra members are an object")
    {
        request[member] = value.clone();
    }
    request
}

fn deletion_cases() -> Vec<EndpointCase> {
    vec![
        EndpointCase {
            case: "deleted while the create's transition is still outstanding",
            name: "orders-endpoint",
            bus: "orders",
            secondary: "us-west-2",
            health_check: "abcdef0123456789",
            extra: json!({ "RoleArn": role_arn("global-endpoint-replication") }),
            settle_before_delete: false,
        },
        EndpointCase {
            case: "deleted after the endpoint settled into ACTIVE",
            name: "shipments.v2_endpoint",
            bus: "shipments.v2",
            secondary: "us-east-1",
            health_check: "0123456789abcdef",
            extra: json!({
                "Description": "the shipments global endpoint",
                "ReplicationConfig": { "State": state::REPLICATION_STATE_DISABLED },
            }),
            settle_before_delete: true,
        },
        EndpointCase {
            case: "replication enabled explicitly, with a role on a path",
            name: "A.b-c_D9",
            bus: "bus-with-hyphens",
            secondary: "ap-southeast-1",
            health_check: "health-check-7",
            extra: json!({
                "Description": "",
                "ReplicationConfig": { "State": state::REPLICATION_STATE_ENABLED },
                "RoleArn": role_arn("replication/path+role"),
            }),
            settle_before_delete: true,
        },
    ]
}

/// A delete removes the endpoint it names, answers no members, and is idempotent
/// in that name: the name holds nothing afterwards, a second delete is accepted,
/// and the name is free for another create.
#[tokio::test]
async fn a_deleted_endpoint_is_removed_and_the_delete_is_idempotent() {
    for endpoint in deletion_cases() {
        let case = endpoint.case;
        let name = endpoint.name;
        let router = emulator();

        let (status, created) = events(&router, "CreateEndpoint", &create_request(&endpoint)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateEndpoint answered {status} {created}"
        );
        if endpoint.settle_before_delete {
            tokio::time::sleep(past_the_transition(state::ENDPOINT_TRANSITION)).await;
        }
        let (status, before) = events(&router, "DescribeEndpoint", &json!({ "Name": name })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeEndpoint answered {status} {before}"
        );

        let (status, deleted) = events(&router, "DeleteEndpoint", &json!({ "Name": name })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DeleteEndpoint answered {status} {deleted}"
        );
        assert!(
            carries_no_members(&deleted),
            "case {case:?}: expected a body with no members, got {deleted:?}"
        );

        // Every reader agrees the endpoint is gone.
        let (status, gone) = events(&router, "DescribeEndpoint", &json!({ "Name": name })).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a deleted endpoint is reported missing, got {status} {gone}"
        );
        assert_eq!(
            error_code(&gone),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {gone}"
        );
        assert_eq!(
            gone["message"],
            json!(format!("Endpoint {name} does not exist in region {REGION}.")),
            "case {case:?}: expected the missing endpoint and the Region searched to be \
             named, got {gone}"
        );
        assert!(
            listed_names(&events(&router, "ListEndpoints", &json!({})).await.1).is_empty(),
            "case {case:?}: a deleted endpoint is listed nowhere"
        );

        // The delete is idempotent in the name: deleting what is already gone is
        // the same success, not a refusal.
        let (status, again) = events(&router, "DeleteEndpoint", &json!({ "Name": name })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a repeated DeleteEndpoint answered {status} {again}"
        );
        assert!(
            carries_no_members(&again),
            "case {case:?}: expected a body with no members, got {again:?}"
        );

        // The name is free again, and what is created under it is a NEW
        // endpoint: its subdomain is minted with it, never derived from the name.
        let (status, recreated) =
            events(&router, "CreateEndpoint", &create_request(&endpoint)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the name of a deleted endpoint is free again, got {status} {recreated}"
        );
        assert_eq!(
            recreated["State"],
            json!(state::ENDPOINT_STATE_CREATING),
            "case {case:?}: a created endpoint is answered in {}, got {recreated}",
            state::ENDPOINT_STATE_CREATING
        );
        let (status, read_back) =
            events(&router, "DescribeEndpoint", &json!({ "Name": name })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeEndpoint answered {status} {read_back}"
        );
        assert_ne!(
            read_back["EndpointId"], before["EndpointId"],
            "case {case:?}: a recreated endpoint carries its own subdomain, got {read_back}"
        );
    }
}

/// A delete of a name this account holds no endpoint under is accepted with no
/// members, and it removes nothing else.
#[tokio::test]
async fn deleting_an_endpoint_no_create_stored_is_accepted() {
    let router = emulator();
    // One stored endpoint, so the accepted deletes below are shown not to depend
    // on the store being empty — and so the "removes nothing else" assertion has
    // something to read back.
    let stored = EndpointCase {
        case: "the endpoint no delete below names",
        name: "the-stored-endpoint",
        bus: "orders",
        secondary: "us-west-2",
        health_check: "abcdef0123456789",
        extra: json!({ "RoleArn": role_arn("global-endpoint-replication") }),
        settle_before_delete: true,
    };
    let (status, created) = events(&router, "CreateEndpoint", &create_request(&stored)).await;
    assert_eq!(status, 200, "CreateEndpoint answered {status} {created}");

    // The names differ along their form and length: the shortest and the longest
    // name `EndpointName`'s modelled `length 1..64` admits are both exercised
    // beside the dotted, underscored and mixed-case forms its pattern admits.
    let mut names: Vec<String> = [
        "a",
        "never-created",
        "the-stored-endpoint.suffix",
        "The_Endpoint.9",
    ]
    .iter()
    .map(|name| (*name).to_string())
    .collect();
    names.push("e".repeat(64));

    for name in names {
        let (status, accepted) = events(&router, "DeleteEndpoint", &json!({ "Name": name })).await;
        assert_eq!(
            status, 200,
            "case {name:?}: DeleteEndpoint answered {status} {accepted}"
        );
        assert!(
            carries_no_members(&accepted),
            "case {name:?}: expected a body with no members, got {accepted:?}"
        );
    }

    // The rule this table ends on: a delete that named no stored endpoint
    // removed none.
    let (status, survivor) =
        events(&router, "DescribeEndpoint", &json!({ "Name": stored.name })).await;
    assert_eq!(
        status, 200,
        "a delete of another name leaves the stored endpoint alone, got {status} {survivor}"
    );
    assert_eq!(
        survivor["Arn"], created["Arn"],
        "the stored endpoint is left exactly as it was, got {survivor}"
    );
    assert_eq!(
        listed_names(&events(&router, "ListEndpoints", &json!({})).await.1),
        vec![stored.name.to_string()],
        "a delete of another name leaves the listing alone"
    );
}

/// A delete reaches exactly the endpoint it names: the others are still reported,
/// unchanged and in name order.
#[tokio::test]
async fn a_delete_reaches_only_the_endpoint_it_names() {
    let router = emulator();
    let endpoints = [
        EndpointCase {
            case: "the first endpoint in name order",
            name: "alpha-endpoint",
            bus: "orders",
            secondary: "us-west-2",
            health_check: "abcdef0123456789",
            extra: json!({ "RoleArn": role_arn("alpha-replication") }),
            settle_before_delete: false,
        },
        EndpointCase {
            case: "the endpoint the delete names",
            name: "beta.endpoint_2",
            bus: "shipments",
            secondary: "us-east-1",
            health_check: "0123456789abcdef",
            extra: json!({ "ReplicationConfig": { "State": state::REPLICATION_STATE_DISABLED } }),
            settle_before_delete: false,
        },
        EndpointCase {
            case: "the last endpoint in name order",
            name: "gamma-endpoint",
            bus: "invoices",
            secondary: "ap-southeast-1",
            health_check: "health-check-7",
            extra: json!({
                "Description": "the invoices global endpoint",
                "RoleArn": role_arn("gamma-replication"),
            }),
            settle_before_delete: false,
        },
    ];
    for endpoint in &endpoints {
        let (status, created) = events(&router, "CreateEndpoint", &create_request(endpoint)).await;
        assert_eq!(
            status, 200,
            "case {:?}: CreateEndpoint answered {status} {created}",
            endpoint.case
        );
    }
    tokio::time::sleep(past_the_transition(state::ENDPOINT_TRANSITION)).await;

    // What each surviving endpoint reports BEFORE the delete, so the delete can
    // be shown to have left it alone.
    let mut before: Vec<(&str, Value)> = Vec::new();
    for endpoint in &endpoints {
        if endpoint.name == "beta.endpoint_2" {
            continue;
        }
        let (status, read) =
            events(&router, "DescribeEndpoint", &json!({ "Name": endpoint.name })).await;
        assert_eq!(
            status, 200,
            "case {:?}: DescribeEndpoint answered {status} {read}",
            endpoint.case
        );
        before.push((endpoint.name, read));
    }

    let (status, deleted) =
        events(&router, "DeleteEndpoint", &json!({ "Name": "beta.endpoint_2" })).await;
    assert_eq!(
        status, 200,
        "DeleteEndpoint answered {status} {deleted}"
    );

    assert_eq!(
        listed_names(&events(&router, "ListEndpoints", &json!({})).await.1),
        vec!["alpha-endpoint".to_string(), "gamma-endpoint".to_string()],
        "expected exactly the endpoints the delete did not name, in name order"
    );
    for (name, read_before) in before {
        let (status, read_after) = events(&router, "DescribeEndpoint", &json!({ "Name": name })).await;
        assert_eq!(
            status, 200,
            "case {name:?}: DescribeEndpoint answered {status} {read_after}"
        );
        assert_eq!(
            read_after, read_before,
            "case {name:?}: a delete of another endpoint changes nothing here, got {read_after}"
        );
    }
}
