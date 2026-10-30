// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `CreateTransitGatewayRouteTableAnnouncement`, driven over the wire
//! this service speaks (`ec2Query`), with `DescribeTransitGatewayRouteTableAnnouncements`
//! reading back what the create stored (README.md / OPERATIONS.md "Wire protocol").
//!
//! The rule under test is the one that operation follows: "Advertises a new transit gateway
//! route table" — one call mints an announcement of the route table it names across the
//! peering attachment it names, and every fact the announcement reports about the two sides
//! is read off the STORED route table and the STORED attachment rather than echoed from the
//! request: the announcing gateway is the one the announced route table belongs to, the peer
//! is the OTHER transit gateway of that attachment, each side's core network is the one the
//! attachment records for it, the direction is the one the announcing side makes, and the
//! announcement is handed back in the TRANSIENT state of a create with the clock owing it the
//! state an announcement in force is in. Every id the request names is resolved before
//! anything is written, so a value that is not an id of its kind, an id nothing holds, an
//! attachment of another kind, an attachment that is not in service and a route table of a
//! gateway that attachment does not peer are each refused and leave the store empty.
//!
//! Each table row is a distinct input along what that rule generalises over: which SIDE of
//! the peering attachment owns the announced route table (so which side is the peer), whether
//! the attachment records a core network for its sides at all, how many tags the request
//! applies, and — for the refusals — which id of the request is the unusable one, how it is
//! unusable, and which refusal wins when more than one applies.
//!
//! The peering attachment each case announces over is seeded through the store's own writer
//! (`insert_transit_gateway_attachment`), as every transit gateway suite in this crate does,
//! because `CreateTransitGatewayPeeringAttachment` is still a scaffold stub.

use std::time::Duration;

use ec2::ops::types::{
    PeeringTgwInfo, TRANSIT_GATEWAY_ROUTE_TABLE_ANNOUNCEMENT_DIRECTION_VALUES,
    TRANSIT_GATEWAY_ROUTE_TABLE_ANNOUNCEMENT_STATE_VALUES, TransitGatewayPeeringAttachment,
    TransitGatewayVpcAttachment,
};
use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, DRY_RUN_OPERATION, INCORRECT_STATE,
    INVALID_PARAMETER_COMBINATION, INVALID_PARAMETER_VALUE, INVALID_ROUTE_TABLE_ID_MALFORMED,
    INVALID_ROUTE_TABLE_ID_NOT_FOUND, INVALID_TRANSIT_GATEWAY_ATTACHMENT_ID_MALFORMED,
    INVALID_TRANSIT_GATEWAY_ATTACHMENT_ID_NOT_FOUND,
    TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE,
    TRANSIT_GATEWAY_ATTACHMENT_STATE_PENDING_ACCEPTANCE,
    TRANSIT_GATEWAY_ROUTE_TABLE_ANNOUNCEMENT_ID_PREFIX, TRANSITION_DELAY,
    TransitGatewayAttachmentView,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, elements, error_code, error_message, indexed};
use super::wire::{Param, param};

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// The ec2Query `Action` under test.
const ACTION: &str = "CreateTransitGatewayRouteTableAnnouncement";

/// The describe that reads the announcement back.
const DESCRIBE: &str = "DescribeTransitGatewayRouteTableAnnouncements";

/// The HTTP status EC2 answers `DryRunOperation` with.
const DRY_RUN_STATUS: u16 = 412;

/// The Region the gateway on the far side of the seeded peering attachment lives in — a
/// peering attachment joins two gateways, which is why the peer is not in [`REGION`].
const PEER_REGION: &str = "us-west-2";

/// The transit gateway on the far side of the seeded peering attachment.
const PEER_GATEWAY: &str = "tgw-00000000000000aaa";

/// Another gateway again, for the attachment that peers two gateways the announced route
/// table belongs to NEITHER of.
const FOREIGN_GATEWAY: &str = "tgw-00000000000000bbb";

/// The core network the announcing side of the attachment belongs to, when it names one.
const LOCAL_CORE_NETWORK: &str = "core-network-0123456789abcdef0";

