// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `DeleteConnectPeer`, driven over HTTP through the real router
//! (the harness and the peer table live in `crate::connect_peer_lifecycle`, so this suite
//! and the create/read/list suite share ONE definition of what a Connect peer can be).
//!
//! The rule under test: a delete of a core network Connect peer REMOVES the one stored
//! record the id names and reports exactly that record in `DELETING`, so every other
//! reader of the store — the peer's own reader, the listing, the network-resource
//! inventory and its tags — stops reporting it while every peer the delete did not name is
//! left where it was; an id naming no stored peer, never created or already deleted, is
//! refused with `ResourceNotFoundException` (HTTP 404).
//!
//! The rows are `crate::connect_peer_lifecycle::PEER_CASES`, which span the class: peers
//! differing in the tunnel protocol and edge location of the attachment they are carried
//! over (GRE and NO_ENCAP, three Regions), in address family (IPv4, IPv6), in how many
//! inside CIDR blocks they name (none, one, two), in peer ASN (absent, and each bound of
//! the AS number domain), in whether a subnet is named, and in how many tags they carry.
//! Nothing reaches into `crate::state`: every peer is created, read back and deleted
//! through an operation.

use serde_json::{json, Value};

use crate::connect_peer_lifecycle::{
    case_subnet, create_connect_peer, request_of, subject, PeerCase, Subject, CONNECT_PEER,
    PEER_CASES,
};
use crate::global_network_lifecycle::{error_code, expected_arn, is_minted_id, Emulator};
use crate::resource_tagging::list_tags;

/// `GetConnectPeer`, status and body as answered — a deleted peer is a refusal, so this
/// suite cannot use the asserting reader the create/read suite shares.
async fn get_connect_peer(emulator: &Emulator, id: &str) -> (u16, Value) {
    emulator.get(&format!("/connect-peers/{id}")).await
}

/// `DeleteConnectPeer`.
async fn delete_connect_peer(emulator: &Emulator, id: &str) -> (u16, Value) {
    emulator.delete(&format!("/connect-peers/{id}")).await
}

/// The `ConnectPeerId` of a create or delete answer, or a panic naming what was there
/// instead.
fn connect_peer_id(case: &str, wrapper: &Value) -> String {
    wrapper["ConnectPeer"]["ConnectPeerId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no ConnectPeer.ConnectPeerId in {wrapper}"))
        .to_string()
}

