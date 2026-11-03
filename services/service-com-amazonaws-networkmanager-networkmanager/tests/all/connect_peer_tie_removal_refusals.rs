// SPDX-License-Identifier: CC0-1.0

//! What a removal of this service answers while a stored Connect peer association still
//! stands on a precondition that removal would take away, driven over HTTP.
//!
//! The rule under test: a removal is refused — with the dependency `ValidationException`
//! (400) every removal of this service refuses by, writing nothing — while a stored tie of
//! the addressed global network would be left standing on a precondition the removal takes
//! away, and the identical call succeeds once the tie is gone. The two Connect peer
//! association kinds this service stores are made only over a registration of a transit
//! gateway the peer could have been created on, a device of the global network, and (where
//! one is named) a link that device is already associated with
//! (`smithy_query query="doc" shape="AssociateTransitGatewayConnectPeer,AssociateConnectPeer"`),
//! so `DeregisterTransitGateway`, `DeleteDevice`, `DeleteLink` and `DisassociateLink` each
//! refuse while such a tie stands over what they remove.
//!
//! The tables span what the rule generalises over: for the registration leg the Connect
//! peer ARN's partition, Region, account and id width, how the optional link member is
//! spelled and whether the device sits at a site, plus WHICH registrations would remain
//! (another gateway in the peer's Region and account keeps the precondition satisfied, one
//! in another Region or another account never carried the peer); for the device and link
//! legs both association kinds, over a link and with no link at all.
//!
//! Every call goes through the real `MultiServiceRouter`, so the restJson1 routes, the
//! label bindings and the error envelopes are exercised as a client hits them. Nothing
//! reaches into `crate::state`: a tie is only ever made and removed through an operation
//! and read back through its own reader.

use serde_json::{json, Value};

use crate::connect_peer_association_lifecycle::{fixture as cloud_wan_fixture, CASES};
use crate::device_lifecycle::create_device_ok;
use crate::delete_live_reference_refusal::refusal_message;
use crate::disassociate_connect_peer::disassociate as disassociate_connect_peer;
use crate::global_network_lifecycle::{error_code, Emulator};
use crate::link_lifecycle::{create_link_ok, plain_link};
use crate::transit_gateway_connect_peer_association_lifecycle::{
    associate as associate_connect_peer, connect_peer_arn, disassociate as disassociate_peer,
    get_associations, register_gateway,
};
use crate::transit_gateway_peering::{own_account, transit_gateway_arn};
use crate::transit_gateway_registration_lifecycle::{arns_of, deregister, get_registrations};

/// The message a refused `DeregisterTransitGateway` carries while one of the global
/// network's transit gateway Connect peer associations could only have stood on the
/// registration it removes: the dependency wording this service refuses the removal of a
/// CONTAINER with, naming the registration as the subject and the blocking tie's kind as the
/// dependent.
const REGISTRATION_BLOCKED: &str = "Transit Gateway Registration cannot be deleted due to an existing transit gateway connect peer association associated with it.";

/// How a refusal spells each global network OBJECT this suite asks a delete of, as
/// `crate::delete_live_reference_refusal` spells them: every word of the ARN resource type
/// capitalised.
const DEVICE_PARENT: &str = "Device";
/// See [`DEVICE_PARENT`].
const LINK_PARENT: &str = "Link";

/// Assert `body` is this service's dependency refusal, carrying `message` exactly: HTTP 400,
/// `ValidationException`, the catch-all `Reason` (no input field is at fault) and no
/// `Fields` entry.
///
/// `crate::delete_dependency_refusal` asserts the same envelope about the deletes it covers.
/// Here the whole message is the expectation: for the registration, because one kind's name
/// is a substring of another's ("connect peer association" of "transit gateway connect peer
/// association"), so only the exact wording can show that the refusal named the kind that
/// blocks and not the other one; for a device or a link, because their recorded template
/// names the object and no dependent at all.
///
/// `pub(crate)` because `tests/all/disassociate_link_tie_scope.rs` holds the same refusal
/// to the same envelope, so both suites assert it through one helper.
pub(crate) fn assert_dependency_refusal(case: &str, status: u16, body: &Value, message: &str) {
    assert_eq!(
        status, 400,
        "case {case:?}: a dependency refusal is HTTP 400, got {status} with {body}"
    );
    assert_eq!(
        error_code(body),
        "ValidationException",
        "case {case:?}: expected ValidationException, got {body}"
    );
    assert_eq!(
        body["Reason"],
        json!("Other"),
        "case {case:?}: no input field is at fault, so the reason is the modelled Other, got {body}"
    );
    assert_eq!(
        body["Fields"],
        Value::Null,
        "case {case:?}: a dependency refusal blames no field, got {body}"
    );
    assert_eq!(
        body["Message"].as_str(),
        Some(message),
        "case {case:?}: expected the dependency message {message:?}, got {body}"
    );
}

