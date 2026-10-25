// SPDX-License-Identifier: CC0-1.0

//! The references an update of a bridge member carries, end to end over HTTP: a gateway, a
//! flow and a bridge are created, then UpdateBridgeSource / UpdateBridgeOutput is called,
//! then DescribeBridge reports what was stored.
//!
//! The rule under test: a bridge member that names another resource is RESOLVED against the
//! store before it is written, so an update may only name a resource the account holds —
//! a gateway network name (`BridgeNetworkSource$NetworkName`,
//! `BridgeNetworkOutput$NetworkName`) must be one the gateway the bridge is placed on
//! declares (`GatewayNetwork$Name`: "this name is used to reference the network and must be
//! unique among networks in this gateway"), and a flow ARN (`BridgeFlowSource$FlowArn`,
//! "the ARN of the cloud flow used as a source of this bridge") must name a flow the
//! account holds. A reference that resolves to nothing is refused with the modelled
//! `BadRequestException` (HTTP 400) in this service's validation envelope, and the stored
//! member is left exactly as it was.
//!
//! The rows below vary what that rule generalises over: which MEMBER carries the reference
//! (a source's network name, a source's flow ARN, an output's network name), which kind of
//! value it is (a name, an ARN), and HOW the value misses — a name no gateway declares, a
//! name another gateway declares, a name that differs from a declared one only in case or
//! in surrounding space, an ARN of a flow this account never minted, and an ARN of a flow
//! that was minted and then deleted. The accepting rows span every network the placement
//! gateway declares and every flow the account holds, so the rule is asserted as a SET
//! membership and not as one allowed value.
//!
//! The harness is the one `flow_lifecycle` owns (one router, one shared engine), so every
//! case here drives the same request path a MediaConnect client reaches.

use serde_json::{json, Value};

use service_com_amazonaws_mediaconnect_mediaconnect::state as mc;

use crate::bridge_lifecycle::{cloud_flow, create_bridge, describe_bridge, gateway_with_networks};
use crate::flow_lifecycle::{Emulator, REGION};

/// The networks the placement gateway declares — `mc::MAX_GATEWAY_NETWORKS` of them, which
/// is every network one gateway may carry, so the accepting rows below cover the whole
/// declared set of this bridge's gateway.
const PLACEMENT_NETWORKS: [&str; 4] = ["net-1", "net-2", "net-3", "net-4"];

/// The network declared on ANOTHER gateway: a name that exists in the store, but not on the
/// gateway the bridge under test is placed on.
const OTHER_GATEWAY_NETWORK: &str = "elsewhere-net";

/// `PUT /v1/bridges/{BridgeArn}/sources/{SourceName}`.
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

/// `PUT /v1/bridges/{BridgeArn}/outputs/{OutputName}`.
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

/// A flow ARN of the form this service mints, for a flow it never minted: well formed
/// enough to satisfy the `^arn:.+:mediaconnect.+:flow:.+$` pattern the model puts on
/// `UpdateBridgeFlowSourceRequest$FlowArn`, so the request reaches the handler.
fn unheld_flow_arn() -> String {
    format!("arn:aws:mediaconnect:{REGION}:111122223333:flow:1-0000dead:never-minted")
}

/// The bridge every case addresses, its placement gateway and the flows it may name.
struct Fixture {
    /// An INGRESS bridge on the placement gateway, carrying one network source (`net-src`,
    /// on `net-1`) and one flow source (`flow-src`, on [`Self::held_flow`]).
    ingress_arn: String,
    /// An EGRESS bridge on the same gateway, carrying one network output (`net-out`, on
    /// `net-1`): the only bridge kind a request configures a network output on.
    egress_arn: String,
    /// The flow the ingress bridge's flow source starts on, and a second one an update may
    /// point it at.
    held_flow: String,
    other_held_flow: String,
    /// A flow this account minted and then DELETED, so its ARN resolves nothing.
    deleted_flow: String,
}

