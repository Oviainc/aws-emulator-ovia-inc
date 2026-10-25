// SPDX-License-Identifier: CC0-1.0

//! Which bridge takes the outputs a REQUEST configures, and how many, end to end over
//! HTTP: CreateBridge / AddBridgeOutputs -> DescribeBridge.
//!
//! The rule under test is about the direction a bridge carries content in. An output a
//! request configures is a network output — the only member `AddBridgeOutputRequest` has,
//! and one "delivered to your premises" — which is what a cloud-to-ground bridge
//! delivers, so only a cloud-to-ground bridge takes it; a ground-to-cloud bridge delivers
//! into the cloud, its outputs are the flow outputs the service materialises when a flow
//! takes the bridge as its source, and it takes none a request carries. And whichever
//! bridge takes them, it holds no more outputs than its own type declares
//! (`IngressGatewayBridge$MaxOutputs` for a ground-to-cloud bridge; a cloud-to-ground
//! bridge's `EgressGatewayBridge` declares no output count, so the only bound on it is the
//! one this crate applies to every bridge, `mc::MAX_BRIDGE_OUTPUTS`). A refused request
//! changes nothing.
//!
//! The rows below differ along what the rule generalises over: the bridge TYPE addressed,
//! the maximum the bridge DECLARES (zero, one, several, the crate's own bound), how many
//! outputs the CREATE carries, how many a later ADD carries, and how many the bridge
//! already holds when the add arrives.
//!
//! The harness is the one `flow_lifecycle` owns (one router, one shared engine), so every
//! case here drives the same request path a MediaConnect client reaches.

use serde_json::{json, Value};

use service_com_amazonaws_mediaconnect_mediaconnect::state as mc;

use crate::bridge_lifecycle::{create_bridge, describe_bridge, minted_flow_arn, minted_gateway_arn};
use crate::bridge_members_and_deletion::{add_outputs, network_output, network_source};
use crate::flow_lifecycle::Emulator;

/// The one source every create below carries, of the kind the bridge's own direction
/// admits: a cloud-to-ground (egress) bridge's content "comes from an existing MediaConnect
/// flow", a ground-to-cloud (ingress) bridge's "originates at your premises".
fn admitted_source(bridge_type: &Value) -> Value {
    if bridge_type.get("egressGatewayBridge").is_some() {
        json!({ "flowSource": {
            "flowArn": minted_flow_arn(1, "flow-output-admission"),
            "name": "src",
        }})
    } else {
        network_source("src", 5_000, "rtp")
    }
}

/// `created` outputs for a create request and `added` ones for a later add request, named
/// apart so no name is ever in use twice on one bridge.
fn outputs(prefix: &str, count: usize) -> Vec<Value> {
    (0..count)
        .map(|index| {
            network_output(
                &format!("{prefix}-{index}"),
                6_000 + index as i64,
                "udp",
                32,
            )
        })
        .collect()
}

/// One row: the bridge a create describes, the outputs it carries, the outputs one later
/// add carries, and what the rule says each of the two requests must answer.
struct Row {
    case: &'static str,
    /// The bridge-type member the create names, with its settings: a cloud-to-ground
    /// (`egressGatewayBridge`) or a ground-to-cloud (`ingressGatewayBridge`) bridge.
    bridge_type: Value,
    /// How many outputs the create carries.
    created: usize,
    /// How many outputs one AddBridgeOutputs request carries afterwards.
    added: usize,
    /// Whether the create must be accepted (201) rather than refused (400).
    create_accepted: bool,
    /// Whether the add must be accepted (202) rather than refused (400). Read only when
    /// the create was accepted.
    add_accepted: bool,
}