/// `DeleteDevice` for `device` of `network`.
async fn delete_device(emulator: &Emulator, network: &str, device: &str) -> (u16, Value) {
    emulator
        .delete(&format!("/global-networks/{network}/devices/{device}"))
        .await
}

/// `DeleteLink` for `link` of `network`.
async fn delete_link(emulator: &Emulator, network: &str, link: &str) -> (u16, Value) {
    emulator
        .delete(&format!("/global-networks/{network}/links/{link}"))
        .await
}

/// `DisassociateLink` for the `device`/`link` pair of `network`.
async fn disassociate_link(
    emulator: &Emulator,
    network: &str,
    device: &str,
    link: &str,
) -> (u16, Value) {
    emulator
        .delete(&format!(
            "/global-networks/{network}/link-associations?deviceId={device}&linkId={link}"
        ))
        .await
}

/// `AssociateLink` for the `device`/`link` pair of `network`, which must succeed.
async fn associate_link_ok(emulator: &Emulator, case: &str, network: &str, device: &str, link: &str) {
    let (status, body) = emulator
        .post(
            &format!("/global-networks/{network}/link-associations"),
            json!({ "DeviceId": device, "LinkId": link }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: AssociateLink expected 200, got {status} with {body}"
    );
}

/// The global network a row is exercised in: a site, a device at that site, a device at no
/// site, and a link of the site the sited device is associated with.
struct Network {
    id: String,
    sited_device: String,
    siteless_device: String,
    link: String,
}

/// Build the global network above, named `case`.
async fn network(emulator: &Emulator, case: &str) -> Network {
    let id = emulator.create_global_network(case).await;
    let site = emulator.create_site(&id, case).await;
    let sited_device = create_device_ok(emulator, &id, json!({ "SiteId": site })).await;
    let siteless_device = create_device_ok(emulator, &id, json!({ "Description": case })).await;
    let link = create_link_ok(emulator, &id, &site, &plain_link()).await;
    associate_link_ok(emulator, case, &id, &sited_device, &link).await;
    Network {
        id,
        sited_device,
        siteless_device,
        link,
    }
}

/// The `TransitGatewayConnectPeerArn`s `GetTransitGatewayConnectPeerAssociations` reports
/// for `network`.
async fn associated_peers(emulator: &Emulator, case: &str, network: &str) -> Vec<String> {
    let (status, body) = get_associations(emulator, network, "").await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetTransitGatewayConnectPeerAssociations expected 200, got {status} with {body}"
    );
    crate::transit_gateway_connect_peer_association_lifecycle::arns_of(case, &body)
}

/// The `TransitGatewayArn`s `GetTransitGatewayRegistrations` reports for `network`.
async fn registered_gateways(emulator: &Emulator, case: &str, network: &str) -> Vec<String> {
    let (status, body) = get_registrations(emulator, network, "").await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetTransitGatewayRegistrations expected 200, got {status} with {body}"
    );
    arns_of(&body)
}

/// One row of the registration table: the Connect peer tied, and how it is tied.
struct Row {
    name: &'static str,
    /// The partition, Region and account the row's gateway and its Connect peer live in —
    /// a Connect peer is created on a transit gateway, so it shares all three.
    partition: &'static str,
    region: &'static str,
    account: String,
    /// How many hex digits the peer's id carries; the published bounds of an EC2 resource
    /// id are 8 and 17 (`^arn:[^:]{1,63}:ec2:[^:]{0,63}:[^:]{0,63}:vpc\/vpc-[0-9a-f]{8,17}$`
    /// for `VpcArn`, `smithy_query shape=VpcArn`).
    digits: usize,
    /// Whether the row ties the peer over the link its device is associated with.
    over_link: bool,
    /// Whether the row ties the device that sits at a site.
    sited_device: bool,
}

