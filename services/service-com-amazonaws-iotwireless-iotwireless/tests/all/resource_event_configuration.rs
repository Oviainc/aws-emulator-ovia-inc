// SPDX-License-Identifier: CC0-1.0

//! A resource's event configuration, end to end over HTTP.
//!
//! Every case drives the real `restJson1` wire path — `MultiServiceRouter::dispatch`
//! with a signed `http::Request` — so what is asserted is what an SDK client sees, not a
//! handler return value.
//!
//! The rules under test, each asserted over a SET of inputs rather than one recorded
//! request:
//!
//! * `GetResourceEventConfiguration` reports a status for every event topic the
//!   addressed resource publishes and for none that belongs to another kind of resource,
//!   and that status is `Disabled` until an update sets it;
//! * `UpdateResourceEventConfiguration` persists the statuses it carries and leaves a
//!   topic it does not name with the status it had, so the pair round-trips over every
//!   topic and both modelled `EventNotificationTopicStatus` values;
//! * the configuration belongs to the RESOURCE, so it is the same whichever of that
//!   resource's identifiers addressed it;
//! * a read changes nothing a later caller sees;
//! * every entry of the modelled `IdentifierType` catalog that resolves no resource is a
//!   404 on both operations, and an identifier kind outside the catalog is refused.

use std::collections::{BTreeMap, BTreeSet};

use http::{Method, StatusCode};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The status an event topic carries until an update enables it — the constant the
/// handler reports, so the suite and the service cannot drift apart.
use service_com_amazonaws_iotwireless_iotwireless::state::EVENT_TOPIC_DISABLED;

// The ONE wire harness this crate's suites share.
use super::destination_and_wireless_device_lifecycle::{call, error_code, router};

/// `IdentifierType`'s modelled values — the catalog of identifier kinds these two
/// operations accept, from the model-generated constant.
const IDENTIFIER_TYPES: &[&str] =
    service_com_amazonaws_iotwireless_iotwireless::ops::types::IDENTIFIER_TYPE_VALUES;

/// `EventNotificationTopicStatus`'s modelled values, from the same generated source.
const TOPIC_STATUSES: &[&str] =
    service_com_amazonaws_iotwireless_iotwireless::ops::types::EVENT_NOTIFICATION_TOPIC_STATUS_VALUES;

/// Every event topic a WIRELESS DEVICE publishes, as `"<Section>[.<Technology>].<Topic>"`.
///
/// Source: the sections `GetResourceEventConfiguration` returns and their members
/// (`smithy_query(query="closure", operation="GetResourceEventConfiguration")`) — the
/// four whose resource-id topic is a `WirelessDeviceIdEventTopic`.
const DEVICE_TOPICS: &[&str] = &[
    "DeviceRegistrationState.Sidewalk.AmazonIdEventTopic",
    "DeviceRegistrationState.WirelessDeviceIdEventTopic",
    "Join.LoRaWAN.DevEuiEventTopic",
    "Join.WirelessDeviceIdEventTopic",
    "MessageDeliveryStatus.Sidewalk.AmazonIdEventTopic",
    "MessageDeliveryStatus.WirelessDeviceIdEventTopic",
    "Proximity.Sidewalk.AmazonIdEventTopic",
    "Proximity.WirelessDeviceIdEventTopic",
];

/// Every event topic a WIRELESS GATEWAY publishes, from the same source: the one section
/// whose resource-id topic is a `WirelessGatewayIdEventTopic`.
const GATEWAY_TOPICS: &[&str] = &[
    "ConnectionStatus.LoRaWAN.GatewayEuiEventTopic",
    "ConnectionStatus.WirelessGatewayIdEventTopic",
];

/// `GET /event-configurations/{Identifier}?identifierType=…`.
async fn get_event_configuration(
    router: &MultiServiceRouter,
    identifier_type: &str,
    identifier: &str,
) -> (StatusCode, Value) {
    call(
        router,
        Method::GET,
        &format!("/event-configurations/{identifier}?identifierType={identifier_type}"),
        None,
    )
    .await
}

/// `PATCH /event-configurations/{Identifier}?identifierType=…`.
async fn update_event_configuration(
    router: &MultiServiceRouter,
    identifier_type: &str,
    identifier: &str,
    body: Value,
) -> (StatusCode, Value) {
    call(
        router,
        Method::PATCH,
        &format!("/event-configurations/{identifier}?identifierType={identifier_type}"),
        Some(body),
    )
    .await
}

