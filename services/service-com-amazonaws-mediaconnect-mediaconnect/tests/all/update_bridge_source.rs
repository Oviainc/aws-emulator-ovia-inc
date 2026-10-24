// SPDX-License-Identifier: CC0-1.0

//! UpdateBridgeSource, end to end over HTTP: CreateBridge -> UpdateBridgeSource ->
//! DescribeBridge.
//!
//! The rule under test: UpdateBridgeSource is a PARTIAL update of ONE source of the STORED
//! bridge — the source that bridge addresses by the requested name. Every setting the
//! request carries replaces that setting on the stored source, member by member through
//! the request-only wrapper shape; every setting it omits, and every member of the stored
//! shape the request shape has no counterpart for (the source's `Name`, a flow source's
//! `OutputArn`), is left exactly as it was; no other source of the bridge and nothing else
//! about the bridge moves; and the answer is the stored source, so DescribeBridge
//! afterwards reports the same source.
//!
//! The rows below vary what that rule generalises over: the KIND of source (flow /
//! network), WHICH members of the wrapper one request names (one, several, all of them,
//! an empty wrapper, no wrapper at all), whether the member being merged into already has
//! a stored value or not, and WHICH source of the bridge is addressed (the only one, the
//! first, the middle, the last of three).
//!
//! The harness is the one `flow_lifecycle` owns (one router, one shared engine), so every
//! case here drives the same request path a MediaConnect client reaches.

use std::time::Duration;

use serde_json::{json, Value};

use service_com_amazonaws_mediaconnect_mediaconnect::ops::types::PROTOCOL_VALUES;
use service_com_amazonaws_mediaconnect_mediaconnect::state as mc;

use crate::bridge_lifecycle::{create_bridge, describe_bridge, minted_flow_arn, minted_gateway_arn};
use crate::flow_lifecycle::{Emulator, REGION};

async fn update_source(
    emulator: &Emulator,
    bridge_arn: &str,
    source_name: &str,
    body: Value,
) -> (u16, Option<String>, Value) {
    emulator
        .send(
            "PUT",
            &format!("/v1/bridges/{bridge_arn}/sources/{source_name}"),
            Some(body),
        )
        .await
}

/// One network source, as a request configures it and as a bridge reports it.
fn network_source(name: &str, port: i64, protocol: &str) -> Value {
    json!({ "networkSource": {
        "multicastIp": "224.0.1.1",
        "name": name,
        "networkName": "net-1",
        "port": port,
        "protocol": protocol,
    }})
}

/// One flow source, as a request configures it and as a bridge reports it.
fn flow_source(name: &str, flow_arn: &str) -> Value {
    json!({ "flowSource": { "flowArn": flow_arn, "name": name }})
}

/// Create one ingress bridge carrying `sources` and answer its ARN. The create is the
/// subject of `bridge_lifecycle`; here it is only the fixture every update addresses.
async fn bridge_with(emulator: &Emulator, name: &str, sources: Vec<Value>) -> (String, Value) {
    let (status, code, created) = create_bridge(
        emulator,
        json!({
            "name": name,
            "placementArn": minted_gateway_arn(1, "gw-update-source"),
            "ingressGatewayBridge": { "maxBitrate": 20_000_000, "maxOutputs": 8 },
            "sources": sources,
        }),
    )
    .await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "fixture {name:?}: create expected 201, got {status} {code:?}: {created}"
    );
    let arn = created["bridge"]["bridgeArn"]
        .as_str()
        .unwrap_or_else(|| panic!("fixture {name:?}: the create answers an ARN: {created}"))
        .to_string();
    (arn, created)
}

/// One row of the UpdateBridgeSource table: the sources the bridge starts with, the source
/// the request addresses, the update it carries, and the stored source the rule says that
/// source must be afterwards.
struct UpdateCase {
    case: &'static str,
    sources: Vec<Value>,
    source_name: &'static str,
    update: Value,
    expected: Value,
}