/// The rows, differing along everything the rule generalises over: the ARN's partition,
/// Region and id width, the optional link member named and omitted, and a device at a site
/// beside one at none. The account is the one that owns the global network on every row: a
/// gateway is registered only from that account, so a peer that sits on a registered
/// gateway belongs to it too.
fn rows() -> Vec<Row> {
    vec![
        Row {
            name: "the shortest published id, over a link of the device's site",
            partition: "aws",
            region: "us-west-2",
            account: own_account(),
            digits: 8,
            over_link: true,
            sited_device: true,
        },
        Row {
            name: "the longest published id in another Region, naming no link",
            partition: "aws",
            region: "eu-central-1",
            account: own_account(),
            digits: 17,
            over_link: false,
            sited_device: true,
        },
        Row {
            name: "another partition and Region, tied to a device that sits at no site",
            partition: "aws-cn",
            region: "cn-north-1",
            account: own_account(),
            digits: 12,
            over_link: false,
            sited_device: false,
        },
        Row {
            name: "a third Region",
            partition: "aws",
            region: "ap-southeast-2",
            account: own_account(),
            digits: 10,
            over_link: false,
            sited_device: false,
        },
        Row {
            name: "a third partition",
            partition: "aws-us-gov",
            region: "us-gov-west-1",
            account: own_account(),
            digits: 16,
            over_link: false,
            sited_device: true,
        },
    ]
}

#[tokio::test]
async fn deregistering_the_gateway_a_connect_peer_association_stands_on_is_refused() {
    for row in rows() {
        let case = row.name;
        let emulator = Emulator::new();
        let built = network(&emulator, case).await;

        let gateway = transit_gateway_arn(row.partition, row.region, &row.account, 1, 17);
        register_gateway(&emulator, &built.id, &gateway).await;

        let peer = connect_peer_arn(row.partition, row.region, &row.account, 1, row.digits);
        let device = if row.sited_device {
            &built.sited_device
        } else {
            &built.siteless_device
        };
        let mut request = json!({
            "TransitGatewayConnectPeerArn": peer,
            "DeviceId": device,
        });
        if row.over_link {
            request["LinkId"] = json!(built.link);
        }
        let (status, body) = associate_connect_peer(&emulator, &built.id, request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: AssociateTransitGatewayConnectPeer expected 200, got {status} with {body}"
        );

        // The association stands on this registration — it is the only gateway of this
        // global network the peer could have been created on — so the deregister is refused.
        let (status, refused) = deregister(&emulator, &built.id, &gateway).await;
        assert_dependency_refusal(case, status, &refused, REGISTRATION_BLOCKED);

        // And it wrote nothing: both the registration and the tie over it are still stored.
        assert_eq!(
            registered_gateways(&emulator, case, &built.id).await,
            vec![gateway.clone()],
            "case {case:?}: a refused deregister must leave the registration stored"
        );
        assert_eq!(
            associated_peers(&emulator, case, &built.id).await,
            vec![peer.clone()],
            "case {case:?}: a refused deregister must leave the association stored"
        );

        // Untying the peering satisfies the precondition the refusal was about, so the
        // identical call now succeeds and removes the registration.
        let (status, removed) = disassociate_peer(&emulator, &built.id, &peer).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DisassociateTransitGatewayConnectPeer expected 200, got {status} with {removed}"
        );
        let (status, removed) = deregister(&emulator, &built.id, &gateway).await;
        assert_eq!(
            status, 200,
            "case {case:?}: with the tie gone the same deregister must succeed, got {status} with {removed}"
        );
        assert_eq!(
            removed["TransitGatewayRegistration"]["State"]["Code"],
            json!("DELETING"),
            "case {case:?}: the removed registration must be reported DELETING, got {removed}"
        );
        assert!(
            registered_gateways(&emulator, case, &built.id).await.is_empty(),
            "case {case:?}: the deregistered gateway must not be listed any more"
        );
    }
}