/// The core network the peer side belongs to, when it names one.
const PEER_CORE_NETWORK: &str = "core-network-00000000000000aaa";

/// Long enough past [`TRANSITION_DELAY`] that a move the clock owes is certainly due.
fn past_the_transition() -> Duration {
    TRANSITION_DELAY + Duration::from_millis(250)
}

/// A member of `TransitGatewayRouteTableAnnouncementState`, from the modelled catalog
/// (`smithy_query(query="shape",shape="TransitGatewayRouteTableAnnouncementState")`, whose
/// values are `available | pending | failing | failed | deleting | deleted`) rather than
/// written out here.
fn announcement_state(name: &str) -> &'static str {
    TRANSIT_GATEWAY_ROUTE_TABLE_ANNOUNCEMENT_STATE_VALUES
        .iter()
        .find(|value| **value == name)
        .unwrap_or_else(|| {
            panic!("{name:?} is no TransitGatewayRouteTableAnnouncementState member")
        })
}

/// A member of `TransitGatewayRouteTableAnnouncementDirection`, from the modelled catalog
/// (`smithy_query(query="shape",shape="TransitGatewayRouteTableAnnouncementDirection")`,
/// whose values are `outgoing | incoming`) rather than written out here.
fn announcement_direction(name: &str) -> &'static str {
    TRANSIT_GATEWAY_ROUTE_TABLE_ANNOUNCEMENT_DIRECTION_VALUES
        .iter()
        .find(|value| **value == name)
        .unwrap_or_else(|| {
            panic!("{name:?} is no TransitGatewayRouteTableAnnouncementDirection member")
        })
}

/// One call of `action` signed for the Region above.
async fn invoke(router: &MultiServiceRouter, action: &str, params: &[Param]) -> (u16, String) {
    super::ec2_query::call(router, REGION, action, params).await
}

/// A transit gateway created through the wire, as its `TransitGatewayId` and the id of the
/// route table it associates by default ("we create a default transit gateway route table" —
/// `CreateTransitGateway`).
async fn transit_gateway(router: &MultiServiceRouter) -> (String, String) {
    let (status, body) = invoke(router, "CreateTransitGateway", &[]).await;
    assert_eq!(
        status, 200,
        "CreateTransitGateway must succeed, got {status} with body {body}"
    );
    let gateway_id = element(&body, "transitGatewayId")
        .expect("a created transit gateway reports its own id");
    let route_table_id = element(&body, "associationDefaultRouteTableId")
        .expect("a gateway created with the documented defaults has an association route table");
    (gateway_id, route_table_id)
}

/// One side of a peering attachment, as the attachment records it.
fn side(gateway_id: &str, core_network: Option<&str>, region: &str) -> PeeringTgwInfo {
    PeeringTgwInfo {
        transit_gateway_id: Some(gateway_id.to_string()),
        core_network_id: core_network.map(str::to_string),
        owner_id: Some("210987654321".to_string()),
        region: Some(region.to_string()),
    }
}

/// Store a peering attachment joining `requester` and `accepter` in `state`, and answer its
/// `TransitGatewayAttachmentId`.
fn seed_peering(
    engine: &engine::InMemoryEngine,
    requester: PeeringTgwInfo,
    accepter: PeeringTgwInfo,
    state: &str,
) -> String {
    engine
        .state::<Ec2State>()
        .lock()
        .insert_transit_gateway_attachment(TransitGatewayAttachmentView::Peering(
            TransitGatewayPeeringAttachment {
                requester_tgw_info: Some(requester),
                accepter_tgw_info: Some(accepter),
                state: Some(state.to_string()),
                ..Default::default()
            },
        ))
        .expect("the store holds the seeded peering attachment")
}

/// Store a VPC attachment of `gateway_id` and answer its `TransitGatewayAttachmentId` — an
/// attachment of a kind no route table announcement can be made over.
fn seed_vpc_attachment(engine: &engine::InMemoryEngine, gateway_id: &str) -> String {
    engine
        .state::<Ec2State>()
        .lock()
        .insert_transit_gateway_attachment(TransitGatewayAttachmentView::Vpc(
            TransitGatewayVpcAttachment {
                transit_gateway_id: Some(gateway_id.to_string()),
                vpc_id: Some("vpc-0123456789abcdef0".to_string()),
                vpc_owner_id: Some("210987654321".to_string()),
                state: Some(TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE.to_string()),
                ..Default::default()
            },
        ))
        .expect("the store holds the seeded VPC attachment")
}

