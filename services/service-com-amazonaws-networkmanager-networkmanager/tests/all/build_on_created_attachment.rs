// SPDX-License-Identifier: CC0-1.0

//! What a create that builds a resource ON a stored attachment requires of that attachment,
//! driven end to end over HTTP through the real router (the harness lives in
//! `crate::global_network_lifecycle`, the core-network helpers in
//! `crate::attachment_lifecycle` and `crate::attachment_acceptance`).
//!
//! The rule under test: a resource is built on an attachment that EXISTS — "a core network
//! Connect attachment uses an existing VPC attachment as the underlying transport mechanism"
//! (`smithy_query query="doc" shape="CreateConnectAttachment"`), and a Connect peer runs
//! inside the tunnel of a Connect attachment (`smithy_query query="doc"
//! shape="CreateConnectPeer"`) — so an attachment whose request the core network policy is
//! still holding for an acceptance (`PENDING_ATTACHMENT_ACCEPTANCE`) and one whose request
//! was turned down (`REJECTED`) can carry nothing: the create is refused with the modelled
//! `ValidationException` (HTTP 400) naming the member that carried the reference, and stores
//! nothing. An attachment the acceptance (or a policy that asked for none) has STARTED does
//! exist, whether the transition the clock owes it has fallen due or not, so a create over
//! it is served.
//!
//! The tables below span that class: rows differing in the lifecycle state the attachment
//! built on is in when the create is issued (all four an attachment can be in before
//! anything is built on it), in how it was brought into that state (a policy claiming
//! nothing, an acceptance granted, an acceptance refused, the settle span elapsed), in
//! Region and so in edge location, in tunnel protocol and in whether the create carries
//! tags. Nothing reaches into `crate::state`: every attachment and peer is created, decided,
//! listed and read back through an operation.

use serde_json::{json, Value};

use crate::attachment_acceptance::{accept, core_network_with, read_vpc};
use crate::attachment_lifecycle::{
    attachment_id, create_connect_attachment, create_vpc_attachment, subnet_arn, vpc_arn,
};
use crate::connect_peer_lifecycle::create_connect_peer;
use crate::delete_attachment::requires_acceptance;
use crate::global_network_lifecycle::{error_code, ids_of, Emulator, PAST_SETTLE};

/// The four `AttachmentState` values an attachment can be in before anything is built on it
/// (`smithy_query shape=AttachmentState`).
const PENDING_ACCEPTANCE: &str = "PENDING_ATTACHMENT_ACCEPTANCE";
/// See [`PENDING_ACCEPTANCE`].
const CREATING: &str = "CREATING";
/// See [`PENDING_ACCEPTANCE`].
const AVAILABLE: &str = "AVAILABLE";
/// See [`PENDING_ACCEPTANCE`].
const REJECTED: &str = "REJECTED";

/// The members the two creates carry their reference in, as this service names them in a
/// refusal.
const TRANSPORT_ATTACHMENT_ID: &str = "transportAttachmentId";
/// See [`TRANSPORT_ATTACHMENT_ID`].
const CONNECT_ATTACHMENT_ID: &str = "connectAttachmentId";

/// The `Reason` a `ValidationException` of this service carries when the request's own
/// members all satisfy their constraints yet the request is invalid against stored state
/// (`smithy_query shape=ValidationExceptionReason`).
const REASON_OTHER: &str = "Other";

/// `RejectAttachment` (`POST /attachments/{AttachmentId}/reject`).
async fn reject(emulator: &Emulator, id: &str) -> (u16, Value) {
    emulator
        .call("POST", &format!("/attachments/{id}/reject"), None)
        .await
}

/// The `attachment-policies` of a policy that holds only CONNECT attachments for an
/// acceptance, so the VPC transport under one is created without waiting for a decision
/// while the Connect attachment carried over it waits for one. The key names are the
/// published Cloud WAN core network policy schema's, the same source
/// `crate::attachment_acceptance` builds its documents from.
fn connect_requires_acceptance() -> Value {
    json!([{
        "rule-number": 30,
        "conditions": [{ "type": "attachment-type", "operator": "equals", "value": "CONNECT" }],
        "action": {
            "association-method": "constant",
            "segment": "prod",
            "require-attachment-acceptance": true,
        },
    }])
}