fn update_cases() -> Vec<UpdateCase> {
    let flow_a = minted_flow_arn(2, "flow-a");
    let flow_b = minted_flow_arn(3, "flow-b");
    let attachment = json!({ "vpcInterfaceName": "vpc-1" });
    let multicast_settings = json!({ "multicastSourceIp": "10.9.8.7" });

    // A flow source that already carries a VPC interface attachment, and a network source
    // that already carries multicast source settings: the rows that merge into an existing
    // value start from these.
    let flow_with_attachment = json!({ "flowSource": {
        "flowArn": flow_a,
        "flowVpcInterfaceAttachment": attachment,
        "name": "flow-src",
    }});
    let network_with_settings = json!({ "networkSource": {
        "multicastIp": "224.0.1.1",
        "multicastSourceSettings": multicast_settings,
        "name": "net-src",
        "networkName": "net-1",
        "port": 5000,
        "protocol": "rtp",
    }});

    vec![
        UpdateCase {
            case: "flow source, FlowArn alone: the only source of the bridge",
            sources: vec![flow_source("flow-src", &flow_a)],
            source_name: "flow-src",
            update: json!({ "flowSource": { "flowArn": flow_b } }),
            expected: json!({ "flowSource": { "flowArn": flow_b, "name": "flow-src" } }),
        },
        UpdateCase {
            case: "flow source, FlowVpcInterfaceAttachment alone, none stored: the \
                   attachment is written and FlowArn keeps its stored value",
            sources: vec![flow_source("flow-src", &flow_a)],
            source_name: "flow-src",
            update: json!({ "flowSource": { "flowVpcInterfaceAttachment": attachment } }),
            expected: json!({ "flowSource": {
                "flowArn": flow_a,
                "flowVpcInterfaceAttachment": attachment,
                "name": "flow-src",
            }}),
        },
        UpdateCase {
            case: "flow source with a stored attachment, FlowArn alone: the attachment the \
                   request omits is kept",
            sources: vec![flow_with_attachment.clone()],
            source_name: "flow-src",
            update: json!({ "flowSource": { "flowArn": flow_b } }),
            expected: json!({ "flowSource": {
                "flowArn": flow_b,
                "flowVpcInterfaceAttachment": attachment,
                "name": "flow-src",
            }}),
        },
        UpdateCase {
            case: "flow source, both wrapper members at once",
            sources: vec![flow_with_attachment.clone()],
            source_name: "flow-src",
            update: json!({ "flowSource": {
                "flowArn": flow_b,
                "flowVpcInterfaceAttachment": { "vpcInterfaceName": "vpc-2" },
            }}),
            expected: json!({ "flowSource": {
                "flowArn": flow_b,
                "flowVpcInterfaceAttachment": { "vpcInterfaceName": "vpc-2" },
                "name": "flow-src",
            }}),
        },
        UpdateCase {
            case: "flow source, an empty wrapper: it names no member, so nothing moves",
            sources: vec![flow_with_attachment.clone()],
            source_name: "flow-src",
            update: json!({ "flowSource": {} }),
            expected: flow_with_attachment.clone(),
        },
        UpdateCase {
            case: "flow source, a request that carries no wrapper at all: a partial update \
                   of nothing changes nothing",
            sources: vec![flow_with_attachment.clone()],
            source_name: "flow-src",
            update: json!({}),
            expected: flow_with_attachment.clone(),
        },
        UpdateCase {
            case: "network source, Port alone: every other member keeps its stored value",
            sources: vec![network_source("net-src", 5000, "rtp")],
            source_name: "net-src",
            update: json!({ "networkSource": { "port": 6001 } }),
            expected: json!({ "networkSource": {
                "multicastIp": "224.0.1.1",
                "name": "net-src",
                "networkName": "net-1",
                "port": 6001,
                "protocol": "rtp",
            }}),
        },
        UpdateCase {
            case: "network source, MulticastIp and Protocol together",
            sources: vec![network_source("net-src", 5000, "rtp")],
            source_name: "net-src",
            update: json!({ "networkSource": { "multicastIp": "239.1.2.3", "protocol": "udp" } }),
            expected: json!({ "networkSource": {
                "multicastIp": "239.1.2.3",
                "name": "net-src",
                "networkName": "net-1",
                "port": 5000,
                "protocol": "udp",
            }}),
        },
        UpdateCase {
            case: "network source, NetworkName alone",
            sources: vec![network_source("net-src", 5000, "rtp")],
            source_name: "net-src",
            update: json!({ "networkSource": { "networkName": "net-2" } }),
            expected: json!({ "networkSource": {
                "multicastIp": "224.0.1.1",
                "name": "net-src",
                "networkName": "net-2",
                "port": 5000,
                "protocol": "rtp",
            }}),
        },
        UpdateCase {
            case: "network source, MulticastSourceSettings alone, none stored",
            sources: vec![network_source("net-src", 5000, "rtp")],
            source_name: "net-src",
            update: json!({ "networkSource": { "multicastSourceSettings": multicast_settings } }),
            expected: json!({ "networkSource": {
                "multicastIp": "224.0.1.1",
                "multicastSourceSettings": multicast_settings,
                "name": "net-src",
                "networkName": "net-1",
                "port": 5000,
                "protocol": "rtp",
            }}),
        },
        UpdateCase {
            case: "network source with stored multicast source settings, Port alone: the \
                   settings the request omits are kept",
            sources: vec![network_with_settings.clone()],
            source_name: "net-src",
            update: json!({ "networkSource": { "port": 7002 } }),
            expected: json!({ "networkSource": {
                "multicastIp": "224.0.1.1",
                "multicastSourceSettings": multicast_settings,
                "name": "net-src",
                "networkName": "net-1",
                "port": 7002,
                "protocol": "rtp",
            }}),
        },
        UpdateCase {
            case: "network source, every wrapper member in one request",
            sources: vec![network_with_settings.clone()],
            source_name: "net-src",
            update: json!({ "networkSource": {
                "multicastIp": "239.9.9.9",
                "multicastSourceSettings": { "multicastSourceIp": "10.1.1.1" },
                "networkName": "net-3",
                "port": 1,
                "protocol": "rist",
            }}),
            expected: json!({ "networkSource": {
                "multicastIp": "239.9.9.9",
                "multicastSourceSettings": { "multicastSourceIp": "10.1.1.1" },
                "name": "net-src",
                "networkName": "net-3",
                "port": 1,
                "protocol": "rist",
            }}),
        },
        UpdateCase {
            case: "the FIRST of three sources: the other two are left alone",
            sources: vec![
                network_source("first", 5100, "rtp"),
                flow_source("middle", &flow_a),
                network_source("last", 5102, "udp"),
            ],
            source_name: "first",
            update: json!({ "networkSource": { "port": 5199 } }),
            expected: json!({ "networkSource": {
                "multicastIp": "224.0.1.1",
                "name": "first",
                "networkName": "net-1",
                "port": 5199,
                "protocol": "rtp",
            }}),
        },
        UpdateCase {
            case: "the MIDDLE of three sources, which is a flow source among network ones",
            sources: vec![
                network_source("first", 5100, "rtp"),
                flow_source("middle", &flow_a),
                network_source("last", 5102, "udp"),
            ],
            source_name: "middle",
            update: json!({ "flowSource": { "flowArn": flow_b } }),
            expected: json!({ "flowSource": { "flowArn": flow_b, "name": "middle" } }),
        },
        UpdateCase {
            case: "the LAST of three sources",
            sources: vec![
                network_source("first", 5100, "rtp"),
                flow_source("middle", &flow_a),
                network_source("last", 5102, "udp"),
            ],
            source_name: "last",
            update: json!({ "networkSource": { "protocol": "srt-listener" } }),
            expected: json!({ "networkSource": {
                "multicastIp": "224.0.1.1",
                "name": "last",
                "networkName": "net-1",
                "port": 5102,
                "protocol": "srt-listener",
            }}),
        },
    ]
}

