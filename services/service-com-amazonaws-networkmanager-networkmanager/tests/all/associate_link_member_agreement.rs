// SPDX-License-Identifier: CC0-1.0

//! The reading an OPTIONAL reference member gets, pinned across EVERY operation that takes
//! the same one — here the optional link member of the three associate operations of this
//! service.
//!
//! The rule under test: an optional reference member is read the same way by every operation
//! that carries it — the empty string is the spelling for giving no value, so it names no
//! resource, resolves nothing and is held as none, exactly as omitting the member is, while
//! any non-empty value stays a reference that must resolve against the same stored state and
//! is refused the same way when it does not, whichever operation carried it.
//!
//! `LinkId` is optional on `AssociateCustomerGateway`, `AssociateConnectPeer` and
//! `AssociateTransitGatewayConnectPeer`, and all three bind the one modelled shape, `len
//! 0..50` over `pattern ^[\s\S]*$` (`smithy_query query="operation"
//! operation="AssociateCustomerGateway,AssociateConnectPeer,AssociateTransitGatewayConnectPeer"`),
//! so the empty string satisfies the constraint walk on all three and reaches each handler.
//! The service documents that spelling as how a caller gives no value ("To remove
//! information for any of the parameters, specify an empty string", `smithy_query query="doc"
//! shape="UpdateDevice"`). The tables below therefore span the OPERATION as well as the
//! spelling: one request body shape served by two of the three and refused by the third is
//! exactly what they refuse to let pass.
//!
//! Every call goes through the real `MultiServiceRouter`, so the restJson1 routes, the
//! label/query bindings and the error envelopes are exercised as a client hits them. Nothing
//! reaches into `crate::state`: every association is read back through its own implemented
//! reader. The harness is the one `global_network_lifecycle` defines, and the
//! record-comparison helper and the unstored-id constant are the ones
//! `empty_optional_reference` defines for the same rule.
//!
//! `empty_optional_reference` pins that rule per WRITER, one operation at a time; what is
//! added here is the agreement BETWEEN the operations, which no single-operation suite can
//! see. The customer-gateway rows it shares with that suite are kept deliberately: they are
//! the reference the other two operations are compared against, so dropping them would leave
//! the comparison with nothing to anchor on.

use serde_json::{json, Value};

use crate::attachment_lifecycle::{
    attachment_id, core_network_at, create_connect_attachment, create_vpc_attachment, subnet_arn,
    vpc_arn,
};
use crate::connect_peer_association_lifecycle::create_connect_peer;
use crate::customer_gateway_association_lifecycle::plain_cgw_arn;
use crate::device_lifecycle::create_device_ok;
use crate::empty_optional_reference::{without, UNSTORED_LINK};
use crate::global_network_lifecycle::{error_code, Emulator};
use crate::link_lifecycle::{create_link_ok, plain_link};
use crate::resource_tagging::escape;
use crate::transit_gateway_connect_peer_association_lifecycle::{
    connect_peer_arn, register_gateway,
};
use crate::transit_gateway_peering::{own_account, transit_gateway_arn};

/// The Region every subject of the tables lives in: the core network's only edge, and the
/// Region of the registered transit gateway the transit gateway Connect peers sit on.
const EDGE: &str = "us-west-2";

/// The three associate operations of this service whose optional link member the rule covers.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Associate {
    CustomerGateway,
    ConnectPeer,
    TransitGatewayConnectPeer,
}

const ASSOCIATES: [Associate; 3] = [
    Associate::CustomerGateway,
    Associate::ConnectPeer,
    Associate::TransitGatewayConnectPeer,
];

