// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of what a `DeleteConnectPeer` takes with the peer it removes,
//! driven over HTTP through the real router.
//!
//! The rule under test: a record the store holds only as a TIE to a resource does not
//! outlive that resource, so deleting a core network Connect peer drops the association
//! that tied it to a device (`AssociateConnectPeer`) and the telemetry registration that
//! association made for it, and neither the listing of the ties nor the telemetry of the
//! global network reports a peer that is gone. Removing the tie and removing the peer are
//! therefore interchangeable ends: in either order the end state is the same one, and
//! neither record is ever left behind with nothing able to remove it.
//!
//! The table is the one `crate::connect_peer_association_lifecycle` defines — rows
//! differing in the Connect attachment (and so the edge location) the peer hangs off, in
//! whether the device sits at a site, and in how the optional link member is spelled — so
//! every shape of tie that associate can make is also one a delete has to take with it.
//!
//! Nothing reaches into `crate::state`: a tie is made through `AssociateConnectPeer`, read
//! back through `GetConnectPeerAssociations` and `GetNetworkTelemetry`, and removed only
//! through `DeleteConnectPeer` or `DisassociateConnectPeer`.

use serde_json::Value;

use crate::connect_peer_association_lifecycle::{
    associate_all, association_of, fixture, get_associations_ok, reported, Fixture, CASES,
};
use crate::connect_peer_lifecycle::CONNECT_PEER;
use crate::disassociate_connect_peer::{disassociate, telemetry_peers};
use crate::global_network_lifecycle::{error_code, Emulator, PAST_SETTLE};

/// The `ResourceType` a refusal reports for the TIE itself, as against the `connect-peer`
/// of the peer: a Connect peer association carries no id and no ARN of its own
/// (`smithy_query shape=ConnectPeerAssociation`), so the peer identifies it.
const CONNECT_PEER_ASSOCIATION: &str = "connect-peer-association";

/// `DeleteConnectPeer`.
async fn delete_connect_peer(emulator: &Emulator, peer: &str) -> (u16, Value) {
    emulator.delete(&format!("/connect-peers/{peer}")).await
}

/// `DeleteConnectPeer` that must succeed.
async fn delete_connect_peer_ok(emulator: &Emulator, case: &str, peer: &str) {
    let (status, body) = delete_connect_peer(emulator, peer).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DeleteConnectPeer expected 200, got {status} with {body}"
    );
    assert_eq!(
        body["ConnectPeer"]["ConnectPeerId"].as_str(),
        Some(peer),
        "case {case:?}: the delete must report the peer it removed, got {body}"
    );
}

/// `GetConnectPeer`.
async fn get_connect_peer(emulator: &Emulator, peer: &str) -> (u16, Value) {
    emulator.get(&format!("/connect-peers/{peer}")).await
}

/// Assert the store holds nothing that names `peer` any more: the peer itself is a
/// not-found, the ties of `fixture`'s global network are exactly `remaining`, and so are
/// the telemetry sessions it reports.
async fn assert_peer_and_its_ties_are_gone(
    emulator: &Emulator,
    case: &str,
    fixture: &Fixture,
    peer: &str,
    remaining: &[String],
) {
    let (status, body) = get_connect_peer(emulator, peer).await;
    assert_eq!(
        status, 404,
        "case {case:?}: GetConnectPeer on the deleted {peer} expected 404, got {status} with \
         {body}"
    );
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "case {case:?}: expected ResourceNotFoundException for {peer}, got {body}"
    );
    assert_eq!(
        body["ResourceType"].as_str(),
        Some(CONNECT_PEER),
        "case {case:?}: the refusal must name the peer as the resource, got {body}"
    );

    let page = get_associations_ok(emulator, case, &fixture.global_network, "").await;
    assert!(
        association_of(case, &page, peer).is_none(),
        "case {case:?}: the tie of the deleted {peer} is still reported in {page}"
    );
    assert_eq!(
        reported(case, &page),
        remaining,
        "case {case:?}: expected exactly {remaining:?} still tied after removing {peer}, got \
         {page}"
    );
    assert_eq!(
        telemetry_peers(emulator, case, &fixture.global_network).await,
        remaining,
        "case {case:?}: expected the sessions of {remaining:?} after removing {peer}"
    );

    // Narrowed to the peer the delete removed, so the listing is asked for it by name and
    // not only read whole.
    let narrowed = get_associations_ok(
        emulator,
        case,
        &fixture.global_network,
        &format!("connectPeerIds={peer}"),
    )
    .await;
    assert!(
        reported(case, &narrowed).is_empty(),
        "case {case:?}: asking for {peer} by name must report no tie, got {narrowed}"
    );
}

