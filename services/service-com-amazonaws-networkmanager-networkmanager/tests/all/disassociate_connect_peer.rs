// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `DisassociateConnectPeer`, driven over HTTP.
//!
//! Every call goes through the real `MultiServiceRouter`, so the restJson1 route, the
//! label bindings and the error envelopes are exercised as a client hits them. Nothing
//! reaches into `crate::state`: a tie is made through `AssociateConnectPeer`, read back
//! through `GetConnectPeerAssociations` and `GetNetworkTelemetry`, and only ever removed
//! through `DisassociateConnectPeer`.
//!
//! The rule under test: the operation removes the stored tie the addressed global network
//! holds for the named Connect peer — the peer IS the association's identity — reports the
//! association it removed as `DELETING` with the device and link it was made over,
//! deregisters that peering from the global network's telemetry, and leaves the peer free
//! to be associated again; a global network the store does not hold, and a peer that
//! global network holds no association for (never associated, already disassociated, or
//! associated in ANOTHER global network), are both refused as not found and remove
//! nothing.
//!
//! The table is the one `connect_peer_association_lifecycle` defines — rows differing in
//! the Connect attachment (and so the edge location) the peer hangs off, in whether the
//! device sits at a site, and in how the optional link member is spelled — so every shape
//! of tie that associate can make is also removed here.

use crate::connect_peer_association_lifecycle::{
    assert_association, associate, associate_all, association_of, fixture, get_associations_ok,
    reported, CASES,
};
use crate::global_network_lifecycle::{error_code, Emulator, PAST_SETTLE};
use crate::resource_not_found_message::{assert_not_found_answer, GLOBAL_NETWORK};

/// The `ResourceType` a refusal reports for the ASSOCIATION itself, as against the
/// `connect-peer` of the peer: a Connect peer association carries no id and no ARN of its
/// own (`smithy_query shape=ConnectPeerAssociation`), so the peer identifies it and this
/// is how the refusal spells its type.
const CONNECT_PEER_ASSOCIATION: &str = "connect-peer-association";

/// An id of the minted shape (`connect-peer-<17 lowercase hex>`, `smithy_query
/// shape=ConnectPeerId`) that names nothing stored: the leading `f` puts it above anything
/// the monotonic id counter can mint.
const UNKNOWN_PEER: &str = "connect-peer-f0000000000000001";

/// A global-network id of the minted shape that names nothing stored.
const UNKNOWN_NETWORK: &str = "global-network-f0000000000000001";

/// The URI `DisassociateConnectPeer` is addressed through.
fn uri(network: &str, peer: &str) -> String {
    format!("/global-networks/{network}/connect-peer-associations/{peer}")
}

/// `DisassociateConnectPeer` on `network` for `peer`.
async fn disassociate(
    emulator: &Emulator,
    network: &str,
    peer: &str,
) -> (u16, serde_json::Value) {
    emulator.delete(&uri(network, peer)).await
}

/// `DisassociateConnectPeer` that must succeed, answering the association it reported.
async fn disassociate_ok(
    emulator: &Emulator,
    case: &str,
    network: &str,
    peer: &str,
) -> serde_json::Value {
    let (status, body) = disassociate(emulator, network, peer).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DisassociateConnectPeer expected 200, got {status} with {body}"
    );
    body["ConnectPeerAssociation"].clone()
}