/// Every row: the update writes what it carries onto the addressed source, keeps what it
/// omits, leaves every other source and the rest of the bridge alone, and DescribeBridge
/// afterwards reports the source it wrote.
#[tokio::test]
async fn update_bridge_source_applies_the_settings_it_carries_and_keeps_the_rest() {
    let emulator = Emulator::new();

    for (index, case) in update_cases().into_iter().enumerate() {
        let UpdateCase {
            case,
            sources,
            source_name,
            update,
            expected,
        } = case;
        let (bridge_arn, created) =
            bridge_with(&emulator, &format!("bridge-update-source-{index}"), sources).await;
        let created_sources = created["bridge"]["sources"]
            .as_array()
            .cloned()
            .unwrap_or_default();

        let (status, code, answered) =
            update_source(&emulator, &bridge_arn, source_name, update).await;
        assert_eq!(
            (status, code.as_deref()),
            (202, None),
            "case {case:?}: update expected 202, got {status} {code:?}: {answered}"
        );
        assert_eq!(
            answered["bridgeArn"].as_str(),
            Some(bridge_arn.as_str()),
            "case {case:?}: the answer names the bridge that was updated, got {:?}",
            answered["bridgeArn"]
        );
        assert_eq!(
            answered["source"], expected,
            "case {case:?}: expected the stored source {expected}, got {:?}",
            answered["source"]
        );

        // The round trip: the answer is the STORED record, so DescribeBridge repeats it —
        // the addressed source updated, in its place, and every other source untouched.
        let (status, code, described) = describe_bridge(&emulator, &bridge_arn).await;
        assert_eq!(
            (status, code.as_deref()),
            (200, None),
            "case {case:?}: describe expected 200, got {status} {code:?}: {described}"
        );
        let described_sources = described["bridge"]["sources"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            described_sources.len(),
            created_sources.len(),
            "case {case:?}: an update adds and removes no source, got {described}"
        );
        for (position, before) in created_sources.iter().enumerate() {
            let after = &described_sources[position];
            let name = source_name_of(before);
            if name == source_name {
                assert_eq!(
                    after, &expected,
                    "case {case:?}: DescribeBridge must report the updated source at \
                     position {position}, got {after}"
                );
            } else {
                assert_eq!(
                    after, before,
                    "case {case:?}: the source {name:?} the request did not address is \
                     left alone, got {after}"
                );
            }
        }
        for member in ["bridgeArn", "name", "placementArn", "outputs"] {
            assert_eq!(
                described["bridge"][member], created["bridge"][member],
                "case {case:?}: an update of a source leaves {member} alone, got {:?}",
                described["bridge"][member]
            );
        }
    }
}

