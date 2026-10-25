// SPDX-License-Identifier: CC0-1.0

//! What a bridge's DIRECTION admits on the CREATE, end to end over HTTP: CreateBridge ->
//! DescribeBridge / ListBridges.
//!
//! One bridge has one direction, and the direction decides both the KIND of source it
//! ingests and whether a request may configure outputs on it at all:
//!
//! * a cloud-to-ground (egress) bridge's "content comes from an existing MediaConnect flow
//!   and is delivered to your premises" (`AddEgressGatewayBridgeRequest`), so its sources
//!   are flow sources and its outputs are the network outputs a request configures — the
//!   only member `AddBridgeOutputRequest` has — bounded by the count this crate applies to
//!   every bridge, its `EgressGatewayBridge` declaring none of its own; and
//! * a ground-to-cloud (ingress) bridge's "content originates at your premises and is
//!   delivered to the cloud" (`AddIngressGatewayBridgeRequest`), so its sources are network
//!   sources and its outputs are the flow outputs the service materialises when a cloud
//!   flow takes the bridge as its source — which is what its REQUIRED
//!   `IngressGatewayBridge$MaxOutputs` ("the maximum number of expected outputs on the
//!   ingress bridge") bounds — and none a request carries.
//!
//! A create outside either half is refused with the modelled `BadRequestException` (HTTP
//! 400) in this service's validation envelope and STORES NOTHING, so no later
//! DescribeBridge or ListBridges reports a bridge whose source or output contradicts its
//! own direction.
//!
//! The rows below differ along what the rule generalises over: the bridge type the create
//! names, the KIND of each source it carries (one kind, the other, both mixed), how many
//! sources it carries under which failover state, whether it configures outputs, how many,
//! and the maximum a ground-to-cloud bridge declares (none, one, several) against that
//! count.
//!
//! The harness is the one `flow_lifecycle` owns (one router, one shared engine), so every
//! case here drives the same request path a MediaConnect client reaches.

use serde_json::{json, Value};

use service_com_amazonaws_mediaconnect_mediaconnect::state as mc;

use crate::bridge_lifecycle::{create_bridge, describe_bridge, minted_gateway_arn};
use crate::bridge_members_and_deletion::{flow_source, network_output, network_source};
use crate::flow_lifecycle::Emulator;

/// `count` network outputs, named apart so no name is ever in use twice on one bridge.
fn network_outputs(count: usize) -> Vec<Value> {
    (0..count)
        .map(|index| network_output(&format!("out-{index}"), 6_000 + index as i64, "udp", 32))
        .collect()
}

/// One row: the create a client sends, and whether the rule admits it.
struct Row {
    case: &'static str,
    /// The bridge-type member the create names, with its settings — exactly one of
    /// `egressGatewayBridge` (cloud-to-ground) and `ingressGatewayBridge`
    /// (ground-to-cloud).
    bridge_type: Value,
    /// The sources the create carries, each of one kind or the other.
    sources: Vec<Value>,
    /// The outputs the create configures, all network outputs.
    outputs: Vec<Value>,
    /// `sourceFailoverConfig`, when the row names one: it is what widens the source count
    /// to a pair.
    failover: Option<Value>,
    /// Whether the create must be accepted (201) rather than refused (400).
    accepted: bool,
}