/// The `ResourceId`s one telemetry page reports for the Connect peers of `network`.
async fn telemetry_peers(emulator: &Emulator, case: &str, network: &str) -> Vec<String> {
    let (status, body) = emulator
        .get(&format!(
            "/global-networks/{network}/network-telemetry?resourceType=connect-peer"
        ))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetNetworkTelemetry expected 200, got {status} with {body}"
    );
    let mut ids: Vec<String> = body["NetworkTelemetry"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no NetworkTelemetry array in {body}"))
        .iter()
        .map(|entry| {
            entry["ResourceId"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: no ResourceId in {entry}"))
                .to_string()
        })
        .collect();
    ids.sort();
    ids
}

#[tokio::test]
async fn every_tie_the_table_makes_is_removed_and_reported_deleting() {
    let emulator = Emulator::new();
    let fixture = fixture(&emulator, "disassociate", 0).await;
    associate_all(&emulator, &fixture).await;

    // Past the settle span, so every association is AVAILABLE when it is removed: the
    // state the removal reports is `DELETING` whatever state the clock computed for it.
    tokio::time::sleep(PAST_SETTLE).await;

    let mut remaining: Vec<String> = fixture.peers.clone();
    remaining.sort();

    for (index, case) in CASES.iter().enumerate() {
        let peer = &fixture.peers[index];
        let removed = disassociate_ok(&emulator, case.name, &fixture.global_network, peer).await;
        assert_association(case, &fixture, peer, &removed, "DELETING");

        // The removed tie is gone from the listing and every tie not named is still held,
        // in the stable peer-id order the store keys them by.
        remaining.retain(|held| held != peer);
        let page = get_associations_ok(&emulator, case.name, &fixture.global_network, "").await;
        assert_eq!(
            reported(case.name, &page),
            remaining,
            "case {:?}: expected exactly {remaining:?} still associated, got {page}",
            case.name
        );
        assert!(
            association_of(case.name, &page, peer).is_none(),
            "case {:?}: peer {peer} is still reported in {page}",
            case.name
        );
    }
}

#[tokio::test]
async fn a_removed_tie_frees_its_peer_to_be_associated_with_another_device() {
    let emulator = Emulator::new();
    let fixture = fixture(&emulator, "re-association", 0).await;
    associate_all(&emulator, &fixture).await;

    // The first row ties its peer to the device that sits at a site, over that device's
    // link; the row below re-ties the same peer to the device that sits at no site.
    let row = &CASES[0];
    let peer = fixture.peers[0].clone();
    let rebound = &CASES[2];
    assert_ne!(
        rebound.device(&fixture),
        row.device(&fixture),
        "case re-association: the two rows must name different devices"
    );

    disassociate_ok(&emulator, "re-association", &fixture.global_network, &peer).await;

    // With the tie gone the peer is free: the request that conflicted while it was held
    // now makes a new association, which the listing reports over the NEW device.
    let (status, body) = associate(
        &emulator,
        &fixture.global_network,
        rebound.request(&fixture, &peer),
    )
    .await;
    assert_eq!(
        status, 200,
        "case re-association: AssociateConnectPeer expected 200, got {status} with {body}"
    );
    assert_association(
        rebound,
        &fixture,
        &peer,
        &body["ConnectPeerAssociation"],
        "PENDING",
    );

    let page = get_associations_ok(
        &emulator,
        "re-association",
        &fixture.global_network,
        &format!("connectPeerIds={peer}"),
    )
    .await;
    let held = association_of("re-association", &page, &peer)
        .unwrap_or_else(|| panic!("case re-association: peer {peer} is not reported in {page}"));
    assert_association(rebound, &fixture, &peer, &held, "PENDING");
}

#[tokio::test]
async fn removing_a_tie_deregisters_only_that_peering_from_the_global_networks_telemetry() {
    let emulator = Emulator::new();
    let fixture = fixture(&emulator, "telemetry", 0).await;
    associate_all(&emulator, &fixture).await;

    let mut expected: Vec<String> = fixture.peers.clone();
    expected.sort();
    assert_eq!(
        telemetry_peers(&emulator, "before any removal", &fixture.global_network).await,
        expected,
        "case before any removal: expected one session per associated peer"
    );

    for (index, case) in CASES.iter().enumerate() {
        let peer = &fixture.peers[index];
        disassociate_ok(&emulator, case.name, &fixture.global_network, peer).await;
        expected.retain(|held| held != peer);
        assert_eq!(
            telemetry_peers(&emulator, case.name, &fixture.global_network).await,
            expected,
            "case {:?}: expected the sessions of {expected:?} after removing {peer}",
            case.name
        );
    }
}

#[tokio::test]
async fn a_disassociate_the_store_cannot_serve_is_refused_and_removes_nothing() {
    let emulator = Emulator::new();
    let first = fixture(&emulator, "refusals", 0).await;
    let second = fixture(&emulator, "another global network", 100).await;
    associate_all(&emulator, &first).await;

    // One tie in the OTHER global network, so a row can address an association that
    // exists but not through the global network the path names.
    let elsewhere = &CASES[0];
    let elsewhere_peer = second.peers[0].clone();
    let (status, body) = associate(
        &emulator,
        &second.global_network,
        elsewhere.request(&second, &elsewhere_peer),
    )
    .await;
    assert_eq!(
        status, 200,
        "case the other global network: AssociateConnectPeer expected 200, got {status} with {body}"
    );

    // One tie of `first` removed up front, so a row can address a tie already removed.
    let gone_peer = first.peers[1].clone();
    disassociate_ok(&emulator, "removed up front", &first.global_network, &gone_peer).await;

    // Each row: the global network and peer the request addresses, and the resource the
    // refusal has to identify.
    let rows: Vec<(&str, String, String, &str)> = vec![
        (
            "an unknown global network",
            UNKNOWN_NETWORK.to_string(),
            first.peers[0].clone(),
            GLOBAL_NETWORK,
        ),
        (
            "a peer that names nothing stored",
            first.global_network.clone(),
            UNKNOWN_PEER.to_string(),
            CONNECT_PEER_ASSOCIATION,
        ),
        (
            "a tie already removed",
            first.global_network.clone(),
            gone_peer.clone(),
            CONNECT_PEER_ASSOCIATION,
        ),
        (
            "a stored peer that was never associated",
            second.global_network.clone(),
            second.peers[2].clone(),
            CONNECT_PEER_ASSOCIATION,
        ),
        (
            "a tie made in another global network",
            first.global_network.clone(),
            elsewhere_peer.clone(),
            CONNECT_PEER_ASSOCIATION,
        ),
    ];

    for (case, network, peer, resource_type) in rows {
        // The refusal carries the constant message plus the identifying detail, asserted
        // the one place this service's not-found rule lives.
        let resource_id = if resource_type == GLOBAL_NETWORK {
            network.clone()
        } else {
            peer.clone()
        };
        let answered = assert_not_found_answer(
            &emulator,
            case,
            "DELETE",
            &uri(&network, &peer),
            None,
            &resource_id,
            resource_type,
        )
        .await;
        assert!(
            answered["ConnectPeerAssociation"].is_null(),
            "case {case:?}: a refused request must report no association, got {answered}"
        );
        assert_eq!(
            error_code(&answered),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {answered}"
        );
    }

    // Nothing any refusal addressed was removed: the ties still held are every one of
    // `first` but the peer removed up front, and the one tie of the other global network.
    let mut still_held: Vec<String> = first
        .peers
        .iter()
        .filter(|peer| *peer != &gone_peer)
        .cloned()
        .collect();
    still_held.sort();
    let page = get_associations_ok(&emulator, "after the refusals", &first.global_network, "").await;
    assert_eq!(
        reported("after the refusals", &page),
        still_held,
        "case after the refusals: expected exactly {still_held:?}, got {page}"
    );
    let page =
        get_associations_ok(&emulator, "the other global network", &second.global_network, "").await;
    assert_eq!(
        reported("the other global network", &page),
        vec![elsewhere_peer.clone()],
        "case the other global network: expected its one tie, got {page}"
    );
}
