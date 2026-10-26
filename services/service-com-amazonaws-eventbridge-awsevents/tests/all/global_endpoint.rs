// SPDX-License-Identifier: CC0-1.0

//! The global-endpoint resource end to end over the wire (`awsJson1_1`,
//! `POST /` with `X-Amz-Target: AWSEvents.<Operation>`) through the real
//! `MultiServiceRouter`: `CreateEndpoint` writes an endpoint and
//! `DescribeEndpoint` reports it.
//!
//! The RULES asserted here:
//!
//! * a reporting read answers from PERSISTED state — an endpoint a create
//!   stored reads back under its name with every member it was created with, and
//!   a name no create stored is `ResourceNotFoundException` (HTTP 400);
//! * a request the service refuses stores NOTHING, so the read that follows any
//!   refused create reports the endpoint as missing — whatever the request was
//!   wrong about;
//! * an endpoint AWS answers in a non-terminal state reaches its next one on its
//!   own: `CREATING` -> `ACTIVE`, and a caller reading straight after the create
//!   still observes `CREATING`;
//! * a read changes nothing a later caller sees, and the identity a create minted
//!   (the ARN, the subdomain and the URL built from it) is reported unchanged by
//!   every read;
//! * `HomeRegion` SELECTS the endpoint the read is about: a read naming the
//!   Region the endpoint was created in finds it, and one naming another Region
//!   finds nothing;
//! * the endpoint's name is its identity (a second create is
//!   `ResourceAlreadyExistsException`, and the stored endpoint survives it).
//!
//! Each table spans the class rather than one recorded case: the rows differ
//! along the endpoint's description, its replication state and role, the bus name
//! and Regions of its pair, its secondary route and its health check — and, for
//! the refusals, along which member of the request is wrong (a malformed value,
//! a value outside a modelled enum, a list of the wrong length, and two
//! combinations of members that cannot hold together).

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_eventbridge_awsevents::state;
use service_com_amazonaws_eventbridge_awsevents::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below is signed for, which is the home Region of
/// every endpoint it creates.
const REGION: &str = "eu-central-1";

/// A Region no request below is signed for, used to show that `HomeRegion`
/// selects the endpoint rather than being ignored.
const OTHER_REGION: &str = "us-east-1";

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

/// The ARN of a Route 53 health check, in the form `Primary$HealthCheck` is
/// modelled with (`^arn:aws([a-z]|\-)*:route53:::healthcheck/[\-a-z0-9]+$`).
fn health_check_arn(id: &str) -> String {
    format!("arn:aws:route53:::healthcheck/{id}")
}

/// The ARN of an IAM role, in the form `CreateEndpoint$RoleArn` is modelled with.
fn role_arn(name: &str) -> String {
    format!(
        "arn:aws:iam::{}:role/{name}",
        engine::emulator_account_id()
    )
}

/// The routing configuration of an endpoint whose secondary Region is `route`.
fn routing_config(health_check: &str, route: &str) -> Value {
    json!({
        "FailoverConfig": {
            "Primary": { "HealthCheck": health_check_arn(health_check) },
            "Secondary": { "Route": route },
        },
    })
}

/// The pair of event buses of an endpoint: ONE bus name in the home Region and
/// in `secondary`.
fn event_buses(bus: &str, secondary: &str) -> Value {
    json!([
        { "EventBusArn": bus_arn(REGION, bus) },
        { "EventBusArn": bus_arn(secondary, bus) },
    ])
}

/// One endpoint the create table writes.
struct EndpointCase {
    case: &'static str,
    /// The bus name the pair carries, identical in both Regions.
    bus: &'static str,
    /// The secondary Region of the pair and of the routing policy.
    secondary: &'static str,
    /// The health check id the failover policy watches.
    health_check: &'static str,
    /// Extra members of the `CreateEndpoint` request.
    extra: Value,
    /// The `ReplicationConfig.State` the endpoint must report, which is the one
    /// the request named or the default it left out.
    replication: &'static str,
}

