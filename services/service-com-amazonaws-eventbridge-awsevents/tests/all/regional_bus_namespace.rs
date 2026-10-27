// SPDX-License-Identifier: CC0-1.0

//! The event-bus namespace end to end over the wire (`awsJson1_1`, `POST /` with
//! `X-Amz-Target: AWSEvents.<Operation>`) through the real `MultiServiceRouter`,
//! addressed from several Regions.
//!
//! The RULE asserted here: an event bus is a REGIONAL resource, so its identity
//! is the Region it lives in together with its name — a bus name is unique only
//! WITHIN one Region, the same name in another Region is another bus, and every
//! operation that addresses a bus, a rule of a bus, or the list of buses
//! resolves it in the Region the call was signed for. Each Region holds the
//! account's `default` bus before any call is made.
//!
//! The rows span the class rather than one case: they differ along the bus name's
//! form, along which Region a call is signed for (three of them, so "the other
//! Region" is never one fixed Region), and along which surface addresses the bus
//! — create, describe, list, delete, and the rule operations bound to it.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_eventbridge_awsevents::state::{DEFAULT_EVENT_BUS_NAME, MAX_BUS_REGIONS};
use service_com_amazonaws_eventbridge_awsevents::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The Regions the rows below are signed for. Three, so that "another Region" is
/// never one fixed Region, and spanning three partitions' worth of prefixes.
const REGIONS: [&str; 3] = ["us-west-2", "us-east-1", "eu-central-1"];

/// Bus names the modelled `EventBusName` pattern (`^[/\.\-_A-Za-z0-9]+$`)
/// admits, differing along the characters they carry.
const NAMES: [&str; 3] = ["orders", "orders.eu", "orders_eu-1"];