/// How a row brings the attachment it builds on into the state it is in when the create is
/// issued — every path there this service has.
#[derive(Clone, Copy, Debug)]
enum Brought {
    /// A policy claiming the attachment for an acceptance, left undecided.
    LeftPending,
    /// The same, then `RejectAttachment`.
    Rejected,
    /// The same, then `AcceptAttachment`, which starts the attachment.
    Accepted,
    /// A policy asking for no acceptance, so the attachment is started by its create.
    Started,
    /// The same, with the settle span the clock owes let elapse.
    Settled,
}

impl Brought {
    /// Whether the attachment's core network policy holds the attachment for an acceptance.
    fn holds_for_acceptance(self) -> bool {
        match self {
            Brought::LeftPending | Brought::Rejected | Brought::Accepted => true,
            Brought::Started | Brought::Settled => false,
        }
    }
}

/// One row of the transport table: the state the transport of a `CreateConnectAttachment` is
/// in, how it got there, and what the create over it must answer.
struct TransportCase {
    /// Names the case in every assertion message.
    name: &'static str,
    /// The Region of the core network's single edge location, and so of both attachments.
    region: &'static str,
    /// The tunnel protocol the Connect create asks for — both values `TunnelProtocol`
    /// enumerates (`smithy_query shape=TunnelProtocol`).
    protocol: &'static str,
    /// The `Tags` the Connect create carries, or `Value::Null` for no `Tags` member.
    tags: Value,
    brought: Brought,
    /// The state the transport must be in when the Connect create is issued.
    state_before: &'static str,
    /// Whether a transport in that state can carry a Connect attachment.
    refused: bool,
}

/// The table: rows differ along the state the transport is in, the path that put it there,
/// the Region, the tunnel protocol and the tags — everything the rule generalises over.
fn transport_cases() -> Vec<TransportCase> {
    vec![
        TransportCase {
            name: "a transport still waiting for the acceptance its policy asked for",
            region: "us-west-2",
            protocol: "GRE",
            tags: Value::Null,
            brought: Brought::LeftPending,
            state_before: PENDING_ACCEPTANCE,
            refused: true,
        },
        TransportCase {
            name: "a transport whose attachment request was turned down",
            region: "eu-west-1",
            protocol: "NO_ENCAP",
            tags: json!([{ "Key": "env", "Value": "prod" }]),
            brought: Brought::Rejected,
            state_before: REJECTED,
            refused: true,
        },
        TransportCase {
            name: "a transport the acceptance started, still CREATING",
            region: "sa-east-1",
            protocol: "NO_ENCAP",
            tags: Value::Null,
            brought: Brought::Accepted,
            state_before: CREATING,
            refused: false,
        },
        TransportCase {
            name: "a transport no rule held, started by its own create",
            region: "ap-south-1",
            protocol: "GRE",
            tags: json!([{ "Key": "team", "Value": "net" }, { "Key": "only", "Value": "" }]),
            brought: Brought::Started,
            state_before: CREATING,
            refused: false,
        },
        TransportCase {
            name: "a transport already AVAILABLE",
            region: "us-east-1",
            protocol: "GRE",
            tags: Value::Null,
            brought: Brought::Settled,
            state_before: AVAILABLE,
            refused: false,
        },
    ]
}

/// The ids `ListAttachments` reports for `core_network` — the stored inventory of it, read
/// through an operation.
async fn stored_attachments(emulator: &Emulator, case: &str, core_network: &str) -> Vec<String> {
    let (status, body) = emulator
        .get(&format!("/attachments?coreNetworkId={core_network}"))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListAttachments expected 200, got {status} with {body}"
    );
    let mut ids = ids_of(&body, "Attachments", "AttachmentId");
    ids.sort();
    ids
}

/// The peer ids `ListConnectPeers` reports for `attachment`, read through an operation.
async fn stored_peers(emulator: &Emulator, case: &str, attachment: &str) -> Vec<String> {
    let (status, body) = emulator
        .get(&format!("/connect-peers?connectAttachmentId={attachment}"))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListConnectPeers expected 200, got {status} with {body}"
    );
    let mut ids = ids_of(&body, "ConnectPeers", "ConnectPeerId");
    ids.sort();
    ids
}