/// A created endpoint carries the identity AWS mints for it, reads back under its
/// name with every member it was created with, and settles into `ACTIVE` on its
/// own.
#[tokio::test]
async fn a_created_endpoint_is_read_back_and_settles_into_active() {
    let cases = [
        EndpointCase {
            case: "replication left out, which defaults it to ENABLED",
            bus: "orders",
            secondary: "us-west-2",
            health_check: "abcdef0123456789",
            extra: json!({ "RoleArn": role_arn("global-endpoint-replication") }),
            replication: state::REPLICATION_STATE_ENABLED,
        },
        EndpointCase {
            case: "replication disabled, so no role is needed",
            bus: "shipments.v2",
            secondary: OTHER_REGION,
            health_check: "0123456789abcdef",
            extra: json!({
                "Description": "the shipments global endpoint",
                "ReplicationConfig": { "State": state::REPLICATION_STATE_DISABLED },
            }),
            replication: state::REPLICATION_STATE_DISABLED,
        },
        EndpointCase {
            case: "replication enabled explicitly, with a description and a role",
            bus: "bus-with-hyphens",
            secondary: "ap-southeast-1",
            health_check: "health-check-7",
            extra: json!({
                "Description": "",
                "ReplicationConfig": { "State": state::REPLICATION_STATE_ENABLED },
                "RoleArn": role_arn("replication/path+role"),
            }),
            replication: state::REPLICATION_STATE_ENABLED,
        },
    ];

    for EndpointCase {
        case,
        bus,
        secondary,
        health_check,
        extra,
        replication,
    } in cases
    {
        let router = emulator();
        let name = "the-endpoint";
        let routing = routing_config(health_check, secondary);
        let buses = event_buses(bus, secondary);
        let mut request = json!({
            "Name": name,
            "RoutingConfig": routing,
            "EventBuses": buses,
        });
        for (member, value) in extra.as_object().expect("the extra members are an object") {
            request[member] = value.clone();
        }

        let (status, created) = events(&router, "CreateEndpoint", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateEndpoint answered {status} {created}"
        );
        let expected_arn = format!(
            "arn:aws:events:{REGION}:{}:endpoint/{name}",
            engine::emulator_account_id()
        );
        assert_eq!(
            created["Arn"],
            json!(expected_arn),
            "case {case:?}: expected the ARN {expected_arn:?}, got {created}"
        );
        assert_eq!(
            created["Name"],
            json!(name),
            "case {case:?}: the create reports the name it stored, got {created}"
        );
        assert_eq!(
            created["State"],
            json!(state::ENDPOINT_STATE_CREATING),
            "case {case:?}: a created endpoint is answered in {}, got {created}",
            state::ENDPOINT_STATE_CREATING
        );
        assert_eq!(
            created["RoutingConfig"], routing,
            "case {case:?}: the create reports the routing policy it stored, got {created}"
        );
        assert_eq!(
            created["EventBuses"], buses,
            "case {case:?}: the create reports the bus pair it stored, got {created}"
        );
        assert_eq!(
            created["ReplicationConfig"]["State"],
            json!(replication),
            "case {case:?}: expected the replication state {replication:?}, got {created}"
        );

        // The transient state is OBSERVABLE: a caller reading straight after the
        // create still sees it.
        let (status, transient) =
            events(&router, "DescribeEndpoint", &json!({ "Name": name })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeEndpoint answered {status} {transient}"
        );
        assert_eq!(
            transient["State"],
            json!(state::ENDPOINT_STATE_CREATING),
            "case {case:?}: a read taken at once still reports {}, got {transient}",
            state::ENDPOINT_STATE_CREATING
        );

        tokio::time::sleep(past_the_transition(state::ENDPOINT_TRANSITION)).await;
        let (status, settled) = events(&router, "DescribeEndpoint", &json!({ "Name": name })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeEndpoint answered {status} {settled}"
        );
        assert_eq!(
            settled["State"],
            json!(state::ENDPOINT_STATE_ACTIVE),
            "case {case:?}: the clock owes the endpoint {}, got {settled}",
            state::ENDPOINT_STATE_ACTIVE
        );
        assert_eq!(
            settled["Arn"],
            json!(expected_arn),
            "case {case:?}: a read reports the stored ARN, got {settled}"
        );
        assert_eq!(
            settled["RoutingConfig"], routing,
            "case {case:?}: a read reports the stored routing policy, got {settled}"
        );
        assert_eq!(
            settled["EventBuses"], buses,
            "case {case:?}: a read reports the stored bus pair, got {settled}"
        );
        assert_eq!(
            settled["ReplicationConfig"]["State"],
            json!(replication),
            "case {case:?}: a read reports the replication in force, got {settled}"
        );
        assert_eq!(
            settled["RoleArn"], created["RoleArn"],
            "case {case:?}: a read reports the role the create stored, got {settled}"
        );
        assert_eq!(
            settled["Description"], request["Description"],
            "case {case:?}: a read reports the description the create stored, got {settled}"
        );
        assert!(
            settled["CreationTime"].as_f64().is_some(),
            "case {case:?}: expected a CreationTime on a read, got {settled}"
        );
        assert!(
            settled["LastModifiedTime"].as_f64().is_some(),
            "case {case:?}: expected a LastModifiedTime on a read, got {settled}"
        );

        // The subdomain and the URL built from it are ONE fact: "if the URL for
        // Endpoint is https://abcde.veo.endpoints.event.amazonaws.com, then the
        // EndpointId is abcde.veo" (`Endpoint$EndpointId`).
        let endpoint_id = settled["EndpointId"].as_str().unwrap_or_default();
        let (subdomain, suffix) = endpoint_id.split_once('.').unwrap_or_default();
        assert!(
            !subdomain.is_empty()
                && !suffix.is_empty()
                && endpoint_id
                    .chars()
                    .all(|character| character.is_ascii_lowercase()
                        || character.is_ascii_digit()
                        || character == '-'
                        || character == '.'),
            "case {case:?}: expected an EndpointId of two dot-separated segments, \
             got {endpoint_id:?}"
        );
        assert_eq!(
            settled["EndpointUrl"],
            json!(format!(
                "https://{endpoint_id}.{}",
                state::ENDPOINT_URL_DOMAIN
            )),
            "case {case:?}: the URL is the EndpointId under the published domain, got {settled}"
        );

        // Nothing about the endpoint moves because it was read.
        let (_, again) = events(&router, "DescribeEndpoint", &json!({ "Name": name })).await;
        for member in [
            "Arn",
            "State",
            "EndpointId",
            "EndpointUrl",
            "CreationTime",
            "LastModifiedTime",
        ] {
            assert_eq!(
                again[member], settled[member],
                "case {case:?}: {member} must not change because the endpoint was read, \
                 got {again}"
            );
        }
    }
}