/// The `ConnectPeerId`s `ListConnectPeers` reports for `core_network`, in the order
/// returned.
async fn listed(emulator: &Emulator, core_network: &str) -> Vec<String> {
    let (status, body) = emulator
        .get(&format!("/connect-peers?coreNetworkId={core_network}"))
        .await;
    assert_eq!(
        status, 200,
        "ListConnectPeers expected 200, got {status} with {body}"
    );
    body["ConnectPeers"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a ConnectPeers array in {body}"))
        .iter()
        .map(|summary| {
            summary["ConnectPeerId"]
                .as_str()
                .unwrap_or_else(|| panic!("no ConnectPeerId in {summary}"))
                .to_string()
        })
        .collect()
}

/// The `Count` `GetNetworkResourceCounts` reports for the Connect peers of `network`.
async fn counted(emulator: &Emulator, network: &str) -> i64 {
    let (status, body) = emulator
        .get(&format!(
            "/global-networks/{network}/network-resource-count?resourceType=connect-peer"
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

/// Create the peer of `case` on `subject`, asserting the create succeeded, and answer its
/// id with the `ConnectPeer` the create reported — the record the delete has to report
/// back, in `DELETING` rather than the state the create reported.
async fn create(emulator: &Emulator, subject: &Subject, case: &PeerCase) -> (String, Value) {
    let name = case.name;
    let attachment = &subject.attachments[case.transport];
    let (status, body) = create_connect_peer(emulator, request_of(case, attachment)).await;
    assert_eq!(
        status, 200,
        "case {name:?}: CreateConnectPeer expected 200, got {status} with {body}"
    );
    let id = connect_peer_id(name, &body);
    assert!(
        is_minted_id(&id, CONNECT_PEER),
        "case {name:?}: the id is minted by the service, got {id:?}"
    );
    (id, body["ConnectPeer"].clone())
}

/// Assert `body` is the modelled not-found for the Connect peer `id`: HTTP 404 with the
/// error code, the resource type and the id the request asked for.
fn assert_not_found(case: &str, operation: &str, status: u16, body: &Value, id: &str) {
    assert_eq!(
        status, 404,
        "case {case:?}: {operation} on an id naming no stored peer expected 404, got {status} \
         with {body}"
    );
    assert_eq!(
        error_code(body),
        "ResourceNotFoundException",
        "case {case:?}: {operation} expected ResourceNotFoundException, got {body}"
    );
    assert_eq!(
        body["ResourceType"],
        json!(CONNECT_PEER),
        "case {case:?}: {operation} must name the resource type, got {body}"
    );
    assert_eq!(
        body["ResourceId"],
        json!(id),
        "case {case:?}: {operation} must name the id asked for, got {body}"
    );
}

#[tokio::test]
async fn a_delete_removes_the_peer_it_names_reports_it_deleting_and_leaves_the_others() {
    let emulator = Emulator::new();
    let subject = subject(&emulator, "peer deletes", 0).await;

    let mut created: Vec<(&PeerCase, String, Value)> = Vec::new();
    for case in PEER_CASES.iter() {
        let (id, at_create) = create(&emulator, &subject, case).await;
        created.push((case, id, at_create));
    }

    assert_eq!(
        counted(&emulator, &subject.global_network).await,
        created.len() as i64,
        "the inventory must count every stored peer before any delete"
    );

    for (position, (case, id, at_create)) in created.iter().enumerate() {
        let name = case.name;
        let arn = expected_arn(CONNECT_PEER, id);

        let (status, deleted) = delete_connect_peer(&emulator, id).await;
        assert_eq!(
            status, 200,
            "case {name:?}: DeleteConnectPeer expected 200, got {status} with {deleted}"
        );
        // The delete reports the record it removed: the very peer the create stored, with
        // the state of one on its way out rather than the state the clock would compute.
        let mut expected = at_create.clone();
        expected["State"] = json!("DELETING");
        assert_eq!(
            deleted["ConnectPeer"], expected,
            "case {name:?}: the delete must report the stored peer in DELETING, got {deleted}"
        );
        // Named explicitly because a peer's tags are what the delete takes with it.
        assert_eq!(
            deleted["ConnectPeer"]["Tags"],
            at_create["Tags"],
            "case {name:?}: the delete must report the tags the peer held, got {deleted}"
        );
        // And the subnet the create named, where it named one: the removed record is
        // reported in full rather than as a shell carrying only an id.
        if case.subnet {
            assert_eq!(
                deleted["ConnectPeer"]["SubnetArn"],
                json!(case_subnet(case)),
                "case {name:?}: the delete must report the subnet the peer reached its \
                 appliance through, got {deleted}"
            );
        }

        // The record is really gone, not re-labelled: the peer's own reader, a second
        // delete and its tags all answer not-found afterwards.
        let (status, body) = get_connect_peer(&emulator, id).await;
        assert_not_found(name, "GetConnectPeer", status, &body, id);
        let (status, body) = delete_connect_peer(&emulator, id).await;
        assert_not_found(name, "a second DeleteConnectPeer", status, &body, id);
        let (status, body) = list_tags(&emulator, &arn).await;
        assert_eq!(
            status, 404,
            "case {name:?}: ListTagsForResource on a deleted peer expected 404, got {status} \
             with {body}"
        );

        // Every peer the delete did not name is where it was: readable, listed and counted.
        let remaining: Vec<String> = created
            .iter()
            .skip(position + 1)
            .map(|(_, id, _)| id.clone())
            .collect();
        for other_id in &remaining {
            let (status, body) = get_connect_peer(&emulator, other_id).await;
            assert_eq!(
                status, 200,
                "case {name:?}: deleting it must leave {other_id} readable, got {status} with \
                 {body}"
            );
        }
        let mut expected_ids = remaining.clone();
        expected_ids.sort();
        assert_eq!(
            listed(&emulator, &subject.core_network).await,
            expected_ids,
            "case {name:?}: the listing must drop exactly the deleted peer"
        );
        assert_eq!(
            counted(&emulator, &subject.global_network).await,
            remaining.len() as i64,
            "case {name:?}: the inventory must count only the peers still stored"
        );
    }
}

#[tokio::test]
async fn a_delete_of_an_id_naming_no_stored_peer_is_refused_and_removes_nothing() {
    let emulator = Emulator::new();
    let subject = subject(&emulator, "peer delete refusals", 0).await;

    // One stored peer, so a refused delete has something it could have removed.
    let (stored, _) = create(&emulator, &subject, &PEER_CASES[0]).await;

    // And one that WAS stored and is already gone, so the rows cover both ways an id can
    // name nothing: never minted, and minted and then deleted.
    let (gone, _) = create(&emulator, &subject, &PEER_CASES[1]).await;
    let (status, body) = delete_connect_peer(&emulator, &gone).await;
    assert_eq!(
        status, 200,
        "setup DeleteConnectPeer expected 200, got {status} with {body}"
    );

    // Rows differing in WHY the id names nothing: never minted (at each end of the id
    // width the model's `^connect-peer-([0-9a-f]{8,17})$` allows), and already deleted.
    for (row, id) in [
        (
            "an id that was never minted",
            "connect-peer-0000000000000ffff",
        ),
        ("the shortest id the pattern allows", "connect-peer-0000000a"),
        ("an id that was deleted already", gone.as_str()),
    ] {
        let (status, body) = delete_connect_peer(&emulator, id).await;
        assert_not_found(row, "DeleteConnectPeer", status, &body, id);
    }

    // Nothing the refusals named was removed, and the peer that IS stored is untouched.
    let (status, body) = get_connect_peer(&emulator, &stored).await;
    assert_eq!(
        status, 200,
        "a refused delete must leave the stored peer readable, got {status} with {body}"
    );
    assert_eq!(
        listed(&emulator, &subject.core_network).await,
        vec![stored.clone()],
        "a refused delete must leave the listing as it was"
    );
    assert_eq!(
        counted(&emulator, &subject.global_network).await,
        1,
        "a refused delete must leave the inventory as it was"
    );
}

#[tokio::test]
async fn a_peer_can_be_deleted_before_the_transition_the_clock_owes_falls_due() {
    let emulator = Emulator::new();
    let subject = subject(&emulator, "peer delete while creating", 0).await;

    // Deleted straight after the create, while the peer is still CREATING: the delete is
    // about what is STORED, so it does not wait for the state the clock owes. One row of
    // each tunnel protocol, since the protocol is what the peer takes from its attachment.
    for case in [&PEER_CASES[0], &PEER_CASES[4]] {
        let name = case.name;
        let (id, at_create) = create(&emulator, &subject, case).await;
        assert_eq!(
            at_create["State"],
            json!("CREATING"),
            "case {name:?}: a created peer waits in CREATING, got {at_create}"
        );

        let (status, deleted) = delete_connect_peer(&emulator, &id).await;
        assert_eq!(
            status, 200,
            "case {name:?}: DeleteConnectPeer on a CREATING peer expected 200, got {status} \
             with {deleted}"
        );
        assert_eq!(
            deleted["ConnectPeer"]["State"],
            json!("DELETING"),
            "case {name:?}: the removed record is reported in DELETING, got {deleted}"
        );
        let (status, body) = get_connect_peer(&emulator, &id).await;
        assert_not_found(name, "GetConnectPeer", status, &body, &id);
    }

    assert_eq!(
        counted(&emulator, &subject.global_network).await,
        0,
        "every peer created here was deleted, so the inventory counts none"
    );
    assert!(
        listed(&emulator, &subject.core_network).await.is_empty(),
        "every peer created here was deleted, so the listing reports none"
    );
}