/// The ids of `fixture`'s peers, sorted: the order both the tie listing and the telemetry
/// listing report them in.
fn sorted_peers(fixture: &Fixture) -> Vec<String> {
    let mut peers = fixture.peers.clone();
    peers.sort();
    peers
}

#[tokio::test]
async fn deleting_an_associated_peer_drops_its_tie_and_its_telemetry_registration() {
    let emulator = Emulator::new();
    let fixture = fixture(&emulator, "delete cascade", 0).await;
    associate_all(&emulator, &fixture).await;

    // Past the settle span, so every tie is AVAILABLE and its session has come up when the
    // peer is deleted: what the delete takes with it does not depend on the clock.
    tokio::time::sleep(PAST_SETTLE).await;

    let mut remaining = sorted_peers(&fixture);
    let page = get_associations_ok(&emulator, "before any delete", &fixture.global_network, "").await;
    assert_eq!(
        reported("before any delete", &page),
        remaining,
        "case before any delete: expected one tie per associated peer, got {page}"
    );
    assert_eq!(
        telemetry_peers(&emulator, "before any delete", &fixture.global_network).await,
        remaining,
        "case before any delete: expected one session per associated peer"
    );

    for (index, case) in CASES.iter().enumerate() {
        let peer = &fixture.peers[index];
        delete_connect_peer_ok(&emulator, case.name, peer).await;
        remaining.retain(|held| held != peer);
        assert_peer_and_its_ties_are_gone(&emulator, case.name, &fixture, peer, &remaining).await;

        // The tie really went with the peer rather than being hidden by a reader: the one
        // operation that removes a tie has nothing left to remove for this peer.
        let (status, body) = disassociate(&emulator, &fixture.global_network, peer).await;
        assert_eq!(
            status, 404,
            "case {:?}: DisassociateConnectPeer after the delete expected 404, got {status} \
             with {body}",
            case.name
        );
        assert_eq!(
            body["ResourceType"].as_str(),
            Some(CONNECT_PEER_ASSOCIATION),
            "case {:?}: the refusal must name the association as the resource, got {body}",
            case.name
        );
    }

    assert!(
        remaining.is_empty(),
        "every peer of the fixture was deleted, so none is left: {remaining:?}"
    );
}

/// Which end of the tie a row removes first — the two removals are interchangeable, so the
/// end state has to be the same one either way.
#[derive(Debug)]
enum Order {
    /// Delete the peer, then try to untie it.
    DeleteFirst,
    /// Untie the peer, then delete it.
    DisassociateFirst,
}

#[tokio::test]
async fn either_order_of_the_two_removals_leaves_the_same_end_state() {
    let emulator = Emulator::new();
    let fixture = fixture(&emulator, "removal order", 0).await;
    associate_all(&emulator, &fixture).await;

    let mut remaining = sorted_peers(&fixture);

    // Both orders over rows of the table, so each order is driven on more than one shape
    // of tie (different edge, device and link spelling).
    for (index, order) in [
        (0, Order::DeleteFirst),
        (1, Order::DisassociateFirst),
        (2, Order::DeleteFirst),
        (3, Order::DisassociateFirst),
    ] {
        let case = format!("{} / {order:?}", CASES[index].name);
        let peer = &fixture.peers[index];
        match order {
            Order::DeleteFirst => {
                delete_connect_peer_ok(&emulator, &case, peer).await;
                let (status, body) = disassociate(&emulator, &fixture.global_network, peer).await;
                assert_eq!(
                    status, 404,
                    "case {case:?}: the tie went with the peer, so untying it expected 404, \
                     got {status} with {body}"
                );
            }
            Order::DisassociateFirst => {
                let (status, body) = disassociate(&emulator, &fixture.global_network, peer).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: DisassociateConnectPeer expected 200, got {status} with \
                     {body}"
                );
                delete_connect_peer_ok(&emulator, &case, peer).await;
            }
        }
        remaining.retain(|held| held != peer);
        assert_peer_and_its_ties_are_gone(&emulator, &case, &fixture, peer, &remaining).await;
    }
}