/// `HomeRegion` selects the endpoint the read is about: the Region the endpoint
/// was created in finds it, and any other Region finds nothing.
#[tokio::test]
async fn a_read_finds_the_endpoint_only_in_its_home_region() {
    let router = emulator();
    let name = "home-region-endpoint";
    let (status, created) = events(
        &router,
        "CreateEndpoint",
        &json!({
            "Name": name,
            "RoutingConfig": routing_config("abcdef0123456789", "us-west-2"),
            "EventBuses": event_buses("orders", "us-west-2"),
            "ReplicationConfig": { "State": state::REPLICATION_STATE_DISABLED },
        }),
    )
    .await;
    assert_eq!(status, 200, "CreateEndpoint answered {status} {created}");

    // (the HomeRegion the read names, whether the endpoint is found)
    let cases: [(Option<&str>, bool); 4] = [
        (None, true),
        (Some(REGION), true),
        (Some(OTHER_REGION), false),
        (Some("ap-southeast-1"), false),
    ];
    for (home_region, found) in cases {
        let mut request = json!({ "Name": name });
        if let Some(home_region) = home_region {
            request["HomeRegion"] = json!(home_region);
        }
        let (status, body) = events(&router, "DescribeEndpoint", &request).await;
        if found {
            assert_eq!(
                status, 200,
                "case {home_region:?}: expected the endpoint to be found, got {status} {body}"
            );
            assert_eq!(
                body["Name"],
                json!(name),
                "case {home_region:?}: expected the endpoint {name:?}, got {body}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {home_region:?}: expected HTTP 400, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {home_region:?}: expected ResourceNotFoundException, got {body}"
            );
        }
    }
}