/// Every event topic a `GetResourceEventConfiguration` answer reports, flattened to
/// `"<Section>[.<Technology>].<Topic>" -> status`, so one assertion covers the whole
/// answer rather than the members a case remembered to name.
fn reported_topics(answer: &Value) -> BTreeMap<String, String> {
    let mut topics = BTreeMap::new();
    let Some(sections) = answer.as_object() else {
        return topics;
    };
    for (section, members) in sections {
        let Some(members) = members.as_object() else {
            continue;
        };
        for (member, value) in members {
            match value {
                Value::String(status) => {
                    topics.insert(format!("{section}.{member}"), status.clone());
                }
                Value::Object(technology) => {
                    for (topic, status) in technology {
                        if let Some(status) = status.as_str() {
                            topics.insert(
                                format!("{section}.{member}.{topic}"),
                                status.to_string(),
                            );
                        }
                    }
                }
                _ => {}
            }
        }
    }
    topics
}

/// The statuses a never-updated resource of this kind reports: every topic it publishes
/// at [`EVENT_TOPIC_DISABLED`].
fn all_disabled(topics: &[&str]) -> BTreeMap<String, String> {
    topics
        .iter()
        .map(|topic| ((*topic).to_string(), EVENT_TOPIC_DISABLED.to_string()))
        .collect()
}

/// Seed a destination, since `CreateWirelessDevice.DestinationName` is resolved against
/// real state.
async fn seed_destination(router: &MultiServiceRouter, name: &str) -> String {
    let (status, body) = call(
        router,
        Method::POST,
        "/destinations",
        Some(json!({
            "Name": name,
            "ExpressionType": "RuleName",
            "Expression": format!("rule-for-{name}"),
            "RoleArn": "arn:aws:iam::000000000000:role/iotwireless-destination",
        })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "seeding destination {name}: expected 201, got {status} {body}"
    );
    name.to_string()
}

/// Seed a wireless device of `device_type` and return its minted `Id`.
async fn seed_device(
    router: &MultiServiceRouter,
    destination: &str,
    device_type: &str,
    dev_eui: Option<&str>,
) -> String {
    let mut body = json!({ "Type": device_type, "DestinationName": destination });
    if let Some(dev_eui) = dev_eui {
        body["LoRaWAN"] = json!({ "DevEui": dev_eui });
    }
    let (status, created) = call(router, Method::POST, "/wireless-devices", Some(body)).await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "seeding a {device_type} device: expected 201, got {status} {created}"
    );
    created["Id"].as_str().expect("an Id").to_string()
}

/// Seed a wireless gateway and return its minted `Id`.
async fn seed_gateway(router: &MultiServiceRouter, gateway_eui: &str) -> String {
    let (status, created) = call(
        router,
        Method::POST,
        "/wireless-gateways",
        Some(json!({ "LoRaWAN": { "GatewayEui": gateway_eui, "RfRegion": "US915" } })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "seeding a gateway: expected 201, got {status} {created}"
    );
    created["Id"].as_str().expect("an Id").to_string()
}

/// A resource that has never been updated reports a status for every event topic it
/// publishes, for none of another kind's, and every one of them is `Disabled` — over
/// both resource kinds, both of each resource's identifier kinds, and both
/// `WirelessDeviceType` technologies (the sections follow the resource KIND, so a
/// LoRaWAN device reports the Sidewalk topics too).
#[tokio::test]
async fn an_unconfigured_resource_reports_every_topic_it_publishes_as_disabled() {
    let router = router();
    let destination = seed_destination(&router, "EventConfigDest").await;
    let lorawan_device = seed_device(
        &router,
        &destination,
        "LoRaWAN",
        Some("1000000000000001"),
    )
    .await;
    let sidewalk_device = seed_device(&router, &destination, "Sidewalk", None).await;
    let gateway = seed_gateway(&router, "2000000000000002").await;

    // (case, identifier kind, identifier, the topics that resource publishes)
    let cases: Vec<(&str, &str, String, &[&str])> = vec![
        (
            "LoRaWAN device by id",
            "WirelessDeviceId",
            lorawan_device.clone(),
            DEVICE_TOPICS,
        ),
        (
            "LoRaWAN device by DevEui",
            "DevEui",
            "1000000000000001".to_string(),
            DEVICE_TOPICS,
        ),
        (
            "Sidewalk device by id",
            "WirelessDeviceId",
            sidewalk_device.clone(),
            DEVICE_TOPICS,
        ),
        (
            "gateway by id",
            "WirelessGatewayId",
            gateway.clone(),
            GATEWAY_TOPICS,
        ),
        (
            "gateway by GatewayEui",
            "GatewayEui",
            "2000000000000002".to_string(),
            GATEWAY_TOPICS,
        ),
    ];

    for (case, identifier_type, identifier, published) in cases {
        let (status, answer) = get_event_configuration(&router, identifier_type, &identifier).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "case {case:?}: expected 200, got {status} with {answer}"
        );
        let reported = reported_topics(&answer);
        assert_eq!(
            reported,
            all_disabled(published),
            "case {case:?}: expected every published topic Disabled and nothing else, \
             got {reported:?}"
        );
    }
}

