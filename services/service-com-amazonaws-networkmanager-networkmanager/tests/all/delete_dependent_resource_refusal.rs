// SPDX-License-Identifier: CC0-1.0

//! How a DELETE of this service behaves while the store still holds a RESOURCE OF ITS OWN
//! that hangs off the resource being deleted, driven over HTTP through the real router (the
//! harness lives in `crate::global_network_lifecycle`, the core-network-with-edges helper in
//! `crate::attachment_lifecycle`, the peering helpers in `crate::transit_gateway_peering`
//! and the Connect-peer subject and table in `crate::connect_peer_lifecycle`).
//!
//! The rule under test: a delete is refused with this service's dependency
//! `ValidationException` (HTTP 400) — naming the resource being deleted and EVERY dependent
//! kind that still hangs off it, and no kind that does not — while the store holds any
//! resource with an id of its own whose only parent is that resource, so no delete ever
//! leaves one addressable by its own reader while the inventory that resolves it through its
//! parent has stopped counting it; the refusal changes nothing, and the identical call
//! succeeds once the caller has removed the dependents. The two parents exercised here are
//! the ones whose dependents are reached through a reference on the child: a core network,
//! which holds attachments and peerings, and a Connect attachment, which carries Connect
//! peers.
//!
//! The tables span that class: rows differing in which dependent kinds the parent holds, in
//! how many of each, in the edge location each sits at, and — for the Connect peers — in the
//! tunnel protocol of the attachment carrying them; each table's last row holds nothing, so
//! the same call succeeds and the refusal is shown to track stored state rather than the
//! shape of the request. Nothing reaches into `crate::state`: every resource is created, read
//! back and deleted through an operation.

use serde_json::{json, Value};

use crate::attachment_lifecycle::{
    attachment_id, core_network_at, create_vpc_attachment, subnet_arn, vpc_arn,
};
use crate::connect_peer_lifecycle::{
    create_connect_peer, request_of as connect_peer_request, subject, PEER_CASES,
};
use crate::delete_dependency_refusal::assert_dependency_refusal;
use crate::global_network_lifecycle::{error_code, Emulator};
use crate::transit_gateway_peering::{
    create_peering, own_account, peering_id, transit_gateway_arn,
};

/// The edges the core networks under test configure: an attachment and a peering each sit at
/// one edge of the core network they hang off, so which edge a dependent sits at is a real
/// choice the rows vary.
const EDGES: [&str; 3] = ["us-west-2", "eu-west-1", "ap-southeast-2"];

/// The dependent kinds this suite can tell apart in a refusal message, as the refusal spells
/// them (the ARN resource type read as words: `connect-peer` reads "connect peer"). Used to
/// assert that a refusal names the kinds that block it and NONE of the others; the parent of
/// a refusal is named as a title ("Core Network", "Attachment"), so these lowercase words
/// only ever match a kind the message lists as blocking.
const DEPENDENT_KINDS: [&str; 3] = ["attachment", "peering", "connect peer"];

/// Assert `body` is the dependency refusal for `parent_label`, naming every kind in
/// `blocking` and no other kind of [`DEPENDENT_KINDS`]. The shared assertion covers the
/// status, the error code, the reason, the absent field list and the parent; this adds the
/// kinds that belong to the parents exercised here.
fn assert_blocked_by(
    case: &str,
    parent_label: &str,
    status: u16,
    body: &Value,
    blocking: &[&str],
) {
    assert_dependency_refusal(case, parent_label, status, body, blocking);
    let message = body["Message"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no Message in {body}"));
    for kind in DEPENDENT_KINDS {
        let named = message.contains(kind);
        let expected = blocking.contains(&kind);
        assert_eq!(
            named, expected,
            "case {case:?}: the refusal must name the blocking kinds {blocking:?} and no \
             other, got {message:?}"
        );
    }
}

/// The `Count` `GetNetworkResourceCounts` reports for `resource_type` in `global_network`.
///
/// Addressed by GLOBAL NETWORK rather than through the core network (as
/// `crate::transit_gateway_peering::peering_count` is), because this suite asks for the count
/// after the core network has been deleted, when `GetCoreNetwork` no longer resolves it.
async fn counted(emulator: &Emulator, global_network: &str, resource_type: &str) -> i64 {
    let (status, body) = emulator
        .get(&format!(
            "/global-networks/{global_network}/network-resource-count?resourceType={resource_type}"
        ))
        .await;
    assert_eq!(
        status, 200,
        "GetNetworkResourceCounts expected 200, got {status} with {body}"
    );
    body["NetworkResourceCounts"][0]["Count"]
        .as_i64()
        .unwrap_or_else(|| panic!("expected a Count in {body}"))
}

/// One row of the core-network table: the dependents to hang off a fresh core network, and
/// the kinds the refusal must then name.
struct CoreNetworkCase {
    /// Names the case in every assertion message.
    name: &'static str,
    /// VPC attachments to create on the core network, one per edge in turn.
    attachments: usize,
    /// Transit gateway peerings to create on the core network, one per edge in turn.
    peerings: usize,
    /// The dependent kinds the refusal must name, in no particular order.
    blocking: &'static [&'static str],
}

