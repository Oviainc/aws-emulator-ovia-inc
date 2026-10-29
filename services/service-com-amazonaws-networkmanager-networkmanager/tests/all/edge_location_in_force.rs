// SPDX-License-Identifier: CC0-1.0

//! The ONE rule every operation of this service that places a record at a core network EDGE
//! follows, driven end to end over HTTP through the real router (the harness lives in
//! `crate::global_network_lifecycle`, the core-network-with-edges helper in
//! `crate::attachment_lifecycle`).
//!
//! The rule under test: a core network's edges are the ones the policy IN FORCE on it
//! configures, so a create that places a record at an edge judges that location against the
//! document in force AT THE MOMENT OF THE CALL — answering the modelled `ValidationException`
//! (HTTP 400) naming the member that asked when the live document configures no edge there
//! — whether the request named the location, the Region of an ARN it names implies it, or
//! the handler takes it from a resource created earlier. Executing another policy version is
//! what moves which edges those are, in both directions: a location the version in force
//! drops stops being attachable even for a resource created while it was configured, and one
//! that version adds becomes attachable at once.
//!
//! The tables below span that class: one row per create that places a record at a single
//! edge — seven of them — differing in where the handler takes the location from (a request
//! member, the Region of a VPC / VPN / transit gateway ARN, a list the request carries, a
//! stored peering, a stored VPC attachment, a stored Connect attachment), in the member each
//! refusal names, and in whether the record it would mint is an attachment, a peering or a
//! Connect peer. Every row is asked at BOTH edges of the same core network — the one the
//! executed version dropped and the one it kept — so what the refusal is about is the policy
//! in force and not the shape of the request. Nothing reaches into `crate::state`: every
//! resource is created and read back through an operation.

use serde_json::{json, Value};

use crate::attachment_lifecycle::{
    attachment_id, core_network_at, create_connect_attachment, create_vpc_attachment,
    policy_document, subnet_arn, vpc_arn,
};
use crate::direct_connect_gateway_attachment_lifecycle::gateway_arn;
use crate::global_network_lifecycle::{error_code, Emulator, PAST_SETTLE};
use crate::site_to_site_vpn_attachment_lifecycle::vpn_arn;
use crate::transit_gateway_peering::{create_peering, own_account, peering_id, transit_gateway_arn};

/// The Region whose edge the core network under test configures while its prerequisites are
/// built and no longer configures once the next policy version is in force.
const DROPPED: &str = "us-east-1";

/// The Region whose edge every policy version of that core network configures, so the same
/// create at this edge is the row that must still be accepted.
const KEPT: &str = "eu-west-1";

/// A Region no version configures until one that does is executed: the other direction of
/// the rule.
const ADDED: &str = "ap-southeast-2";

/// The `Reason` a `ValidationException` carries when the member satisfies every modelled
/// constraint and it is the stored policy that configures no such edge (`smithy_query
/// shape=ValidationExceptionReason`).
const REASON_OTHER: &str = "Other";

/// A transit gateway route table ARN in `region`.
///
/// `TransitGatewayRouteTableArn` carries only a length and `^[\s\S]*$`, so the form is the
/// one its own documentation spells — `arn:aws:ec2:us-west-2:123456789012:
/// transit-gateway-route-table/tgw-rtb-9876543210123456` (`smithy_query query="doc"
/// shape="TransitGatewayRouteTableAttachment" member="TransitGatewayRouteTableArn"`).
fn route_table_arn(region: &str, index: usize) -> String {
    format!(
        "arn:aws:ec2:{region}:{}:transit-gateway-route-table/tgw-rtb-{index:08x}",
        own_account()
    )
}

/// A distinct appliance address per Connect peer, out of the block reserved for
/// benchmarking (198.18.0.0/15), so no two peers of one attachment are asked for at the
/// same address.
fn peer_address(index: usize) -> String {
    format!("198.18.{}.{}", index / 256, index % 256)
}

/// One create under test: the request that places a record at one edge, and the member its
/// refusal must name.
struct Row {
    /// Names the case in every assertion message: the operation and where it takes the
    /// edge location from.
    name: String,
    /// The route the create is addressed to.
    uri: &'static str,
    /// The request body.
    request: Value,
    /// The input member a refusal must blame: the one carrying the location, or the
    /// reference the handler read it from.
    member: &'static str,
}