fn rows() -> Vec<Row> {
    // `EgressGatewayBridge` declares a bitrate and no output count at all.
    let egress = json!({ "egressGatewayBridge": { "maxBitrate": 80_000_000 } });
    // `IngressGatewayBridge$MaxOutputs` is REQUIRED and is "the maximum number of expected
    // outputs on the ingress bridge".
    let ingress = |max_outputs: i64| {
        json!({ "ingressGatewayBridge": { "maxBitrate": 20_000_000, "maxOutputs": max_outputs } })
    };
    let bound = mc::MAX_BRIDGE_OUTPUTS;

    vec![
        Row {
            case: "cloud-to-ground bridge, one flow source, no output",
            bridge_type: egress.clone(),
            sources: vec![flow_source("flow-src", 1)],
            outputs: Vec::new(),
            failover: None,
            accepted: true,
        },
        Row {
            case: "cloud-to-ground bridge, one flow source, one network output",
            bridge_type: egress.clone(),
            sources: vec![flow_source("flow-src", 2)],
            outputs: network_outputs(1),
            failover: None,
            accepted: true,
        },
        Row {
            case: "cloud-to-ground bridge, a failover pair of flow sources and several \
                   network outputs",
            bridge_type: egress.clone(),
            sources: vec![flow_source("primary", 3), flow_source("backup", 4)],
            outputs: network_outputs(3),
            failover: Some(json!({ "state": mc::STATE_ENABLED })),
            accepted: true,
        },
        Row {
            case: "cloud-to-ground bridge, one flow source, outputs filling the bound this \
                   crate applies to every bridge",
            bridge_type: egress.clone(),
            sources: vec![flow_source("flow-src", 5)],
            outputs: network_outputs(bound),
            failover: None,
            accepted: true,
        },
        Row {
            case: "cloud-to-ground bridge, one flow source, one output past that bound",
            bridge_type: egress.clone(),
            sources: vec![flow_source("flow-src", 6)],
            outputs: network_outputs(bound + 1),
            failover: None,
            accepted: false,
        },
        Row {
            case: "cloud-to-ground bridge fed by a network source, which originates at the \
                   premises",
            bridge_type: egress.clone(),
            sources: vec![network_source("net-src", 5_000, "rtp")],
            outputs: Vec::new(),
            failover: None,
            accepted: false,
        },
        Row {
            case: "cloud-to-ground bridge whose failover pair mixes a flow source with a \
                   network source",
            bridge_type: egress.clone(),
            sources: vec![flow_source("primary", 7), network_source("backup", 5_001, "udp")],
            outputs: Vec::new(),
            failover: Some(json!({ "state": mc::STATE_ENABLED })),
            accepted: false,
        },
        Row {
            case: "ground-to-cloud bridge, one network source, no output",
            bridge_type: ingress(2),
            sources: vec![network_source("net-src", 5_002, "rtp")],
            outputs: Vec::new(),
            failover: None,
            accepted: true,
        },
        Row {
            case: "ground-to-cloud bridge declaring room for no output at all, one network \
                   source",
            bridge_type: ingress(0),
            sources: vec![network_source("net-src", 5_003, "rtp-fec")],
            outputs: Vec::new(),
            failover: None,
            accepted: true,
        },
        Row {
            case: "ground-to-cloud bridge, a failover pair of network sources",
            bridge_type: ingress(4),
            sources: vec![
                network_source("primary", 5_004, "rtp"),
                network_source("backup", 5_005, "rtp"),
            ],
            outputs: Vec::new(),
            failover: Some(json!({ "state": mc::STATE_ENABLED })),
            accepted: true,
        },
        Row {
            case: "ground-to-cloud bridge fed by a flow source, which originates in the cloud",
            bridge_type: ingress(2),
            sources: vec![flow_source("flow-src", 8)],
            outputs: Vec::new(),
            failover: None,
            accepted: false,
        },
        Row {
            case: "ground-to-cloud bridge carrying one network output inside the maximum it \
                   declares",
            bridge_type: ingress(4),
            sources: vec![network_source("net-src", 5_006, "rtp")],
            outputs: network_outputs(1),
            failover: None,
            accepted: false,
        },
        Row {
            case: "ground-to-cloud bridge declaring room for one, carrying exactly one \
                   network output",
            bridge_type: ingress(1),
            sources: vec![network_source("net-src", 5_007, "rtp")],
            outputs: network_outputs(1),
            failover: None,
            accepted: false,
        },
        Row {
            case: "ground-to-cloud bridge declaring room for none, carrying network outputs",
            bridge_type: ingress(0),
            sources: vec![network_source("net-src", 5_008, "rtp")],
            outputs: network_outputs(2),
            failover: None,
            accepted: false,
        },
        Row {
            case: "ground-to-cloud bridge carrying more network outputs than the maximum it \
                   declares",
            bridge_type: ingress(2),
            sources: vec![network_source("net-src", 5_009, "rtp")],
            outputs: network_outputs(3),
            failover: None,
            accepted: false,
        },
    ]
}

