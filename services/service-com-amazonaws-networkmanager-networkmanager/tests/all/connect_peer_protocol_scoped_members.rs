// SPDX-License-Identifier: CC0-1.0

//! The members of a `CreateConnectPeer` the model scopes to ONE tunnel protocol, driven end
//! to end over HTTP through the real router (the harness lives in
//! `crate::global_network_lifecycle`).
//!
//! The rule under test: a Connect peer carries only the configuration its tunnel protocol —
//! the protocol of the Connect attachment it is carried over — gives meaning to, so a
//! request member the model scopes to a single protocol is accepted only on an attachment
//! running that protocol, and naming it against the other protocol is refused with the
//! modelled `ValidationException` (HTTP 400) naming that member, storing nothing. The two
//! scoped members are `CoreNetworkAddress` ("This only applies only when the protocol is
//! GRE") and `SubnetArn` ("...only when the protocol is NO_ENCAP"): `smithy_query
//! query="doc" shape="CreateConnectPeerRequest" member="CoreNetworkAddress"`,
//! `member="SubnetArn"`.
//!
//! The table below spans that class: rows differing in the attachment's protocol (GRE,
//! NO_ENCAP, and an attachment whose options name none, which excludes nothing), in which
//! scoped member the request names (each, both, neither, and the `SubnetArn` spelling that
//! names no subnet at all), in the edge location the attachment sits at, and in the address
//! family of the two tunnel ends — so a row refused here is refused for the protocol scope
//! and not for the subnet's Region or the pair's address family, which the rows satisfy.
//! Every accepted peer is read back through `GetConnectPeer` and `ListConnectPeers`, and
//! every refused row is shown to have stored nothing. Nothing reaches into `crate::state`:
//! every peer is created, listed and read back through an operation.

use serde_json::{json, Value};

use crate::attachment_lifecycle::{
    attachment_id, core_network_at, create_connect_attachment, create_vpc_attachment, subnet_arn,
    vpc_arn,
};
use crate::connect_peer_lifecycle::create_connect_peer;
use crate::global_network_lifecycle::{error_code, ids_of, is_minted_id, Emulator};

/// The `Reason` a refusal of a request that parses but cannot be served against stored
/// state carries (`smithy_query shape=ValidationExceptionReason`).
const REASON_OTHER: &str = "Other";

/// The two members a refusal here can name, as this service spells its Connect peer members
/// in one.
const CORE_NETWORK_ADDRESS: &str = "coreNetworkAddress";
/// See [`CORE_NETWORK_ADDRESS`].
const SUBNET_ARN: &str = "subnetArn";

/// The edge locations the subject configures, one Connect attachment of each protocol at
/// each.
const EDGES: [&str; 2] = ["us-west-2", "eu-central-1"];

/// The protocols the subject's Connect attachments run: the two the model enumerates
/// (`smithy_query shape=TunnelProtocol`) and an attachment whose `Options` name none, which
/// `CreateConnectAttachment` accepts since `ConnectAttachmentOptions$Protocol` is optional
/// (`smithy_query shape=ConnectAttachmentOptions`).
const PROTOCOLS: [Option<&str>; 3] = [Some("GRE"), Some("NO_ENCAP"), None];

/// The Connect attachments every row hangs a peer off: one per edge location per protocol,
/// keyed by `(edge index, protocol index)`.
struct Subject {
    attachments: Vec<Vec<String>>,
}

impl Subject {
    /// The attachment at `edge` running `protocol`.
    fn attachment(&self, edge: usize, protocol: usize) -> &str {
        &self.attachments[edge][protocol]
    }
}