async fn fixture(emulator: &Emulator) -> Fixture {
    let placement_arn =
        gateway_with_networks(emulator, "gw-references", &PLACEMENT_NETWORKS).await;
    // A second gateway, so a name that IS declared somewhere in the store but not on this
    // bridge's gateway can be asserted as a miss.
    let _ = gateway_with_networks(emulator, "gw-elsewhere", &[OTHER_GATEWAY_NETWORK]).await;

    let held_flow = cloud_flow(emulator, "flow-held").await;
    let other_held_flow = cloud_flow(emulator, "flow-other").await;
    let deleted_flow = cloud_flow(emulator, "flow-deleted").await;
    let (status, code, deleted) = emulator
        .send("DELETE", &format!("/v1/flows/{deleted_flow}"), None)
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (202, None),
        "fixture: the flow to be deleted must delete, got {status} {code:?}: {deleted}"
    );

    let (status, code, created) = create_bridge(
        emulator,
        json!({
            "name": "bridge-references-ingress",
            "placementArn": placement_arn,
            "ingressGatewayBridge": { "maxBitrate": 20_000_000, "maxOutputs": 8 },
            "sources": [
                { "networkSource": {
                    "multicastIp": "224.0.1.1",
                    "name": "net-src",
                    "networkName": PLACEMENT_NETWORKS[0],
                    "port": 5000,
                    "protocol": "rtp",
                }},
                { "flowSource": { "flowArn": held_flow, "name": "flow-src" }},
            ],
        }),
    )
    .await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "fixture: the ingress bridge must be created, got {status} {code:?}: {created}"
    );
    let ingress_arn = created["bridge"]["bridgeArn"]
        .as_str()
        .unwrap_or_else(|| panic!("fixture: the create answers an ARN: {created}"))
        .to_string();

    let (status, code, created) = create_bridge(
        emulator,
        json!({
            "name": "bridge-references-egress",
            "placementArn": placement_arn,
            "egressGatewayBridge": { "maxBitrate": 80_000_000 },
            "sources": [{ "flowSource": { "flowArn": held_flow, "name": "flow-src" }}],
            "outputs": [{ "networkOutput": {
                "ipAddress": "10.0.0.5",
                "name": "net-out",
                "networkName": PLACEMENT_NETWORKS[0],
                "port": 5001,
                "protocol": "rtp",
                "ttl": 64,
            }}],
        }),
    )
    .await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "fixture: the egress bridge must be created, got {status} {code:?}: {created}"
    );
    let egress_arn = created["bridge"]["bridgeArn"]
        .as_str()
        .unwrap_or_else(|| panic!("fixture: the create answers an ARN: {created}"))
        .to_string();

    Fixture {
        ingress_arn,
        egress_arn,
        held_flow,
        other_held_flow,
        deleted_flow,
    }
}

/// The source the bridge addresses by `name`, as DescribeBridge reports it.
async fn described_source(emulator: &Emulator, bridge_arn: &str, name: &str) -> Value {
    let (status, code, described) = describe_bridge(emulator, bridge_arn).await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "describe expected 200, got {status} {code:?}: {described}"
    );
    described["bridge"]["sources"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .find(|source| {
            source["networkSource"]["name"].as_str() == Some(name)
                || source["flowSource"]["name"].as_str() == Some(name)
        })
        .unwrap_or_else(|| panic!("the bridge reports a source named {name:?}: {described}"))
}

/// The output the bridge addresses by `name`, as DescribeBridge reports it.
async fn described_output(emulator: &Emulator, bridge_arn: &str, name: &str) -> Value {
    let (status, code, described) = describe_bridge(emulator, bridge_arn).await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "describe expected 200, got {status} {code:?}: {described}"
    );
    described["bridge"]["outputs"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .find(|output| output["networkOutput"]["name"].as_str() == Some(name))
        .unwrap_or_else(|| panic!("the bridge reports an output named {name:?}: {described}"))
}

/// Assert one refused update: the modelled refusal, this service's validation envelope, and
/// a stored member the refusal left exactly as it was.
fn assert_refused(case: &str, status: u16, code: Option<&str>, parsed: &Value) {
    assert_eq!(
        (status, code),
        (400, Some("BadRequestException")),
        "case {case:?}: expected 400 BadRequestException, got {status} {code:?}: {parsed}"
    );
    let message = parsed["message"].as_str().unwrap_or_default();
    assert!(
        message.starts_with(mc::VALIDATION_FAILED_PREFIX),
        "case {case:?}: a refusal about the request's content is worded in the {:?} \
         envelope, got {message:?}",
        mc::VALIDATION_FAILED_PREFIX
    );
}

