// SPDX-License-Identifier: CC0-1.0

//! UpdateBridgeOutput, end to end over HTTP: CreateBridge -> UpdateBridgeOutput ->
//! DescribeBridge.
//!
//! The rule under test: UpdateBridgeOutput is a PARTIAL update of ONE output of the STORED
//! bridge — the output that bridge addresses by the requested name. Every setting the
//! request carries replaces that setting on the stored output, member by member through the
//! request-only wrapper shape; every setting it omits, and every member of the stored shape
//! the request shape has no counterpart for (the output's `Name`), is left exactly as it
//! was; no other output of the bridge and nothing else about the bridge moves; and the
//! answer is the stored output, so DescribeBridge afterwards reports the same output.
//!
//! The rows below vary what that rule generalises over: WHICH members of the wrapper one
//! request names (one, several, all of them, an empty wrapper, no wrapper at all), the kind
//! of value written (an address, a name, a port, a protocol, a TTL), and WHICH output of
//! the bridge is addressed (the only one, the first, the middle, the last of three).
//!
//! The harness is the one `flow_lifecycle` owns (one router, one shared engine), so every
//! case here drives the same request path a MediaConnect client reaches.

use std::time::Duration;

use serde_json::{json, Value};

use service_com_amazonaws_mediaconnect_mediaconnect::ops::types::PROTOCOL_VALUES;
use service_com_amazonaws_mediaconnect_mediaconnect::state as mc;

use crate::bridge_lifecycle::{cloud_flow, create_bridge, describe_bridge, gateway_with_networks};
use crate::flow_lifecycle::{Emulator, REGION};

/// The networks the fixture gateway declares, which are the only names a network output of
/// a bridge placed on it may carry (`GatewayNetwork$Name`).
const DECLARED_NETWORKS: [&str; 3] = ["net-1", "net-2", "net-3"];

async fn update_output(
    emulator: &Emulator,
    bridge_arn: &str,
    output_name: &str,
    body: Value,
) -> (u16, Option<String>, Value) {
    emulator
        .send(
            "PUT",
            &format!("/v1/bridges/{bridge_arn}/outputs/{output_name}"),
            Some(body),
        )
        .await
}

/// One network output, as a request configures it and as a bridge reports it: the shared
/// fixture builder `bridge_members_and_deletion::network_output` with its `ipAddress` set,
/// so the two bridge-output suites cannot drift on the shape of an output.
fn network_output(name: &str, ip: &str, port: i64, protocol: &str, ttl: i64) -> Value {
    let mut output =
        crate::bridge_members_and_deletion::network_output(name, port, protocol, ttl);
    output["networkOutput"]["ipAddress"] = json!(ip);
    output
}

/// Create one EGRESS (cloud-to-ground) bridge carrying `outputs` and answer its ARN with
/// the created record.
///
/// A network output a request configures is what a cloud-to-ground bridge delivers ("the
/// content comes from an existing MediaConnect flow and is delivered to your premises"), so
/// an egress bridge is the only kind that takes one — a ground-to-cloud bridge's outputs are
/// the flow outputs the service materialises for it. Its source is accordingly the cloud
/// flow the content comes from. The create is the subject of `bridge_lifecycle`; here it is
/// only the fixture every update addresses.
/// What every case below addresses: the gateway the bridge is placed on, declaring
/// [`DECLARED_NETWORKS`], and the cloud flow a cloud-to-ground bridge's content comes from
/// — both resolvable in the store, because a reference that resolves to nothing is refused
/// (`bridge_update_references` is the suite that owns THAT rule).
async fn fixture(emulator: &Emulator, suite: &str) -> (String, String) {
    let placement_arn =
        gateway_with_networks(emulator, &format!("gw-{suite}"), &DECLARED_NETWORKS).await;
    let flow_arn = cloud_flow(emulator, &format!("{suite}-flow")).await;
    (placement_arn, flow_arn)
}