/// Bring one attachment of `brought`'s path to the state a row needs, answering `(the core
/// network, the attachment)`: a VPC attachment under a policy that holds every attachment
/// for an acceptance or under one that holds none, then the decision — if any — the row
/// takes on it. `index` distinguishes the VPC it attaches from every other row's.
async fn transport_in_state(
    emulator: &Emulator,
    case: &TransportCase,
    index: usize,
) -> (String, String) {
    let name = case.name;
    let rules = if case.brought.holds_for_acceptance() {
        requires_acceptance()
    } else {
        Value::Null
    };
    let core_network = core_network_with(emulator, name, case.region, &rules).await;
    let (status, created) = create_vpc_attachment(
        emulator,
        json!({
            "CoreNetworkId": core_network,
            "VpcArn": vpc_arn(case.region, index + 1),
            "SubnetArns": [subnet_arn(case.region, index + 1)],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {name:?}: CreateVpcAttachment expected 200, got {status} with {created}"
    );
    let transport = attachment_id(name, &created["VpcAttachment"]);

    match case.brought {
        Brought::LeftPending | Brought::Started => {}
        Brought::Rejected => {
            let (status, answered) = reject(emulator, &transport).await;
            assert_eq!(
                status, 200,
                "case {name:?}: RejectAttachment expected 200, got {status} with {answered}"
            );
        }
        Brought::Accepted => {
            let (status, answered) = accept(emulator, &transport).await;
            assert_eq!(
                status, 200,
                "case {name:?}: AcceptAttachment expected 200, got {status} with {answered}"
            );
        }
        Brought::Settled => tokio::time::sleep(PAST_SETTLE).await,
    }

    // The state the row says the transport is in, read back through its own reader, so the
    // create below is issued against a known state rather than an assumed one.
    let (status, read) = read_vpc(emulator, &transport).await;
    assert_eq!(
        status, 200,
        "case {name:?}: GetVpcAttachment expected 200, got {status} with {read}"
    );
    assert_eq!(
        read["VpcAttachment"]["Attachment"]["State"],
        json!(case.state_before),
        "case {name:?}: expected the transport to be {} before the create, got {read}",
        case.state_before
    );
    (core_network, transport)
}

/// Assert `refusal` is this service's refusal of the reference `member` carried, naming the
/// attachment it named and the state that attachment is in.
fn assert_reference_refusal(
    case: &str,
    member: &str,
    status: u16,
    refusal: &Value,
    attachment: &str,
    state: &str,
) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with {refusal}"
    );
    assert_eq!(
        error_code(refusal),
        "ValidationException",
        "case {case:?}: expected ValidationException, got {refusal}"
    );
    assert_eq!(
        refusal["Reason"].as_str(),
        Some(REASON_OTHER),
        "case {case:?}: expected the {REASON_OTHER} reason, got {refusal}"
    );
    assert_eq!(
        refusal["Fields"][0]["Name"].as_str(),
        Some(member),
        "case {case:?}: the refusal must name {member}, got {refusal}"
    );
    let message = refusal["Message"].as_str().unwrap_or_default();
    assert!(
        message.contains(attachment) && message.contains(state),
        "case {case:?}: the refusal must name {attachment} and the state {state} it is in, got {message:?}"
    );
}