fn rows() -> Vec<Row> {
    let egress = json!({ "egressGatewayBridge": { "maxBitrate": 20_000_000 } });
    // The declared maximum is `IngressGatewayBridge$MaxOutputs`, "the maximum number of
    // outputs on the ingress bridge".
    let ingress = |max_outputs: i64| {
        json!({ "ingressGatewayBridge": { "maxBitrate": 20_000_000, "maxOutputs": max_outputs } })
    };

    vec![
        Row {
            case: "cloud-to-ground bridge, no output on the create, one added",
            bridge_type: egress.clone(),
            created: 0,
            added: 1,
            create_accepted: true,
            add_accepted: true,
        },
        Row {
            case: "cloud-to-ground bridge, one output on the create, a batch of three added",
            bridge_type: egress.clone(),
            created: 1,
            added: 3,
            create_accepted: true,
            add_accepted: true,
        },
        Row {
            case: "cloud-to-ground bridge already at the bound: one more output is refused",
            bridge_type: egress.clone(),
            created: mc::MAX_BRIDGE_OUTPUTS,
            added: 1,
            create_accepted: true,
            add_accepted: false,
        },
        Row {
            case: "cloud-to-ground bridge, a create one output past the bound",
            bridge_type: egress.clone(),
            created: mc::MAX_BRIDGE_OUTPUTS + 1,
            added: 0,
            create_accepted: false,
            add_accepted: false,
        },
        Row {
            case: "ground-to-cloud bridge declaring room for several: one added output is \
                   still refused",
            bridge_type: ingress(8),
            created: 0,
            added: 1,
            create_accepted: true,
            add_accepted: false,
        },
        Row {
            case: "ground-to-cloud bridge declaring room for several: a batch within the \
                   declared maximum is refused too",
            bridge_type: ingress(8),
            created: 0,
            added: 8,
            create_accepted: true,
            add_accepted: false,
        },
        Row {
            case: "ground-to-cloud bridge declaring room for one: a batch of two is refused \
                   like any other",
            bridge_type: ingress(1),
            created: 0,
            added: 2,
            create_accepted: true,
            add_accepted: false,
        },
        Row {
            case: "ground-to-cloud bridge declaring room for none: one added output is \
                   refused too",
            bridge_type: ingress(0),
            created: 0,
            added: 1,
            create_accepted: true,
            add_accepted: false,
        },
        Row {
            case: "ground-to-cloud bridge, a create carrying one output",
            bridge_type: ingress(8),
            created: 1,
            added: 0,
            create_accepted: false,
            add_accepted: false,
        },
        Row {
            case: "ground-to-cloud bridge declaring room for none, a create carrying outputs",
            bridge_type: ingress(0),
            created: 2,
            added: 0,
            create_accepted: false,
            add_accepted: false,
        },
    ]
}

#[tokio::test]
async fn only_a_cloud_to_ground_bridge_takes_the_outputs_a_request_configures() {
    let emulator = Emulator::new();
    let placement = minted_gateway_arn(1, "gw-output-admission");

    for (index, row) in rows().into_iter().enumerate() {
        let Row {
            case,
            bridge_type,
            created,
            added,
            create_accepted,
            add_accepted,
        } = row;

        let created_outputs = outputs("held", created);
        let mut body = json!({
            "name": format!("bridge-admission-{index}"),
            "placementArn": placement,
            "sources": [admitted_source(&bridge_type)],
        });
        for (member, settings) in bridge_type.as_object().expect("a bridge-type member") {
            body[member] = settings.clone();
        }
        if created > 0 {
            body["outputs"] = json!(created_outputs);
        }

        let (status, code, answered) = create_bridge(&emulator, body).await;
        if !create_accepted {
            assert_eq!(
                (status, code.as_deref()),
                (400, Some("BadRequestException")),
                "case {case:?}: a create carrying outputs that bridge cannot hold expects 400 \
                 BadRequestException, got {status} {code:?}: {answered}"
            );
            let message = answered["message"].as_str().unwrap_or_default();
            assert!(
                message.starts_with(mc::VALIDATION_FAILED_PREFIX),
                "case {case:?}: the refusal states what was wrong, got {message:?}"
            );
            continue;
        }
        assert_eq!(
            (status, code.as_deref()),
            (201, None),
            "case {case:?}: create expected 201, got {status} {code:?}: {answered}"
        );
        let arn = answered["bridge"]["bridgeArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the create answers an ARN: {answered}"))
            .to_string();

        // What the bridge holds before the add, so a refusal can be shown to change
        // nothing.
        let (_, _, before) = describe_bridge(&emulator, &arn).await;
        let held_before = before["bridge"]["outputs"].clone();

        let added_outputs = outputs("added", added);
        let (status, code, answered) =
            add_outputs(&emulator, &arn, json!({ "outputs": added_outputs })).await;
        let (_, _, after) = describe_bridge(&emulator, &arn).await;
        let held_after = after["bridge"]["outputs"].clone();

        if add_accepted {
            assert_eq!(
                (status, code.as_deref()),
                (202, None),
                "case {case:?}: add expected 202, got {status} {code:?}: {answered}"
            );
            assert_eq!(
                answered["outputs"],
                json!(added_outputs),
                "case {case:?}: the answer carries the outputs added, got {answered}"
            );
            // The round trip: the bridge holds what it held, then what the add appended.
            let mut expected: Vec<Value> = created_outputs.clone();
            expected.extend(added_outputs.clone());
            assert_eq!(
                held_after,
                json!(expected),
                "case {case:?}: expected the held outputs then the added ones, got {held_after}"
            );
        } else {
            assert_eq!(
                (status, code.as_deref()),
                (400, Some("BadRequestException")),
                "case {case:?}: add expected 400 BadRequestException, got {status} {code:?}: \
                 {answered}"
            );
            let message = answered["message"].as_str().unwrap_or_default();
            assert!(
                message.starts_with(mc::VALIDATION_FAILED_PREFIX),
                "case {case:?}: the refusal states what was wrong, got {message:?}"
            );
            assert_eq!(
                held_after, held_before,
                "case {case:?}: a refused add leaves the bridge's outputs as they were, got \
                 {held_after}"
            );
        }
    }
}