#[tokio::test]
async fn a_delete_drops_the_ties_of_the_peer_it_names_and_no_others() {
    let emulator = Emulator::new();
    let first = fixture(&emulator, "named peer only", 0).await;
    let second = fixture(&emulator, "another global network", 100).await;
    associate_all(&emulator, &first).await;
    associate_all(&emulator, &second).await;

    // The delete names no global network, so the tie it drops is the one the peer holds
    // wherever it was made: a peer of the SECOND global network here.
    let peer = second.peers[2].clone();
    delete_connect_peer_ok(&emulator, "a peer of the second global network", &peer).await;

    let mut still_tied_second = sorted_peers(&second);
    still_tied_second.retain(|held| held != &peer);
    assert_peer_and_its_ties_are_gone(
        &emulator,
        "a peer of the second global network",
        &second,
        &peer,
        &still_tied_second,
    )
    .await;

    // The other global network kept every tie and every session it held: the delete
    // reached only the records of the peer it named.
    let untouched = sorted_peers(&first);
    let page = get_associations_ok(&emulator, "the other global network", &first.global_network, "")
        .await;
    assert_eq!(
        reported("the other global network", &page),
        untouched,
        "case the other global network: expected exactly {untouched:?} still tied, got {page}"
    );
    assert_eq!(
        telemetry_peers(&emulator, "the other global network", &first.global_network).await,
        untouched,
        "case the other global network: expected the sessions of {untouched:?}"
    );
    for other in &untouched {
        let (status, body) = get_connect_peer(&emulator, other).await;
        assert_eq!(
            status, 200,
            "case the other global network: {other} must still be readable, got {status} with \
             {body}"
        );
        assert_eq!(
            body["ConnectPeer"]["ConnectPeerId"].as_str(),
            Some(other.as_str()),
            "case the other global network: expected the peer {other}, got {body}"
        );
    }
}

#[tokio::test]
async fn deleting_a_peer_that_holds_no_tie_removes_the_peer_alone() {
    let emulator = Emulator::new();
    let fixture = fixture(&emulator, "untied peer", 0).await;

    // Only the first row's peer is tied; the one deleted below holds no tie at all, so the
    // delete has nothing to take with it and must leave the tie that IS held where it was.
    let tied = fixture.peers[0].clone();
    let (status, body) = crate::connect_peer_association_lifecycle::associate(
        &emulator,
        &fixture.global_network,
        CASES[0].request(&fixture, &tied),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup AssociateConnectPeer expected 200, got {status} with {body}"
    );

    let untied = fixture.peers[1].clone();
    delete_connect_peer_ok(&emulator, "a peer that holds no tie", &untied).await;
    assert_eq!(
        reported(
            "a peer that holds no tie",
            &get_associations_ok(
                &emulator,
                "a peer that holds no tie",
                &fixture.global_network,
                ""
            )
            .await
        ),
        vec![tied.clone()],
        "case a peer that holds no tie: the tie of {tied} must be left where it was"
    );
    assert_eq!(
        telemetry_peers(
            &emulator,
            "a peer that holds no tie",
            &fixture.global_network
        )
        .await,
        vec![tied.clone()],
        "case a peer that holds no tie: the session of {tied} must be left where it was"
    );

    // And deleting the tied peer afterwards takes its tie and session with it, leaving the
    // global network reporting neither.
    delete_connect_peer_ok(&emulator, "the tied peer", &tied).await;
    assert_peer_and_its_ties_are_gone(&emulator, "the tied peer", &fixture, &tied, &[]).await;
}