#[tokio::test]
async fn a_connect_attachment_is_built_only_on_a_transport_that_exists() {
    let emulator = Emulator::new();

    for (index, case) in transport_cases().iter().enumerate() {
        let name = case.name;
        let (core_network, transport) = transport_in_state(&emulator, case, index).await;

        let mut request = json!({
            "CoreNetworkId": core_network,
            "EdgeLocation": case.region,
            "TransportAttachmentId": transport,
            "Options": { "Protocol": case.protocol },
        });
        if !case.tags.is_null() {
            request["Tags"] = case.tags.clone();
        }
        let (status, answered) = create_connect_attachment(&emulator, request).await;

        if case.refused {
            assert_reference_refusal(
                name,
                TRANSPORT_ATTACHMENT_ID,
                status,
                &answered,
                &transport,
                case.state_before,
            );
            // The refusal stored NOTHING: the core network still holds the transport alone,
            // and the transport therefore carries no dependent that would block its delete.
            let stored = stored_attachments(&emulator, name, &core_network).await;
            assert_eq!(
                stored,
                vec![transport.clone()],
                "case {name:?}: a refused create stored an attachment, got {stored:?}"
            );
            let (status, removed) = emulator.delete(&format!("/attachments/{transport}")).await;
            assert_eq!(
                status, 200,
                "case {name:?}: nothing was built on the transport, so DeleteAttachment expected 200, got {status} with {removed}"
            );
            continue;
        }

        assert_eq!(
            status, 200,
            "case {name:?}: CreateConnectAttachment expected 200, got {status} with {answered}"
        );
        let connect = attachment_id(name, &answered["ConnectAttachment"]);

        // Read back through the operation that reports a stored Connect attachment: the
        // record names the transport it was built on.
        let (status, read) = emulator
            .get(&format!("/connect-attachments/{connect}"))
            .await;
        assert_eq!(
            status, 200,
            "case {name:?}: GetConnectAttachment expected 200, got {status} with {read}"
        );
        assert_eq!(
            read["ConnectAttachment"]["TransportAttachmentId"],
            json!(transport),
            "case {name:?}: the stored attachment must name its transport, got {read}"
        );
        assert_eq!(
            read["ConnectAttachment"]["Options"]["Protocol"],
            json!(case.protocol),
            "case {name:?}: the stored attachment must report the protocol asked for, got {read}"
        );
        let stored = stored_attachments(&emulator, name, &core_network).await;
        let mut expected = vec![connect.clone(), transport.clone()];
        expected.sort();
        assert_eq!(
            stored, expected,
            "case {name:?}: expected the transport and the attachment built on it, got {stored:?}"
        );
    }
}

/// One row of the peer table: the state the Connect attachment a `CreateConnectPeer` names
/// is in, how it got there, and what the create over it must answer.
struct PeerCase {
    /// Names the case in every assertion message.
    name: &'static str,
    /// The Region of the core network's single edge location.
    region: &'static str,
    brought: Brought,
    /// The state the Connect attachment must be in when the peer create is issued.
    state_before: &'static str,
    /// Whether a Connect attachment in that state can carry a peer.
    refused: bool,
}

/// The table: the same four states, reached the same ways, for the resource built INSIDE a
/// Connect attachment rather than over a transport.
fn peer_cases() -> Vec<PeerCase> {
    vec![
        PeerCase {
            name: "a Connect attachment still waiting for the acceptance its policy asked for",
            region: "us-west-2",
            brought: Brought::LeftPending,
            state_before: PENDING_ACCEPTANCE,
            refused: true,
        },
        PeerCase {
            name: "a Connect attachment whose attachment request was turned down",
            region: "eu-west-2",
            brought: Brought::Rejected,
            state_before: REJECTED,
            refused: true,
        },
        PeerCase {
            name: "a Connect attachment the acceptance started, still CREATING",
            region: "ca-central-1",
            brought: Brought::Accepted,
            state_before: CREATING,
            refused: false,
        },
        PeerCase {
            name: "a Connect attachment no rule held, started by its own create",
            region: "ap-south-1",
            brought: Brought::Started,
            state_before: CREATING,
            refused: false,
        },
        PeerCase {
            name: "a Connect attachment already AVAILABLE",
            region: "us-east-1",
            brought: Brought::Settled,
            state_before: AVAILABLE,
            refused: false,
        },
    ]
}