/// The name a bridge addresses one reported source by, whichever kind it is.
fn source_name_of(source: &Value) -> String {
    source["flowSource"]["name"]
        .as_str()
        .or_else(|| source["networkSource"]["name"].as_str())
        .unwrap_or_default()
        .to_string()
}

/// Every protocol the model declares (`PROTOCOL_VALUES`, generated from
/// `smithy_query(query="shape",shape="Protocol")`) can be written onto a stored network
/// source and is stored as given: the handler resolves no protocol of its own. A value the
/// enum does not declare is refused at the boundary, before the handler runs.
#[tokio::test]
async fn every_modelled_protocol_can_be_written_onto_a_bridge_network_source() {
    let emulator = Emulator::new();
    let (bridge_arn, _) = bridge_with(
        &emulator,
        "bridge-source-protocols",
        vec![network_source("net-src", 5000, "rtp")],
    )
    .await;

    for protocol in PROTOCOL_VALUES {
        let (status, code, answered) = update_source(
            &emulator,
            &bridge_arn,
            "net-src",
            json!({ "networkSource": { "protocol": protocol } }),
        )
        .await;
        assert_eq!(
            (status, code.as_deref()),
            (202, None),
            "case {protocol:?}: update expected 202, got {status} {code:?}: {answered}"
        );
        assert_eq!(
            answered["source"]["networkSource"]["protocol"].as_str(),
            Some(*protocol),
            "case {protocol:?}: the source keeps the protocol the request named, got \
             {answered}"
        );

        let (_, _, described) = describe_bridge(&emulator, &bridge_arn).await;
        assert_eq!(
            described["bridge"]["sources"][0]["networkSource"]["protocol"].as_str(),
            Some(*protocol),
            "case {protocol:?}: DescribeBridge reports the stored protocol, got {described}"
        );
    }

    let (status, code, body) = update_source(
        &emulator,
        &bridge_arn,
        "net-src",
        json!({ "networkSource": { "protocol": "carrier-pigeon" } }),
    )
    .await;
    assert_eq!(
        status, 400,
        "a protocol the Protocol enum does not declare is refused with 400, got {status} \
         {code:?}: {body}"
    );
}