/// A name no create stored is reported missing, by name, whether or not other
/// endpoints exist.
#[tokio::test]
async fn an_endpoint_no_create_stored_is_reported_missing() {
    let router = emulator();
    let (status, body) = events(
        &router,
        "CreateEndpoint",
        &json!({
            "Name": "stored-endpoint",
            "RoutingConfig": routing_config("abcdef0123456789", "us-west-2"),
            "EventBuses": event_buses("orders", "us-west-2"),
            "ReplicationConfig": { "State": state::REPLICATION_STATE_DISABLED },
        }),
    )
    .await;
    assert_eq!(status, 200, "CreateEndpoint answered {status} {body}");

    for name in [
        "never-created",
        "stored-endpoint.suffix",
        "Stored-Endpoint",
        "a",
    ] {
        let (status, body) = events(&router, "DescribeEndpoint", &json!({ "Name": name })).await;
        assert_eq!(status, 400, "case {name:?}: expected HTTP 400, got {body}");
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {name:?}: expected ResourceNotFoundException, got {body}"
        );
        assert_eq!(
            body["message"],
            json!(format!("Endpoint {name} does not exist.")),
            "case {name:?}: expected the missing endpoint to be named, got {body}"
        );
    }
}

/// One refused `CreateEndpoint` request.
struct RefusalCase {
    case: &'static str,
    /// The members of the request, beyond the name.
    request: Value,
}

/// A refused create stores NOTHING: the read that follows it reports the endpoint
/// as missing, whatever the request was wrong about.
#[tokio::test]
async fn a_refused_create_stores_no_endpoint() {
    let cases = [
        RefusalCase {
            case: "a health check that is not a Route 53 health-check ARN",
            request: json!({
                "RoutingConfig": {
                    "FailoverConfig": {
                        "Primary": { "HealthCheck": "not-a-health-check" },
                        "Secondary": { "Route": "us-west-2" },
                    },
                },
                "EventBuses": event_buses("orders", "us-west-2"),
                "ReplicationConfig": { "State": state::REPLICATION_STATE_DISABLED },
            }),
        },
        RefusalCase {
            case: "a routing policy with no failover configuration",
            request: json!({
                "RoutingConfig": {},
                "EventBuses": event_buses("orders", "us-west-2"),
                "ReplicationConfig": { "State": state::REPLICATION_STATE_DISABLED },
            }),
        },
        RefusalCase {
            case: "a secondary route that is not a Region",
            request: json!({
                "RoutingConfig": routing_config("abcdef0123456789", "US-WEST-2"),
                "EventBuses": event_buses("orders", "us-west-2"),
                "ReplicationConfig": { "State": state::REPLICATION_STATE_DISABLED },
            }),
        },
        RefusalCase {
            case: "one event bus instead of the pair the model requires",
            request: json!({
                "RoutingConfig": routing_config("abcdef0123456789", "us-west-2"),
                "EventBuses": [{ "EventBusArn": bus_arn(REGION, "orders") }],
                "ReplicationConfig": { "State": state::REPLICATION_STATE_DISABLED },
            }),
        },
        RefusalCase {
            case: "a replication state outside the modelled enum",
            request: json!({
                "RoutingConfig": routing_config("abcdef0123456789", "us-west-2"),
                "EventBuses": event_buses("orders", "us-west-2"),
                "ReplicationConfig": { "State": "SOMETIMES" },
                "RoleArn": role_arn("replication"),
            }),
        },
        RefusalCase {
            case: "replication enabled by default with no role to replicate as",
            request: json!({
                "RoutingConfig": routing_config("abcdef0123456789", "us-west-2"),
                "EventBuses": event_buses("orders", "us-west-2"),
            }),
        },
        RefusalCase {
            case: "replication enabled explicitly with no role to replicate as",
            request: json!({
                "RoutingConfig": routing_config("abcdef0123456789", "us-west-2"),
                "EventBuses": event_buses("orders", "us-west-2"),
                "ReplicationConfig": { "State": state::REPLICATION_STATE_ENABLED },
            }),
        },
        RefusalCase {
            case: "two differently named event buses",
            request: json!({
                "RoutingConfig": routing_config("abcdef0123456789", "us-west-2"),
                "EventBuses": [
                    { "EventBusArn": bus_arn(REGION, "orders") },
                    { "EventBusArn": bus_arn("us-west-2", "shipments") },
                ],
                "ReplicationConfig": { "State": state::REPLICATION_STATE_DISABLED },
            }),
        },
        RefusalCase {
            case: "both event buses in one Region",
            request: json!({
                "RoutingConfig": routing_config("abcdef0123456789", "us-west-2"),
                "EventBuses": [
                    { "EventBusArn": bus_arn(REGION, "orders") },
                    { "EventBusArn": bus_arn(REGION, "orders") },
                ],
                "ReplicationConfig": { "State": state::REPLICATION_STATE_DISABLED },
            }),
        },
    ];

    for RefusalCase { case, request } in cases {
        let router = emulator();
        let name = "refused-endpoint";
        let mut body = json!({ "Name": name });
        for (member, value) in request.as_object().expect("the request is an object") {
            body[member] = value.clone();
        }

        let (status, refused) = events(&router, "CreateEndpoint", &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the create to be refused with HTTP 400, got \
             {status} {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {refused}"
        );

        // The rule this whole table is about: a refused create created nothing.
        let (status, absent) = events(&router, "DescribeEndpoint", &json!({ "Name": name })).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a refused create must leave no endpoint, got {status} {absent}"
        );
        assert_eq!(
            error_code(&absent),
            "ResourceNotFoundException",
            "case {case:?}: a refused create must leave no endpoint, got {absent}"
        );
        assert_eq!(
            absent["message"],
            json!(format!("Endpoint {name} does not exist.")),
            "case {case:?}: expected the missing endpoint to be named, got {absent}"
        );
    }
}