/// Build the subject: a core network with an edge in each of [`EDGES`], one VPC attachment
/// per edge as the transport, and one Connect attachment per protocol over each transport.
async fn subject(emulator: &Emulator) -> Subject {
    let (_, core_network) = core_network_at(emulator, "protocol scoped members", &EDGES).await;
    let mut attachments = Vec::new();
    for (index, edge) in EDGES.iter().enumerate() {
        let (status, created) = create_vpc_attachment(
            emulator,
            json!({
                "CoreNetworkId": core_network,
                "VpcArn": vpc_arn(edge, index),
                "SubnetArns": [subnet_arn(edge, index)],
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {edge:?}: CreateVpcAttachment expected 200, got {status} with {created}"
        );
        let transport = attachment_id(edge, &created["VpcAttachment"]);

        let mut tunnels = Vec::new();
        for protocol in PROTOCOLS {
            let mut request = json!({
                "CoreNetworkId": core_network,
                "EdgeLocation": edge,
                "TransportAttachmentId": transport,
                "Options": {},
            });
            if let Some(protocol) = protocol {
                request["Options"] = json!({ "Protocol": protocol });
            }
            let (status, created) = create_connect_attachment(emulator, request).await;
            assert_eq!(
                status, 200,
                "case {edge:?}/{protocol:?}: CreateConnectAttachment expected 200, got {status} with {created}"
            );
            tunnels.push(attachment_id(edge, &created["ConnectAttachment"]));
        }
        attachments.push(tunnels);
    }
    Subject { attachments }
}

/// One row of the table: the peer one `CreateConnectPeer` asks for, and what the scoped
/// members it names make of it.
struct Case {
    /// Names the case in every assertion message.
    name: &'static str,
    /// Which entry of [`EDGES`] the attachment sits at.
    edge: usize,
    /// Which entry of [`PROTOCOLS`] the attachment runs.
    protocol: usize,
    /// The appliance's end, which every peering requires whatever its protocol.
    peer_address: &'static str,
    /// The GRE-scoped `CoreNetworkAddress` the request names, in the same address family as
    /// `peer_address`, or `None` for a request naming none.
    core_network_address: Option<&'static str>,
    /// What the request spells for the `NO_ENCAP`-scoped `SubnetArn`: a subnet of the
    /// attachment's own Region, the empty string the modelled pattern admits (which names no
    /// subnet), or nothing at all.
    subnet: Subnet,
    /// The member the refusal has to name, or `None` for a row that must be accepted.
    refused: Option<&'static str>,
}

/// What a row spells for `SubnetArn`. The empty string is one of the two alternatives the
/// modelled pattern accepts (`^arn:…$|^$`, `smithy_query shape=SubnetArn`), and names no
/// subnet.
enum Subnet {
    Absent,
    Empty,
    Named,
}

/// The table: every pairing of a scoped member with a protocol that the two model sentences
/// decide, at both edges and in both address families.
const CASES: [Case; 11] = [
    Case {
        name: "a GRE peer naming the GRE-scoped core network address",
        edge: 0,
        protocol: 0,
        peer_address: "203.0.113.1",
        core_network_address: Some("203.0.113.2"),
        subnet: Subnet::Absent,
        refused: None,
    },
    Case {
        name: "a GRE peer naming neither scoped member",
        edge: 0,
        protocol: 0,
        peer_address: "203.0.113.3",
        core_network_address: None,
        subnet: Subnet::Absent,
        refused: None,
    },
    Case {
        name: "a GRE peer naming the NO_ENCAP-scoped subnet, in the attachment's own Region",
        edge: 0,
        protocol: 0,
        peer_address: "203.0.113.4",
        core_network_address: None,
        subnet: Subnet::Named,
        refused: Some(SUBNET_ARN),
    },
    Case {
        name: "a GRE peer naming both scoped members",
        edge: 0,
        protocol: 0,
        peer_address: "203.0.113.5",
        core_network_address: Some("203.0.113.6"),
        subnet: Subnet::Named,
        refused: Some(SUBNET_ARN),
    },
    Case {
        name: "a GRE peer whose SubnetArn is the spelling that names no subnet",
        edge: 0,
        protocol: 0,
        peer_address: "203.0.113.7",
        core_network_address: Some("203.0.113.8"),
        subnet: Subnet::Empty,
        refused: None,
    },
    Case {
        name: "a NO_ENCAP peer naming the NO_ENCAP-scoped subnet",
        edge: 1,
        protocol: 1,
        peer_address: "192.0.2.10",
        core_network_address: None,
        subnet: Subnet::Named,
        refused: None,
    },
    Case {
        name: "a NO_ENCAP peer naming neither scoped member",
        edge: 1,
        protocol: 1,
        peer_address: "192.0.2.11",
        core_network_address: None,
        subnet: Subnet::Absent,
        refused: None,
    },
    Case {
        name: "a NO_ENCAP peer naming the GRE-scoped core network address",
        edge: 0,
        protocol: 1,
        peer_address: "192.0.2.12",
        core_network_address: Some("192.0.2.13"),
        subnet: Subnet::Absent,
        refused: Some(CORE_NETWORK_ADDRESS),
    },
    Case {
        name: "a NO_ENCAP peer naming an IPv6 core network address of the peer's own family",
        edge: 1,
        protocol: 1,
        peer_address: "2001:db8::1",
        core_network_address: Some("2001:db8::2"),
        subnet: Subnet::Named,
        refused: Some(CORE_NETWORK_ADDRESS),
    },
    Case {
        name: "a peer on an attachment whose options name no protocol, naming both scoped members",
        edge: 0,
        protocol: 2,
        peer_address: "198.51.100.20",
        core_network_address: Some("198.51.100.21"),
        subnet: Subnet::Named,
        refused: None,
    },
    Case {
        name: "an IPv6 peer on an attachment naming no protocol, naming the core network address",
        edge: 1,
        protocol: 2,
        peer_address: "2001:db8::20",
        core_network_address: Some("2001:db8::21"),
        subnet: Subnet::Absent,
        refused: None,
    },
];

/// The subnet ARN a row that names one carries: one in the Region its attachment sits at,
/// so the Region rule a named subnet also goes through is satisfied.
fn case_subnet(case: &Case) -> String {
    subnet_arn(EDGES[case.edge], 900 + case.edge * 10 + case.protocol)
}

/// What a row expects `SubnetArn` to read back as: the subnet it named, and nothing for the
/// spellings that name none.
fn expected_subnet(case: &Case) -> Option<String> {
    match case.subnet {
        Subnet::Named => Some(case_subnet(case)),
        Subnet::Absent | Subnet::Empty => None,
    }
}

/// The `CreateConnectPeer` body a row asks for on `attachment`.
fn request_of(case: &Case, attachment: &str) -> Value {
    let mut body = json!({
        "ConnectAttachmentId": attachment,
        "PeerAddress": case.peer_address,
    });
    let object = body
        .as_object_mut()
        .unwrap_or_else(|| panic!("case {:?}: the request body must be an object", case.name));
    if let Some(address) = case.core_network_address {
        object.insert("CoreNetworkAddress".to_string(), json!(address));
    }
    match case.subnet {
        Subnet::Absent => {}
        Subnet::Empty => {
            object.insert("SubnetArn".to_string(), json!(""));
        }
        Subnet::Named => {
            object.insert("SubnetArn".to_string(), json!(case_subnet(case)));
        }
    }
    body
}

/// The peer ids `ListConnectPeers` reports for `attachment`: that attachment's stored
/// inventory, read through an operation and unpaged.
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

#[tokio::test]
async fn a_scoped_member_is_stored_only_on_a_peering_running_the_protocol_it_applies_to() {
    let emulator = Emulator::new();
    let subject = subject(&emulator).await;

    for case in CASES.iter().filter(|case| case.refused.is_none()) {
        let name = case.name;
        let attachment = subject.attachment(case.edge, case.protocol).to_string();
        let (status, created) = create_connect_peer(&emulator, request_of(case, &attachment)).await;
        assert_eq!(
            status, 200,
            "case {name:?}: CreateConnectPeer expected 200, got {status} with {created}"
        );
        let id = created["ConnectPeer"]["ConnectPeerId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {name:?}: no ConnectPeer.ConnectPeerId in {created}"))
            .to_string();
        assert!(
            is_minted_id(&id, "connect-peer"),
            "case {name:?}: expected a minted connect-peer id, got {id:?}"
        );

        // The stored peer reports the protocol it inherited and the scoped members that
        // protocol gives meaning to — in the configuration, in the BGP session inside it,
        // and at the top level — all from the one stored record.
        let stored = read_back(&emulator, name, &id).await;
        let configuration = &stored["Configuration"];
        assert_eq!(
            configuration["Protocol"].as_str(),
            PROTOCOLS[case.protocol],
            "case {name:?}: expected the attachment's tunnel protocol, got {stored}"
        );
        assert_eq!(
            configuration["CoreNetworkAddress"].as_str(),
            case.core_network_address,
            "case {name:?}: expected the core network address {:?}, got {stored}",
            case.core_network_address
        );
        assert_eq!(
            configuration["BgpConfigurations"][0]["CoreNetworkAddress"].as_str(),
            case.core_network_address,
            "case {name:?}: expected the BGP session's core network address {:?}, got {stored}",
            case.core_network_address
        );
        let subnet = expected_subnet(case);
        assert_eq!(
            stored["SubnetArn"].as_str().map(str::to_string),
            subnet,
            "case {name:?}: expected the subnet {subnet:?}, got {stored}"
        );

        let listed = stored_ids(&emulator, &attachment).await;
        assert!(
            listed.contains(&id),
            "case {name:?}: ListConnectPeers must report the accepted peer {id}, got {listed:?}"
        );
    }
}

#[tokio::test]
async fn a_scoped_member_named_against_the_other_protocol_is_refused_and_stores_nothing() {
    let emulator = Emulator::new();
    let subject = subject(&emulator).await;

    // Whatever each row names, no peer may be stored for it: the inventory of every
    // attachment under test stays the one it started with.
    let mut before: Vec<(String, Vec<String>)> = Vec::new();
    for edge in 0..EDGES.len() {
        for protocol in 0..PROTOCOLS.len() {
            let attachment = subject.attachment(edge, protocol).to_string();
            let ids = stored_ids(&emulator, &attachment).await;
            before.push((attachment, ids));
        }
    }

    for case in CASES.iter() {
        let Some(blamed) = case.refused else {
            continue;
        };
        let name = case.name;
        let attachment = subject.attachment(case.edge, case.protocol).to_string();
        let (status, refusal) = create_connect_peer(&emulator, request_of(case, &attachment)).await;
        assert_eq!(
            status, 400,
            "case {name:?}: CreateConnectPeer expected 400, got {status} with {refusal}"
        );
        assert_eq!(
            error_code(&refusal),
            "ValidationException",
            "case {name:?}: expected ValidationException, got {refusal}"
        );
        assert_eq!(
            refusal["Reason"].as_str(),
            Some(REASON_OTHER),
            "case {name:?}: expected the {REASON_OTHER} reason, got {refusal}"
        );
        assert_eq!(
            refusal["Fields"][0]["Name"].as_str(),
            Some(blamed),
            "case {name:?}: the refusal must name {blamed}, got {refusal}"
        );
        assert!(
            refusal["Fields"][0]["Message"]
                .as_str()
                .is_some_and(|message| message.contains(blamed)),
            "case {name:?}: the named field must explain itself, got {refusal}"
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