/// One `awsJson1_1` call to EventBridge over the router, signed for `region`.
async fn events(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("events.{region}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("AWSEvents.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTTESTTEST/20260101/{region}/events/\
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

/// `arn:aws:events:<region>:<account>:event-bus/<name>` — built here rather than
/// read from the implementation.
fn event_bus_arn(region: &str, name: &str) -> String {
    format!(
        "arn:aws:events:{region}:{}:event-bus/{name}",
        engine::emulator_account_id()
    )
}

/// The names `ListEventBuses` reports in `region`.
async fn listed_buses(router: &MultiServiceRouter, region: &str) -> Vec<String> {
    let (status, body) = events(router, region, "ListEventBuses", &json!({})).await;
    assert_eq!(
        status, 200,
        "case {region}: ListEventBuses answered {status} {body}"
    );
    body["EventBuses"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|bus| bus["Name"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// One name creates one bus PER REGION: the create succeeds in every Region, the
/// ARN each one answers carries its own Region, and each Region's listing holds
/// its own bus beside its own `default` bus.
#[tokio::test]
async fn one_name_is_a_separate_bus_in_every_region() {
    for name in NAMES {
        let router = emulator();
        for region in REGIONS {
            let (status, body) =
                events(&router, region, "CreateEventBus", &json!({ "Name": name })).await;
            assert_eq!(
                status, 200,
                "case {name} in {region}: a bus unique to this Region must be created, \
                 got {status} {body}"
            );
            assert_eq!(
                body["EventBusArn"],
                json!(event_bus_arn(region, name)),
                "case {name} in {region}: expected an ARN carrying this Region, got {body}"
            );
        }

        for region in REGIONS {
            let (status, body) =
                events(&router, region, "DescribeEventBus", &json!({ "Name": name })).await;
            assert_eq!(
                status, 200,
                "case {name} in {region}: the bus of this Region must read back, \
                 got {status} {body}"
            );
            assert_eq!(
                body["Arn"],
                json!(event_bus_arn(region, name)),
                "case {name} in {region}: a read reports the ARN of this Region's bus, got {body}"
            );
            assert_eq!(
                listed_buses(&router, region).await,
                vec![DEFAULT_EVENT_BUS_NAME.to_string(), name.to_string()],
                "case {name} in {region}: this Region lists its own buses only"
            );

            // A second create in the SAME Region is the name that already
            // exists — which is what the per-Region namespace means.
            let (status, body) =
                events(&router, region, "CreateEventBus", &json!({ "Name": name })).await;
            assert_eq!(
                status, 400,
                "case {name} in {region}: a second create in one Region must be refused, \
                 got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "ResourceAlreadyExistsException",
                "case {name} in {region}: expected ResourceAlreadyExistsException, got {body}"
            );
        }
    }
}

/// A bus created in one Region is not a bus of any other: every surface that
/// addresses it there reports it missing, and deleting it leaves the buses of the
/// other Regions standing.
#[tokio::test]
async fn a_bus_of_one_region_is_absent_from_the_others() {
    for (index, home) in REGIONS.iter().enumerate() {
        let router = emulator();
        let name = format!("bus-of-{home}");
        let (status, body) =
            events(&router, home, "CreateEventBus", &json!({ "Name": name })).await;
        assert_eq!(
            status, 200,
            "case {home}: CreateEventBus answered {status} {body}"
        );

        for other in REGIONS.iter().filter(|region| *region != home) {
            // (the operation, the request that addresses the bus)
            let addressing: Vec<(&str, Value)> = vec![
                ("DescribeEventBus", json!({ "Name": name })),
                ("ListRules", json!({ "EventBusName": name })),
                (
                    "PutRule",
                    json!({
                        "Name": "on-order",
                        "EventBusName": name,
                        "EventPattern": json!({ "source": ["orders"] }).to_string(),
                    }),
                ),
                (
                    "PutPermission",
                    json!({
                        "EventBusName": name,
                        "Action": "events:PutEvents",
                        "Principal": "*",
                        "StatementId": "anyone",
                    }),
                ),
                ("UpdateEventBus", json!({ "Name": name, "Description": "x" })),
            ];
            for (operation, request) in &addressing {
                let (status, body) = events(&router, other, operation, request).await;
                assert_eq!(
                    status, 400,
                    "case {operation} in {other}: a bus of {home} is not a bus of {other}, \
                     got {status} {body}"
                );
                assert_eq!(
                    error_code(&body),
                    "ResourceNotFoundException",
                    "case {operation} in {other}: expected ResourceNotFoundException, got {body}"
                );
                assert_eq!(
                    body["message"],
                    json!(format!("Event bus {name} does not exist.")),
                    "case {operation} in {other}: expected the missing-bus wording, got {body}"
                );
            }
            assert_eq!(
                listed_buses(&router, other).await,
                vec![DEFAULT_EVENT_BUS_NAME.to_string()],
                "case {other}: a bus created in {home} must not be listed in {other}"
            );
        }

        // The same name is then free to be created in the next Region, and
        // deleting it there leaves the first Region's bus alone.
        let neighbour = REGIONS[(index + 1) % REGIONS.len()];
        let (status, body) =
            events(&router, neighbour, "CreateEventBus", &json!({ "Name": name })).await;
        assert_eq!(
            status, 200,
            "case {neighbour}: the name of a bus in {home} is free here, got {status} {body}"
        );
        let (status, body) =
            events(&router, neighbour, "DeleteEventBus", &json!({ "Name": name })).await;
        assert_eq!(
            status, 200,
            "case {neighbour}: DeleteEventBus answered {status} {body}"
        );
        let (status, body) =
            events(&router, home, "DescribeEventBus", &json!({ "Name": name })).await;
        assert_eq!(
            status, 200,
            "case {home}: a delete in {neighbour} must leave the bus of {home}, \
             got {status} {body}"
        );
    }
}

/// A rule is bound to the bus of ITS Region: the same bus name in another Region
/// carries no such rule, and the rule's ARN carries the Region it was created in.
#[tokio::test]
async fn a_rule_belongs_to_the_bus_of_its_own_region() {
    let router = emulator();
    let bus = "shared-name";
    for region in REGIONS {
        let (status, body) =
            events(&router, region, "CreateEventBus", &json!({ "Name": bus })).await;
        assert_eq!(
            status, 200,
            "case {region}: CreateEventBus answered {status} {body}"
        );
    }

    for home in REGIONS {
        let rule = format!("rule-of-{home}");
        let (status, body) = events(
            &router,
            home,
            "PutRule",
            &json!({
                "Name": rule,
                "EventBusName": bus,
                "EventPattern": json!({ "source": ["orders"] }).to_string(),
            }),
        )
        .await;
        assert_eq!(status, 200, "case {home}: PutRule answered {status} {body}");
        assert_eq!(
            body["RuleArn"],
            json!(format!(
                "arn:aws:events:{home}:{}:rule/{bus}/{rule}",
                engine::emulator_account_id()
            )),
            "case {home}: expected a rule ARN carrying this Region, got {body}"
        );

        for other in REGIONS.iter().filter(|region| **region != home) {
            let (status, body) = events(
                &router,
                other,
                "DescribeRule",
                &json!({ "Name": rule, "EventBusName": bus }),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {other}: a rule of {home}'s bus is not on {other}'s bus of the same \
                 name, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {other}: expected ResourceNotFoundException, got {body}"
            );
        }
    }
}

/// The Region a bus lives in comes from the caller's own credential scope, so
/// how many Regions this store holds buses in is bounded: a create in a Region
/// past that bound is refused with the modelled `LimitExceededException`, and
/// the Regions already held keep working.
#[tokio::test]
async fn the_number_of_regions_the_store_holds_buses_in_is_bounded() {
    let router = emulator();
    // One create per Region, up to the bound. Each row's bus carries the same
    // name, so nothing about it depends on the name.
    for index in 0..MAX_BUS_REGIONS {
        let region = format!("eu-south-{index}");
        let (status, body) = events(
            &router,
            &region,
            "CreateEventBus",
            &json!({ "Name": "orders" }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {region}: a create within the Region bound answered {status} {body}"
        );
    }

    let beyond = format!("eu-south-{MAX_BUS_REGIONS}");
    let (status, body) = events(
        &router,
        &beyond,
        "CreateEventBus",
        &json!({ "Name": "orders" }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {beyond}: a create past the Region bound must be refused, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "LimitExceededException",
        "case {beyond}: expected LimitExceededException past the Region bound, got {body}"
    );

    // A Region the store already holds buses in is unaffected: it still creates,
    // and still lists its own buses only.
    let held = "eu-south-0";
    let (status, body) = events(
        &router,
        held,
        "CreateEventBus",
        &json!({ "Name": "orders.eu" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {held}: a Region already held keeps creating, got {status} {body}"
    );
    assert_eq!(
        listed_buses(&router, held).await,
        vec![
            DEFAULT_EVENT_BUS_NAME.to_string(),
            "orders".to_string(),
            "orders.eu".to_string()
        ],
        "case {held}: the Region lists its own buses"
    );
}

/// Every Region holds the account's `default` bus before any call is made: it
/// reads back, it carries no `CreationTime` (no call created it), and the rules
/// written to it are the rules of THAT Region's default bus.
#[tokio::test]
async fn every_region_holds_the_accounts_default_bus() {
    let router = emulator();
    for region in REGIONS {
        let (status, body) = events(&router, region, "DescribeEventBus", &json!({})).await;
        assert_eq!(
            status, 200,
            "case {region}: the default bus is there before any call, got {status} {body}"
        );
        assert_eq!(
            body["Name"],
            json!(DEFAULT_EVENT_BUS_NAME),
            "case {region}: a request naming no bus addresses the default bus, got {body}"
        );
        assert_eq!(
            body["Arn"],
            json!(event_bus_arn(region, DEFAULT_EVENT_BUS_NAME)),
            "case {region}: expected the default bus ARN of this Region, got {body}"
        );
        assert_eq!(
            body["CreationTime"],
            Value::Null,
            "case {region}: no call created the default bus, so it reports no CreationTime, \
             got {body}"
        );
        assert_eq!(
            listed_buses(&router, region).await,
            vec![DEFAULT_EVENT_BUS_NAME.to_string()],
            "case {region}: a Region nothing was created in holds exactly its default bus"
        );
    }

    for home in REGIONS {
        let rule = format!("default-rule-of-{home}");
        let (status, body) = events(
            &router,
            home,
            "PutRule",
            &json!({
                "Name": rule,
                "EventPattern": json!({ "source": ["orders"] }).to_string(),
            }),
        )
        .await;
        assert_eq!(status, 200, "case {home}: PutRule answered {status} {body}");

        for other in REGIONS.iter().filter(|region| **region != home) {
            let (status, body) =
                events(&router, other, "DescribeRule", &json!({ "Name": rule })).await;
            assert_eq!(
                status, 400,
                "case {other}: a rule on {home}'s default bus is not on {other}'s, \
                 got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {other}: expected ResourceNotFoundException, got {body}"
            );
        }
    }
}