/// An update persists the statuses it carries and leaves every topic it does not name
/// with the status it had, so the pair round-trips: each row names one or more topics,
/// and the whole reported configuration is checked after it.
#[tokio::test]
async fn an_update_persists_the_topics_it_names_and_leaves_the_rest_alone() {
    let router = router();
    let destination = seed_destination(&router, "RoundTripDest").await;
    let device = seed_device(&router, &destination, "LoRaWAN", Some("3000000000000003")).await;

    let enabled = TOPIC_STATUSES
        .iter()
        .find(|status| **status != EVENT_TOPIC_DISABLED)
        .expect("a status other than the disabled one");

    // (case, the patch body, the topics it must leave reported as `enabled`, the ones it
    // must leave reported as disabled) — stated as deltas against the previous row, so a
    // row that names one topic proves the others kept their status.
    let rows: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "one resource-id topic enabled",
            json!({ "DeviceRegistrationState": { "WirelessDeviceIdEventTopic": enabled } }),
            vec!["DeviceRegistrationState.WirelessDeviceIdEventTopic"],
        ),
        (
            "a technology topic of another section enabled",
            json!({ "Join": { "LoRaWAN": { "DevEuiEventTopic": enabled } } }),
            vec![
                "DeviceRegistrationState.WirelessDeviceIdEventTopic",
                "Join.LoRaWAN.DevEuiEventTopic",
            ],
        ),
        (
            "both topics of one section enabled at once",
            json!({
                "Proximity": {
                    "Sidewalk": { "AmazonIdEventTopic": enabled },
                    "WirelessDeviceIdEventTopic": enabled,
                }
            }),
            vec![
                "DeviceRegistrationState.WirelessDeviceIdEventTopic",
                "Join.LoRaWAN.DevEuiEventTopic",
                "Proximity.Sidewalk.AmazonIdEventTopic",
                "Proximity.WirelessDeviceIdEventTopic",
            ],
        ),
        (
            "an empty update changes nothing",
            json!({}),
            vec![
                "DeviceRegistrationState.WirelessDeviceIdEventTopic",
                "Join.LoRaWAN.DevEuiEventTopic",
                "Proximity.Sidewalk.AmazonIdEventTopic",
                "Proximity.WirelessDeviceIdEventTopic",
            ],
        ),
        (
            "the last remaining section enabled",
            json!({
                "MessageDeliveryStatus": {
                    "Sidewalk": { "AmazonIdEventTopic": enabled },
                    "WirelessDeviceIdEventTopic": enabled,
                }
            }),
            vec![
                "DeviceRegistrationState.WirelessDeviceIdEventTopic",
                "Join.LoRaWAN.DevEuiEventTopic",
                "MessageDeliveryStatus.Sidewalk.AmazonIdEventTopic",
                "MessageDeliveryStatus.WirelessDeviceIdEventTopic",
                "Proximity.Sidewalk.AmazonIdEventTopic",
                "Proximity.WirelessDeviceIdEventTopic",
            ],
        ),
        (
            "one topic disabled again",
            json!({
                "DeviceRegistrationState": { "WirelessDeviceIdEventTopic": EVENT_TOPIC_DISABLED }
            }),
            vec![
                "Join.LoRaWAN.DevEuiEventTopic",
                "MessageDeliveryStatus.Sidewalk.AmazonIdEventTopic",
                "MessageDeliveryStatus.WirelessDeviceIdEventTopic",
                "Proximity.Sidewalk.AmazonIdEventTopic",
                "Proximity.WirelessDeviceIdEventTopic",
            ],
        ),
    ];

    for (case, patch, expected_enabled) in rows {
        let (status, body) =
            update_event_configuration(&router, "WirelessDeviceId", &device, patch).await;
        assert_eq!(
            status,
            StatusCode::NO_CONTENT,
            "case {case:?}: expected 204 from the update, got {status} with {body}"
        );
        assert!(
            body.as_object().is_none_or(serde_json::Map::is_empty),
            "case {case:?}: the update models no output members, got {body}"
        );

        let mut expected = all_disabled(DEVICE_TOPICS);
        for topic in &expected_enabled {
            let held = expected
                .get_mut(*topic)
                .unwrap_or_else(|| panic!("case {case:?}: {topic} is not a device topic"));
            *held = (*enabled).to_string();
        }

        let (status, answer) =
            get_event_configuration(&router, "WirelessDeviceId", &device).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "case {case:?}: expected 200 from the read, got {status} with {answer}"
        );
        let reported = reported_topics(&answer);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }
}