/// The maximum a bridge declares, read off its own type: the required
/// `IngressGatewayBridge$MaxOutputs` for a ground-to-cloud bridge, and — for a
/// cloud-to-ground bridge, whose `EgressGatewayBridge` declares no output count at all —
/// the bound this crate applies to every bridge.
///
/// The rows span the declaration's range: no declaration, none, one, several, the crate's
/// own bound, past it, and a negative one the `Integer` member alone does not keep out.
#[test]
fn a_bridge_holds_at_most_the_outputs_its_own_type_declares() {
    let bound = mc::MAX_BRIDGE_OUTPUTS;
    let cases: [(&str, Option<i32>, usize); 7] = [
        ("a cloud-to-ground bridge declares no output count", None, bound),
        ("a ground-to-cloud bridge declaring room for none", Some(0), 0),
        ("a ground-to-cloud bridge declaring room for one", Some(1), 1),
        ("a ground-to-cloud bridge declaring room for several", Some(8), 8),
        (
            "a ground-to-cloud bridge declaring the crate's own bound",
            Some(bound as i32),
            bound,
        ),
        (
            "a declaration past what the store holds is not room for more",
            Some(bound as i32 + 1),
            bound,
        ),
        ("a negative declaration is room for nothing", Some(-1), 0),
    ];

    for (case, declared, expected) in cases {
        let capacity = mc::bridge_output_capacity(declared);
        assert_eq!(
            capacity, expected,
            "case {case:?}: expected a capacity of {expected}, got {capacity:?}"
        );
    }
}

/// The two rules one function decides, so every operation that puts request-configured
/// outputs on a bridge refuses the same requests: a request that adds nothing admits
/// nothing, a cloud-to-ground bridge takes outputs up to the bound, and a ground-to-cloud
/// bridge takes none.
#[test]
fn a_request_configured_output_is_admitted_by_the_bridge_type_and_the_declared_maximum() {
    let bound = mc::MAX_BRIDGE_OUTPUTS;
    // (case, the bridge's declared maximum, outputs held, outputs the request adds,
    //  whether it must be refused)
    let cases: [(&str, Option<i32>, usize, usize, bool); 8] = [
        ("an empty batch on a cloud-to-ground bridge", None, 0, 0, false),
        ("an empty batch on a ground-to-cloud bridge", Some(4), 0, 0, false),
        ("one output on an empty cloud-to-ground bridge", None, 0, 1, false),
        ("a batch filling a cloud-to-ground bridge", None, 1, bound - 1, false),
        ("a batch one past the bound", None, 1, bound, true),
        ("one output on a full cloud-to-ground bridge", None, bound, 1, true),
        ("one output on a ground-to-cloud bridge", Some(4), 0, 1, true),
        ("a batch on a ground-to-cloud bridge declaring room for one", Some(1), 0, 2, true),
    ];

    for (case, declared, held, adding, refused) in cases {
        let rejection = mc::request_bridge_output_rejection(declared, held, adding);
        assert_eq!(
            rejection.is_some(),
            refused,
            "case {case:?}: expected refused={refused}, got {rejection:?}"
        );
    }
}