/// Every announcement id the store holds, in id order.
fn stored_ids(engine: &engine::InMemoryEngine) -> Vec<String> {
    engine
        .state::<Ec2State>()
        .lock()
        .transit_gateway_route_table_announcements
        .keys()
        .cloned()
        .collect()
}

/// The state the store holds for one announcement, or `None` when it holds none.
fn stored_state(engine: &engine::InMemoryEngine, announcement_id: &str) -> Option<String> {
    engine
        .state::<Ec2State>()
        .lock()
        .transit_gateway_route_table_announcements
        .get(announcement_id)
        .and_then(|record| record.view.state.clone())
}

/// The `TagSpecification` form fields that tag the announcement one call creates, naming
/// `resource_type`.
///
/// `CreateTransitGatewayRouteTableAnnouncementRequest$TagSpecifications` is bound to the
/// `TagSpecification` xmlName
/// (`smithy_query(query="operation",operation="CreateTransitGatewayRouteTableAnnouncement")`),
/// so that — not the member name — is the field prefix.
fn tag_params(resource_type: &str, tags: &[(&str, &str)]) -> Vec<Param> {
    let mut params = vec![param("TagSpecification.1.ResourceType", resource_type)];
    for (index, (key, value)) in tags.iter().enumerate() {
        let position = index + 1;
        params.push(param(&format!("TagSpecification.1.Tag.{position}.Key"), key));
        params.push(param(
            &format!("TagSpecification.1.Tag.{position}.Value"),
            value,
        ));
    }
    params
}

/// Assert that the response reports `name` with exactly `expected` — `None` asserting the
/// member is not reported at all.
fn assert_member(case: &str, body: &str, name: &str, expected: Option<&str>) {
    let actual = element(body, name);
    assert_eq!(
        actual.as_deref(),
        expected,
        "case {case:?}: expected {name} = {expected:?}, got {actual:?} in {body}"
    );
}

/// One happy-path row: which side of the peering owns the announced route table, whether the
/// attachment records core networks, and the tags the request applies.
struct Case {
    /// The case name, as every assertion names it.
    name: &'static str,
    /// Whether the announced route table's gateway is the attachment's REQUESTER (`true`) or
    /// its ACCEPTER (`false`) — the peer is the other one either way.
    announcing_requester: bool,
    /// Whether each side of the attachment records a core network.
    core_networks: bool,
    /// The tags the request applies, as (key, value) pairs; empty applies none.
    tags: &'static [(&'static str, &'static str)],
}

/// The happy-path rows: both sides an announcement can be made from, with and without the
/// core networks a side may record, and none, one and several tags.
fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "the requester announces, no core networks, untagged",
            announcing_requester: true,
            core_networks: false,
            tags: &[],
        },
        Case {
            name: "the accepter announces, no core networks, one tag",
            announcing_requester: false,
            core_networks: false,
            tags: &[("Name", "east-to-west")],
        },
        Case {
            name: "the requester announces, both core networks, two tags",
            announcing_requester: true,
            core_networks: true,
            tags: &[("Name", "east-to-west"), ("Team", "network")],
        },
        Case {
            name: "the accepter announces, both core networks, untagged",
            announcing_requester: false,
            core_networks: true,
            tags: &[],
        },
    ]
}