/// `UpdateBridgeSource` is `@idempotent`: sending one request twice leaves the same source,
/// and the second answer is byte for byte the first one. The bridge's STATE is not
/// something this operation writes, so a settled bridge is still in the state the clock
/// left it in.
#[tokio::test]
async fn repeating_one_update_bridge_source_request_leaves_the_same_source() {
    let emulator = Emulator::new();
    let (bridge_arn, _) = bridge_with(
        &emulator,
        "bridge-source-idempotent",
        vec![network_source("net-src", 5000, "rtp")],
    )
    .await;

    // Let the clock settle the creation first, so the only thing that could differ between
    // the two answers is what the update writes.
    tokio::time::sleep(mc::TRANSITION_SPAN + Duration::from_millis(400)).await;

    let request = json!({ "networkSource": { "port": 4321, "protocol": "zixi-push" } });
    let (first_status, _, first) =
        update_source(&emulator, &bridge_arn, "net-src", request.clone()).await;
    let (second_status, _, second) =
        update_source(&emulator, &bridge_arn, "net-src", request).await;
    assert_eq!(
        (first_status, second_status),
        (202, 202),
        "both updates expected 202, got {first_status} and {second_status}: {second}"
    );
    assert_eq!(
        second, first,
        "repeating one update answers the same source, got {second} after {first}"
    );

    let (_, _, described) = describe_bridge(&emulator, &bridge_arn).await;
    assert_eq!(
        described["bridge"]["bridgeState"].as_str(),
        Some(mc::BRIDGE_STATE_STANDBY),
        "UpdateBridgeSource does not move the bridge's state, got {described}"
    );
}

/// One request per way an update of a source cannot be satisfied, the refusal the model
/// names for it, and the envelope this service words it in. Nothing is written for any of
/// them.
#[tokio::test]
async fn an_update_bridge_source_request_the_source_cannot_satisfy_is_refused_and_writes_nothing()
{
    let emulator = Emulator::new();
    let flow_arn = minted_flow_arn(4, "flow-refusals");
    let (bridge_arn, created) = bridge_with(
        &emulator,
        "bridge-source-refusals",
        vec![
            flow_source("flow-src", &flow_arn),
            network_source("net-src", 5000, "rtp"),
        ],
    )
    .await;
    // A bridge ARN of the form this service mints, for a bridge nothing ever created.
    let unknown_bridge = engine::emulator_arn(
        mc::SERVICE_NAMESPACE,
        REGION,
        &mc::bridge_resource_fragment(99, "never-created"),
    );

    let cases: Vec<(&str, &str, &str, Value, u16, &str, &str)> = vec![
        (
            "FlowSource on a source that is a network source",
            &bridge_arn,
            "net-src",
            json!({ "flowSource": { "flowArn": flow_arn } }),
            400,
            "BadRequestException",
            mc::BAD_REQUEST_PREFIX,
        ),
        (
            "NetworkSource on a source that is a flow source",
            &bridge_arn,
            "flow-src",
            json!({ "networkSource": { "port": 9 } }),
            400,
            "BadRequestException",
            mc::BAD_REQUEST_PREFIX,
        ),
        (
            "both wrappers at once: a source is one kind or the other",
            &bridge_arn,
            "net-src",
            json!({
                "flowSource": { "flowArn": flow_arn },
                "networkSource": { "port": 9 },
            }),
            400,
            "BadRequestException",
            mc::VALIDATION_FAILED_PREFIX,
        ),
        (
            "a source name this bridge holds no source for",
            &bridge_arn,
            "no-such-source",
            json!({ "networkSource": { "port": 9 } }),
            404,
            "NotFoundException",
            mc::NOT_FOUND_PREFIX,
        ),
        (
            "a bridge ARN no store holds",
            &unknown_bridge,
            "net-src",
            json!({ "networkSource": { "port": 9 } }),
            404,
            "NotFoundException",
            mc::NOT_FOUND_PREFIX,
        ),
    ];

    for (case, arn, source_name, body, expected_status, expected_code, expected_prefix) in cases {
        let (status, code, parsed) = update_source(&emulator, arn, source_name, body).await;
        assert_eq!(
            (status, code.as_deref()),
            (expected_status, Some(expected_code)),
            "case {case:?}: expected {expected_status} {expected_code}, got {status} \
             {code:?}: {parsed}"
        );
        let message = parsed["message"].as_str().unwrap_or_default();
        assert!(
            message.starts_with(expected_prefix),
            "case {case:?}: the refusal is worded in this service's {expected_prefix:?} \
             envelope, got {message:?}"
        );
    }

    // A refused update writes nothing: the bridge is exactly as it was created.
    let (status, _, described) = describe_bridge(&emulator, &bridge_arn).await;
    assert_eq!(
        status, 200,
        "describe expected 200 after the refusals, got {status}: {described}"
    );
    for member in ["sources", "outputs", "bridgeArn", "name", "placementArn"] {
        assert_eq!(
            described["bridge"][member], created["bridge"][member],
            "a refused update leaves {member} alone, got {:?}",
            described["bridge"][member]
        );
    }
}
