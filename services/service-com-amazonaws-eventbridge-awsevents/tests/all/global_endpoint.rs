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
//!   a name no create stored is `ResourceNotFoundException` (HTTP 400) whose
//!   message names the endpoint AND the Region it was looked for in;
//! * `UpdateEndpoint` applies exactly the members its request carries to the
//!   stored endpoint, leaves every member it does not carry alone, never moves
//!   the identity the create minted, and reports the merged record — which a
//!   later read reports identically;
//! * a request the service refuses stores NOTHING, so the read that follows any
//!   refused create reports the endpoint as missing — whatever the request was
//!   wrong about — and a refused write leaves the stored endpoint exactly as it
//!   was;
//! * an endpoint AWS answers in a non-terminal state reaches its next one on its
//!   own: `CREATING` -> `ACTIVE` and `UPDATING` -> `ACTIVE`, and a caller
//!   reading straight after the call still observes the transient state;
//! * a read changes nothing a later caller sees, and the identity a create minted
//!   (the ARN, the subdomain and the URL built from it) is reported unchanged by
//!   every read;
//! * `HomeRegion` SELECTS the endpoint the read is about: a read naming the
//!   Region the endpoint was created in finds it, and one naming another Region
//!   finds nothing;
//! * a listing reports the endpoints PERSISTED, in name order, narrowed by the
//!   members the request carries, each exactly as the single-endpoint read
//!   reports it, one page at a time, with a token only while a page remains;
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
            // The Region the refusal names is the one it searched: the
            // `HomeRegion` the read asked for, not the endpoint's own.
            let searched = home_region.unwrap_or(REGION);
            assert_eq!(
                body["message"],
                json!(format!(
                    "Endpoint {name} does not exist in region {searched}."
                )),
                "case {home_region:?}: expected the Region searched to be named, got {body}"
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
            json!(format!("Endpoint {name} does not exist in region {REGION}.")),
            "case {name:?}: expected the missing endpoint and the Region searched to be \
             named, got {body}"
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
            json!(format!("Endpoint {name} does not exist in region {REGION}.")),
            "case {case:?}: expected the missing endpoint and the Region searched to be \
             named, got {absent}"
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

/// The `CreateEndpoint` request every write table below starts from: one
/// endpoint carrying a description, a failover policy, a bus pair and
/// replication switched off, so each row can show a write reaching exactly the
/// members it names.
fn base_create(name: &str) -> Value {
    json!({
        "Name": name,
        "Description": "the original description",
        "RoutingConfig": routing_config("abcdef0123456789", "us-west-2"),
        "EventBuses": event_buses("orders", "us-west-2"),
        "ReplicationConfig": { "State": state::REPLICATION_STATE_DISABLED },
    })
}

/// Create that endpoint and read it back once the clock has settled it, so each
/// write below is applied to an endpoint that is serving.
async fn settled_endpoint(router: &MultiServiceRouter, name: &str) -> Value {
    let (status, created) = events(router, "CreateEndpoint", &base_create(name)).await;
    assert_eq!(status, 200, "CreateEndpoint answered {status} {created}");
    tokio::time::sleep(past_the_transition(state::ENDPOINT_TRANSITION)).await;
    let (status, stored) = events(router, "DescribeEndpoint", &json!({ "Name": name })).await;
    assert_eq!(status, 200, "DescribeEndpoint answered {status} {stored}");
    stored
}

/// One `UpdateEndpoint` request and the configuration the endpoint must hold
/// afterwards.
struct WriteCase {
    case: &'static str,
    /// The members of the write beyond the endpoint's name.
    request: Value,
    /// Every member the merged endpoint must report — those the write named and
    /// those it left to the create.
    expect: Value,
}

/// A write applies the members it carries, leaves every member it does not carry
/// alone, answers the endpoint's transient state, and is reported identically by
/// the read that follows once the clock has settled it.
#[tokio::test]
async fn a_write_applies_the_members_it_carries_and_leaves_the_rest() {
    let base_routing = routing_config("abcdef0123456789", "us-west-2");
    let base_buses = event_buses("orders", "us-west-2");
    let new_routing = routing_config("0123456789abcdef", "ap-southeast-1");
    let new_buses = event_buses("shipments.v2", OTHER_REGION);
    let replication_role = role_arn("global-endpoint-replication");
    let cases = [
        WriteCase {
            case: "a new description, and nothing else",
            request: json!({ "Description": "the updated description" }),
            expect: json!({
                "Description": "the updated description",
                "RoutingConfig": base_routing,
                "EventBuses": base_buses,
                "ReplicationConfig": { "State": state::REPLICATION_STATE_DISABLED },
                "RoleArn": null,
            }),
        },
        WriteCase {
            case: "a description emptied, which the modelled length admits",
            request: json!({ "Description": "" }),
            expect: json!({
                "Description": "",
                "RoutingConfig": base_routing,
                "EventBuses": base_buses,
            }),
        },
        WriteCase {
            case: "another failover policy: another health check and another secondary Region",
            request: json!({ "RoutingConfig": new_routing }),
            expect: json!({
                "Description": "the original description",
                "RoutingConfig": new_routing,
                "EventBuses": base_buses,
            }),
        },
        WriteCase {
            case: "another bus pair, in another pair of Regions",
            request: json!({ "EventBuses": new_buses }),
            expect: json!({
                "Description": "the original description",
                "RoutingConfig": base_routing,
                "EventBuses": new_buses,
            }),
        },
        WriteCase {
            case: "replication switched on, with the role it runs as",
            request: json!({
                "ReplicationConfig": { "State": state::REPLICATION_STATE_ENABLED },
                "RoleArn": replication_role,
            }),
            expect: json!({
                "Description": "the original description",
                "ReplicationConfig": { "State": state::REPLICATION_STATE_ENABLED },
                "RoleArn": replication_role,
            }),
        },
        WriteCase {
            case: "a role for a replication that stays switched off",
            request: json!({ "RoleArn": role_arn("standby") }),
            expect: json!({
                "ReplicationConfig": { "State": state::REPLICATION_STATE_DISABLED },
                "RoleArn": role_arn("standby"),
            }),
        },
        WriteCase {
            case: "every member the write can carry, at once",
            request: json!({
                "Description": "everything replaced",
                "RoutingConfig": new_routing,
                "EventBuses": new_buses,
                "ReplicationConfig": { "State": state::REPLICATION_STATE_ENABLED },
                "RoleArn": replication_role,
            }),
            expect: json!({
                "Description": "everything replaced",
                "RoutingConfig": new_routing,
                "EventBuses": new_buses,
                "ReplicationConfig": { "State": state::REPLICATION_STATE_ENABLED },
                "RoleArn": replication_role,
            }),
        },
    ];

    for WriteCase {
        case,
        request,
        expect,
    } in cases
    {
        let router = emulator();
        let name = "written-endpoint";
        let stored = settled_endpoint(&router, name).await;

        let mut body = json!({ "Name": name });
        for (member, value) in request.as_object().expect("the write is an object") {
            body[member] = value.clone();
        }
        let (status, written) = events(&router, "UpdateEndpoint", &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateEndpoint answered {status} {written}"
        );
        assert_eq!(
            written["State"],
            json!(state::ENDPOINT_STATE_UPDATING),
            "case {case:?}: a written endpoint is answered in {}, got {written}",
            state::ENDPOINT_STATE_UPDATING
        );
        for (member, value) in expect.as_object().expect("the expectation is an object") {
            // `UpdateEndpointResponse` models no `Description` member (its
            // members are Name, Arn, RoutingConfig, ReplicationConfig,
            // EventBuses, RoleArn, EndpointId, EndpointUrl and State), so the
            // description is asserted on the read below.
            if member == "Description" {
                continue;
            }
            assert_eq!(
                &written[member], value,
                "case {case:?}: expected {member} to be {value}, got {written}"
            );
        }
        // The endpoint's identity is not a member of the write, so a write never
        // moves it.
        for member in ["Name", "Arn", "EndpointId", "EndpointUrl"] {
            assert_eq!(
                written[member], stored[member],
                "case {case:?}: {member} must not change because the endpoint was \
                 written, got {written}"
            );
        }

        // The transient state is OBSERVABLE: a read taken straight after the
        // write still sees it.
        let (status, transient) =
            events(&router, "DescribeEndpoint", &json!({ "Name": name })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeEndpoint answered {status} {transient}"
        );
        assert_eq!(
            transient["State"],
            json!(state::ENDPOINT_STATE_UPDATING),
            "case {case:?}: a read taken at once still reports {}, got {transient}",
            state::ENDPOINT_STATE_UPDATING
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
            "case {case:?}: the clock owes the written endpoint {}, got {settled}",
            state::ENDPOINT_STATE_ACTIVE
        );
        // The rule this table is about: the read reports what the write merged.
        for (member, value) in expect.as_object().expect("the expectation is an object") {
            assert_eq!(
                &settled[member], value,
                "case {case:?}: expected a read to report {member} as {value}, got {settled}"
            );
        }
        assert_eq!(
            settled["CreationTime"], stored["CreationTime"],
            "case {case:?}: a write does not re-create the endpoint, got {settled}"
        );
        let (before, after) = (
            stored["LastModifiedTime"].as_f64().unwrap_or_default(),
            settled["LastModifiedTime"].as_f64().unwrap_or_default(),
        );
        assert!(
            after > before,
            "case {case:?}: expected the write to move LastModifiedTime past \
             {before}, got {after}"
        );
    }
}

/// A write to a name no create stored changes nothing and is refused with the
/// modelled missing-resource error, naming the endpoint and the Region searched.
#[tokio::test]
async fn a_write_to_an_endpoint_no_create_stored_is_refused() {
    let router = emulator();
    let stored_name = "stored-endpoint";
    let stored = settled_endpoint(&router, stored_name).await;

    for name in [
        "never-created",
        "stored-endpoint.suffix",
        "Stored-Endpoint",
        "a",
    ] {
        let (status, refused) = events(
            &router,
            "UpdateEndpoint",
            &json!({ "Name": name, "Description": "should not apply" }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {name:?}: expected HTTP 400, got {status} {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ResourceNotFoundException",
            "case {name:?}: expected ResourceNotFoundException, got {refused}"
        );
        assert_eq!(
            refused["message"],
            json!(format!("Endpoint {name} does not exist in region {REGION}.")),
            "case {name:?}: expected the missing endpoint and the Region searched to be \
             named, got {refused}"
        );
    }

    // A refused write reaches no other endpoint either.
    let (status, again) = events(
        &router,
        "DescribeEndpoint",
        &json!({ "Name": stored_name }),
    )
    .await;
    assert_eq!(status, 200, "DescribeEndpoint answered {status} {again}");
    assert_eq!(
        again, stored,
        "a write to a name no create stored must leave every endpoint alone, got {again}"
    );
}

/// A write the service refuses stores NOTHING: the stored endpoint reads back
/// exactly as it was, whatever the write was wrong about.
#[tokio::test]
async fn a_refused_write_changes_nothing() {
    let cases = [
        RefusalCase {
            case: "a replication state outside the modelled enum",
            request: json!({ "ReplicationConfig": { "State": "SOMETIMES" } }),
        },
        RefusalCase {
            case: "replication switched on with no role to replicate as",
            request: json!({
                "ReplicationConfig": { "State": state::REPLICATION_STATE_ENABLED },
            }),
        },
        RefusalCase {
            case: "one event bus instead of the pair the model requires",
            request: json!({ "EventBuses": [{ "EventBusArn": bus_arn(REGION, "orders") }] }),
        },
        RefusalCase {
            case: "two differently named event buses",
            request: json!({
                "EventBuses": [
                    { "EventBusArn": bus_arn(REGION, "orders") },
                    { "EventBusArn": bus_arn("us-west-2", "shipments") },
                ],
            }),
        },
        RefusalCase {
            case: "both event buses in one Region",
            request: json!({
                "EventBuses": [
                    { "EventBusArn": bus_arn(REGION, "orders") },
                    { "EventBusArn": bus_arn(REGION, "orders") },
                ],
            }),
        },
        RefusalCase {
            case: "a health check that is not a Route 53 health-check ARN",
            request: json!({
                "RoutingConfig": {
                    "FailoverConfig": {
                        "Primary": { "HealthCheck": "not-a-health-check" },
                        "Secondary": { "Route": "us-west-2" },
                    },
                },
            }),
        },
        RefusalCase {
            case: "a routing policy with no failover configuration",
            request: json!({ "RoutingConfig": {} }),
        },
        RefusalCase {
            case: "a secondary route that is not a Region",
            request: json!({ "RoutingConfig": routing_config("abcdef0123456789", "US-WEST-2") }),
        },
    ];

    for RefusalCase { case, request } in cases {
        let router = emulator();
        let name = "unwritten-endpoint";
        let stored = settled_endpoint(&router, name).await;

        let mut body = json!({ "Name": name });
        for (member, value) in request.as_object().expect("the write is an object") {
            body[member] = value.clone();
        }
        let (status, refused) = events(&router, "UpdateEndpoint", &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the write to be refused with HTTP 400, got \
             {status} {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {refused}"
        );

        let (status, again) = events(&router, "DescribeEndpoint", &json!({ "Name": name })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeEndpoint answered {status} {again}"
        );
        assert_eq!(
            again, stored,
            "case {case:?}: a refused write must change nothing, got {again}"
        );
    }
}

/// The endpoints the listing table below creates, in an order that is NOT the
/// order a listing reports them in, so the ordering asserted is the listing's
/// own.
const LISTED: [&str; 4] = ["zeta-endpoint", "alpha-endpoint", "alpha.two", "beta"];

/// The same names in the order a listing walks them: the byte order of the name,
/// which is the order the store keeps and the order a page token resumes in.
const LISTED_IN_ORDER: [&str; 4] = ["alpha-endpoint", "alpha.two", "beta", "zeta-endpoint"];

/// The names a listing reported, in the order it reported them.
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

/// One listing request and the endpoints it must report.
struct ListingCase {
    case: &'static str,
    /// The members of the `ListEndpoints` request.
    request: Value,
    /// The names it must report, in the order it must report them.
    expect: Vec<&'static str>,
    /// Whether endpoints the request matched are left for a next page, which is
    /// the only condition a page token is reported under.
    truncated: bool,
}

/// A listing reports the endpoints PERSISTED, in name order, narrowed by the
/// members the request carries, and reports each one exactly as a single-endpoint
/// read does.
#[tokio::test]
async fn a_listing_reports_the_stored_endpoints_narrowed_by_the_request() {
    let router = emulator();
    for name in LISTED {
        let (status, created) = events(&router, "CreateEndpoint", &base_create(name)).await;
        assert_eq!(
            status, 200,
            "CreateEndpoint {name:?} answered {status} {created}"
        );
    }
    tokio::time::sleep(past_the_transition(state::ENDPOINT_TRANSITION)).await;

    let cases = [
        ListingCase {
            case: "no filter at all, which reports every endpoint",
            request: json!({}),
            expect: LISTED_IN_ORDER.to_vec(),
            truncated: false,
        },
        ListingCase {
            case: "a prefix two names share",
            request: json!({ "NamePrefix": "alpha" }),
            expect: vec!["alpha-endpoint", "alpha.two"],
            truncated: false,
        },
        ListingCase {
            case: "a prefix only one name carries",
            request: json!({ "NamePrefix": "alpha-" }),
            expect: vec!["alpha-endpoint"],
            truncated: false,
        },
        ListingCase {
            case: "a whole name as the prefix",
            request: json!({ "NamePrefix": "zeta-endpoint" }),
            expect: vec!["zeta-endpoint"],
            truncated: false,
        },
        ListingCase {
            case: "a prefix no name carries",
            request: json!({ "NamePrefix": "gamma" }),
            expect: vec![],
            truncated: false,
        },
        ListingCase {
            case: "the Region the endpoints are homed in",
            request: json!({ "HomeRegion": REGION }),
            expect: LISTED_IN_ORDER.to_vec(),
            truncated: false,
        },
        ListingCase {
            case: "a Region no endpoint is homed in",
            request: json!({ "HomeRegion": OTHER_REGION }),
            expect: vec![],
            truncated: false,
        },
        ListingCase {
            case: "a prefix and the home Region together",
            request: json!({ "NamePrefix": "beta", "HomeRegion": REGION }),
            expect: vec!["beta"],
            truncated: false,
        },
        ListingCase {
            case: "a prefix in a Region those endpoints are not homed in",
            request: json!({ "NamePrefix": "alpha", "HomeRegion": "ap-southeast-1" }),
            expect: vec![],
            truncated: false,
        },
        ListingCase {
            case: "a page smaller than the number of endpoints",
            request: json!({ "MaxResults": 2 }),
            expect: vec!["alpha-endpoint", "alpha.two"],
            truncated: true,
        },
        ListingCase {
            case: "a page exactly as large as the number of endpoints",
            request: json!({ "MaxResults": LISTED.len() }),
            expect: LISTED_IN_ORDER.to_vec(),
            truncated: false,
        },
        ListingCase {
            case: "a page as large as the modelled maximum",
            request: json!({ "MaxResults": 100 }),
            expect: LISTED_IN_ORDER.to_vec(),
            truncated: false,
        },
        ListingCase {
            case: "the smallest page the modelled range admits, narrowed by a prefix",
            request: json!({ "MaxResults": 1, "NamePrefix": "alpha" }),
            expect: vec!["alpha-endpoint"],
            truncated: true,
        },
    ];

    for ListingCase {
        case,
        request,
        expect,
        truncated,
    } in cases
    {
        let (status, listing) = events(&router, "ListEndpoints", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListEndpoints answered {status} {listing}"
        );
        assert_eq!(
            listed_names(&listing),
            expect,
            "case {case:?}: expected exactly {expect:?}, got {listing}"
        );
        // A token is reported only when a next page exists.
        assert_eq!(
            listing["NextToken"].is_null(),
            !truncated,
            "case {case:?}: a token is reported only when a page remains, got {listing}"
        );
    }

    // A listing reports each endpoint exactly as the single-endpoint read does:
    // both are built from the one stored record.
    let (_, listing) = events(&router, "ListEndpoints", &json!({})).await;
    let entries = listing["Endpoints"]
        .as_array()
        .expect("the listing reports an array")
        .clone();
    for entry in entries {
        let name = entry["Name"].as_str().unwrap_or_default();
        let (status, described) = events(&router, "DescribeEndpoint", &json!({ "Name": name })).await;
        assert_eq!(
            status, 200,
            "case {name:?}: DescribeEndpoint answered {status} {described}"
        );
        for member in [
            "Name",
            "Arn",
            "Description",
            "RoutingConfig",
            "EventBuses",
            "ReplicationConfig",
            "RoleArn",
            "EndpointId",
            "EndpointUrl",
            "State",
            "CreationTime",
            "LastModifiedTime",
        ] {
            assert_eq!(
                entry[member], described[member],
                "case {name:?}: the listing and the read must agree on {member}, got \
                 {entry} then {described}"
            );
        }
    }
}

/// A listing walks every endpoint one page at a time, each page resuming strictly
/// after the last name of the one before, and the last page carries no token.
#[tokio::test]
async fn a_listing_pages_through_every_endpoint() {
    let router = emulator();
    for name in LISTED {
        let (status, created) = events(&router, "CreateEndpoint", &base_create(name)).await;
        assert_eq!(
            status, 200,
            "CreateEndpoint {name:?} answered {status} {created}"
        );
    }

    for page_size in 1..=LISTED.len() + 1 {
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        for page in 0..=LISTED.len() {
            let mut request = json!({ "MaxResults": page_size });
            if let Some(token) = &token {
                request["NextToken"] = json!(token);
            }
            let (status, listing) = events(&router, "ListEndpoints", &request).await;
            assert_eq!(
                status, 200,
                "case page size {page_size}: ListEndpoints answered {status} {listing}"
            );
            let names = listed_names(&listing);
            assert!(
                names.len() <= page_size,
                "case page size {page_size}: a page must hold at most {page_size} \
                 endpoints, got {listing}"
            );
            walked.extend(names);
            token = listing["NextToken"].as_str().map(str::to_string);
            if token.is_none() {
                break;
            }
            assert!(
                page < LISTED.len(),
                "case page size {page_size}: the walk must end, got {listing}"
            );
        }
        assert!(
            token.is_none(),
            "case page size {page_size}: the last page carries no token, got {token:?}"
        );
        assert_eq!(
            walked, LISTED_IN_ORDER,
            "case page size {page_size}: expected the walk to report every endpoint once, \
             in name order"
        );
    }

    // A token this service did not issue names no position to resume from.
    let (status, refused) = events(
        &router,
        "ListEndpoints",
        &json!({ "NextToken": "not-a-token-this-service-issued" }),
    )
    .await;
    assert_eq!(
        status, 400,
        "an unissued page token is refused, got {status} {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "ValidationException",
        "expected ValidationException, got {refused}"
    );
}
