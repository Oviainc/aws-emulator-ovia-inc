// SPDX-License-Identifier: CC0-1.0

//! The ONE reading this crate gives an optional reference member, driven end to end over
//! HTTP through the real router (the harness lives in `crate::global_network_lifecycle`).
//!
//! The rule under test: an optional reference member whose modelled shape admits the empty
//! string reads that empty string as naming NO resource, so a request spelling it empty is
//! served exactly as the request that omits the member — the same stored record, the same
//! answer — while any non-empty value stays a reference to the resource it names. Every
//! handler of this crate applies that reading through one shared normalisation, so this
//! suite drives it through an operation on each side of the change that removed the last
//! hand-rolled copy of it: `CreateConnectPeer`'s `SubnetArn`, whose modelled pattern spells
//! the empty string as an alternative of its own (`^arn:…:subnet/subnet-[0-9a-f]{8,17}$|^$`,
//! `smithy_query query="shape" shape="SubnetArn"`), and `CreateConnection`'s `LinkId` and
//! `ConnectedLinkId`, modelled `len 0..50` over `^[\s\S]*$` (`smithy_query
//! query="operation" operation="CreateConnection"`).
//!
//! The rows span what the rule generalises over: the operation that carries the member,
//! which member of a multi-reference request is the one spelled empty, the form of the value
//! the member carries when it does name something (an ARN, an id), and — for the peering
//! rows — the edge location it sits at and the address family its two ends are in.
//!
//! Nothing reaches into `crate::state`: every record is created and read back through an
//! operation.

use serde_json::{json, Value};

use crate::attachment_lifecycle::{
    attachment_id, core_network_at, create_connect_attachment, create_vpc_attachment, subnet_arn,
    vpc_arn,
};
use crate::connect_peer_lifecycle::create_connect_peer;
use crate::connection_lifecycle::{create_connection, fixture, read_connection, Fixture};
use crate::empty_optional_reference::without;
use crate::global_network_lifecycle::Emulator;

/// The edge locations the Connect peer rows sit at, with one `NO_ENCAP` Connect attachment
/// each: `SubnetArn` "only applies only when the protocol is NO_ENCAP" (`smithy_query
/// query="doc" shape="CreateConnectPeerRequest" member="SubnetArn"`), so a peer that names
/// one has to be carried over that protocol.
const PEER_EDGES: [&str; 2] = ["us-west-2", "eu-central-1"];

/// The operation a row drives the shared reading through.
enum Subject {
    /// `CreateConnectPeer` on the Connect attachment at this entry of [`PEER_EDGES`],
    /// peering to this appliance address.
    ConnectPeer {
        edge: usize,
        peer_address: &'static str,
    },
    /// `CreateConnection` between the two seeded devices.
    Connection,
}

impl Subject {
    /// The members the service mints or derives per call, which two records of the same
    /// request never share (the id, the ARN, the creation instant, the clock-derived state).
    fn volatile(&self) -> &'static [&'static str] {
        match self {
            Subject::ConnectPeer { .. } => &["ConnectPeerId", "CreatedAt", "State"],
            Subject::Connection => &["ConnectionId", "ConnectionArn", "CreatedAt", "State"],
        }
    }
}

/// One row: the operation under test and the optional reference member of its request that
/// the row omits, spells empty, and names a resource with.
struct Row {
    /// Names the case in every assertion message.
    case: &'static str,
    subject: Subject,
    member: &'static str,
}

/// The table.
const ROWS: [Row; 4] = [
    Row {
        case: "the subnet of an IPv4 peering, an ARN-shaped reference",
        subject: Subject::ConnectPeer {
            edge: 0,
            peer_address: "203.0.113.1",
        },
        member: "SubnetArn",
    },
    Row {
        case: "the subnet of an IPv6 peering at another edge",
        subject: Subject::ConnectPeer {
            edge: 1,
            peer_address: "2001:db8::1",
        },
        member: "SubnetArn",
    },
    Row {
        case: "the first link of a connection, an id-shaped reference",
        subject: Subject::Connection,
        member: "LinkId",
    },
    Row {
        case: "the second link of a connection, the other member of the same request",
        subject: Subject::Connection,
        member: "ConnectedLinkId",
    },
];

/// The resources every row is driven against: one `NO_ENCAP` Connect attachment per entry of
/// [`PEER_EDGES`], and the devices and links a connection ties together.
struct Fixtures {
    attachments: Vec<String>,
    connections: Fixture,
}