/// Bring one Connect attachment to the state a peer row needs, answering it: a core network
/// whose policy holds CONNECT attachments for an acceptance (or holds nothing), a VPC
/// transport that exists under it, the Connect attachment over that transport, and the
/// decision — if any — the row takes on that Connect attachment.
async fn connect_attachment_in_state(
    emulator: &Emulator,
    case: &PeerCase,
    index: usize,
) -> String {
    let name = case.name;
    let rules = if case.brought.holds_for_acceptance() {
        connect_requires_acceptance()
    } else {
        Value::Null
    };
    let core_network = core_network_with(emulator, name, case.region, &rules).await;
    let (status, created) = create_vpc_attachment(
        emulator,
        json!({
            "CoreNetworkId": core_network,
            "VpcArn": vpc_arn(case.region, index + 1),
            "SubnetArns": [subnet_arn(case.region, index + 1)],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {name:?}: CreateVpcAttachment expected 200, got {status} with {created}"
    );
    let transport = attachment_id(name, &created["VpcAttachment"]);

    let (status, created) = create_connect_attachment(
        emulator,
        json!({
            "CoreNetworkId": core_network,
            "EdgeLocation": case.region,
            "TransportAttachmentId": transport,
            "Options": { "Protocol": "GRE" },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {name:?}: CreateConnectAttachment expected 200, got {status} with {created}"
    );
    let connect = attachment_id(name, &created["ConnectAttachment"]);

    match case.brought {
        Brought::LeftPending | Brought::Started => {}
        Brought::Rejected => {
            let (status, answered) = reject(emulator, &connect).await;
            assert_eq!(
                status, 200,
                "case {name:?}: RejectAttachment expected 200, got {status} with {answered}"
            );
        }
        Brought::Accepted => {
            let (status, answered) = accept(emulator, &connect).await;
            assert_eq!(
                status, 200,
                "case {name:?}: AcceptAttachment expected 200, got {status} with {answered}"
            );
        }
        Brought::Settled => tokio::time::sleep(PAST_SETTLE).await,
    }

    let (status, read) = emulator
        .get(&format!("/connect-attachments/{connect}"))
        .await;
    assert_eq!(
        status, 200,
        "case {name:?}: GetConnectAttachment expected 200, got {status} with {read}"
    );
    assert_eq!(
        read["ConnectAttachment"]["Attachment"]["State"],
        json!(case.state_before),
        "case {name:?}: expected the Connect attachment to be {} before the create, got {read}",
        case.state_before
    );
    connect
}

#[tokio::test]
async fn a_connect_peer_is_built_only_in_a_connect_attachment_that_exists() {
    let emulator = Emulator::new();

    for (index, case) in peer_cases().iter().enumerate() {
        let name = case.name;
        let connect = connect_attachment_in_state(&emulator, case, index).await;

        // One appliance address per row, out of the block reserved for benchmarking
        // (198.18.0.0/15), so no two rows ask for the same peer.
        let (status, answered) = create_connect_peer(
            &emulator,
            json!({
                "ConnectAttachmentId": connect,
                "PeerAddress": format!("198.18.0.{}", index + 1),
            }),
        )
        .await;

        if case.refused {
            assert_reference_refusal(
                name,
                CONNECT_ATTACHMENT_ID,
                status,
                &answered,
                &connect,
                case.state_before,
            );
            let stored = stored_peers(&emulator, name, &connect).await;
            assert!(
                stored.is_empty(),
                "case {name:?}: a refused create stored a peer, got {stored:?}"
            );
            continue;
        }

        assert_eq!(
            status, 200,
            "case {name:?}: CreateConnectPeer expected 200, got {status} with {answered}"
        );
        let peer = answered["ConnectPeer"]["ConnectPeerId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {name:?}: no ConnectPeer.ConnectPeerId in {answered}"))
            .to_string();

        // Read back through the operation that reports a stored peer: it names the Connect
        // attachment it runs inside.
        let (status, read) = emulator.get(&format!("/connect-peers/{peer}")).await;
        assert_eq!(
            status, 200,
            "case {name:?}: GetConnectPeer expected 200, got {status} with {read}"
        );
        assert_eq!(
            read["ConnectPeer"]["ConnectAttachmentId"],
            json!(connect),
            "case {name:?}: the stored peer must name its Connect attachment, got {read}"
        );
        let stored = stored_peers(&emulator, name, &connect).await;
        assert_eq!(
            stored,
            vec![peer.clone()],
            "case {name:?}: expected only the peer this row created, got {stored:?}"
        );
    }
}
