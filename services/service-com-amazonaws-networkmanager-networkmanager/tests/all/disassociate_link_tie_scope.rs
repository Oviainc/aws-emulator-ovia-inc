// SPDX-License-Identifier: CC0-1.0

//! Which stored ties block `DisassociateLink` and which do not, driven over HTTP.
//!
//! The rule under test: `DisassociateLink` removes the device/link pair the request
//! addresses unless a stored tie of the SAME global network names both that device and
//! that link, in which case the call is refused with the dependency `ValidationException`
//! (400) naming the tie, and writes nothing — while a tie that names the same device over
//! another link, another device over the same link, or no link at all does not stand on
//! the addressed pair and so does not block it. Three kinds of record can carry such a
//! pair, and each is made only over a link the device it names is already associated with
//! ("If you specify a link, it must be associated with the specified device",
//! `smithy_query query="doc" shape="AssociateCustomerGateway"`, and the same sentence for
//! `AssociateTransitGatewayConnectPeer` and `AssociateConnectPeer`), so untying the pair
//! under one of them would leave it standing on a precondition it no longer satisfies and
//! the rule holds for all three alike.
//!
//! The table spans what the rule generalises over: each of those three kinds crossed with
//! every placement of the tie relative to the addressed pair. `crate::connect_peer_tie_removal_refusals`
//! covers the removals a Connect peer association blocks; what this suite adds is the
//! SCOPE of the refusal — the customer gateway kind in the same wording, and the
//! placements that must NOT block — so the refusal cannot be read as "any tie of the
//! global network blocks any pair".
//!
//! Every call goes through the real `MultiServiceRouter`, so the restJson1 routes, the
//! label/query bindings and the error envelopes are exercised as a client hits them.
//! Nothing reaches into `crate::state`: a tie is only ever made, read and removed through
//! an operation.
//!
//! The harness is the one `global_network_lifecycle` defines — one shared engine, this
//! service, the production dispatch.

use serde_json::{json, Value};

use crate::attachment_lifecycle::{
    attachment_id, create_connect_attachment, create_vpc_attachment, policy_document, subnet_arn,
    vpc_arn,
};
use crate::connect_peer_association_lifecycle::{
    associate as associate_connect_peer, create_connect_peer,
    get_associations as get_connect_peer_associations,
};
use crate::connect_peer_tie_removal_refusals::assert_dependency_refusal;
use crate::core_network_lifecycle::create_core_network;
use crate::customer_gateway_association_lifecycle::{
    associate as associate_customer_gateway, disassociate as disassociate_customer_gateway,
    get_associations as get_customer_gateway_associations, plain_cgw_arn, register_vpn_carrier,
};
use crate::device_lifecycle::create_device_ok;
use crate::disassociate_connect_peer::disassociate as disassociate_connect_peer;
use crate::global_network_lifecycle::Emulator;
use crate::link_association_lifecycle::{
    associate_ok as associate_link_ok, disassociate as disassociate_link,
    get_associations as get_link_associations, pairs_of,
};
use crate::link_lifecycle::{create_link_ok, plain_link};
use crate::transit_gateway_connect_peer_association_lifecycle::{
    associate as associate_transit_gateway_connect_peer, connect_peer_arn,
    disassociate as disassociate_transit_gateway_connect_peer,
    get_associations as get_transit_gateway_connect_peer_associations, register_gateway,
};
use crate::transit_gateway_peering::{own_account, transit_gateway_arn};

/// The Region the transit gateway, its Connect peer and the core network's single edge all
/// live in: a Connect peer is created on a transit gateway, so a registration in the
/// peer's Region is what lets the peering be associated at all.
const REGION: &str = "us-west-2";