/// Build [`Fixtures`].
async fn fixtures(emulator: &Emulator) -> Fixtures {
    let (_, core_network) = core_network_at(emulator, "one reading", &PEER_EDGES).await;
    let mut attachments = Vec::new();
    for (index, edge) in PEER_EDGES.iter().enumerate() {
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

        let (status, created) = create_connect_attachment(
            emulator,
            json!({
                "CoreNetworkId": core_network,
                "EdgeLocation": edge,
                "TransportAttachmentId": transport,
                "Options": { "Protocol": "NO_ENCAP" },
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {edge:?}: CreateConnectAttachment expected 200, got {status} with {created}"
        );
        attachments.push(attachment_id(edge, &created["ConnectAttachment"]));
    }

    Fixtures {
        attachments,
        connections: fixture(emulator, "one reading for connections").await,
    }
}

/// The value a row's member carries when it DOES name a resource: a subnet in the Region the
/// peering's edge sits in, or one of the seeded links.
fn named_value(fixtures: &Fixtures, row: &Row) -> String {
    match row.subject {
        Subject::ConnectPeer { edge, .. } => subnet_arn(PEER_EDGES[edge], 0x900 + edge),
        Subject::Connection => match row.member {
            "LinkId" => fixtures.connections.link.clone(),
            _ => fixtures.connections.connected_link.clone(),
        },
    }
}

/// Send one row's create with its member spelled `value` (`None` omits it entirely), and
/// answer the created record together with the stored one its implemented reader reports.
async fn create_and_read(
    emulator: &Emulator,
    fixtures: &Fixtures,
    row: &Row,
    value: Option<&str>,
) -> (Value, Value) {
    let case = row.case;
    match row.subject {
        Subject::ConnectPeer { edge, peer_address } => {
            let mut body = json!({
                "ConnectAttachmentId": fixtures.attachments[edge],
                "PeerAddress": peer_address,
            });
            if let Some(value) = value {
                body[row.member] = json!(value);
            }
            let (status, answer) = create_connect_peer(emulator, body.clone()).await;
            assert_eq!(
                status, 200,
                "case {case:?}: CreateConnectPeer({body}) expected 200, got {status} with {answer}"
            );
            let created = answer["ConnectPeer"].clone();
            let id = created["ConnectPeerId"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: no ConnectPeerId in {created}"))
                .to_string();

            let (status, read) = emulator.get(&format!("/connect-peers/{id}")).await;
            assert_eq!(
                status, 200,
                "case {case:?}: GetConnectPeer({id}) expected 200, got {status} with {read}"
            );
            (created, read["ConnectPeer"].clone())
        }
        Subject::Connection => {
            let seed = &fixtures.connections;
            let mut body = json!({
                "DeviceId": seed.device,
                "ConnectedDeviceId": seed.connected_device,
            });
            if let Some(value) = value {
                body[row.member] = json!(value);
            }
            let (status, answer) = create_connection(emulator, &seed.network, body.clone()).await;
            assert_eq!(
                status, 200,
                "case {case:?}: CreateConnection({body}) expected 200, got {status} with {answer}"
            );
            let created = answer["Connection"].clone();
            let id = created["ConnectionId"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: no ConnectionId in {created}"))
                .to_string();

            let read = read_connection(emulator, &seed.network, &id).await;
            (created, read)
        }
    }
}

#[tokio::test]
async fn an_empty_optional_reference_is_served_as_the_omitted_member() {
    let emulator = Emulator::new();
    let fixtures = fixtures(&emulator).await;

    for row in ROWS.iter() {
        let case = row.case;
        let member = row.member;
        let volatile = row.subject.volatile();

        let (empty_created, empty_read) = create_and_read(&emulator, &fixtures, row, Some("")).await;
        let (omitted_created, omitted_read) = create_and_read(&emulator, &fixtures, row, None).await;

        for (spelling, created) in [
            ("spelled as the empty string", &empty_created),
            ("omitted", &omitted_created),
        ] {
            assert_eq!(
                created[member],
                Value::Null,
                "case {case:?}: {member} {spelling} must name no resource, got {created}"
            );
        }
        assert_eq!(
            without(&empty_created, volatile),
            without(&omitted_created, volatile),
            "case {case:?}: the empty spelling of {member} must answer what omitting it \
             answers, got {empty_created} against {omitted_created}"
        );

        // ROUND TRIP: the stored record is what the reader answers, so the two spellings
        // agree in the store and not only in the create's answer.
        assert_eq!(
            empty_read[member],
            Value::Null,
            "case {case:?}: the stored record of the empty {member} must name no resource, \
             got {empty_read}"
        );
        assert_eq!(
            without(&empty_read, volatile),
            without(&omitted_read, volatile),
            "case {case:?}: the empty spelling of {member} must store what omitting it \
             stores, got {empty_read} against {omitted_read}"
        );

        // The other half of the reading: a non-empty value is a reference to the resource it
        // names, so only the empty string resolves nothing.
        let named = named_value(&fixtures, row);
        let (named_created, named_read) =
            create_and_read(&emulator, &fixtures, row, Some(&named)).await;
        assert_eq!(
            named_created[member],
            json!(named),
            "case {case:?}: {member} naming {named} must be held as that reference, \
             got {named_created}"
        );
        assert_eq!(
            named_read[member],
            json!(named),
            "case {case:?}: the stored record must hold {member} as {named}, got {named_read}"
        );
    }
}