impl Associate {
    /// The operation's name, as every assertion message below spells it.
    fn name(self) -> &'static str {
        match self {
            Self::CustomerGateway => "AssociateCustomerGateway",
            Self::ConnectPeer => "AssociateConnectPeer",
            Self::TransitGatewayConnectPeer => "AssociateTransitGatewayConnectPeer",
        }
    }

    /// The collection under the global network in the path that the associate POSTs to and
    /// its reader GETs from.
    fn collection(self) -> &'static str {
        match self {
            Self::CustomerGateway => "customer-gateway-associations",
            Self::ConnectPeer => "connect-peer-associations",
            Self::TransitGatewayConnectPeer => "transit-gateway-connect-peer-associations",
        }
    }

    /// The member the created association sits under in the answer. The reader's list member
    /// is this one pluralised.
    fn wrapper(self) -> &'static str {
        match self {
            Self::CustomerGateway => "CustomerGatewayAssociation",
            Self::ConnectPeer => "ConnectPeerAssociation",
            Self::TransitGatewayConnectPeer => "TransitGatewayConnectPeerAssociation",
        }
    }

    /// The member naming the resource the association ties, which is the association's
    /// identity.
    fn subject_member(self) -> &'static str {
        match self {
            Self::CustomerGateway => "CustomerGatewayArn",
            Self::ConnectPeer => "ConnectPeerId",
            Self::TransitGatewayConnectPeer => "TransitGatewayConnectPeerArn",
        }
    }

    /// The reader's query parameter that selects by [`Self::subject_member`].
    fn selector(self) -> &'static str {
        match self {
            Self::CustomerGateway => "customerGatewayArns",
            Self::ConnectPeer => "connectPeerIds",
            Self::TransitGatewayConnectPeer => "transitGatewayConnectPeerArns",
        }
    }
}

/// How a row spells the optional link member.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Spelling {
    /// The link the device is associated with.
    Named,
    /// The member is absent from the request.
    Omitted,
    /// The member is present and empty.
    Empty,
}

/// The spellings, in the order the rows below drive them.
const SPELLINGS: [Spelling; 3] = [Spelling::Named, Spelling::Omitted, Spelling::Empty];

/// What one operation answered for one spelling: the status, and the link the STORED
/// association carries.
type Answer = (u16, Option<String>);

/// The global network of the tables, carrying everything the three associates need: a core
/// network with a Connect attachment, so Cloud WAN Connect peers can be minted; a registered
/// transit gateway in [`EDGE`], so a transit gateway Connect peer of that Region is
/// acceptable; a device at a site with one link associated with it; a second link of that
/// site no device is associated with; and a link of ANOTHER global network.
struct Fixture {
    network: String,
    connect_attachment: String,
    device: String,
    link: String,
    unassociated_link: String,
    foreign_link: String,
    /// Distinguishes the resource each call ties, so no two calls of a table collide on
    /// identity.
    minted: usize,
}