/// The id of the customer gateway a row ties, as `cgw-{hex}` with the 17 hex digits the
/// longest published EC2 resource id carries (`^arn:[^:]{1,63}:ec2:[^:]{0,63}:[^:]{0,63}:vpn-connection\/vpn-[0-9a-f]{8,17}$`
/// for `VpnConnectionArn`, `smithy_query shape=VpnConnectionArn`).
const CUSTOMER_GATEWAY_HEX: &str = "0a1b2c3d4e5f60718";

/// Which kind of stored record a row hangs over the network's device/link pairs.
#[derive(Clone, Copy)]
enum Kind {
    CustomerGateway,
    TransitGatewayConnectPeer,
    ConnectPeer,
}

impl Kind {
    /// Names the kind in every assertion message.
    fn label(self) -> &'static str {
        match self {
            Kind::CustomerGateway => "a customer gateway association",
            Kind::TransitGatewayConnectPeer => "a transit gateway Connect peer association",
            Kind::ConnectPeer => "a Connect peer association",
        }
    }
}

/// Where a row's tie sits relative to the device/link pair the `DisassociateLink` under
/// test addresses.
#[derive(Clone, Copy)]
enum Placement {
    /// Over the addressed device and the addressed link: the pair the call would untie.
    AddressedPair,
    /// The addressed device, but another link that device is associated with.
    SameDeviceOtherLink,
    /// The addressed link, but another device that link is associated with.
    SameLinkOtherDevice,
    /// The addressed device with no link at all.
    NoLink,
}

/// The rows' placements, in the order the tests walk them.
const PLACEMENTS: [Placement; 4] = [
    Placement::AddressedPair,
    Placement::SameDeviceOtherLink,
    Placement::SameLinkOtherDevice,
    Placement::NoLink,
];

impl Placement {
    /// Names the placement in every assertion message.
    fn label(self) -> &'static str {
        match self {
            Placement::AddressedPair => "the addressed device over the addressed link",
            Placement::SameDeviceOtherLink => "the addressed device over another of its links",
            Placement::SameLinkOtherDevice => "another device over the addressed link",
            Placement::NoLink => "the addressed device over no link at all",
        }
    }

    /// Whether a tie at this placement stands on the addressed pair, which is what the
    /// refusal is about.
    fn blocks(self) -> bool {
        matches!(self, Placement::AddressedPair)
    }

    /// The device the row's tie names.
    fn device(self, network: &Network) -> String {
        match self {
            Placement::SameLinkOtherDevice => network.other_device.clone(),
            _ => network.device.clone(),
        }
    }

    /// The link the row's tie names, or `None` where it names none.
    fn link(self, network: &Network) -> Option<String> {
        match self {
            Placement::AddressedPair | Placement::SameLinkOtherDevice => {
                Some(network.link.clone())
            }
            Placement::SameDeviceOtherLink => Some(network.other_link.clone()),
            Placement::NoLink => None,
        }
    }
}

/// The global network a row runs in: two devices and two links of one site, with three of
/// the four pairs associated, so a tie can be hung on the addressed pair, on the addressed
/// device over the other link, or on the other device over the addressed link.
struct Network {
    id: String,
    /// The device of the pair the `DisassociateLink` under test addresses.
    device: String,
    /// Another device of the same site, associated with [`Self::link`].
    other_device: String,
    /// The link of the pair the `DisassociateLink` under test addresses.
    link: String,
    /// Another link of the same site, associated with [`Self::device`].
    other_link: String,
}

impl Network {
    /// The pairs that must survive a `DisassociateLink` of the addressed pair: every
    /// association the call does not address, in key order.
    fn surviving_pairs(&self) -> Vec<(String, String)> {
        let mut pairs = vec![
            (self.device.clone(), self.other_link.clone()),
            (self.other_device.clone(), self.link.clone()),
        ];
        pairs.sort();
        pairs
    }
}