/// Every network the placement gateway declares can be written onto a network source, and
/// DescribeBridge reports it; the stored multicast IP, port and protocol the request omits
/// are untouched.
#[tokio::test]
async fn a_network_source_can_be_moved_to_every_network_its_placement_gateway_declares() {
    let emulator = Emulator::new();
    let fixture = fixture(&emulator).await;

    for network_name in PLACEMENT_NETWORKS {
        let (status, code, answered) = update_source(
            &emulator,
            &fixture.ingress_arn,
            "net-src",
            json!({ "networkSource": { "networkName": network_name } }),
        )
        .await;
        assert_eq!(
            (status, code.as_deref()),
            (202, None),
            "case {network_name:?}: a declared network is accepted, got {status} {code:?}: \
             {answered}"
        );
        let stored = described_source(&emulator, &fixture.ingress_arn, "net-src").await;
        assert_eq!(
            stored["networkSource"]["networkName"].as_str(),
            Some(network_name),
            "case {network_name:?}: the stored source ingests on the network the request \
             named, got {stored}"
        );
    }

    // The members the requests above never carried are still what the create stored.
    let stored = described_source(&emulator, &fixture.ingress_arn, "net-src").await;
    assert_eq!(
        (
            stored["networkSource"]["multicastIp"].as_str(),
            stored["networkSource"]["port"].as_i64(),
            stored["networkSource"]["protocol"].as_str(),
        ),
        (Some("224.0.1.1"), Some(5000), Some("rtp")),
        "moving a source between networks writes nothing else, got {stored}"
    );
}

/// A network name the placement gateway does not declare is refused, whichever way it
/// misses, and the stored source keeps the network it was on.
#[tokio::test]
async fn a_network_source_cannot_be_moved_to_a_network_the_placement_gateway_does_not_declare() {
    let emulator = Emulator::new();
    let fixture = fixture(&emulator).await;
    let before = described_source(&emulator, &fixture.ingress_arn, "net-src").await;

    let cases: Vec<(&str, String)> = vec![
        ("a name no gateway declares", "no-such-network".to_string()),
        (
            "a name ANOTHER gateway declares, not this bridge's",
            OTHER_GATEWAY_NETWORK.to_string(),
        ),
        (
            "a declared name in a different case: a network name is matched exactly",
            PLACEMENT_NETWORKS[0].to_uppercase(),
        ),
        (
            "a declared name with surrounding space",
            format!(" {} ", PLACEMENT_NETWORKS[0]),
        ),
        ("the empty name", String::new()),
        (
            "a declared name with another appended",
            format!("{}{}", PLACEMENT_NETWORKS[0], PLACEMENT_NETWORKS[1]),
        ),
    ];

    for (case, network_name) in cases {
        let (status, code, parsed) = update_source(
            &emulator,
            &fixture.ingress_arn,
            "net-src",
            json!({ "networkSource": { "networkName": network_name, "port": 6002 } }),
        )
        .await;
        assert_refused(case, status, code.as_deref(), &parsed);
        let after = described_source(&emulator, &fixture.ingress_arn, "net-src").await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused update writes nothing, not even the port it carried \
             beside the bad network name, got {after}"
        );
    }

    // The rule resolves the member the REQUEST carries: an update that names no network at
    // all still applies, on the network the source is already on.
    let (status, code, answered) = update_source(
        &emulator,
        &fixture.ingress_arn,
        "net-src",
        json!({ "networkSource": { "port": 6002 } }),
    )
    .await;
    assert_eq!(
        (status, code.as_deref()),
        (202, None),
        "an update that carries no network name is accepted, got {status} {code:?}: \
         {answered}"
    );
    let after = described_source(&emulator, &fixture.ingress_arn, "net-src").await;
    assert_eq!(
        (
            after["networkSource"]["port"].as_i64(),
            after["networkSource"]["networkName"].as_str(),
        ),
        (Some(6002), before["networkSource"]["networkName"].as_str()),
        "it writes its port and keeps the stored network, got {after}"
    );
}