/// The configuration belongs to the RESOURCE: a resource configured through one of its
/// identifiers reports the same statuses through every other identifier that addresses
/// it, for both resource kinds.
#[tokio::test]
async fn a_configuration_belongs_to_the_resource_not_to_the_identifier_used() {
    let router = router();
    let destination = seed_destination(&router, "AliasDest").await;
    let device = seed_device(&router, &destination, "LoRaWAN", Some("4000000000000004")).await;
    let gateway = seed_gateway(&router, "5000000000000005").await;

    let enabled = TOPIC_STATUSES
        .iter()
        .find(|status| **status != EVENT_TOPIC_DISABLED)
        .expect("a status other than the disabled one");

    /// One row: the identifier the update addresses the resource by, the identifier the
    /// read uses, the patch, and the topic it enables.
    struct Alias {
        case: &'static str,
        write_type: &'static str,
        write_id: String,
        read_type: &'static str,
        read_id: String,
        patch: Value,
        topic: &'static str,
    }

    let cases = vec![
        Alias {
            case: "device configured by DevEui, read by id",
            write_type: "DevEui",
            write_id: "4000000000000004".to_string(),
            read_type: "WirelessDeviceId",
            read_id: device.clone(),
            patch: json!({ "Join": { "WirelessDeviceIdEventTopic": enabled } }),
            topic: "Join.WirelessDeviceIdEventTopic",
        },
        Alias {
            case: "device configured by id, read by DevEui",
            write_type: "WirelessDeviceId",
            write_id: device.clone(),
            read_type: "DevEui",
            read_id: "4000000000000004".to_string(),
            patch: json!({ "Proximity": { "Sidewalk": { "AmazonIdEventTopic": enabled } } }),
            topic: "Proximity.Sidewalk.AmazonIdEventTopic",
        },
        Alias {
            case: "gateway configured by GatewayEui, read by id",
            write_type: "GatewayEui",
            write_id: "5000000000000005".to_string(),
            read_type: "WirelessGatewayId",
            read_id: gateway.clone(),
            patch: json!({ "ConnectionStatus": { "WirelessGatewayIdEventTopic": enabled } }),
            topic: "ConnectionStatus.WirelessGatewayIdEventTopic",
        },
        Alias {
            case: "gateway configured by id, read by GatewayEui",
            write_type: "WirelessGatewayId",
            write_id: gateway.clone(),
            read_type: "GatewayEui",
            read_id: "5000000000000005".to_string(),
            patch: json!({
                "ConnectionStatus": { "LoRaWAN": { "GatewayEuiEventTopic": enabled } }
            }),
            topic: "ConnectionStatus.LoRaWAN.GatewayEuiEventTopic",
        },
    ];

    for Alias {
        case,
        write_type,
        write_id,
        read_type,
        read_id,
        patch,
        topic,
    } in cases
    {
        let (status, body) =
            update_event_configuration(&router, write_type, &write_id, patch).await;
        assert_eq!(
            status,
            StatusCode::NO_CONTENT,
            "case {case:?}: expected 204 from the update, got {status} with {body}"
        );

        let (status, answer) = get_event_configuration(&router, read_type, &read_id).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "case {case:?}: expected 200 from the read, got {status} with {answer}"
        );
        let reported = reported_topics(&answer);
        assert_eq!(
            reported.get(topic).map(String::as_str),
            Some(*enabled),
            "case {case:?}: expected {topic} {enabled:?} through the other identifier, \
             got {reported:?}"
        );
    }
}