/// Build the global network above, named `case`.
async fn network(emulator: &Emulator, case: &str) -> Network {
    let id = emulator.create_global_network(case).await;
    let site = emulator.create_site(&id, case).await;
    let device = create_device_ok(emulator, &id, json!({ "SiteId": site })).await;
    let other_device = create_device_ok(emulator, &id, json!({ "SiteId": site })).await;
    let link = create_link_ok(emulator, &id, &site, &plain_link()).await;
    let other_link = create_link_ok(emulator, &id, &site, &plain_link()).await;
    for (device_id, link_id) in [
        (&device, &link),
        (&device, &other_link),
        (&other_device, &link),
    ] {
        associate_link_ok(emulator, &id, device_id, link_id).await;
    }
    Network {
        id,
        device,
        other_device,
        link,
        other_link,
    }
}

/// A Connect peer of a core network of `network`: the policy configures one edge, a VPC
/// attachment carries the Connect attachment, and the peer hangs off that attachment —
/// the chain `AssociateConnectPeer` requires the peer to sit at the end of.
async fn connect_peer(emulator: &Emulator, case: &str, network: &str) -> String {
    let (status, created) = create_core_network(
        emulator,
        json!({
            "GlobalNetworkId": network,
            "Description": case,
            "PolicyDocument": policy_document(&[REGION]),
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateCoreNetwork expected 200, got {status} with {created}"
    );
    let core_network = created["CoreNetwork"]["CoreNetworkId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no CoreNetworkId in {created}"))
        .to_string();

    let (status, created) = create_vpc_attachment(
        emulator,
        json!({
            "CoreNetworkId": core_network,
            "VpcArn": vpc_arn(REGION, 1),
            "SubnetArns": [subnet_arn(REGION, 1)],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateVpcAttachment expected 200, got {status} with {created}"
    );
    let transport = attachment_id(case, &created["VpcAttachment"]);

    let (status, created) = create_connect_attachment(
        emulator,
        json!({
            "CoreNetworkId": core_network,
            "EdgeLocation": REGION,
            "TransportAttachmentId": transport,
            "Options": { "Protocol": "GRE" },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateConnectAttachment expected 200, got {status} with {created}"
    );
    let attachment = attachment_id(case, &created["ConnectAttachment"]);
    create_connect_peer(emulator, &attachment, 0).await
}

/// The tie a row hangs over the network: what its own reader and its remover address it
/// by, and how a refusal that names it reads.
struct Tie {
    /// The peer or gateway the tie names, which identifies it everywhere.
    id: String,
    /// How a dependency refusal names the tie.
    subject: String,
}

/// Hang a tie of `kind` at `placement` over `network`, asserting the associate succeeded.
async fn tie(
    emulator: &Emulator,
    case: &str,
    kind: Kind,
    network: &Network,
    placement: Placement,
) -> Tie {
    let device = placement.device(network);
    let link = placement.link(network);
    match kind {
        Kind::CustomerGateway => {
            let arn = plain_cgw_arn(CUSTOMER_GATEWAY_HEX);
            // A customer gateway is associated only where the global network carries, in
            // the gateway's Region, a transit gateway or core network edge its VPN
            // attachment could be on.
            register_vpn_carrier(emulator, &network.id, REGION).await;
            let (status, created) = associate_customer_gateway(
                emulator,
                &network.id,
                request(json!({ "CustomerGatewayArn": arn, "DeviceId": device }), &link),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: AssociateCustomerGateway expected 200, got {status} with {created}"
            );
            Tie {
                subject: format!("customer gateway {arn}"),
                id: arn,
            }
        }
        Kind::TransitGatewayConnectPeer => {
            let account = own_account();
            register_gateway(
                emulator,
                &network.id,
                &transit_gateway_arn("aws", REGION, &account, 1, 17),
            )
            .await;
            let arn = connect_peer_arn("aws", REGION, &account, 1, 17);
            let (status, created) = associate_transit_gateway_connect_peer(
                emulator,
                &network.id,
                request(
                    json!({ "TransitGatewayConnectPeerArn": arn, "DeviceId": device }),
                    &link,
                ),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: AssociateTransitGatewayConnectPeer expected 200, got {status} with {created}"
            );
            Tie {
                subject: format!("transit gateway Connect peer {arn}"),
                id: arn,
            }
        }
        Kind::ConnectPeer => {
            let peer = connect_peer(emulator, case, &network.id).await;
            let (status, created) = associate_connect_peer(
                emulator,
                &network.id,
                request(json!({ "ConnectPeerId": peer, "DeviceId": device }), &link),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: AssociateConnectPeer expected 200, got {status} with {created}"
            );
            Tie {
                subject: format!("Connect peer {peer}"),
                id: peer,
            }
        }
    }
}

/// `body` with the optional `LinkId` member added where the row names a link: all three
/// associates spell that member the same way and all three leave it out to name no link.
fn request(mut body: Value, link: &Option<String>) -> Value {
    if let Some(link) = link {
        body["LinkId"] = json!(link);
    }
    body
}

/// The one association of `kind` that `id` names, as its own reader reports it, or `None`
/// when the reader reports none.
async fn reported_tie(
    emulator: &Emulator,
    case: &str,
    kind: Kind,
    network: &str,
    id: &str,
) -> Option<Value> {
    let (list, id_member, (status, body)) = match kind {
        Kind::CustomerGateway => (
            "CustomerGatewayAssociations",
            "CustomerGatewayArn",
            get_customer_gateway_associations(emulator, network, "").await,
        ),
        Kind::TransitGatewayConnectPeer => (
            "TransitGatewayConnectPeerAssociations",
            "TransitGatewayConnectPeerArn",
            get_transit_gateway_connect_peer_associations(emulator, network, "").await,
        ),
        Kind::ConnectPeer => (
            "ConnectPeerAssociations",
            "ConnectPeerId",
            get_connect_peer_associations(emulator, network, "").await,
        ),
    };
    assert_eq!(
        status, 200,
        "case {case:?}: reading the {list} of {network} expected 200, got {status} with {body}"
    );
    body[list]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a {list} array in {body}"))
        .iter()
        .find(|association| association[id_member].as_str() == Some(id))
        .cloned()
}

/// Assert the tie is still stored and still names the link it was made over.
async fn assert_tie_intact(
    emulator: &Emulator,
    case: &str,
    kind: Kind,
    network: &str,
    tie: &Tie,
    link: &Option<String>,
) {
    let association = reported_tie(emulator, case, kind, network, &tie.id)
        .await
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: {} must still be reported by its own reader",
                tie.subject
            )
        });
    match link {
        Some(link) => assert_eq!(
            association["LinkId"].as_str(),
            Some(link.as_str()),
            "case {case:?}: expected the tie to still name link {link}, got {association}"
        ),
        None => assert!(
            association["LinkId"].is_null(),
            "case {case:?}: a tie made over no link must report none, got {association}"
        ),
    }
}

/// Remove the tie through the disassociate that owns its kind.
async fn remove_tie(emulator: &Emulator, kind: Kind, network: &str, id: &str) -> (u16, Value) {
    match kind {
        Kind::CustomerGateway => disassociate_customer_gateway(emulator, network, id).await,
        Kind::TransitGatewayConnectPeer => {
            disassociate_transit_gateway_connect_peer(emulator, network, id).await
        }
        Kind::ConnectPeer => disassociate_connect_peer(emulator, network, id).await,
    }
}

/// The device/link pairs `GetLinkAssociations` reports for the network's two devices, in
/// key order. The reader narrows by device or by link and refuses a request naming
/// neither, so the pairs are read one device at a time and merged.
async fn associated_pairs(
    emulator: &Emulator,
    case: &str,
    network: &Network,
) -> Vec<(String, String)> {
    let mut pairs: Vec<(String, String)> = Vec::new();
    for device in [&network.device, &network.other_device] {
        let (status, body) =
            get_link_associations(emulator, &network.id, &format!("deviceId={device}")).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetLinkAssociations({device}) expected 200, got {status} with {body}"
        );
        pairs.extend(pairs_of(&body));
    }
    pairs.sort();
    pairs
}

/// Assert a `DisassociateLink` succeeded, reported the addressed pair as `DELETING`, and
/// left exactly the pairs it did not address stored.
async fn assert_untied(
    emulator: &Emulator,
    case: &str,
    status: u16,
    body: &Value,
    network: &Network,
) {
    assert_eq!(
        status, 200,
        "case {case:?}: DisassociateLink expected 200, got {status} with {body}"
    );
    let removed = &body["LinkAssociation"];
    assert_eq!(
        (removed["DeviceId"].as_str(), removed["LinkId"].as_str()),
        (
            Some(network.device.as_str()),
            Some(network.link.as_str())
        ),
        "case {case:?}: expected the removed association to name the addressed pair, got {removed}"
    );
    assert_eq!(
        removed["LinkAssociationState"],
        json!("DELETING"),
        "case {case:?}: the removed association must be reported DELETING, got {removed}"
    );
    assert_eq!(
        associated_pairs(emulator, case, network).await,
        network.surviving_pairs(),
        "case {case:?}: DisassociateLink must remove exactly the pair it addressed"
    );
}

/// Walk one row: hang a tie of `kind` at `placement`, then call `DisassociateLink` on the
/// addressed pair and hold it to the rule.
async fn row(kind: Kind, placement: Placement) {
    let case = format!("{} over {}", kind.label(), placement.label());
    let case = case.as_str();
    let emulator = Emulator::new();
    let built = network(&emulator, case).await;
    let hung = tie(&emulator, case, kind, &built, placement).await;
    let tied_link = placement.link(&built);

    let (status, body) = disassociate_link(&emulator, &built.id, &built.device, &built.link).await;
    if placement.blocks() {
        assert_dependency_refusal(
            case,
            status,
            &body,
            &format!(
                "{} must be disassociated before device {} is disassociated from link {}",
                hung.subject, built.device, built.link
            ),
        );

        // The refusal wrote nothing: the addressed pair is still associated, every other
        // pair is untouched, and the tie still stands over the pair it was made on.
        let mut expected = built.surviving_pairs();
        expected.push((built.device.clone(), built.link.clone()));
        expected.sort();
        assert_eq!(
            associated_pairs(&emulator, case, &built).await,
            expected,
            "case {case:?}: a refused DisassociateLink must leave every pair associated"
        );
        assert_tie_intact(&emulator, case, kind, &built.id, &hung, &tied_link).await;

        // Removing the tie satisfies the precondition the refusal was about, so the
        // identical call now succeeds.
        let (status, removed) = remove_tie(&emulator, kind, &built.id, &hung.id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: removing the tie expected 200, got {status} with {removed}"
        );
        let (status, body) =
            disassociate_link(&emulator, &built.id, &built.device, &built.link).await;
        assert_untied(&emulator, case, status, &body, &built).await;
    } else {
        // The tie stands on a pair this call does not untie, so it blocks nothing and
        // survives the call naming the link or the device it shares.
        assert_untied(&emulator, case, status, &body, &built).await;
        assert_tie_intact(&emulator, case, kind, &built.id, &hung, &tied_link).await;
    }
}

#[tokio::test]
async fn a_customer_gateway_association_blocks_only_the_pair_it_names() {
    for placement in PLACEMENTS {
        row(Kind::CustomerGateway, placement).await;
    }
}

#[tokio::test]
async fn a_transit_gateway_connect_peer_association_blocks_only_the_pair_it_names() {
    for placement in PLACEMENTS {
        row(Kind::TransitGatewayConnectPeer, placement).await;
    }
}

#[tokio::test]
async fn a_connect_peer_association_blocks_only_the_pair_it_names() {
    for placement in PLACEMENTS {
        row(Kind::ConnectPeer, placement).await;
    }
}