/// The creates whose edge location comes from the REQUEST — a member of it, the Region of an
/// ARN it names, or a list it carries — so they need nothing built at that edge beforehand.
///
/// `index` offsets every resource ARN, so two calls of this never name the same VPC,
/// gateway or VPN connection.
fn request_rows(core_network: &str, region: &str, index: usize) -> Vec<Row> {
    vec![
        Row {
            name: format!("CreateVpcAttachment at {region}, the Region of the VPC it names"),
            uri: "/vpc-attachments",
            request: json!({
                "CoreNetworkId": core_network,
                "VpcArn": vpc_arn(region, index),
                "SubnetArns": [subnet_arn(region, index)],
            }),
            member: "vpcArn",
        },
        Row {
            name: format!(
                "CreateSiteToSiteVpnAttachment at {region}, the Region of the VPN connection"
            ),
            uri: "/site-to-site-vpn-attachments",
            request: json!({
                "CoreNetworkId": core_network,
                "VpnConnectionArn": vpn_arn("aws", region, &own_account(), &format!("{index:08x}")),
            }),
            member: "vpnConnectionArn",
        },
        Row {
            name: format!(
                "CreateDirectConnectGatewayAttachment at {region}, the list the request carries"
            ),
            uri: "/direct-connect-gateway-attachments",
            request: json!({
                "CoreNetworkId": core_network,
                "DirectConnectGatewayArn": gateway_arn(index),
                "EdgeLocations": [region],
            }),
            member: "edgeLocations",
        },
        Row {
            name: format!(
                "CreateTransitGatewayPeering at {region}, the Region of the gateway it peers"
            ),
            uri: "/transit-gateway-peerings",
            request: json!({
                "CoreNetworkId": core_network,
                "TransitGatewayArn": transit_gateway_arn("aws", region, &own_account(), index, 8),
            }),
            member: "transitGatewayArn",
        },
    ]
}

/// The creates that INHERIT their edge location from a resource created earlier: the ones
/// whose location was judged against the document in force back then and nowhere else.
fn inherited_rows(core_network: &str, at: &AtEdge, index: usize) -> Vec<Row> {
    vec![
        Row {
            name: format!(
                "CreateConnectAttachment at {}, the edge of the transport it is carried over",
                at.region
            ),
            uri: "/connect-attachments",
            request: json!({
                "CoreNetworkId": core_network,
                "EdgeLocation": at.region,
                "TransportAttachmentId": at.transport,
                "Options": { "Protocol": "GRE" },
            }),
            member: "edgeLocation",
        },
        Row {
            name: format!(
                "CreateConnectPeer at {}, the edge of the Connect attachment it peers on",
                at.region
            ),
            uri: "/connect-peers",
            request: json!({
                "ConnectAttachmentId": at.connect_attachment,
                "PeerAddress": peer_address(index),
            }),
            member: "connectAttachmentId",
        },
        Row {
            name: format!(
                "CreateTransitGatewayRouteTableAttachment at {}, the edge of the peering it is \
                 created over",
                at.region
            ),
            uri: "/transit-gateway-route-table-attachments",
            request: json!({
                "PeeringId": at.peering,
                "TransitGatewayRouteTableArn": route_table_arn(at.region, index),
            }),
            member: "peeringId",
        },
    ]
}

/// What the inheriting creates hang off at one edge: resources created while the policy in
/// force still configured that edge.
struct AtEdge {
    /// The Region these resources sit in, which is the edge location each carries.
    region: &'static str,
    /// A peering a transit gateway route table attachment is created over.
    peering: String,
    /// A VPC attachment a Connect attachment is carried over.
    transport: String,
    /// A Connect attachment a Connect peer is created on.
    connect_attachment: String,
}