/// A read reports the configuration without changing it: repeated reads of the same
/// resource, through either of its identifiers, report the same statuses.
#[tokio::test]
async fn a_read_changes_nothing_a_later_caller_sees() {
    let router = router();
    let destination = seed_destination(&router, "RereadDest").await;
    let device = seed_device(&router, &destination, "LoRaWAN", Some("6000000000000006")).await;

    let enabled = TOPIC_STATUSES
        .iter()
        .find(|status| **status != EVENT_TOPIC_DISABLED)
        .expect("a status other than the disabled one");

    // Before any update, and after one, so an answer the reader DEFAULTS and an answer it
    // reads from the store are both checked for stability.
    for case in ["unconfigured", "configured"] {
        if case == "configured" {
            let (status, body) = update_event_configuration(
                &router,
                "WirelessDeviceId",
                &device,
                json!({ "Join": { "LoRaWAN": { "DevEuiEventTopic": enabled } } }),
            )
            .await;
            assert_eq!(
                status,
                StatusCode::NO_CONTENT,
                "case {case:?}: expected 204 from the update, got {status} with {body}"
            );
        }

        let (_, first) = get_event_configuration(&router, "WirelessDeviceId", &device).await;
        let (_, second) = get_event_configuration(&router, "WirelessDeviceId", &device).await;
        let (_, by_dev_eui) =
            get_event_configuration(&router, "DevEui", "6000000000000006").await;
        assert_eq!(
            reported_topics(&first),
            reported_topics(&second),
            "case {case:?}: a repeated read must report the same statuses, got {first} then \
             {second}"
        );
        assert_eq!(
            reported_topics(&first),
            reported_topics(&by_dev_eui),
            "case {case:?}: the other identifier must report the same statuses, got {first} \
             and {by_dev_eui}"
        );
    }
}

/// A section the resolved resource does not publish is reported only once an update has
/// set a status for it, and then exactly as it was stored: the reader reads the record
/// back rather than inventing a status for a topic nobody asked about or dropping one a
/// caller did ask for. The model constrains each status's value, not which identifier
/// kinds accept which section, so neither side narrows it further.
#[tokio::test]
async fn a_section_the_resource_does_not_publish_is_reported_only_once_it_is_set() {
    let router = router();
    let destination = seed_destination(&router, "CrossKindDest").await;
    let device = seed_device(&router, &destination, "LoRaWAN", Some("9000000000000009")).await;
    let gateway = seed_gateway(&router, "a00000000000000a").await;

    let enabled = TOPIC_STATUSES
        .iter()
        .find(|status| **status != EVENT_TOPIC_DISABLED)
        .expect("a status other than the disabled one");

    /// One row: the resource, the topics it publishes, and the foreign section an update
    /// sets on it.
    struct CrossKind {
        case: &'static str,
        identifier_type: &'static str,
        identifier: String,
        published: &'static [&'static str],
        patch: Value,
        topic: &'static str,
    }

    let cases = vec![
        CrossKind {
            case: "a gateway section set on a device",
            identifier_type: "WirelessDeviceId",
            identifier: device.clone(),
            published: DEVICE_TOPICS,
            patch: json!({ "ConnectionStatus": { "WirelessGatewayIdEventTopic": enabled } }),
            topic: "ConnectionStatus.WirelessGatewayIdEventTopic",
        },
        CrossKind {
            case: "a device section set on a gateway",
            identifier_type: "WirelessGatewayId",
            identifier: gateway.clone(),
            published: GATEWAY_TOPICS,
            patch: json!({ "Join": { "LoRaWAN": { "DevEuiEventTopic": enabled } } }),
            topic: "Join.LoRaWAN.DevEuiEventTopic",
        },
    ];

    for CrossKind {
        case,
        identifier_type,
        identifier,
        published,
        patch,
        topic,
    } in cases
    {
        // Before the update, the answer carries only the topics this resource publishes.
        let (_, answer) = get_event_configuration(&router, identifier_type, &identifier).await;
        let reported = reported_topics(&answer);
        assert_eq!(
            reported,
            all_disabled(published),
            "case {case:?}: expected only the published topics before the update, \
             got {reported:?}"
        );

        let (status, body) =
            update_event_configuration(&router, identifier_type, &identifier, patch).await;
        assert_eq!(
            status,
            StatusCode::NO_CONTENT,
            "case {case:?}: expected 204 from the update, got {status} with {body}"
        );

        // After it, the stored status is reported back — nothing is dropped, and no
        // status is invented for the rest of that section.
        let (_, answer) = get_event_configuration(&router, identifier_type, &identifier).await;
        let reported = reported_topics(&answer);
        let mut expected = all_disabled(published);
        expected.insert(topic.to_string(), (*enabled).to_string());
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?} after setting {topic}, got {reported:?}"
        );
    }
}