#[tokio::test]
async fn only_the_last_registration_the_peer_could_have_been_created_on_blocks_the_deregister() {
    let emulator = Emulator::new();
    let case = "one of two gateways in the peer's Region and account";
    let built = network(&emulator, case).await;

    // The peer lives in one Region and account, so a registered gateway can carry it only
    // if it lives in the same two. A gateway is registered only from the account that owns
    // the global network, so what varies between the registrations below is the Region: two
    // of the three are in the peer's.
    let region = "us-west-2";
    let account = own_account();
    let carrying = transit_gateway_arn("aws", region, &account, 1, 17);
    let also_carrying = transit_gateway_arn("aws", region, &account, 2, 17);
    let other_region = transit_gateway_arn("aws", "eu-central-1", &account, 3, 17);
    for gateway in [&carrying, &also_carrying, &other_region] {
        register_gateway(&emulator, &built.id, gateway).await;
    }

    let peer = connect_peer_arn("aws", region, &account, 1, 17);
    let (status, body) = associate_connect_peer(
        &emulator,
        &built.id,
        json!({
            "TransitGatewayConnectPeerArn": peer,
            "DeviceId": built.sited_device,
            "LinkId": built.link,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: AssociateTransitGatewayConnectPeer expected 200, got {status} with {body}"
    );

    // No tie stands on a registration of another Region: that gateway could never have
    // carried this peer, so its deregister is unaffected.
    let (status, removed) = deregister(&emulator, &built.id, &other_region).await;
    assert_eq!(
        status, 200,
        "case a gateway in another Region: nothing stands on this registration, so the deregister must succeed, got {status} with {removed}"
    );

    // One of the two that could carry the peer still leaves the other, so the precondition
    // the tie stands on keeps holding and the deregister succeeds.
    let (status, removed) = deregister(&emulator, &built.id, &carrying).await;
    assert_eq!(
        status, 200,
        "case {case:?}: another registration still carries the peer, so the deregister must succeed, got {status} with {removed}"
    );

    // The last one does not: removing it is what would leave the tie standing on nothing.
    let (status, refused) = deregister(&emulator, &built.id, &also_carrying).await;
    assert_dependency_refusal(
        "the last gateway in the peer's Region and account",
        status,
        &refused,
        REGISTRATION_BLOCKED,
    );
    assert_eq!(
        registered_gateways(&emulator, case, &built.id).await,
        vec![also_carrying.clone()],
        "case {case:?}: a refused deregister must leave the registration stored"
    );

    let (status, removed) = disassociate_peer(&emulator, &built.id, &peer).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DisassociateTransitGatewayConnectPeer expected 200, got {status} with {removed}"
    );
    let (status, removed) = deregister(&emulator, &built.id, &also_carrying).await;
    assert_eq!(
        status, 200,
        "case {case:?}: with the tie gone the same deregister must succeed, got {status} with {removed}"
    );
}

#[tokio::test]
async fn deleting_the_device_or_link_a_transit_gateway_connect_peer_association_names_is_refused() {
    let emulator = Emulator::new();
    let case = "a transit gateway Connect peer over a device and a link";
    let built = network(&emulator, case).await;

    let gateway = transit_gateway_arn("aws", "us-west-2", &own_account(), 1, 17);
    register_gateway(&emulator, &built.id, &gateway).await;

    // One peer carried over the device/link pair, and one tied to the device at no site,
    // which names no link at all: the second row is what shows the tie alone blocking the
    // delete, with no link association in the message.
    let over_link = connect_peer_arn("aws", "us-west-2", &own_account(), 1, 17);
    let over_device = connect_peer_arn("aws", "us-west-2", &own_account(), 2, 17);
    for (peer, device, link) in [
        (&over_link, &built.sited_device, Some(&built.link)),
        (&over_device, &built.siteless_device, None),
    ] {
        let mut request = json!({ "TransitGatewayConnectPeerArn": peer, "DeviceId": device });
        if let Some(link) = link {
            request["LinkId"] = json!(link);
        }
        let (status, body) = associate_connect_peer(&emulator, &built.id, request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: AssociateTransitGatewayConnectPeer expected 200, got {status} with {body}"
        );
    }

    let (status, refused) = delete_device(&emulator, &built.id, &built.sited_device).await;
    assert_dependency_refusal(
        "the device the peering is reached over",
        status,
        &refused,
        &refusal_message(DEVICE_PARENT, &built.sited_device),
    );

    let (status, refused) = delete_device(&emulator, &built.id, &built.siteless_device).await;
    assert_dependency_refusal(
        "the device of a peering that names no link",
        status,
        &refused,
        &refusal_message(DEVICE_PARENT, &built.siteless_device),
    );

    let (status, refused) = delete_link(&emulator, &built.id, &built.link).await;
    assert_dependency_refusal(
        "the link the peering is carried over",
        status,
        &refused,
        &refusal_message(LINK_PARENT, &built.link),
    );

    // The device/link tie the peering was made over cannot be untied either: the refusal
    // names the peer still carried over the pair.
    let (status, refused) =
        disassociate_link(&emulator, &built.id, &built.sited_device, &built.link).await;
    assert_dependency_refusal(
        "the device/link pair the peering is carried over",
        status,
        &refused,
        &format!(
            "transit gateway Connect peer {over_link} must be disassociated before device {} is disassociated from link {}",
            built.sited_device, built.link
        ),
    );

    // Untying both peerings satisfies every precondition the refusals were about, so the
    // identical calls succeed.
    for peer in [&over_link, &over_device] {
        let (status, removed) = disassociate_peer(&emulator, &built.id, peer).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DisassociateTransitGatewayConnectPeer expected 200, got {status} with {removed}"
        );
    }
    let (status, removed) =
        disassociate_link(&emulator, &built.id, &built.sited_device, &built.link).await;
    assert_eq!(
        status, 200,
        "case {case:?}: with the peerings gone DisassociateLink must succeed, got {status} with {removed}"
    );
    for (what, (status, removed)) in [
        ("DeleteLink", delete_link(&emulator, &built.id, &built.link).await),
        (
            "DeleteDevice",
            delete_device(&emulator, &built.id, &built.sited_device).await,
        ),
        (
            "DeleteDevice at no site",
            delete_device(&emulator, &built.id, &built.siteless_device).await,
        ),
    ] {
        assert_eq!(
            status, 200,
            "case {case:?}: with the ties gone {what} must succeed, got {status} with {removed}"
        );
    }
}