/// The endpoint's name is its identity: a second create under one name is
/// refused and the stored endpoint survives it.
#[tokio::test]
async fn a_second_create_under_one_name_is_refused_and_changes_nothing() {
    let router = emulator();
    let name = "twice-created";
    let first = json!({
        "Name": name,
        "Description": "the first endpoint",
        "RoutingConfig": routing_config("abcdef0123456789", "us-west-2"),
        "EventBuses": event_buses("orders", "us-west-2"),
        "ReplicationConfig": { "State": state::REPLICATION_STATE_DISABLED },
    });
    let (status, created) = events(&router, "CreateEndpoint", &first).await;
    assert_eq!(status, 200, "CreateEndpoint answered {status} {created}");
    let (_, stored) = events(&router, "DescribeEndpoint", &json!({ "Name": name })).await;

    let (status, refused) = events(
        &router,
        "CreateEndpoint",
        &json!({
            "Name": name,
            "Description": "the second endpoint",
            "RoutingConfig": routing_config("0123456789abcdef", OTHER_REGION),
            "EventBuses": event_buses("shipments", OTHER_REGION),
            "ReplicationConfig": { "State": state::REPLICATION_STATE_DISABLED },
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "a second create under one name is refused, got {status} {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "ResourceAlreadyExistsException",
        "expected ResourceAlreadyExistsException, got {refused}"
    );
    assert_eq!(
        refused["message"],
        json!(format!("Endpoint {name} already exists.")),
        "expected the endpoint that already exists to be named, got {refused}"
    );

    let (status, again) = events(&router, "DescribeEndpoint", &json!({ "Name": name })).await;
    assert_eq!(status, 200, "DescribeEndpoint answered {status} {again}");
    for member in ["Description", "RoutingConfig", "EventBuses", "EndpointId"] {
        assert_eq!(
            again[member], stored[member],
            "{member} must survive a refused second create, got {again}"
        );
    }
}