/// An identifier the account holds no resource for is a 404 on BOTH operations, over
/// every entry of the modelled `IdentifierType` catalog — including a real resource
/// addressed under the identifier kind of the OTHER resource kind — and an identifier
/// kind the catalog does not list is refused.
#[tokio::test]
async fn an_identifier_nothing_holds_is_not_found_for_both_operations() {
    let router = router();
    let destination = seed_destination(&router, "AbsentDest").await;
    let device = seed_device(&router, &destination, "LoRaWAN", Some("7000000000000007")).await;
    let gateway = seed_gateway(&router, "8000000000000008").await;

    // Every catalog entry with a value nothing holds, plus each real id under the wrong
    // kind: a device id is no gateway id, and a gateway's EUI is no `DevEui`.
    let mut cases: Vec<(String, &str, String)> = IDENTIFIER_TYPES
        .iter()
        .map(|identifier_type| {
            (
                format!("absent {identifier_type}"),
                *identifier_type,
                "00000000-0000-4000-8000-000000000000".to_string(),
            )
        })
        .collect();
    cases.push((
        "a device id as a gateway id".to_string(),
        "WirelessGatewayId",
        device.clone(),
    ));
    cases.push((
        "a gateway id as a device id".to_string(),
        "WirelessDeviceId",
        gateway.clone(),
    ));
    cases.push((
        "a GatewayEui as a DevEui".to_string(),
        "DevEui",
        "8000000000000008".to_string(),
    ));
    cases.push((
        "a DevEui as a GatewayEui".to_string(),
        "GatewayEui",
        "7000000000000007".to_string(),
    ));

    // The catalog must be covered by the table, so a model that grows an identifier kind
    // fails here rather than going untested.
    let covered: BTreeSet<&str> = cases.iter().map(|(_, kind, _)| *kind).collect();
    for identifier_type in IDENTIFIER_TYPES {
        assert!(
            covered.contains(identifier_type),
            "the table must cover the modelled identifier kind {identifier_type:?}, \
             got {covered:?}"
        );
    }

    for (case, identifier_type, identifier) in cases {
        let (status, body) = get_event_configuration(&router, identifier_type, &identifier).await;
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "case {case:?}: expected 404 from the read, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException from the read, got {body}"
        );

        let (status, body) = update_event_configuration(
            &router,
            identifier_type,
            &identifier,
            json!({ "Join": { "WirelessDeviceIdEventTopic": "Enabled" } }),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "case {case:?}: expected 404 from the update, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException from the update, got {body}"
        );
    }

    // An identifier kind outside the catalog never reaches the store.
    for identifier_type in ["ThingName", "SidewalkManufacturingSn", ""] {
        let (status, body) = get_event_configuration(&router, identifier_type, &device).await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "case {identifier_type:?}: expected 400 for an identifier kind the catalog does \
             not list, got {status} with {body}"
        );
    }

    // The refused calls stored nothing: the device still reports every topic disabled.
    let (status, answer) = get_event_configuration(&router, "WirelessDeviceId", &device).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "expected 200 reading the device back, got {status} with {answer}"
    );
    let reported = reported_topics(&answer);
    assert_eq!(
        reported,
        all_disabled(DEVICE_TOPICS),
        "a refused call must persist nothing, got {reported:?}"
    );
}