/// One call mints an announcement of the named route table across the named peering
/// attachment, reporting both gateways as the STORED attachment records them, in the
/// transient state of a create with `available` owed to the clock — and the describe reads
/// back exactly what was stored.
#[tokio::test]
async fn one_call_announces_the_named_route_table_across_the_named_peering() {
    for case in cases() {
        let name = case.name;
        let (router, engine) = super::ec2_query::emulator();
        let (gateway_id, route_table_id) = transit_gateway(&router).await;
        let local_core = case.core_networks.then_some(LOCAL_CORE_NETWORK);
        let peer_core = case.core_networks.then_some(PEER_CORE_NETWORK);
        let announcing = side(&gateway_id, local_core, REGION);
        let peer = side(PEER_GATEWAY, peer_core, PEER_REGION);
        let attachment_id = if case.announcing_requester {
            seed_peering(
                &engine,
                announcing,
                peer,
                TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE,
            )
        } else {
            seed_peering(
                &engine,
                peer,
                announcing,
                TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE,
            )
        };

        let mut params = vec![
            param("TransitGatewayRouteTableId", &route_table_id),
            param("PeeringAttachmentId", &attachment_id),
        ];
        if !case.tags.is_empty() {
            params.extend(tag_params(
                "transit-gateway-route-table-announcement",
                case.tags,
            ));
        }
        let (status, body) = invoke(&router, ACTION, &params).await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected HTTP 200 from {ACTION}, got {status} with body {body}"
        );

        let announcement_id = element(&body, "transitGatewayRouteTableAnnouncementId")
            .unwrap_or_else(|| {
                panic!("case {name:?}: the create must report the announcement id, got {body}")
            });
        assert!(
            announcement_id.starts_with(&format!("{TRANSIT_GATEWAY_ROUTE_TABLE_ANNOUNCEMENT_ID_PREFIX}-")),
            "case {name:?}: expected an id the service minted under its own prefix, got \
             {announcement_id:?}"
        );
        assert_member(name, &body, "transitGatewayId", Some(&gateway_id));
        assert_member(name, &body, "peerTransitGatewayId", Some(PEER_GATEWAY));
        assert_member(name, &body, "coreNetworkId", local_core);
        assert_member(name, &body, "peerCoreNetworkId", peer_core);
        assert_member(name, &body, "peeringAttachmentId", Some(&attachment_id));
        assert_member(
            name,
            &body,
            "transitGatewayRouteTableId",
            Some(&route_table_id),
        );
        // The direction an announcement made BY the gateway that owns the announced table
        // carries, whichever side of the attachment that gateway is.
        assert_member(
            name,
            &body,
            "announcementDirection",
            Some(announcement_direction("outgoing")),
        );
        assert_member(name, &body, "state", Some(announcement_state("pending")));
        assert!(
            element(&body, "creationTime").is_some(),
            "case {name:?}: the create must report when the announcement was created, got {body}"
        );
        assert_eq!(
            elements(&body, "key"),
            case.tags
                .iter()
                .map(|(key, _)| (*key).to_string())
                .collect::<Vec<_>>(),
            "case {name:?}: expected the tag keys the request applied, got {body}"
        );
        assert_eq!(
            elements(&body, "value"),
            case.tags
                .iter()
                .map(|(_, value)| (*value).to_string())
                .collect::<Vec<_>>(),
            "case {name:?}: expected the tag values the request applied, got {body}"
        );
        assert_eq!(
            stored_state(&engine, &announcement_id).as_deref(),
            Some(announcement_state("pending")),
            "case {name:?}: the store must hold the announcement in the state the response \
             reported"
        );

        // The move the clock owes a create: the announcement is in force once the span has
        // passed, whether or not anybody called again.
        tokio::time::sleep(past_the_transition()).await;
        let (status, read_back) = invoke(
            &router,
            DESCRIBE,
            &indexed(
                "TransitGatewayRouteTableAnnouncementIds",
                &[announcement_id.as_str()],
            ),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected HTTP 200 from {DESCRIBE}, got {status} with body \
             {read_back}"
        );
        assert_eq!(
            elements(&read_back, "transitGatewayRouteTableAnnouncementId"),
            vec![announcement_id.clone()],
            "case {name:?}: the describe must report the one announcement the create stored, \
             got {read_back}"
        );
        assert_member(name, &read_back, "transitGatewayId", Some(&gateway_id));
        assert_member(name, &read_back, "peerTransitGatewayId", Some(PEER_GATEWAY));
        assert_member(name, &read_back, "coreNetworkId", local_core);
        assert_member(name, &read_back, "peerCoreNetworkId", peer_core);
        assert_member(name, &read_back, "peeringAttachmentId", Some(&attachment_id));
        assert_member(
            name,
            &read_back,
            "transitGatewayRouteTableId",
            Some(&route_table_id),
        );
        assert_member(
            name,
            &read_back,
            "announcementDirection",
            Some(announcement_direction("outgoing")),
        );
        assert_member(
            name,
            &read_back,
            "state",
            Some(announcement_state("available")),
        );
    }
}