/// The rows differ along which dependent kinds the core network holds, how many of each and
/// which edges they sit at — everything the rule generalises over. The last row holds
/// nothing, so it is the control: the same call succeeds.
const CORE_NETWORK_CASES: [CoreNetworkCase; 6] = [
    CoreNetworkCase {
        name: "one peering",
        attachments: 0,
        peerings: 1,
        blocking: &["peering"],
    },
    CoreNetworkCase {
        name: "three peerings, one at each edge",
        attachments: 0,
        peerings: 3,
        blocking: &["peering"],
    },
    CoreNetworkCase {
        name: "one attachment",
        attachments: 1,
        peerings: 0,
        blocking: &["attachment"],
    },
    CoreNetworkCase {
        name: "two attachments and two peerings",
        attachments: 2,
        peerings: 2,
        blocking: &["attachment", "peering"],
    },
    CoreNetworkCase {
        name: "two attachments and one peering",
        attachments: 2,
        peerings: 1,
        blocking: &["attachment", "peering"],
    },
    CoreNetworkCase {
        name: "nothing at all",
        attachments: 0,
        peerings: 0,
        blocking: &[],
    },
];

/// Build the core network a row describes, answering `(global network, core network, peering
/// ids, attachment ids)`. `base` keeps the VPCs and transit gateways of two rows apart: a VPC
/// is attached once and a gateway is peered once.
async fn build(
    emulator: &Emulator,
    case: &CoreNetworkCase,
    base: usize,
) -> (String, String, Vec<String>, Vec<String>) {
    let name = case.name;
    let (global_network, core_network) = core_network_at(emulator, name, &EDGES).await;

    let mut attachments = Vec::new();
    for index in 0..case.attachments {
        let position = base + index;
        let region = EDGES[index % EDGES.len()];
        let (status, created) = create_vpc_attachment(
            emulator,
            json!({
                "CoreNetworkId": core_network,
                "VpcArn": vpc_arn(region, position),
                "SubnetArns": [subnet_arn(region, position)],
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?}: CreateVpcAttachment expected 200, got {status} with {created}"
        );
        attachments.push(attachment_id(name, &created["VpcAttachment"]));
    }

    let mut peerings = Vec::new();
    for index in 0..case.peerings {
        let position = base + index;
        let region = EDGES[index % EDGES.len()];
        let (status, created) = create_peering(
            emulator,
            json!({
                "CoreNetworkId": core_network,
                "TransitGatewayArn": transit_gateway_arn("aws", region, &own_account(), position, 17),
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?}: CreateTransitGatewayPeering expected 200, got {status} with {created}"
        );
        peerings.push(peering_id(name, &created["TransitGatewayPeering"]));
    }

    (global_network, core_network, peerings, attachments)
}

#[tokio::test]
async fn delete_core_network_is_refused_while_it_holds_attachments_or_peerings() {
    for (row, case) in CORE_NETWORK_CASES.iter().enumerate() {
        let emulator = Emulator::new();
        let name = case.name;
        let base = 100 + row * 10;
        let (global_network, core_network, peerings, attachments) =
            build(&emulator, case, base).await;

        let (status, body) = emulator
            .delete(&format!("/core-networks/{core_network}"))
            .await;

        if case.blocking.is_empty() {
            assert_eq!(
                status, 200,
                "case {name:?}: a core network holding nothing deletes, got {status} with {body}"
            );
            assert_eq!(
                body["CoreNetwork"]["State"],
                json!("DELETING"),
                "case {name:?}: the removed record reports DELETING, got {body}"
            );
            continue;
        }

        assert_blocked_by(name, "Core Network", status, &body, case.blocking);

        // The refusal changed nothing: the core network, every peering and every attachment
        // are still stored, and the inventory that resolves a peering through its core
        // network counts exactly the peerings their own reader still serves — the two
        // readers agree, which is what a delete that removed the parent would break.
        let (status, read) = emulator
            .get(&format!("/core-networks/{core_network}"))
            .await;
        assert_eq!(
            status, 200,
            "case {name:?}: a refused delete must leave the core network readable, got {status} \
             with {read}"
        );
        for peering in &peerings {
            let (status, read) = emulator
                .get(&format!("/transit-gateway-peerings/{peering}"))
                .await;
            assert_eq!(
                status, 200,
                "case {name:?}: a refused delete must leave {peering} readable, got {status} \
                 with {read}"
            );
            assert_eq!(
                read["TransitGatewayPeering"]["Peering"]["CoreNetworkId"],
                json!(core_network),
                "case {name:?}: {peering} must still name the core network it hangs off, got \
                 {read}"
            );
        }
        assert_eq!(
            counted(&emulator, &global_network, "peering").await,
            peerings.len() as i64,
            "case {name:?}: the inventory must count every peering its own reader serves"
        );
        for attachment in &attachments {
            let (status, read) = emulator
                .get(&format!("/vpc-attachments/{attachment}"))
                .await;
            assert_eq!(
                status, 200,
                "case {name:?}: a refused delete must leave {attachment} readable, got {status} \
                 with {read}"
            );
        }

        // Once the dependents are gone the identical call succeeds, so the refusal tracked
        // stored state rather than the shape of the request.
        for peering in &peerings {
            let (status, deleted) = emulator.delete(&format!("/peerings/{peering}")).await;
            assert_eq!(
                status, 200,
                "case {name:?}: DeletePeering expected 200, got {status} with {deleted}"
            );
        }
        for attachment in &attachments {
            let (status, deleted) = emulator.delete(&format!("/attachments/{attachment}")).await;
            assert_eq!(
                status, 200,
                "case {name:?}: DeleteAttachment expected 200, got {status} with {deleted}"
            );
        }
        let (status, deleted) = emulator
            .delete(&format!("/core-networks/{core_network}"))
            .await;
        assert_eq!(
            status, 200,
            "case {name:?}: with the dependents gone the same delete must succeed, got {status} \
             with {deleted}"
        );
        assert_eq!(
            deleted["CoreNetwork"]["State"],
            json!("DELETING"),
            "case {name:?}: the removed record reports DELETING, got {deleted}"
        );

        // And no dependent outlived the parent: the peering reader and the inventory agree
        // that nothing is left, so no record names a core network no read can resolve.
        let (status, gone) = emulator
            .get(&format!("/core-networks/{core_network}"))
            .await;
        assert_eq!(
            status, 404,
            "case {name:?}: GetCoreNetwork after the delete expected 404, got {status} with {gone}"
        );
        for peering in &peerings {
            let (status, gone) = emulator
                .get(&format!("/transit-gateway-peerings/{peering}"))
                .await;
            assert_eq!(
                status, 404,
                "case {name:?}: {peering} must not outlive its core network, got {status} with \
                 {gone}"
            );
            assert_eq!(
                error_code(&gone),
                "ResourceNotFoundException",
                "case {name:?}: expected ResourceNotFoundException for {peering}, got {gone}"
            );
        }
        assert_eq!(
            counted(&emulator, &global_network, "peering").await,
            0,
            "case {name:?}: the inventory must count no peering once none is stored"
        );
    }
}

/// One row of the Connect-attachment table: which rows of
/// `crate::connect_peer_lifecycle::PEER_CASES` to create, all of them carried over the same
/// Connect attachment, and the kinds the refusal must then name.
struct ConnectCase {
    name: &'static str,
    /// Indices into `PEER_CASES`; every row named here must share one transport, which is
    /// the attachment the delete names.
    peers: &'static [usize],
    blocking: &'static [&'static str],
}

/// The rows differ in how many Connect peers ride the attachment and in the tunnel protocol
/// and edge location of that attachment (`PEER_CASES` rows 0 and 2 share a GRE transport,
/// row 1 is on a second GRE transport, row 4 is on a NO_ENCAP one). The last row carries no
/// peer, so it is the control: the same call succeeds.
const CONNECT_CASES: [ConnectCase; 4] = [
    ConnectCase {
        name: "one peer on a GRE transport",
        peers: &[1],
        blocking: &["connect peer"],
    },
    ConnectCase {
        name: "two peers on one GRE transport",
        peers: &[0, 2],
        blocking: &["connect peer"],
    },
    ConnectCase {
        name: "one peer on a NO_ENCAP transport",
        peers: &[4],
        blocking: &["connect peer"],
    },
    ConnectCase {
        name: "no peer at all",
        peers: &[],
        blocking: &[],
    },
];

#[tokio::test]
async fn delete_attachment_is_refused_while_the_connect_attachment_carries_peers() {
    for case in CONNECT_CASES.iter() {
        let emulator = Emulator::new();
        let name = case.name;
        let subject = subject(&emulator, name, 0).await;
        // Every row of a case shares one transport, and a row with no peer takes the last
        // Connect attachment of the subject, so each case deletes exactly one attachment.
        let transport = case
            .peers
            .first()
            .map(|row| PEER_CASES[*row].transport)
            .unwrap_or(0);
        let attachment = subject.attachments[transport].clone();

        let mut peers = Vec::new();
        for row in case.peers {
            let peer_case = &PEER_CASES[*row];
            assert_eq!(
                peer_case.transport, transport,
                "case {name:?}: every peer of a row must ride the attachment under test"
            );
            let (status, created) =
                create_connect_peer(&emulator, connect_peer_request(peer_case, &attachment)).await;
            assert_eq!(
                status, 200,
                "case {name:?}: CreateConnectPeer expected 200, got {status} with {created}"
            );
            peers.push(
                created["ConnectPeer"]["ConnectPeerId"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {name:?}: no ConnectPeerId in {created}"))
                    .to_string(),
            );
        }

        let (status, body) = emulator
            .delete(&format!("/attachments/{attachment}"))
            .await;

        if case.blocking.is_empty() {
            assert_eq!(
                status, 200,
                "case {name:?}: an attachment carrying nothing deletes, got {status} with {body}"
            );
            assert_eq!(
                body["Attachment"]["State"],
                json!("DELETING"),
                "case {name:?}: the removed record reports DELETING, got {body}"
            );
            continue;
        }

        assert_blocked_by(name, "Attachment", status, &body, case.blocking);

        // The refusal changed nothing: the attachment and every peer it carries are still
        // stored, and the inventory counts exactly the peers their own reader serves.
        let (status, read) = emulator
            .get(&format!("/connect-attachments/{attachment}"))
            .await;
        assert_eq!(
            status, 200,
            "case {name:?}: a refused delete must leave the attachment readable, got {status} \
             with {read}"
        );
        for peer in &peers {
            let (status, read) = emulator.get(&format!("/connect-peers/{peer}")).await;
            assert_eq!(
                status, 200,
                "case {name:?}: a refused delete must leave {peer} readable, got {status} with \
                 {read}"
            );
            assert_eq!(
                read["ConnectPeer"]["ConnectAttachmentId"],
                json!(attachment),
                "case {name:?}: {peer} must still name the attachment it is carried over, got \
                 {read}"
            );
        }
        assert_eq!(
            counted(&emulator, &subject.global_network, "connect-peer").await,
            peers.len() as i64,
            "case {name:?}: the inventory must count every peer its own reader serves"
        );

        // Once the peers are gone the identical call succeeds, and no peer outlives the
        // attachment that carried it.
        for peer in &peers {
            let (status, deleted) = emulator.delete(&format!("/connect-peers/{peer}")).await;
            assert_eq!(
                status, 200,
                "case {name:?}: DeleteConnectPeer expected 200, got {status} with {deleted}"
            );
        }
        let (status, deleted) = emulator
            .delete(&format!("/attachments/{attachment}"))
            .await;
        assert_eq!(
            status, 200,
            "case {name:?}: with the peers gone the same delete must succeed, got {status} with \
             {deleted}"
        );
        assert_eq!(
            deleted["Attachment"]["State"],
            json!("DELETING"),
            "case {name:?}: the removed record reports DELETING, got {deleted}"
        );
        for peer in &peers {
            let (status, gone) = emulator.get(&format!("/connect-peers/{peer}")).await;
            assert_eq!(
                status, 404,
                "case {name:?}: {peer} must not outlive its attachment, got {status} with {gone}"
            );
        }
        assert_eq!(
            counted(&emulator, &subject.global_network, "connect-peer").await,
            0,
            "case {name:?}: the inventory must count no peer once none is stored"
        );
    }
}
