// SPDX-License-Identifier: CC0-1.0

//! The pair of tunnel addresses a `CreateConnectPeer` carries, driven end to end over HTTP
//! through the real router (the harness lives in `crate::global_network_lifecycle`, the
//! Connect attachments come from `crate::connect_peer_lifecycle::subject`).
//!
//! The rule under test: the two ends of one BGP session are IP addresses of ONE address
//! family, so a create whose appliance end and core-network end are in different families
//! describes a peering that cannot exist and is refused with the modelled
//! `ValidationException` (HTTP 400) naming the member at fault, storing nothing; a member
//! whose value is no IP address at all is refused the same way, since a value with no
//! family cannot be in the peer's.
//!
//! The tables below span that class: the accepted rows differ in family (IPv4, IPv6), in the
//! textual form each address is written in (compressed and expanded IPv6, upper and lower
//! case) and in whether the core-network end is named at all, and are read back through
//! `GetConnectPeer`; the refused rows differ in which family each end is in, in which
//! direction the pair straddles the two, and in how a value fails to be an address at all
//! (a name, a prefix, a truncated dotted quad, an address with a port). Nothing reaches into
//! `crate::state`: every peer is created, listed and read back through an operation.

use serde_json::{json, Value};

use crate::connect_peer_lifecycle::{create_connect_peer, subject, Subject};
use crate::global_network_lifecycle::{error_code, ids_of, Emulator};

/// The `Reason` this service's `ValidationException` carries when one input member's VALUE
/// is at fault, whatever is wrong with it (`smithy_query shape=ValidationExceptionReason`).
const REASON_FIELD: &str = "FieldValidationFailed";

/// The two members a refusal here can name, as this service spells its Connect peer members
/// in one.
const PEER_ADDRESS: &str = "peerAddress";
/// See [`PEER_ADDRESS`].
const CORE_NETWORK_ADDRESS: &str = "coreNetworkAddress";

/// One row: the pair of addresses a create asks to peer between.
struct Pair {
    /// Names the case in every assertion message.
    name: &'static str,
    /// Which entry of `crate::connect_peer_lifecycle::TRANSPORTS` carries the peer. Both
    /// rows' attachments are GRE ones, the protocol a core network address applies to ("This
    /// only applies only when the protocol is GRE", `smithy_query query="doc"
    /// shape="CreateConnectPeerRequest" member="CoreNetworkAddress"`).
    transport: usize,
    /// The appliance's end, which every peering requires.
    peer_address: &'static str,
    /// The core network's end, or `None` for a request naming none.
    core_network_address: Option<&'static str>,
}

/// The pairs that MUST be accepted: one family on both ends, however each end is written,
/// plus the rows naming no core-network end at all, which carry no second family.
const ACCEPTED: [Pair; 6] = [
    Pair {
        name: "both ends IPv4",
        transport: 0,
        peer_address: "203.0.113.1",
        core_network_address: Some("203.0.113.2"),
    },
    Pair {
        name: "both ends IPv6",
        transport: 1,
        peer_address: "2001:db8::1",
        core_network_address: Some("2001:db8::2"),
    },
    Pair {
        name: "both ends IPv6, one compressed and one expanded",
        transport: 0,
        peer_address: "2001:db8:0:0:0:0:0:3",
        core_network_address: Some("2001:db8::4"),
    },
    Pair {
        name: "both ends IPv6, written in upper case",
        transport: 1,
        peer_address: "2001:DB8::5",
        core_network_address: Some("2001:DB8::6"),
    },
    Pair {
        name: "an IPv4 appliance end and no core network end",
        transport: 0,
        peer_address: "198.51.100.7",
        core_network_address: None,
    },
    Pair {
        name: "an IPv6 appliance end and no core network end",
        transport: 1,
        peer_address: "2001:db8::7",
        core_network_address: None,
    },
];

/// The pairs that MUST be refused, each with the member the refusal has to name: a pair
/// straddling the two families in either direction, and a value that is no IP address at
/// all — a name, a prefix, a truncated dotted quad, an address carrying a port.
const REFUSED: [(Pair, &str); 8] = [
    (
        Pair {
            name: "an IPv6 appliance end and an IPv4 core network end",
            transport: 0,
            peer_address: "2001:db8::1",
            core_network_address: Some("203.0.113.2"),
        },
        CORE_NETWORK_ADDRESS,
    ),
    (
        Pair {
            name: "an IPv4 appliance end and an IPv6 core network end",
            transport: 1,
            peer_address: "203.0.113.3",
            core_network_address: Some("2001:db8::4"),
        },
        CORE_NETWORK_ADDRESS,
    ),
    (
        Pair {
            name: "an expanded IPv6 appliance end and an IPv4 core network end",
            transport: 0,
            peer_address: "2001:db8:0:0:0:0:0:5",
            core_network_address: Some("198.51.100.6"),
        },
        CORE_NETWORK_ADDRESS,
    ),
    (
        Pair {
            name: "an appliance end that is a name, not an address",
            transport: 0,
            peer_address: "appliance.example.com",
            core_network_address: Some("203.0.113.8"),
        },
        PEER_ADDRESS,
    ),
    (
        Pair {
            name: "an appliance end carrying a prefix length",
            transport: 1,
            peer_address: "203.0.113.0/29",
            core_network_address: None,
        },
        PEER_ADDRESS,
    ),
    (
        Pair {
            name: "an appliance end carrying a port",
            transport: 0,
            peer_address: "203.0.113.9:179",
            core_network_address: None,
        },
        PEER_ADDRESS,
    ),
    (
        Pair {
            name: "a truncated dotted quad as the core network end",
            transport: 1,
            peer_address: "203.0.113.10",
            core_network_address: Some("198.51.100"),
        },
        CORE_NETWORK_ADDRESS,
    ),
    (
        Pair {
            name: "a core network end that is a prefix",
            transport: 0,
            peer_address: "2001:db8::11",
            core_network_address: Some("fd00::/125"),
        },
        CORE_NETWORK_ADDRESS,
    ),
];