/// Every bridge this store reports, under the name each one carries.
async fn listed_names(emulator: &Emulator, case: &str) -> Vec<String> {
    let (status, code, listed) = emulator
        .send("GET", "/v1/bridges?maxResults=100", None)
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "case {case:?}: ListBridges expected 200, got {status} {code:?}: {listed}"
    );
    listed["bridges"]
        .as_array()
        .map(|bridges| {
            bridges
                .iter()
                .filter_map(|bridge| bridge["name"].as_str().map(ToString::to_string))
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn create_bridge_admits_only_the_sources_and_outputs_the_bridge_s_direction_carries() {
    let emulator = Emulator::new();
    let placement = minted_gateway_arn(1, "gw-direction-contract");

    for (index, row) in rows().into_iter().enumerate() {
        let Row {
            case,
            bridge_type,
            sources,
            outputs,
            failover,
            accepted,
        } = row;

        let name = format!("bridge-direction-{index}");
        let mut body = json!({
            "name": name,
            "placementArn": placement,
            "sources": sources,
        });
        for (member, settings) in bridge_type.as_object().expect("one bridge-type member") {
            body[member] = settings.clone();
        }
        if !outputs.is_empty() {
            body["outputs"] = json!(outputs);
        }
        if let Some(failover) = failover.as_ref() {
            body["sourceFailoverConfig"] = failover.clone();
        }

        let before = listed_names(&emulator, case).await;
        let (status, code, answered) = create_bridge(&emulator, body).await;

        if !accepted {
            assert_eq!(
                (status, code.as_deref()),
                (400, Some("BadRequestException")),
                "case {case:?}: expected 400 BadRequestException, got {status} {code:?}: \
                 {answered}"
            );
            let message = answered["message"].as_str().unwrap_or_default();
            let detail = message
                .strip_prefix(mc::VALIDATION_FAILED_PREFIX)
                .map(str::trim)
                .unwrap_or_default();
            assert!(
                !detail.is_empty(),
                "case {case:?}: the refusal states what was unacceptable in this service's \
                 validation envelope, got {message:?}"
            );
            // A refused create stores nothing: the store reports exactly the bridges it
            // reported before the call, and nothing under the refused name.
            let after = listed_names(&emulator, case).await;
            assert_eq!(
                after, before,
                "case {case:?}: a refused create stores no bridge, got {after:?}"
            );
            assert!(
                !after.contains(&name),
                "case {case:?}: a refused create stores no bridge, yet {name:?} is listed: \
                 {after:?}"
            );
            continue;
        }

        assert_eq!(
            (status, code.as_deref()),
            (201, None),
            "case {case:?}: expected 201, got {status} {code:?}: {answered}"
        );
        let arn = answered["bridge"]["bridgeArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the create answers an ARN: {answered}"))
            .to_string();

        // The round trip: what DescribeBridge reports afterwards is the bridge the create
        // stored — the direction it was created with, every source, and the outputs only a
        // cloud-to-ground bridge carries.
        let (status, code, described) = describe_bridge(&emulator, &arn).await;
        assert_eq!(
            (status, code.as_deref()),
            (200, None),
            "case {case:?}: describe expected 200, got {status} {code:?}: {described}"
        );
        let bridge = described["bridge"].clone();
        for (member, settings) in bridge_type.as_object().expect("one bridge-type member") {
            assert_eq!(
                bridge[member], *settings,
                "case {case:?}: the stored bridge carries the direction the create named, got \
                 {bridge}"
            );
            let other = if member == "egressGatewayBridge" {
                "ingressGatewayBridge"
            } else {
                "egressGatewayBridge"
            };
            assert_eq!(
                bridge[other],
                Value::Null,
                "case {case:?}: a bridge has ONE direction, got {bridge}"
            );
        }
        assert_eq!(
            bridge["sources"],
            json!(sources),
            "case {case:?}: the stored bridge holds the sources the create named, got {bridge}"
        );
        let expected_outputs = if outputs.is_empty() {
            Value::Null
        } else {
            json!(outputs)
        };
        assert_eq!(
            bridge["outputs"], expected_outputs,
            "case {case:?}: expected the outputs the create named ({expected_outputs}), got {}",
            bridge["outputs"]
        );
    }
}