/// Build the prerequisites at `region` on `core_network`, which the policy in force must
/// configure an edge for right now. `index` offsets every ARN they name.
async fn resources_at(
    emulator: &Emulator,
    core_network: &str,
    region: &'static str,
    index: usize,
) -> AtEdge {
    let (status, created) = create_vpc_attachment(
        emulator,
        json!({
            "CoreNetworkId": core_network,
            "VpcArn": vpc_arn(region, index),
            "SubnetArns": [subnet_arn(region, index)],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the transport at {region}: CreateVpcAttachment expected 200, got {status} with {created}"
    );
    let transport = attachment_id(region, &created["VpcAttachment"]);

    let (status, created) = create_connect_attachment(
        emulator,
        json!({
            "CoreNetworkId": core_network,
            "EdgeLocation": region,
            "TransportAttachmentId": &transport,
            "Options": { "Protocol": "GRE" },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the Connect attachment at {region}: expected 200, got {status} with {created}"
    );
    let connect_attachment = attachment_id(region, &created["ConnectAttachment"]);

    let (status, created) = create_peering(
        emulator,
        json!({
            "CoreNetworkId": core_network,
            "TransitGatewayArn": transit_gateway_arn("aws", region, &own_account(), index, 8),
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the peering at {region}: CreateTransitGatewayPeering expected 200, got {status} with {created}"
    );
    let peering = peering_id(region, &created["TransitGatewayPeering"]);

    AtEdge {
        region,
        peering,
        transport,
        connect_attachment,
    }
}

/// `PutCoreNetworkPolicy` of a document configuring an edge in each of `locations`, then
/// `ExecuteCoreNetworkChangeSet` of the version it minted, so that document is the one IN
/// FORCE when this returns.
async fn put_in_force(emulator: &Emulator, core_network: &str, locations: &[&str]) {
    let (status, submitted) = emulator
        .post(
            &format!("/core-networks/{core_network}/core-network-policy"),
            json!({ "PolicyDocument": policy_document(locations) }),
        )
        .await;
    assert_eq!(
        status, 200,
        "PutCoreNetworkPolicy({locations:?}): expected 200, got {status} with {submitted}"
    );
    let version = submitted["CoreNetworkPolicy"]["PolicyVersionId"]
        .as_i64()
        .unwrap_or_else(|| panic!("no PolicyVersionId in {submitted}"));
    // A put only PREPARES the change set; it is READY_TO_EXECUTE once the span its
    // generation owes has fallen due.
    tokio::time::sleep(PAST_SETTLE).await;
    let (status, executed) = emulator
        .post(
            &format!(
                "/core-networks/{core_network}/core-network-change-sets/{version}/execute"
            ),
            json!({}),
        )
        .await;
    assert_eq!(
        status, 200,
        "ExecuteCoreNetworkChangeSet({version}): expected 200, got {status} with {executed}"
    );
    // The execution owes the core network a settle span of its own.
    tokio::time::sleep(PAST_SETTLE).await;
}

/// The edge locations `GetCoreNetwork` reports for `core_network`, which is what the policy
/// in force configures.
async fn reported_edges(emulator: &Emulator, core_network: &str) -> Vec<String> {
    let (status, body) = emulator
        .get(&format!("/core-networks/{core_network}"))
        .await;
    assert_eq!(
        status, 200,
        "GetCoreNetwork: expected 200, got {status} with {body}"
    );
    body["CoreNetwork"]["Edges"]
        .as_array()
        .unwrap_or_else(|| panic!("no CoreNetwork.Edges array in {body}"))
        .iter()
        .map(|edge| {
            edge["EdgeLocation"]
                .as_str()
                .unwrap_or_else(|| panic!("no EdgeLocation in {edge}"))
                .to_string()
        })
        .collect()
}

/// Assert `row` is refused as a create at an edge `core_network` configures none at, and
/// that the refusal stores nothing: the identical call answers the same way a second time.
async fn assert_no_such_edge(emulator: &Emulator, row: &Row, core_network: &str, region: &str) {
    for attempt in 1..=2 {
        let (status, refused) = emulator.post(row.uri, row.request.clone()).await;
        assert_eq!(
            status, 400,
            "case {:?} (attempt {attempt}): expected 400, got {status} with {refused}",
            row.name
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {:?} (attempt {attempt}): wrong error code in {refused}",
            row.name
        );
        assert_eq!(
            refused["Fields"][0]["Name"],
            json!(row.member),
            "case {:?} (attempt {attempt}): the refusal must name {}, got {refused}",
            row.name,
            row.member
        );
        assert_eq!(
            refused["Reason"],
            json!(REASON_OTHER),
            "case {:?} (attempt {attempt}): the member satisfies every modelled constraint, so \
             the reason is {REASON_OTHER}, got {refused}",
            row.name
        );
        let message = refused["Fields"][0]["Message"].as_str().unwrap_or_default();
        assert!(
            message.contains(core_network) && message.contains(region),
            "case {:?} (attempt {attempt}): the refusal must name the core network and the \
             location it configures no edge in, got {refused}",
            row.name
        );
    }
}

/// Assert `row` is accepted, answering the response body.
async fn assert_accepted(emulator: &Emulator, row: &Row) -> Value {
    let (status, created) = emulator.post(row.uri, row.request.clone()).await;
    assert_eq!(
        status, 200,
        "case {:?}: expected 200, got {status} with {created}",
        row.name
    );
    created
}

#[tokio::test]
async fn every_create_judges_its_edge_location_against_the_policy_in_force() {
    let emulator = Emulator::new();
    let (_, core_network) = core_network_at(&emulator, "edge in force", &[DROPPED, KEPT]).await;

    // Both edges are configured right now, so the resources the inheriting creates hang off
    // are built at each of them.
    let dropped = resources_at(&emulator, &core_network, DROPPED, 0x11).await;
    let kept = resources_at(&emulator, &core_network, KEPT, 0x21).await;

    // The policy put in force now configures only one of the two edges. Nothing removes the
    // records already sitting at the other one — their creates were judged against the
    // document in force back then — so every row below asks for a NEW record at an edge this
    // core network has stopped having.
    put_in_force(&emulator, &core_network, &[KEPT]).await;
    assert_eq!(
        reported_edges(&emulator, &core_network).await,
        vec![KEPT.to_string()],
        "the executed version must leave {KEPT} as the only edge"
    );

    let mut refused = request_rows(&core_network, DROPPED, 0x31);
    refused.extend(inherited_rows(&core_network, &dropped, 0x31));
    for row in &refused {
        assert_no_such_edge(&emulator, row, &core_network, DROPPED).await;
    }

    // The same seven creates at the edge the executed version KEPT are accepted, so what the
    // refusals above are about is the policy in force rather than the request: each row
    // differs from its refused twin only in the edge location it lands on.
    let mut accepted = request_rows(&core_network, KEPT, 0x41);
    accepted.extend(inherited_rows(&core_network, &kept, 0x41));
    for row in &accepted {
        assert_accepted(&emulator, row).await;
    }

    // The accepted transit gateway route table attachment — the create that takes its edge
    // from a stored peering — reads back through its own reader at the edge the policy in
    // force configures, so the rule refuses nothing it should store.
    let row = inherited_rows(&core_network, &kept, 0x51)
        .pop()
        .expect("inherited_rows ends with the route table attachment row");
    let created = assert_accepted(&emulator, &row).await;
    let attachment = attachment_id(&row.name, &created["TransitGatewayRouteTableAttachment"]);
    let (status, read) = emulator
        .get(&format!(
            "/transit-gateway-route-table-attachments/{attachment}"
        ))
        .await;
    assert_eq!(
        status, 200,
        "case {:?}: GetTransitGatewayRouteTableAttachment expected 200, got {status} with {read}",
        row.name
    );
    assert_eq!(
        read["TransitGatewayRouteTableAttachment"]["Attachment"]["EdgeLocation"],
        json!(KEPT),
        "case {:?}: the stored attachment must sit at an edge the policy in force configures, \
         got {read}",
        row.name
    );
}

#[tokio::test]
async fn an_edge_a_later_policy_version_adds_is_attachable_at_once() {
    let emulator = Emulator::new();
    let (_, core_network) = core_network_at(&emulator, "edge added", &[KEPT]).await;

    // The other direction of the same rule: while the document in force configures no edge
    // at this location every create that would place a record there is refused — the rows
    // whose location comes from the request alone, since nothing can be built at an edge
    // that does not exist.
    for row in &request_rows(&core_network, ADDED, 0x61) {
        assert_no_such_edge(&emulator, row, &core_network, ADDED).await;
    }

    put_in_force(&emulator, &core_network, &[KEPT, ADDED]).await;
    let mut edges = reported_edges(&emulator, &core_network).await;
    edges.sort();
    let mut expected = vec![ADDED.to_string(), KEPT.to_string()];
    expected.sort();
    assert_eq!(
        edges, expected,
        "the executed version must add {ADDED} to the edges reported"
    );

    // The very requests refused a moment ago are accepted now, with nothing about them
    // changed: the location is judged against the document in force at the moment of the
    // call.
    for row in &request_rows(&core_network, ADDED, 0x61) {
        assert_accepted(&emulator, row).await;
    }
}