/// Two calls over the same pair mint two announcements with DIFFERENT ids, each stored: the
/// create is a write, so a second one is not a report of the first.
#[tokio::test]
async fn each_call_mints_its_own_announcement() {
    let case = "two creates over one pair";
    let (router, engine) = super::ec2_query::emulator();
    let (gateway_id, route_table_id) = transit_gateway(&router).await;
    let attachment_id = seed_peering(
        &engine,
        side(&gateway_id, None, REGION),
        side(PEER_GATEWAY, None, PEER_REGION),
        TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE,
    );
    let params = vec![
        param("TransitGatewayRouteTableId", &route_table_id),
        param("PeeringAttachmentId", &attachment_id),
    ];

    let mut minted = Vec::new();
    for attempt in 1..=2 {
        let (status, body) = invoke(&router, ACTION, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: call {attempt} must succeed, got {status} with body {body}"
        );
        minted.push(
            element(&body, "transitGatewayRouteTableAnnouncementId").unwrap_or_else(|| {
                panic!("case {case:?}: call {attempt} must report an announcement id, got {body}")
            }),
        );
    }
    assert_ne!(
        minted[0], minted[1],
        "case {case:?}: expected two distinct announcement ids, got {minted:?}"
    );
    let mut held = stored_ids(&engine);
    held.sort();
    let mut expected = minted.clone();
    expected.sort();
    assert_eq!(
        held, expected,
        "case {case:?}: the store must hold both announcements, got {held:?}"
    );
}

