// SPDX-License-Identifier: CC0-1.0

//! What bounds a core network's transit gateway PEERINGS — the connections every transit
//! gateway route table attachment is created over — driven over HTTP through the real
//! router (the harness lives in `crate::global_network_lifecycle`, the
//! core-network-with-edges helper in `crate::attachment_lifecycle`). Nothing reaches into
//! `crate::state`: every peering is created and read back through an operation.
//!
//! The lifecycle of a peering and of an attachment over it is covered by
//! `crate::transit_gateway_route_table_attachment_lifecycle`. What this suite adds are the
//! two rules that keep a repeated create from growing the store without end, and the
//! refusal each answers with:
//!
//! * a core network holds at most a bounded number of peerings (`state`'s
//!   `MAX_PEERINGS_PER_CORE_NETWORK`); a create past that cap is a field failure on
//!   `CoreNetworkId` (HTTP 400) that stores NOTHING, so the refusal is the same however
//!   often it is repeated, and another core network is unaffected by its neighbour's cap;
//! * one transit gateway is peered once: a second peering of the same gateway is the
//!   service's concurrent-state conflict (HTTP 409) naming the peering that already holds
//!   it, and stores nothing either.
//!
//! The rows differ along what those rules generalise over: the Region of the peered
//! gateway (each of the core network's three edges in turn), the gateway id, and — for the
//! conflict — which of the core network's edges the repeated gateway sits at.

use serde_json::{json, Value};
use service_com_amazonaws_networkmanager_networkmanager::state::MAX_PEERINGS_PER_CORE_NETWORK;

use crate::attachment_lifecycle::core_network_at;
use crate::global_network_lifecycle::{error_code, Emulator, PAST_SETTLE};

/// How many peerings one core network may hold: the bound the service itself declares,
/// read from the crate rather than mirrored by hand, so the rows below cannot disagree
/// with the handler about where the cap is.
const MAX_PEERINGS: usize = MAX_PEERINGS_PER_CORE_NETWORK;

/// The Regions the core networks below configure an edge in, so a row's Region is a real
/// choice rather than the only one available: a peering sits at the edge of the Region its
/// transit gateway is in.
const EDGE_REGIONS: [&str; 3] = ["us-west-2", "eu-west-1", "ap-southeast-2"];

/// `CreateTransitGatewayPeering`.
const PEERING_URI: &str = "/transit-gateway-peerings";

/// The `ResourceType` a peering refusal of this service names, as the id prefix and ARN
/// resource type spell it.
const PEERING: &str = "peering";

/// The ARN of an Amazon VPC transit gateway, in the form the member's own documentation
/// gives (`smithy_query query="doc" shape="CreateTransitGatewayPeeringRequest"
/// member="TransitGatewayArn"`).
fn transit_gateway_arn(region: &str, index: usize) -> String {
    format!("arn:aws:ec2:{region}:123456789012:transit-gateway/tgw-{index:017x}")
}

/// Peer `transit_gateway` with `core_network`, asserting nothing.
async fn peer(emulator: &Emulator, core_network: &str, transit_gateway: &str) -> (u16, Value) {
    emulator
        .post(
            PEERING_URI,
            json!({ "CoreNetworkId": core_network, "TransitGatewayArn": transit_gateway }),
        )
        .await
}

/// A core network configuring one edge per entry of `EDGE_REGIONS`, past the transition
/// its creation owes so its policy — and so its edges — are in force.
async fn core_network_with_every_edge(emulator: &Emulator, name: &str) -> String {
    let (_network, core_network) = core_network_at(emulator, name, &EDGE_REGIONS).await;
    tokio::time::sleep(PAST_SETTLE).await;
    core_network
}

#[tokio::test]
async fn a_core_network_holds_at_most_the_bounded_number_of_peerings() {
    let emulator = Emulator::new();
    let full = core_network_with_every_edge(&emulator, "peering bound").await;
    let spare = core_network_with_every_edge(&emulator, "peering bound neighbour").await;

    // Every row peers a DIFFERENT transit gateway, rotating through the core network's
    // three edges, so what the bound counts is the core network's peerings rather than one
    // edge's.
    for index in 1..=MAX_PEERINGS {
        let region = EDGE_REGIONS[index % EDGE_REGIONS.len()];
        let (status, created) = peer(&emulator, &full, &transit_gateway_arn(region, index)).await;
        assert_eq!(
            status, 200,
            "peering {index} of {MAX_PEERINGS} is within the bound, got {status} with {created}"
        );
    }

    // Past the cap the answer is the same however often it is asked, which is what
    // "nothing was stored" means: a refusal that had stored one would answer differently
    // the second time.
    for attempt in 1..=2 {
        let region = EDGE_REGIONS[attempt % EDGE_REGIONS.len()];
        let (status, refused) = peer(
            &emulator,
            &full,
            &transit_gateway_arn(region, MAX_PEERINGS + attempt),
        )
        .await;
        assert_eq!(
            status, 400,
            "attempt {attempt} past the bound: expected 400, got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "attempt {attempt} past the bound: wrong error code in {refused}"
        );
        assert_eq!(
            refused["Fields"][0]["Name"],
            json!("coreNetworkId"),
            "attempt {attempt} past the bound: the refusal must name the member that asked, \
             got {refused}"
        );
    }

    // The bound is per CORE NETWORK: the neighbour that holds none still peers.
    let (status, created) = peer(
        &emulator,
        &spare,
        &transit_gateway_arn(EDGE_REGIONS[0], MAX_PEERINGS + 10),
    )
    .await;
    assert_eq!(
        status, 200,
        "a core network at no cap still peers, got {status} with {created}"
    );
}

#[tokio::test]
async fn one_transit_gateway_is_peered_once() {
    let emulator = Emulator::new();
    let core_network = core_network_with_every_edge(&emulator, "one peering per gateway").await;
    let neighbour = core_network_with_every_edge(&emulator, "another core network").await;

    // Each row is a gateway at a different edge of the same core network, peered once and
    // then peered again: the second call is the conflict, whichever edge it sits at.
    for (index, region) in EDGE_REGIONS.iter().enumerate() {
        let gateway = transit_gateway_arn(region, index + 1);
        let (status, created) = peer(&emulator, &core_network, &gateway).await;
        assert_eq!(
            status, 200,
            "case {region}: the first peering of {gateway} expected 200, got {status} with {created}"
        );
        let peering_id = created["TransitGatewayPeering"]["Peering"]["PeeringId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {region}: no PeeringId in {created}"))
            .to_string();

        for (case, network) in [
            ("the same core network", &core_network),
            ("another core network", &neighbour),
        ] {
            let (status, refused) = peer(&emulator, network, &gateway).await;
            assert_eq!(
                status, 409,
                "case {region} / {case}: a second peering of one gateway is a conflict, got \
                 {status} with {refused}"
            );
            assert_eq!(
                error_code(&refused),
                "ConflictException",
                "case {region} / {case}: wrong error code in {refused}"
            );
            assert_eq!(
                refused["ResourceId"],
                json!(peering_id),
                "case {region} / {case}: the conflict must name the peering that holds the \
                 gateway, got {refused}"
            );
            assert_eq!(
                refused["ResourceType"],
                json!(PEERING),
                "case {region} / {case}: the conflict must name the resource type, got {refused}"
            );
        }
    }
}