/// The `CreateConnectPeer` body one row asks for on `attachment`.
fn request_of(pair: &Pair, attachment: &str) -> Value {
    let mut body = json!({
        "ConnectAttachmentId": attachment,
        "PeerAddress": pair.peer_address,
    });
    if let Some(address) = pair.core_network_address {
        body.as_object_mut()
            .unwrap_or_else(|| panic!("case {:?}: the request body must be an object", pair.name))
            .insert("CoreNetworkAddress".to_string(), json!(address));
    }
    body
}

/// The peer ids `ListConnectPeers` reports for `attachment` — that attachment's stored
/// inventory, read through an operation and unpaged, so it carries every selected peer.
async fn stored_ids(emulator: &Emulator, attachment: &str) -> Vec<String> {
    let (status, body) = emulator
        .get(&format!("/connect-peers?connectAttachmentId={attachment}"))
        .await;
    assert_eq!(
        status, 200,
        "ListConnectPeers expected 200, got {status} with {body}"
    );
    ids_of(&body, "ConnectPeers", "ConnectPeerId")
}

/// The `ConnectPeer` `GetConnectPeer` answers for `peer`.
async fn read_back(emulator: &Emulator, case: &str, peer: &str) -> Value {
    let (status, body) = emulator.get(&format!("/connect-peers/{peer}")).await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetConnectPeer expected 200, got {status} with {body}"
    );
    body["ConnectPeer"].clone()
}

/// Build the subject every test here hangs its peers off.
async fn built(emulator: &Emulator, name: &str) -> Subject {
    subject(emulator, name, 0).await
}

#[tokio::test]
async fn a_peering_between_two_addresses_of_one_family_is_stored_as_asked() {
    let emulator = Emulator::new();
    let subject = built(&emulator, "same family pairs").await;

    for pair in &ACCEPTED {
        let case = pair.name;
        let attachment = subject.attachments[pair.transport].clone();
        let (status, created) =
            create_connect_peer(&emulator, request_of(pair, &attachment)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateConnectPeer expected 200, got {status} with {created}"
        );
        let id = created["ConnectPeer"]["ConnectPeerId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no ConnectPeer.ConnectPeerId in {created}"))
            .to_string();

        // The pair is stored as the request wrote it, not normalised: the peer reports back
        // the two addresses it was asked to peer between.
        let stored = read_back(&emulator, case, &id).await;
        let configuration = &stored["Configuration"];
        assert_eq!(
            configuration["PeerAddress"].as_str(),
            Some(pair.peer_address),
            "case {case:?}: expected the appliance end {:?}, got {configuration}",
            pair.peer_address
        );
        assert_eq!(
            configuration["CoreNetworkAddress"].as_str(),
            pair.core_network_address,
            "case {case:?}: expected the core network end {:?}, got {configuration}",
            pair.core_network_address
        );
        assert!(
            stored_ids(&emulator, &attachment).await.contains(&id),
            "case {case:?}: ListConnectPeers must report the accepted peer {id}"
        );
    }
}

#[tokio::test]
async fn a_pair_that_is_not_two_addresses_of_one_family_is_refused_and_stores_nothing() {
    let emulator = Emulator::new();
    let subject = built(&emulator, "mixed family pairs").await;

    // Whatever each row is refused for, no peer may be stored for it: the inventory of every
    // attachment under test stays the one it started with.
    let mut before: Vec<(String, Vec<String>)> = Vec::new();
    for attachment in &subject.attachments {
        before.push((
            attachment.clone(),
            stored_ids(&emulator, attachment).await,
        ));
    }

    for (pair, blamed) in &REFUSED {
        let case = pair.name;
        let attachment = subject.attachments[pair.transport].clone();
        let (status, refusal) =
            create_connect_peer(&emulator, request_of(pair, &attachment)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: CreateConnectPeer expected 400, got {status} with {refusal}"
        );
        assert_eq!(
            error_code(&refusal),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {refusal}"
        );
        assert_eq!(
            refusal["Reason"].as_str(),
            Some(REASON_FIELD),
            "case {case:?}: expected the {REASON_FIELD} reason, got {refusal}"
        );
        assert_eq!(
            refusal["Fields"][0]["Name"].as_str(),
            Some(*blamed),
            "case {case:?}: the refusal must name {blamed}, got {refusal}"
        );
        assert!(
            refusal["Fields"][0]["Message"]
                .as_str()
                .is_some_and(|message| !message.is_empty()),
            "case {case:?}: the named field must explain itself, got {refusal}"
        );
    }

    for (attachment, started_with) in &before {
        let now = stored_ids(&emulator, attachment).await;
        assert_eq!(
            &now, started_with,
            "a refused create stored a peer on {attachment}: expected {started_with:?}, got {now:?}"
        );
    }
}