/// A flow source may be pointed at any flow the account holds, and at no other ARN: an ARN
/// this account never minted and one whose flow was deleted are both refused, and the
/// stored source keeps the flow it named.
#[tokio::test]
async fn a_flow_source_can_only_be_pointed_at_a_flow_the_account_holds() {
    let emulator = Emulator::new();
    let fixture = fixture(&emulator).await;

    for flow_arn in [&fixture.other_held_flow, &fixture.held_flow] {
        let (status, code, answered) = update_source(
            &emulator,
            &fixture.ingress_arn,
            "flow-src",
            json!({ "flowSource": { "flowArn": flow_arn } }),
        )
        .await;
        assert_eq!(
            (status, code.as_deref()),
            (202, None),
            "case {flow_arn:?}: a flow the account holds is accepted, got {status} {code:?}: \
             {answered}"
        );
        let stored = described_source(&emulator, &fixture.ingress_arn, "flow-src").await;
        assert_eq!(
            stored["flowSource"]["flowArn"].as_str(),
            Some(flow_arn.as_str()),
            "case {flow_arn:?}: the stored source names the flow the request named, got \
             {stored}"
        );
    }

    let before = described_source(&emulator, &fixture.ingress_arn, "flow-src").await;
    let cases: Vec<(&str, String)> = vec![
        ("an ARN this account never minted", unheld_flow_arn()),
        (
            "the ARN of a flow that was minted and then deleted",
            fixture.deleted_flow.clone(),
        ),
        (
            "a held flow's ARN with another segment appended",
            format!("{}-not-it", fixture.held_flow),
        ),
    ];

    for (case, flow_arn) in cases {
        let (status, code, parsed) = update_source(
            &emulator,
            &fixture.ingress_arn,
            "flow-src",
            json!({ "flowSource": {
                "flowArn": flow_arn,
                "flowVpcInterfaceAttachment": { "vpcInterfaceName": "vpc-1" },
            }}),
        )
        .await;
        assert_refused(case, status, code.as_deref(), &parsed);
        let after = described_source(&emulator, &fixture.ingress_arn, "flow-src").await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused update writes nothing, not even the VPC interface \
             attachment it carried beside the bad flow ARN, got {after}"
        );
    }
}

/// The same rule on the output side: a network output may be delivered on every network the
/// placement gateway declares and on no other name.
#[tokio::test]
async fn a_network_output_resolves_its_network_name_on_the_same_placement_gateway() {
    let emulator = Emulator::new();
    let fixture = fixture(&emulator).await;

    for network_name in PLACEMENT_NETWORKS {
        let (status, code, answered) = update_output(
            &emulator,
            &fixture.egress_arn,
            "net-out",
            json!({ "networkOutput": { "networkName": network_name } }),
        )
        .await;
        assert_eq!(
            (status, code.as_deref()),
            (202, None),
            "case {network_name:?}: a declared network is accepted, got {status} {code:?}: \
             {answered}"
        );
        let stored = described_output(&emulator, &fixture.egress_arn, "net-out").await;
        assert_eq!(
            stored["networkOutput"]["networkName"].as_str(),
            Some(network_name),
            "case {network_name:?}: the stored output is delivered on the network the \
             request named, got {stored}"
        );
    }

    let before = described_output(&emulator, &fixture.egress_arn, "net-out").await;
    let cases: Vec<(&str, String)> = vec![
        ("a name no gateway declares", "no-such-network".to_string()),
        (
            "a name ANOTHER gateway declares, not this bridge's",
            OTHER_GATEWAY_NETWORK.to_string(),
        ),
        (
            "a declared name in a different case: a network name is matched exactly",
            PLACEMENT_NETWORKS[1].to_uppercase(),
        ),
        ("the empty name", String::new()),
    ];

    for (case, network_name) in cases {
        let (status, code, parsed) = update_output(
            &emulator,
            &fixture.egress_arn,
            "net-out",
            json!({ "networkOutput": { "networkName": network_name, "ttl": 7 } }),
        )
        .await;
        assert_refused(case, status, code.as_deref(), &parsed);
        let after = described_output(&emulator, &fixture.egress_arn, "net-out").await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused update writes nothing, not even the TTL it carried \
             beside the bad network name, got {after}"
        );
    }
}