/// Build the fixture on a fresh emulator.
async fn fixture(emulator: &Emulator) -> Fixture {
    let (network, core_network) = core_network_at(emulator, "link member agreement", &[EDGE]).await;

    let (status, created) = create_vpc_attachment(
        emulator,
        json!({
            "CoreNetworkId": core_network,
            "VpcArn": vpc_arn(EDGE, 1),
            "SubnetArns": [subnet_arn(EDGE, 1)],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the fixture's CreateVpcAttachment expected 200, got {status} with {created}"
    );
    let transport = attachment_id("the transport attachment", &created["VpcAttachment"]);

    let (status, created) = create_connect_attachment(
        emulator,
        json!({
            "CoreNetworkId": core_network,
            "EdgeLocation": EDGE,
            "TransportAttachmentId": transport,
            "Options": { "Protocol": "GRE" },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the fixture's CreateConnectAttachment expected 200, got {status} with {created}"
    );
    let connect_attachment = attachment_id("the Connect attachment", &created["ConnectAttachment"]);

    // A transit gateway Connect peer can only be associated when its global network holds a
    // registration for a transit gateway it could have been created on.
    register_gateway(
        emulator,
        &network,
        &transit_gateway_arn("aws", EDGE, &own_account(), 1, 17),
    )
    .await;

    let site = emulator.create_site(&network, "hq").await;
    let device = create_device_ok(emulator, &network, json!({ "SiteId": site })).await;
    let link = create_link_ok(emulator, &network, &site, &plain_link()).await;
    let unassociated_link = create_link_ok(emulator, &network, &site, &plain_link()).await;
    let (status, body) = emulator
        .post(
            &format!("/global-networks/{network}/link-associations"),
            json!({ "DeviceId": device, "LinkId": link }),
        )
        .await;
    assert_eq!(
        status, 200,
        "the fixture's AssociateLink expected 200, got {status} with {body}"
    );

    let elsewhere = emulator.create_global_network("another global network").await;
    let elsewhere_site = emulator.create_site(&elsewhere, "hq").await;
    let foreign_link = create_link_ok(emulator, &elsewhere, &elsewhere_site, &plain_link()).await;

    Fixture {
        network,
        connect_attachment,
        device,
        link,
        unassociated_link,
        foreign_link,
        minted: 0,
    }
}

/// Mint the resource the next association of `operation` ties.
async fn next_subject(fixture: &mut Fixture, emulator: &Emulator, operation: Associate) -> String {
    fixture.minted += 1;
    let index = fixture.minted;
    match operation {
        // A customer gateway and a transit gateway Connect peer belong to Amazon VPC, which
        // this service never mints: the request names them by ARN. The id carries 17 hex
        // digits, the widest the EC2 ARN patterns of this model admit.
        Associate::CustomerGateway => plain_cgw_arn(&format!("{index:017x}")),
        Associate::TransitGatewayConnectPeer => {
            connect_peer_arn("aws", EDGE, &own_account(), index, 17)
        }
        // A Cloud WAN Connect peer IS a resource of this service, so one is created on the
        // fixture's Connect attachment.
        Associate::ConnectPeer => {
            create_connect_peer(emulator, &fixture.connect_attachment, index).await
        }
    }
}

/// The associate body for `operation` tying `subject` to the fixture's device, with the
/// optional link member omitted (`None`) or carrying the given value.
fn request(operation: Associate, fixture: &Fixture, subject: &str, link: Option<&str>) -> Value {
    let mut body = serde_json::Map::new();
    body.insert(operation.subject_member().to_string(), json!(subject));
    body.insert("DeviceId".to_string(), json!(fixture.device));
    if let Some(link) = link {
        body.insert("LinkId".to_string(), json!(link));
    }
    Value::Object(body)
}

/// Send one associate, answering the status and the response body.
async fn associate(
    emulator: &Emulator,
    operation: Associate,
    network: &str,
    body: Value,
) -> (u16, Value) {
    emulator
        .post(
            &format!("/global-networks/{network}/{}", operation.collection()),
            body,
        )
        .await
}

/// The association `operation`'s reader answers for `subject`, or `None` when it answers
/// none. An id travels in a query string, so its `:` and `/` are percent-encoded.
async fn read_back(
    emulator: &Emulator,
    case: &str,
    operation: Associate,
    network: &str,
    subject: &str,
) -> Option<Value> {
    let uri = format!(
        "/global-networks/{network}/{}?{}={}",
        operation.collection(),
        operation.selector(),
        escape(subject)
    );
    let (status, body) = emulator.get(&uri).await;
    assert_eq!(
        status, 200,
        "case {case:?}: {uri} expected 200, got {status} with {body}"
    );
    let list = format!("{}s", operation.wrapper());
    let page = body[&list]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no {list} array in {body}"));
    assert!(
        page.len() <= 1,
        "case {case:?}: one id selects at most one association, got {body}"
    );
    page.first().cloned()
}

/// The `LinkId` a record carries, as an owned value so two answers compare directly.
fn link_of(association: &Value) -> Option<String> {
    association["LinkId"].as_str().map(str::to_string)
}

#[tokio::test]
async fn every_associate_reads_an_empty_link_member_as_no_link_and_a_named_one_as_that_link() {
    let emulator = Emulator::new();
    let mut fixture = fixture(&emulator).await;

    // What each operation answered, row by row: the status and the link the STORED
    // association carries. The rule is about the spelling, so the three operations must agree
    // row for row — one body shape served by two of them and refused by the third is the
    // defect this pins.
    let mut outcomes: Vec<(Associate, Vec<Answer>)> = Vec::new();

    for operation in ASSOCIATES {
        let name = operation.name();
        // The stored record each spelling produced, in `SPELLINGS` order.
        let mut stored: Vec<Value> = Vec::new();
        let mut answered: Vec<Answer> = Vec::new();

        for spelling in SPELLINGS {
            let case = format!("{name} with the link member {spelling:?}");
            let subject = next_subject(&mut fixture, &emulator, operation).await;
            let named = match spelling {
                Spelling::Named => Some(fixture.link.clone()),
                Spelling::Empty => Some(String::new()),
                Spelling::Omitted => None,
            };
            let body = request(operation, &fixture, &subject, named.as_deref());
            let (status, answer) = associate(&emulator, operation, &fixture.network, body.clone())
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: {body} must be served, got {status} with {answer}"
            );

            let expected = match spelling {
                Spelling::Named => Some(fixture.link.clone()),
                Spelling::Omitted | Spelling::Empty => None,
            };
            let created = answer[operation.wrapper()].clone();
            // A missing `LinkId` and a missing association both read as `None`, so the
            // association the answer carries is identified first: that is what makes the
            // `None` rows below fail for the right reason.
            assert_eq!(
                created[operation.subject_member()].as_str(),
                Some(subject.as_str()),
                "case {case:?}: the answer must carry the association of {subject}, got {answer}"
            );
            assert_eq!(
                link_of(&created),
                expected,
                "case {case:?}: expected LinkId {expected:?}, got {created}"
            );

            // ROUND TRIP: the stored tie is what the operation's own reader answers, so the
            // reading is in the store and not only in the associate's answer.
            let read = read_back(&emulator, &case, operation, &fixture.network, &subject)
                .await
                .unwrap_or_else(|| panic!("case {case:?}: the association must be stored"));
            assert_eq!(
                link_of(&read),
                expected,
                "case {case:?}: the stored association must carry LinkId {expected:?}, got {read}"
            );
            assert_eq!(
                read["DeviceId"].as_str(),
                Some(fixture.device.as_str()),
                "case {case:?}: the stored association must name the device, got {read}"
            );

            answered.push((status, link_of(&read)));
            stored.push(read);
        }

        // The two spellings of "no value" are one request: the records they store have to
        // agree member for member, bar the resource each ties (the association's identity)
        // and the clock-derived state.
        let volatile = [operation.subject_member(), "State"];
        let omitted = &stored[1];
        let empty = &stored[2];
        assert_eq!(
            without(empty, &volatile),
            without(omitted, &volatile),
            "case {name} with an empty link member: it must store what omitting it stores, \
             got {empty} against {omitted}"
        );

        outcomes.push((operation, answered));
    }

    let (first, reference) = &outcomes[0];
    for (operation, answered) in &outcomes[1..] {
        assert_eq!(
            answered,
            reference,
            "case {} against {}: every associate must answer each spelling of the optional \
             link member alike, got {answered:?} against {reference:?} over {SPELLINGS:?}",
            operation.name(),
            first.name()
        );
    }
}

#[tokio::test]
async fn every_associate_refuses_a_link_that_resolves_nothing_alike_and_stores_nothing() {
    let emulator = Emulator::new();
    let mut fixture = fixture(&emulator).await;

    // The other half of the rule: a NON-EMPTY value is a reference, so it must name a link of
    // the addressed global network that the named device is associated with. The rows span
    // the ways such a value fails — never stored, stored under another global network, blank
    // (which is a value and not the absence of one), and a link of this global network the
    // device is not associated with — and the three operations must fail each one alike.
    let rows: Vec<(&str, String, u16, &str)> = vec![
        (
            "a link id nothing was stored under",
            UNSTORED_LINK.to_string(),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a link of another global network",
            fixture.foreign_link.clone(),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a blank link id, which is a value and not the absence of one",
            " ".to_string(),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a link of this global network the device is not associated with",
            fixture.unassociated_link.clone(),
            400,
            "ValidationException",
        ),
    ];

    for (row, link, expected_status, expected_code) in rows {
        for operation in ASSOCIATES {
            let case = format!("{} with {row}", operation.name());
            let subject = next_subject(&mut fixture, &emulator, operation).await;
            let body = request(operation, &fixture, &subject, Some(&link));
            let (status, answer) = associate(&emulator, operation, &fixture.network, body.clone())
                .await;
            assert_eq!(
                status, expected_status,
                "case {case:?}: {body} expected {expected_status}, got {status} with {answer}"
            );
            assert_eq!(
                error_code(&answer),
                expected_code,
                "case {case:?}: expected {expected_code}, got {answer}"
            );

            let stored = read_back(&emulator, &case, operation, &fixture.network, &subject).await;
            assert!(
                stored.is_none(),
                "case {case:?}: a refused associate must store nothing, got {stored:?}"
            );
        }
    }
}