#[tokio::test]
async fn deleting_the_device_or_link_a_core_network_connect_peer_association_names_is_refused() {
    let emulator = Emulator::new();
    let case = "a core network Connect peer over a device and a link";
    let built = cloud_wan_fixture(&emulator, case, 0).await;

    // The first row of the shared table ties its peer over the link of the sited device,
    // the third ties one to the device that sits at no site and names no link, so the two
    // together span the device leg with and without a link in the way.
    let over_link = built.peers[0].clone();
    let over_device = built.peers[2].clone();
    for (index, peer) in [(0, &over_link), (2, &over_device)] {
        let (status, body) = crate::connect_peer_association_lifecycle::associate(
            &emulator,
            &built.global_network,
            CASES[index].request(&built, peer),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: AssociateConnectPeer expected 200, got {status} with {body}"
        );
    }

    let (status, refused) =
        delete_device(&emulator, &built.global_network, &built.sited_device).await;
    assert_dependency_refusal(
        "the device the peering is reached over",
        status,
        &refused,
        &refusal_message(DEVICE_PARENT, &built.sited_device),
    );

    let (status, refused) =
        delete_device(&emulator, &built.global_network, &built.siteless_device).await;
    assert_dependency_refusal(
        "the device of a peering that names no link",
        status,
        &refused,
        &refusal_message(DEVICE_PARENT, &built.siteless_device),
    );

    let (status, refused) = delete_link(&emulator, &built.global_network, &built.link).await;
    assert_dependency_refusal(
        "the link the peering is carried over",
        status,
        &refused,
        &refusal_message(LINK_PARENT, &built.link),
    );

    let (status, refused) = disassociate_link(
        &emulator,
        &built.global_network,
        &built.sited_device,
        &built.link,
    )
    .await;
    assert_dependency_refusal(
        "the device/link pair the peering is carried over",
        status,
        &refused,
        &format!(
            "Connect peer {over_link} must be disassociated before device {} is disassociated from link {}",
            built.sited_device, built.link
        ),
    );

    for peer in [&over_link, &over_device] {
        let (status, removed) =
            disassociate_connect_peer(&emulator, &built.global_network, peer).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DisassociateConnectPeer expected 200, got {status} with {removed}"
        );
    }
    let (status, removed) = disassociate_link(
        &emulator,
        &built.global_network,
        &built.sited_device,
        &built.link,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: with the peerings gone DisassociateLink must succeed, got {status} with {removed}"
    );
    for (what, (status, removed)) in [
        (
            "DeleteLink",
            delete_link(&emulator, &built.global_network, &built.link).await,
        ),
        (
            "DeleteDevice",
            delete_device(&emulator, &built.global_network, &built.sited_device).await,
        ),
        (
            "DeleteDevice at no site",
            delete_device(&emulator, &built.global_network, &built.siteless_device).await,
        ),
    ] {
        assert_eq!(
            status, 200,
            "case {case:?}: with the ties gone {what} must succeed, got {status} with {removed}"
        );
    }
}