async fn bridge_with(
    emulator: &Emulator,
    name: &str,
    placement_arn: &str,
    flow_arn: &str,
    outputs: Vec<Value>,
) -> (String, Value) {
    let (status, code, created) = create_bridge(
        emulator,
        json!({
            "name": name,
            "placementArn": placement_arn,
            "egressGatewayBridge": { "maxBitrate": 80_000_000 },
            "sources": [{ "flowSource": {
                "flowArn": flow_arn,
                "name": "flow-src",
            }}],
            "outputs": outputs,
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

/// One row of the UpdateBridgeOutput table: the outputs the bridge starts with, the output
/// the request addresses, the update it carries, and the stored output the rule says that
/// output must be afterwards.
struct UpdateCase {
    case: &'static str,
    outputs: Vec<Value>,
    output_name: &'static str,
    update: Value,
    expected: Value,
}

fn update_cases() -> Vec<UpdateCase> {
    let three = || {
        vec![
            network_output("first", "10.0.1.1", 6100, "rtp", 8),
            network_output("middle", "10.0.1.2", 6101, "udp", 16),
            network_output("last", "10.0.1.3", 6102, "rtp-fec", 32),
        ]
    };

    vec![
        UpdateCase {
            case: "Port alone: every other member keeps its stored value",
            outputs: vec![network_output("net-out", "10.0.0.5", 5001, "rtp", 64)],
            output_name: "net-out",
            update: json!({ "networkOutput": { "port": 6001 } }),
            expected: network_output("net-out", "10.0.0.5", 6001, "rtp", 64),
        },
        UpdateCase {
            case: "IpAddress alone",
            outputs: vec![network_output("net-out", "10.0.0.5", 5001, "rtp", 64)],
            output_name: "net-out",
            update: json!({ "networkOutput": { "ipAddress": "192.168.7.9" } }),
            expected: network_output("net-out", "192.168.7.9", 5001, "rtp", 64),
        },
        UpdateCase {
            case: "NetworkName alone",
            outputs: vec![network_output("net-out", "10.0.0.5", 5001, "rtp", 64)],
            output_name: "net-out",
            update: json!({ "networkOutput": { "networkName": "net-2" } }),
            expected: json!({ "networkOutput": {
                "ipAddress": "10.0.0.5",
                "name": "net-out",
                "networkName": "net-2",
                "port": 5001,
                "protocol": "rtp",
                "ttl": 64,
            }}),
        },
        UpdateCase {
            case: "Ttl alone, and down to the smallest hop count a packet can carry",
            outputs: vec![network_output("net-out", "10.0.0.5", 5001, "rtp", 64)],
            output_name: "net-out",
            update: json!({ "networkOutput": { "ttl": 1 } }),
            expected: network_output("net-out", "10.0.0.5", 5001, "rtp", 1),
        },
        UpdateCase {
            case: "Protocol alone",
            outputs: vec![network_output("net-out", "10.0.0.5", 5001, "rtp", 64)],
            output_name: "net-out",
            update: json!({ "networkOutput": { "protocol": "udp" } }),
            expected: network_output("net-out", "10.0.0.5", 5001, "udp", 64),
        },
        UpdateCase {
            case: "IpAddress, Port and Ttl together: three members in one request",
            outputs: vec![network_output("net-out", "10.0.0.5", 5001, "rtp", 64)],
            output_name: "net-out",
            update: json!({
                "networkOutput": { "ipAddress": "10.9.9.9", "port": 1, "ttl": 255 },
            }),
            expected: network_output("net-out", "10.9.9.9", 1, "rtp", 255),
        },
        UpdateCase {
            case: "every wrapper member in one request",
            outputs: vec![network_output("net-out", "10.0.0.5", 5001, "rtp", 64)],
            output_name: "net-out",
            update: json!({ "networkOutput": {
                "ipAddress": "172.16.0.1",
                "networkName": "net-3",
                "port": 7777,
                "protocol": "rist",
                "ttl": 2,
            }}),
            expected: json!({ "networkOutput": {
                "ipAddress": "172.16.0.1",
                "name": "net-out",
                "networkName": "net-3",
                "port": 7777,
                "protocol": "rist",
                "ttl": 2,
            }}),
        },
        UpdateCase {
            case: "an empty wrapper: it names no member, so nothing moves",
            outputs: vec![network_output("net-out", "10.0.0.5", 5001, "rtp", 64)],
            output_name: "net-out",
            update: json!({ "networkOutput": {} }),
            expected: network_output("net-out", "10.0.0.5", 5001, "rtp", 64),
        },
        UpdateCase {
            case: "a request that carries no wrapper at all: a partial update of nothing \
                   changes nothing",
            outputs: vec![network_output("net-out", "10.0.0.5", 5001, "rtp", 64)],
            output_name: "net-out",
            update: json!({}),
            expected: network_output("net-out", "10.0.0.5", 5001, "rtp", 64),
        },
        UpdateCase {
            case: "the FIRST of three outputs: the other two are left alone",
            outputs: three(),
            output_name: "first",
            update: json!({ "networkOutput": { "port": 6199 } }),
            expected: network_output("first", "10.0.1.1", 6199, "rtp", 8),
        },
        UpdateCase {
            case: "the MIDDLE of three outputs",
            outputs: three(),
            output_name: "middle",
            update: json!({ "networkOutput": { "ipAddress": "10.4.4.4", "ttl": 128 } }),
            expected: network_output("middle", "10.4.4.4", 6101, "udp", 128),
        },
        UpdateCase {
            case: "the LAST of three outputs",
            outputs: three(),
            output_name: "last",
            update: json!({ "networkOutput": { "protocol": "srt-listener" } }),
            expected: network_output("last", "10.0.1.3", 6102, "srt-listener", 32),
        },
    ]
}

/// Every row: the update writes what it carries onto the addressed output, keeps what it
/// omits, leaves every other output and the rest of the bridge alone, and DescribeBridge
/// afterwards reports the output it wrote.
#[tokio::test]
async fn update_bridge_output_applies_the_settings_it_carries_and_keeps_the_rest() {
    let emulator = Emulator::new();
    let (placement_arn, flow_arn) = fixture(&emulator, "update-output").await;

    for (index, case) in update_cases().into_iter().enumerate() {
        let UpdateCase {
            case,
            outputs,
            output_name,
            update,
            expected,
        } = case;
        let (bridge_arn, created) = bridge_with(
            &emulator,
            &format!("bridge-update-output-{index}"),
            &placement_arn,
            &flow_arn,
            outputs,
        )
        .await;
        let created_outputs = created["bridge"]["outputs"]
            .as_array()
            .cloned()
            .unwrap_or_default();

        let (status, code, answered) =
            update_output(&emulator, &bridge_arn, output_name, update).await;
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
            answered["output"], expected,
            "case {case:?}: expected the stored output {expected}, got {:?}",
            answered["output"]
        );

        // The round trip: the answer is the STORED record, so DescribeBridge repeats it —
        // the addressed output updated, in its place, and every other output untouched.
        let (status, code, described) = describe_bridge(&emulator, &bridge_arn).await;
        assert_eq!(
            (status, code.as_deref()),
            (200, None),
            "case {case:?}: describe expected 200, got {status} {code:?}: {described}"
        );
        let described_outputs = described["bridge"]["outputs"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            described_outputs.len(),
            created_outputs.len(),
            "case {case:?}: an update adds and removes no output, got {described}"
        );
        for (position, before) in created_outputs.iter().enumerate() {
            let after = &described_outputs[position];
            let name = output_name_of(before);
            if name == output_name {
                assert_eq!(
                    after, &expected,
                    "case {case:?}: DescribeBridge must report the updated output at \
                     position {position}, got {after}"
                );
            } else {
                assert_eq!(
                    after, before,
                    "case {case:?}: the output {name:?} the request did not address is \
                     left alone, got {after}"
                );
            }
        }
        for member in ["bridgeArn", "name", "placementArn", "sources"] {
            assert_eq!(
                described["bridge"][member], created["bridge"][member],
                "case {case:?}: an update of an output leaves {member} alone, got {:?}",
                described["bridge"][member]
            );
        }
    }
}

/// The name a bridge addresses one reported output by, whichever kind it is.
fn output_name_of(output: &Value) -> String {
    output["flowOutput"]["name"]
        .as_str()
        .or_else(|| output["networkOutput"]["name"].as_str())
        .unwrap_or_default()
        .to_string()
}

/// Every protocol the model declares (`PROTOCOL_VALUES`, generated from
/// `smithy_query(query="shape",shape="Protocol")`) can be written onto a stored network
/// output and is stored as given: the handler resolves no protocol of its own. A value the
/// enum does not declare is refused at the boundary, before the handler runs.
#[tokio::test]
async fn every_modelled_protocol_can_be_written_onto_a_bridge_network_output() {
    let emulator = Emulator::new();
    let (placement_arn, flow_arn) = fixture(&emulator, "output-protocols").await;
    let (bridge_arn, _) = bridge_with(
        &emulator,
        "bridge-output-protocols",
        &placement_arn,
        &flow_arn,
        vec![network_output("net-out", "10.0.0.5", 5001, "rtp", 64)],
    )
    .await;

    for protocol in PROTOCOL_VALUES {
        let (status, code, answered) = update_output(
            &emulator,
            &bridge_arn,
            "net-out",
            json!({ "networkOutput": { "protocol": protocol } }),
        )
        .await;
        assert_eq!(
            (status, code.as_deref()),
            (202, None),
            "case {protocol:?}: update expected 202, got {status} {code:?}: {answered}"
        );
        assert_eq!(
            answered["output"]["networkOutput"]["protocol"].as_str(),
            Some(*protocol),
            "case {protocol:?}: the output keeps the protocol the request named, got \
             {answered}"
        );

        let (_, _, described) = describe_bridge(&emulator, &bridge_arn).await;
        assert_eq!(
            described["bridge"]["outputs"][0]["networkOutput"]["protocol"].as_str(),
            Some(*protocol),
            "case {protocol:?}: DescribeBridge reports the stored protocol, got {described}"
        );
    }

    let (status, code, body) = update_output(
        &emulator,
        &bridge_arn,
        "net-out",
        json!({ "networkOutput": { "protocol": "carrier-pigeon" } }),
    )
    .await;
    assert_eq!(
        (status, code.as_deref()),
        (400, Some("BadRequestException")),
        "a protocol the Protocol enum does not declare is refused at the boundary with 400 \
         BadRequestException, got {status} {code:?}: {body}"
    );
}

/// `UpdateBridgeOutput` is `@idempotent`: sending one request twice leaves the same output,
/// and the second answer is byte for byte the first one. The bridge's STATE is not something
/// this operation writes, so a settled bridge is still in the state the clock left it in.
#[tokio::test]
async fn repeating_one_update_bridge_output_request_leaves_the_same_output() {
    let emulator = Emulator::new();
    let (placement_arn, flow_arn) = fixture(&emulator, "output-idempotent").await;
    let (bridge_arn, _) = bridge_with(
        &emulator,
        "bridge-output-idempotent",
        &placement_arn,
        &flow_arn,
        vec![network_output("net-out", "10.0.0.5", 5001, "rtp", 64)],
    )
    .await;

    // Let the clock settle the creation first, so the only thing that could differ between
    // the two answers is what the update writes.
    tokio::time::sleep(mc::TRANSITION_SPAN + Duration::from_millis(400)).await;

    let request = json!({ "networkOutput": { "port": 4321, "ttl": 12, "protocol": "zixi-push" } });
    let (first_status, _, first) =
        update_output(&emulator, &bridge_arn, "net-out", request.clone()).await;
    let (second_status, _, second) =
        update_output(&emulator, &bridge_arn, "net-out", request).await;
    assert_eq!(
        (first_status, second_status),
        (202, 202),
        "both updates expected 202, got {first_status} and {second_status}: {second}"
    );
    assert_eq!(
        second, first,
        "repeating one update answers the same output, got {second} after {first}"
    );

    let (_, _, described) = describe_bridge(&emulator, &bridge_arn).await;
    assert_eq!(
        described["bridge"]["bridgeState"].as_str(),
        Some(mc::BRIDGE_STATE_STANDBY),
        "UpdateBridgeOutput does not move the bridge's state, got {described}"
    );
}

/// One request per way an update of an output cannot be satisfied, the refusal the model
/// names for it, and the envelope this service words it in. Nothing is written for any of
/// them.
#[tokio::test]
async fn an_update_bridge_output_request_that_resolves_nothing_is_refused_and_writes_nothing() {
    let emulator = Emulator::new();
    let (placement_arn, flow_arn) = fixture(&emulator, "output-refusals").await;
    let (bridge_arn, created) = bridge_with(
        &emulator,
        "bridge-output-refusals",
        &placement_arn,
        &flow_arn,
        vec![
            network_output("net-out", "10.0.0.5", 5001, "rtp", 64),
            network_output("other-out", "10.0.0.6", 5002, "udp", 32),
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
            "an output name this bridge holds no output for",
            &bridge_arn,
            "no-such-output",
            json!({ "networkOutput": { "port": 9 } }),
            404,
            "NotFoundException",
            mc::NOT_FOUND_PREFIX,
        ),
        (
            "an output name that is a SOURCE name of this bridge: sources are not outputs",
            &bridge_arn,
            "flow-src",
            json!({ "networkOutput": { "port": 9 } }),
            404,
            "NotFoundException",
            mc::NOT_FOUND_PREFIX,
        ),
        (
            "a bridge ARN no store holds",
            &unknown_bridge,
            "net-out",
            json!({ "networkOutput": { "port": 9 } }),
            404,
            "NotFoundException",
            mc::NOT_FOUND_PREFIX,
        ),
    ];

    for (case, arn, output_name, body, expected_status, expected_code, expected_prefix) in cases {
        let (status, code, parsed) = update_output(&emulator, arn, output_name, body).await;
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