/// What one refusal row sends as the route table id.
enum Table {
    /// The default route table of the gateway the case created.
    Created,
    /// A literal value the store cannot resolve, or cannot read as an id at all.
    Literal(&'static str),
}

/// What one refusal row sends as the peering attachment id.
enum Attachment {
    /// A peering attachment in service, joining the created gateway and the peer.
    Available,
    /// The same attachment, still waiting for the peer to accept it.
    PendingAcceptance,
    /// An attachment of the created gateway that is not a peering attachment.
    OtherKind,
    /// A peering attachment joining two gateways the announced route table belongs to
    /// neither of.
    Foreign,
    /// A literal value the store cannot resolve, or cannot read as an id at all.
    Literal(&'static str),
}

/// One refusal row: what the request names, what else it carries, and what it is answered.
struct Refusal {
    /// The case name, as every assertion names it.
    name: &'static str,
    /// The route table id the request names.
    table: Table,
    /// The peering attachment id the request names.
    attachment: Attachment,
    /// Any further form fields the row sends.
    extra: &'static [(&'static str, &'static str)],
    /// The HTTP status the refusal carries.
    status: u16,
    /// The error code it carries.
    code: &'static str,
    /// The message it carries, for the rows whose message is built from values the row
    /// itself names; `None` where the message quotes a service-minted id.
    message: Option<&'static str>,
}

/// The refusal rows: every way an id of this request can be unusable, the refusals that
/// depend on the state and the relation of the two resources, the tag specification outside
/// the type this action tags, the permission check — and, for the rows where two refusals
/// apply, which one wins.
fn refusals() -> Vec<Refusal> {
    vec![
        Refusal {
            name: "a route table id with too few hexadecimal digits",
            table: Table::Literal("tgw-rtb-0123456789abcde"),
            attachment: Attachment::Available,
            extra: &[],
            status: 400,
            code: INVALID_ROUTE_TABLE_ID_MALFORMED,
            message: Some("Invalid Transit Gateway Route Table id tgw-rtb-0123456789abcde."),
        },
        Refusal {
            name: "a route table id in upper case",
            table: Table::Literal("tgw-rtb-0123456789ABCDEF0"),
            attachment: Attachment::Available,
            extra: &[],
            status: 400,
            code: INVALID_ROUTE_TABLE_ID_MALFORMED,
            message: Some("Invalid Transit Gateway Route Table id tgw-rtb-0123456789ABCDEF0."),
        },
        Refusal {
            name: "a route table prefix with no suffix at all",
            table: Table::Literal("tgw-rtb"),
            attachment: Attachment::Available,
            extra: &[],
            status: 400,
            code: INVALID_ROUTE_TABLE_ID_MALFORMED,
            message: Some("Invalid Transit Gateway Route Table id tgw-rtb."),
        },
        Refusal {
            name: "an id of another resource kind",
            table: Table::Literal("vpc-0123456789abcdef0"),
            attachment: Attachment::Available,
            extra: &[],
            status: 400,
            code: INVALID_ROUTE_TABLE_ID_MALFORMED,
            message: Some("Invalid Transit Gateway Route Table id vpc-0123456789abcdef0."),
        },
        Refusal {
            // The announcement id space extends the route table prefix, so this is the row
            // that pins the suffix — not the prefix — as what tells the two kinds apart.
            name: "a route table ANNOUNCEMENT id in place of a route table id",
            table: Table::Literal("tgw-rtb-announce-0123456789abcdef0"),
            attachment: Attachment::Available,
            extra: &[],
            status: 400,
            code: INVALID_ROUTE_TABLE_ID_MALFORMED,
            message: Some(
                "Invalid Transit Gateway Route Table id tgw-rtb-announce-0123456789abcdef0.",
            ),
        },
        Refusal {
            name: "a well-formed route table id nothing holds",
            table: Table::Literal("tgw-rtb-00000000000000aaa"),
            attachment: Attachment::Available,
            extra: &[],
            status: 400,
            code: INVALID_ROUTE_TABLE_ID_NOT_FOUND,
            message: Some(
                "Transit Gateway Route Table tgw-rtb-00000000000000aaa was deleted or does not \
                 exist.",
            ),
        },
        Refusal {
            name: "a peering attachment id with too few hexadecimal digits",
            table: Table::Created,
            attachment: Attachment::Literal("tgw-attach-0123456789abcde"),
            extra: &[],
            status: 400,
            code: INVALID_TRANSIT_GATEWAY_ATTACHMENT_ID_MALFORMED,
            message: Some("Invalid Transit Gateway Attachment id."),
        },
        Refusal {
            name: "a well-formed peering attachment id nothing holds",
            table: Table::Created,
            attachment: Attachment::Literal("tgw-attach-00000000000000aaa"),
            extra: &[],
            status: 400,
            code: INVALID_TRANSIT_GATEWAY_ATTACHMENT_ID_NOT_FOUND,
            message: Some(
                "The transit-gateway-attachment ID 'tgw-attach-00000000000000aaa' does not exist",
            ),
        },
        Refusal {
            name: "an attachment that is not a peering attachment",
            table: Table::Created,
            attachment: Attachment::OtherKind,
            extra: &[],
            status: 400,
            code: INVALID_TRANSIT_GATEWAY_ATTACHMENT_ID_NOT_FOUND,
            message: None,
        },
        Refusal {
            name: "a peering attachment still waiting to be accepted",
            table: Table::Created,
            attachment: Attachment::PendingAcceptance,
            extra: &[],
            status: 400,
            code: INCORRECT_STATE,
            message: None,
        },
        Refusal {
            name: "a peering attachment that does not peer the route table's gateway",
            table: Table::Created,
            attachment: Attachment::Foreign,
            extra: &[],
            status: 400,
            code: INVALID_PARAMETER_COMBINATION,
            message: None,
        },
        Refusal {
            name: "a tag specification naming another resource type",
            table: Table::Created,
            attachment: Attachment::Available,
            extra: &[
                ("TagSpecification.1.ResourceType", "transit-gateway-route-table"),
                ("TagSpecification.1.Tag.1.Key", "Name"),
                ("TagSpecification.1.Tag.1.Value", "misdirected"),
            ],
            status: 400,
            code: INVALID_PARAMETER_VALUE,
            message: Some(
                "'transit-gateway-route-table' is not a valid resource type for tagging on \
                 CreateTransitGatewayRouteTableAnnouncement",
            ),
        },
        Refusal {
            name: "a dry run of a request that would otherwise succeed",
            table: Table::Created,
            attachment: Attachment::Available,
            extra: &[("DryRun", "true")],
            status: DRY_RUN_STATUS,
            code: DRY_RUN_OPERATION,
            message: None,
        },
        Refusal {
            // Both ids are unreadable: the route table is the resource this action
            // announces, so its refusal is the one the caller sees.
            name: "both ids malformed",
            table: Table::Literal("tgw-rtb-0123456789abcde"),
            attachment: Attachment::Literal("tgw-attach-0123456789abcde"),
            extra: &[],
            status: 400,
            code: INVALID_ROUTE_TABLE_ID_MALFORMED,
            message: Some("Invalid Transit Gateway Route Table id tgw-rtb-0123456789abcde."),
        },
        Refusal {
            // The form of the request is answered BEFORE the permission check, so a dry run
            // of a malformed request is still refused as malformed.
            name: "a dry run naming a malformed route table id",
            table: Table::Literal("tgw-rtb-0123456789abcde"),
            attachment: Attachment::Available,
            extra: &[("DryRun", "true")],
            status: 400,
            code: INVALID_ROUTE_TABLE_ID_MALFORMED,
            message: Some("Invalid Transit Gateway Route Table id tgw-rtb-0123456789abcde."),
        },
    ]
}

/// Every refusal above is answered with its own code and writes NOTHING, so a caller can
/// tell "not an id of this kind" from "no such resource" from "that resource cannot do this",
/// and a refused create leaves the announcement store empty.
#[tokio::test]
async fn a_request_it_cannot_serve_is_refused_and_writes_nothing() {
    for refusal in refusals() {
        let name = refusal.name;
        let (router, engine) = super::ec2_query::emulator();
        let (gateway_id, created_table) = transit_gateway(&router).await;
        let route_table_id = match refusal.table {
            Table::Created => created_table,
            Table::Literal(literal) => literal.to_string(),
        };
        let attachment_id = match refusal.attachment {
            Attachment::Available => seed_peering(
                &engine,
                side(&gateway_id, None, REGION),
                side(PEER_GATEWAY, None, PEER_REGION),
                TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE,
            ),
            Attachment::PendingAcceptance => seed_peering(
                &engine,
                side(&gateway_id, None, REGION),
                side(PEER_GATEWAY, None, PEER_REGION),
                TRANSIT_GATEWAY_ATTACHMENT_STATE_PENDING_ACCEPTANCE,
            ),
            Attachment::OtherKind => seed_vpc_attachment(&engine, &gateway_id),
            Attachment::Foreign => seed_peering(
                &engine,
                side(PEER_GATEWAY, None, PEER_REGION),
                side(FOREIGN_GATEWAY, None, PEER_REGION),
                TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE,
            ),
            Attachment::Literal(literal) => literal.to_string(),
        };

        let mut params = vec![
            param("TransitGatewayRouteTableId", &route_table_id),
            param("PeeringAttachmentId", &attachment_id),
        ];
        params.extend(
            refusal
                .extra
                .iter()
                .map(|(field, value)| param(field, value)),
        );
        let (status, body) = invoke(&router, ACTION, &params).await;
        assert_eq!(
            status, refusal.status,
            "case {name:?}: expected HTTP {expected}, got {status} with body {body}",
            expected = refusal.status
        );
        assert_eq!(
            error_code(&body),
            refusal.code,
            "case {name:?}: expected the code {code:?}, got {body}",
            code = refusal.code
        );
        if let Some(expected) = refusal.message {
            assert_eq!(
                error_message(&body),
                expected,
                "case {name:?}: expected the message {expected:?}, got {body}"
            );
        }
        assert!(
            stored_ids(&engine).is_empty(),
            "case {name:?}: a refused create must write nothing, store holds {held:?}",
            held = stored_ids(&engine)
        );
    }
}
