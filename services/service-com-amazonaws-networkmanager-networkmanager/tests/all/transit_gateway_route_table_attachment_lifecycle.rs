// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of the transit gateway route table attachment slice —
//! `CreateTransitGatewayPeering`, `GetTransitGatewayPeering`,
//! `CreateTransitGatewayRouteTableAttachment` and
//! `GetTransitGatewayRouteTableAttachment` — driven over HTTP through the real router (the
//! harness lives in `crate::global_network_lifecycle`, and the core-network-with-edges
//! helpers in `crate::attachment_lifecycle` and `crate::attachment_acceptance`).
//!
//! The rule under test: `CreateTransitGatewayPeering` MINTS a `TRANSIT_GATEWAY` peering of
//! ONE edge location of the core network it names — the edge location being the Region of
//! the transit gateway the request peers with, which that core network's policy has to
//! configure an edge for — and `CreateTransitGatewayRouteTableAttachment` MINTS an
//! attachment of the `TRANSIT_GATEWAY_ROUTE_TABLE` type OVER such a peering, taking its
//! core network and its edge location from that peering rather than from its own request,
//! storing it with an `attachment-<17 hex>` id and this service's ARN for that id and
//! reporting the attached route table as both `ResourceArn` and
//! `TransitGatewayRouteTableArn`; each waits in `CREATING` until the transition the clock
//! owes falls due and is `AVAILABLE` after it; and the two readers report those stored
//! records, answering an id of any other attachment kind as not found, without changing
//! what the next caller sees. Nothing reaches into `crate::state`: every peering and every
//! attachment is created and read back through an operation.
//!
//! The tables below span that class: transit gateway and route table ARNs differing in
//! their Region segment (several Regions and the 63-character bound the service's own
//! `ExternalRegionCode` allows), in partition, in whether they carry an account segment,
//! and in the width of the resource id; peerings and attachments carrying tags and a
//! routing policy label and ones carrying neither; peerings on two different core
//! networks; and refusals differing in which reference or value is at fault.

use serde_json::{json, Value};

use crate::attachment_acceptance::{accept, core_network_with};
use crate::attachment_lifecycle::core_network_at;
use crate::global_network_lifecycle::{
    error_code, expected_arn, is_minted_id, Emulator, PAST_SETTLE,
};
use crate::resource_tagging::tags_uri;

/// The ARN resource type (and id prefix) of an attachment:
/// `pattern ^attachment-([0-9a-f]{8,17})$` on `AttachmentId`
/// (`smithy_query shape=AttachmentId`).
const ATTACHMENT: &str = "attachment";

/// The ARN resource type (and id prefix) of a peering:
/// `pattern ^peering-([0-9a-f]{8,17})$` on `PeeringId` (`smithy_query shape=PeeringId`).
const PEERING: &str = "peering";

/// The id prefix of the peering attachment a transit gateway peering leaves on the peered
/// gateway: `pattern ^tgw-attach-([0-9a-f]{8,17})$` on `TransitGatewayPeeringAttachmentId`
/// (`smithy_query shape=TransitGatewayPeering`).
const TRANSIT_GATEWAY_ATTACHMENT: &str = "tgw-attach";

/// The `AttachmentType` a transit gateway route table attachment reports, one of the five
/// the enum carries (`smithy_query shape=AttachmentType`).
const TRANSIT_GATEWAY_ROUTE_TABLE: &str = "TRANSIT_GATEWAY_ROUTE_TABLE";

/// The one value `PeeringType` carries (`smithy_query shape=PeeringType`).
const TRANSIT_GATEWAY: &str = "TRANSIT_GATEWAY";

/// The two states a freshly created peering or attachment passes through, shared by
/// `PeeringState` and `AttachmentState` (`smithy_query shape=PeeringState,AttachmentState`).
const CREATING: &str = "CREATING";
/// See [`CREATING`].
const AVAILABLE: &str = "AVAILABLE";

/// The `AttachmentState` an attachment the policy asked acceptance for waits in
/// (`smithy_query shape=AttachmentState`).
const PENDING_ACCEPTANCE: &str = "PENDING_ATTACHMENT_ACCEPTANCE";

/// How many attachments one core network holds before a create is refused: the bound
/// `state::MAX_ATTACHMENTS_PER_CORE_NETWORK` fixes.
const MAX_ATTACHMENTS: usize = 100;

/// The `CreateTransitGatewayPeering` route (`POST /transit-gateway-peerings`).
const PEERING_URI: &str = "/transit-gateway-peerings";

/// The `CreateTransitGatewayRouteTableAttachment` route
/// (`POST /transit-gateway-route-table-attachments`).
const CREATE_URI: &str = "/transit-gateway-route-table-attachments";

/// The `GetTransitGatewayRouteTableAttachment` URI of `attachment_id`
/// (`GET /transit-gateway-route-table-attachments/{AttachmentId}`).
fn get_uri(attachment_id: &str) -> String {
    format!("{CREATE_URI}/{attachment_id}")
}

/// The `GetTransitGatewayPeering` URI of `peering_id`
/// (`GET /transit-gateway-peerings/{PeeringId}`).
fn peering_uri(peering_id: &str) -> String {
    format!("{PEERING_URI}/{peering_id}")
}

/// A transit gateway ARN in `region`, carrying `account` and the gateway id `tgw_id`.
///
/// The form is the one the documentation of `TransitGatewayRouteTableArn` spells for the
/// route table of such a gateway — `arn:aws:ec2:us-west-2:123456789012:
/// transit-gateway-route-table/tgw-rtb-9876543210123456` (`smithy_query query="doc"
/// shape="TransitGatewayRouteTableAttachment" member="TransitGatewayRouteTableArn"`) —
/// with the gateway's own resource type. `TransitGatewayArn` itself carries only a length
/// and the permissive `^[\s\S]*$`, so every row below reaches the handler.
fn transit_gateway_arn(partition: &str, region: &str, account: &str, tgw_id: &str) -> String {
    format!("arn:{partition}:ec2:{region}:{account}:transit-gateway/tgw-{tgw_id}")
}

/// A transit gateway route table ARN in `region`. See [`transit_gateway_arn`]: this is the
/// documented form of `TransitGatewayRouteTableArn`.
fn route_table_arn(partition: &str, region: &str, account: &str, table_id: &str) -> String {
    format!(
        "arn:{partition}:ec2:{region}:{account}:transit-gateway-route-table/tgw-rtb-{table_id}"
    )
}

/// One row of the create table: the transit gateway peered with, the route table attached
/// over that peering, the edge location their Region names, and the optional members the
/// two requests carry.
struct RouteTableCase {
    /// Names the case in every assertion message.
    name: &'static str,
    /// Which core network of the two the peering goes on (index into the pair).
    core_network: usize,
    transit_gateway_arn: String,
    transit_gateway_route_table_arn: String,
    /// The `EdgeLocation` both answers must carry: the Region of the ARNs above, which the
    /// row's core network configures an edge for.
    edge_location: String,
    /// The `Tags` each request carries (`Value::Null` for no `Tags` member) and the list
    /// the peering and the attachment must then report.
    tags: Value,
    expected_tags: Value,
    routing_policy_label: Option<&'static str>,
}

/// The table: one row per form the peered gateway's and the attached route table's ARNs
/// take, and per combination of the optional members a create carries.
fn route_table_cases() -> Vec<RouteTableCase> {
    vec![
        RouteTableCase {
            name: "8-hex resource ids, no optional members",
            core_network: 0,
            transit_gateway_arn: transit_gateway_arn("aws", "us-west-2", "111122223333", "0123abcd"),
            transit_gateway_route_table_arn: route_table_arn(
                "aws",
                "us-west-2",
                "111122223333",
                "0123abcd",
            ),
            edge_location: "us-west-2".to_string(),
            tags: Value::Null,
            expected_tags: json!([]),
            routing_policy_label: None,
        },
        RouteTableCase {
            name: "17-digit resource ids, with tags and a routing policy label",
            core_network: 0,
            transit_gateway_arn: transit_gateway_arn(
                "aws",
                "eu-central-1",
                "111122223333",
                "0123456789abcdef0",
            ),
            transit_gateway_route_table_arn: route_table_arn(
                "aws",
                "eu-central-1",
                "111122223333",
                "9876543210123456",
            ),
            edge_location: "eu-central-1".to_string(),
            tags: json!([{ "Key": "env", "Value": "test" }, { "Key": "team", "Value": "net" }]),
            expected_tags: json!([
                { "Key": "env", "Value": "test" },
                { "Key": "team", "Value": "net" },
            ]),
            routing_policy_label: Some("east-west"),
        },
        RouteTableCase {
            name: "another partition and one of its own Regions",
            core_network: 0,
            transit_gateway_arn: transit_gateway_arn(
                "aws-cn",
                "cn-north-1",
                "111122223333",
                "abcdef0123456789a",
            ),
            transit_gateway_route_table_arn: route_table_arn(
                "aws-cn",
                "cn-north-1",
                "111122223333",
                "abcdef0",
            ),
            edge_location: "cn-north-1".to_string(),
            tags: json!([]),
            expected_tags: json!([]),
            routing_policy_label: Some("china"),
        },
        RouteTableCase {
            name: "ARNs with no account segment",
            core_network: 0,
            transit_gateway_arn: transit_gateway_arn("aws", "ap-southeast-4", "", "11111111"),
            transit_gateway_route_table_arn: route_table_arn("aws", "ap-southeast-4", "", "22222222"),
            edge_location: "ap-southeast-4".to_string(),
            tags: json!([{ "Key": "only", "Value": "" }]),
            expected_tags: json!([{ "Key": "only", "Value": "" }]),
            routing_policy_label: None,
        },
        RouteTableCase {
            name: "a 63-character Region, the bound ExternalRegionCode publishes",
            core_network: 1,
            transit_gateway_arn: transit_gateway_arn(
                "aws",
                &"r".repeat(63),
                "111122223333",
                "deadbeef",
            ),
            transit_gateway_route_table_arn: route_table_arn(
                "aws",
                &"r".repeat(63),
                "111122223333",
                "deadbee0",
            ),
            edge_location: "r".repeat(63),
            tags: Value::Null,
            expected_tags: json!([]),
            routing_policy_label: None,
        },
        RouteTableCase {
            name: "a second peering and attachment, on the other core network",
            core_network: 1,
            transit_gateway_arn: transit_gateway_arn("aws", "sa-east-1", "111122223333", "cafebabe0"),
            transit_gateway_route_table_arn: route_table_arn(
                "aws",
                "sa-east-1",
                "111122223333",
                "cafebabe1",
            ),
            edge_location: "sa-east-1".to_string(),
            tags: json!([{ "Key": "env", "Value": "prod" }]),
            expected_tags: json!([{ "Key": "env", "Value": "prod" }]),
            routing_policy_label: None,
        },
    ]
}

/// The `CreateTransitGatewayPeering` body for `case`, on `core_network_id`.
fn peering_body(case: &RouteTableCase, core_network_id: &str) -> Value {
    let mut body = json!({
        "CoreNetworkId": core_network_id,
        "TransitGatewayArn": case.transit_gateway_arn,
    });
    if let (Some(object), false) = (body.as_object_mut(), case.tags.is_null()) {
        object.insert("Tags".to_string(), case.tags.clone());
    }
    body
}

/// The `CreateTransitGatewayRouteTableAttachment` body for `case`, over `peering_id`.
fn create_body(case: &RouteTableCase, peering_id: &str) -> Value {
    let mut body = json!({
        "PeeringId": peering_id,
        "TransitGatewayRouteTableArn": case.transit_gateway_route_table_arn,
    });
    if let Some(object) = body.as_object_mut() {
        if !case.tags.is_null() {
            object.insert("Tags".to_string(), case.tags.clone());
        }
        if let Some(label) = case.routing_policy_label {
            object.insert("RoutingPolicyLabel".to_string(), json!(label));
        }
    }
    body
}

/// Create a peering for `case` on `core_network_id` and answer `(id, response shape)`.
async fn create_peering(
    emulator: &Emulator,
    case: &RouteTableCase,
    core_network_id: &str,
) -> (String, Value) {
    let (status, response) = emulator
        .post(PEERING_URI, peering_body(case, core_network_id))
        .await;
    assert_eq!(
        status, 200,
        "case {:?}: CreateTransitGatewayPeering expected 200, got {status} with {response}",
        case.name
    );
    let shape = response["TransitGatewayPeering"].clone();
    let id = shape["Peering"]["PeeringId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {:?}: no PeeringId in {response}", case.name))
        .to_string();
    (id, shape)
}

#[tokio::test]
async fn a_transit_gateway_route_table_attachment_round_trips_through_stored_state() {
    let emulator = Emulator::new();
    let cases = route_table_cases();
    // Two core networks, each configuring an edge in exactly the Regions its own rows
    // peer in, so a row that landed on the wrong parent would show up.
    let first_edges: Vec<&str> = cases
        .iter()
        .filter(|case| case.core_network == 0)
        .map(|case| case.edge_location.as_str())
        .collect();
    let second_edges: Vec<&str> = cases
        .iter()
        .filter(|case| case.core_network == 1)
        .map(|case| case.edge_location.as_str())
        .collect();
    let core_networks = [
        core_network_at(&emulator, "route table attachments", &first_edges)
            .await
            .1,
        core_network_at(&emulator, "other route table attachments", &second_edges)
            .await
            .1,
    ];
    let mut created: Vec<(&RouteTableCase, String, Value)> = Vec::new();

    for case in &cases {
        let core_network_id = &core_networks[case.core_network];
        let (peering_id, peering_shape) = create_peering(&emulator, case, core_network_id).await;
        let peering = &peering_shape["Peering"];
        assert!(
            is_minted_id(&peering_id, PEERING),
            "case {:?}: expected a peering-<17 hex> id, got {peering_id:?}",
            case.name
        );
        assert_eq!(
            peering["CoreNetworkId"],
            json!(core_network_id),
            "case {:?}: the peering must back-reference its core network, got {peering}",
            case.name
        );
        assert_eq!(
            peering["CoreNetworkArn"],
            json!(expected_arn("core-network", core_network_id)),
            "case {:?}: the core network ARN must name that core network, got {peering}",
            case.name
        );
        assert_eq!(
            peering["PeeringType"],
            json!(TRANSIT_GATEWAY),
            "case {:?}: expected the transit gateway peering type, got {peering}",
            case.name
        );
        assert_eq!(
            peering["OwnerAccountId"],
            json!(engine::emulator_account_id()),
            "case {:?}: the owner is the emulator account, got {peering}",
            case.name
        );
        assert_eq!(
            peering["EdgeLocation"],
            json!(case.edge_location),
            "case {:?}: the edge location is the Region of the peered transit gateway, got {peering}",
            case.name
        );
        assert_eq!(
            peering["ResourceArn"],
            json!(case.transit_gateway_arn),
            "case {:?}: the peered resource is the transit gateway, got {peering}",
            case.name
        );
        assert_eq!(
            peering["State"],
            json!(CREATING),
            "case {:?}: a just-created peering is CREATING, got {peering}",
            case.name
        );
        assert_eq!(
            peering["Tags"], case.expected_tags,
            "case {:?}: the peering's Tags must be stored as asked, got {peering}",
            case.name
        );
        assert_eq!(
            peering["LastModificationErrors"],
            json!([]),
            "case {:?}: no modification of this peering has failed, got {peering}",
            case.name
        );
        assert!(
            peering["CreatedAt"].as_f64().is_some_and(|at| at > 0.0),
            "case {:?}: the peering's CreatedAt must be a timestamp, got {peering}",
            case.name
        );
        assert_eq!(
            peering_shape["TransitGatewayArn"],
            json!(case.transit_gateway_arn),
            "case {:?}: the shape must report the peered transit gateway, got {peering_shape}",
            case.name
        );
        let peering_attachment = peering_shape["TransitGatewayPeeringAttachmentId"]
            .as_str()
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: no TransitGatewayPeeringAttachmentId in {peering_shape}",
                    case.name
                )
            });
        assert!(
            is_minted_id(peering_attachment, TRANSIT_GATEWAY_ATTACHMENT),
            "case {:?}: expected a tgw-attach-<17 hex> id, got {peering_attachment:?}",
            case.name
        );

        // The peering reads back as the create stored it.
        let (status, read_peering) = emulator.get(&peering_uri(&peering_id)).await;
        assert_eq!(
            status, 200,
            "case {:?}: GetTransitGatewayPeering expected 200, got {status} with {read_peering}",
            case.name
        );
        assert_eq!(
            read_peering["TransitGatewayPeering"], peering_shape,
            "case {:?}: the reader must answer the stored peering, got {read_peering}",
            case.name
        );

        let (status, response) = emulator
            .post(CREATE_URI, create_body(case, &peering_id))
            .await;
        assert_eq!(
            status, 200,
            "case {:?}: CreateTransitGatewayRouteTableAttachment expected 200, got {status} with {response}",
            case.name
        );
        let created_shape = &response["TransitGatewayRouteTableAttachment"];
        let attachment = &created_shape["Attachment"];
        let id = attachment["AttachmentId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {:?}: no AttachmentId in {response}", case.name))
            .to_string();
        assert!(
            is_minted_id(&id, ATTACHMENT),
            "case {:?}: expected an attachment-<17 hex> id, got {id:?}",
            case.name
        );
        assert_eq!(
            attachment["CoreNetworkId"],
            json!(core_network_id),
            "case {:?}: the attachment takes its core network from the peering, got {attachment}",
            case.name
        );
        assert_eq!(
            attachment["CoreNetworkArn"],
            json!(expected_arn("core-network", core_network_id)),
            "case {:?}: the core network ARN must name that core network, got {attachment}",
            case.name
        );
        assert_eq!(
            attachment["AttachmentType"],
            json!(TRANSIT_GATEWAY_ROUTE_TABLE),
            "case {:?}: expected the transit gateway route table attachment type, got {attachment}",
            case.name
        );
        assert_eq!(
            attachment["OwnerAccountId"],
            json!(engine::emulator_account_id()),
            "case {:?}: the owner is the emulator account, got {attachment}",
            case.name
        );
        assert_eq!(
            attachment["EdgeLocation"],
            json!(case.edge_location),
            "case {:?}: the attachment takes its edge location from the peering, got {attachment}",
            case.name
        );
        assert!(
            attachment["EdgeLocations"].is_null(),
            "case {:?}: a single-edge attachment reports EdgeLocation, not EdgeLocations, got {attachment}",
            case.name
        );
        assert_eq!(
            attachment["ResourceArn"],
            json!(case.transit_gateway_route_table_arn),
            "case {:?}: the attached resource is the route table, got {attachment}",
            case.name
        );
        assert_eq!(
            created_shape["TransitGatewayRouteTableArn"],
            json!(case.transit_gateway_route_table_arn),
            "case {:?}: the shape must report the attached route table, got {created_shape}",
            case.name
        );
        assert_eq!(
            created_shape["PeeringId"],
            json!(peering_id),
            "case {:?}: the shape must report the peering it was created over, got {created_shape}",
            case.name
        );
        assert_eq!(
            attachment["State"],
            json!(CREATING),
            "case {:?}: a just-created attachment is CREATING, got {attachment}",
            case.name
        );
        assert_eq!(
            attachment["Tags"], case.expected_tags,
            "case {:?}: Tags must be stored as asked, got {attachment}",
            case.name
        );
        assert_eq!(
            attachment["LastModificationErrors"],
            json!([]),
            "case {:?}: no modification of this attachment has failed, got {attachment}",
            case.name
        );
        assert!(
            attachment["SegmentName"].is_null()
                && attachment["AttachmentPolicyRuleNumber"].is_null()
                && attachment["NetworkFunctionGroupName"].is_null()
                && attachment["ProposedSegmentChange"].is_null(),
            "case {:?}: nothing an attachment policy would assign is invented, got {attachment}",
            case.name
        );
        let created_at = attachment["CreatedAt"].clone();
        assert!(
            created_at.as_f64().is_some_and(|at| at > 0.0),
            "case {:?}: CreatedAt must be a timestamp, got {attachment}",
            case.name
        );
        assert_eq!(
            attachment["UpdatedAt"], created_at,
            "case {:?}: nothing has changed the attachment yet, got {attachment}",
            case.name
        );

        // The reader answers the record the create stored, while that record is still
        // CREATING.
        let (status, read) = emulator.get(&get_uri(&id)).await;
        assert_eq!(
            status, 200,
            "case {:?}: GetTransitGatewayRouteTableAttachment expected 200, got {status} with {read}",
            case.name
        );
        assert_eq!(
            read["TransitGatewayRouteTableAttachment"], *created_shape,
            "case {:?}: the reader must answer the stored attachment, got {read}",
            case.name
        );

        // Neither resource's own ARN is a member of its shape, so each is read back the one
        // way a client can: through the tagging slice, which resolves the ARN to the stored
        // resource and its tags.
        for (what, arn) in [
            ("the attachment", expected_arn(ATTACHMENT, &id)),
            ("the peering", expected_arn(PEERING, &peering_id)),
        ] {
            let (status, listed) = emulator.get(&tags_uri(&arn)).await;
            assert_eq!(
                status, 200,
                "case {:?}: ListTagsForResource on {what} expected 200, got {status} with {listed}",
                case.name
            );
            assert_eq!(
                listed["TagList"], case.expected_tags,
                "case {:?}: the ARN of {what} must resolve to its stored tags, got {listed}",
                case.name
            );
        }

        created.push((case, id, created_shape.clone()));
    }

    // Past the settle span the transition the clock owed has fallen due, and nothing the
    // request supplied moved with it.
    tokio::time::sleep(PAST_SETTLE).await;
    for (case, id, at_create) in &created {
        let (status, read) = emulator.get(&get_uri(id)).await;
        assert_eq!(
            status, 200,
            "case {:?}: the reader after the settle span expected 200, got {status} with {read}",
            case.name
        );
        let attachment = &read["TransitGatewayRouteTableAttachment"]["Attachment"];
        assert_eq!(
            attachment["State"],
            json!(AVAILABLE),
            "case {:?}: past the settle span the attachment is AVAILABLE, got {attachment}",
            case.name
        );
        let created_at = attachment["CreatedAt"].as_f64().unwrap_or_default();
        let updated_at = attachment["UpdatedAt"].as_f64().unwrap_or_default();
        assert!(
            updated_at > created_at,
            "case {:?}: the settle transition is a change, so UpdatedAt ({updated_at}) must be past CreatedAt ({created_at}), got {attachment}",
            case.name
        );
        for member in [
            "AttachmentId",
            "CoreNetworkId",
            "CoreNetworkArn",
            "AttachmentType",
            "OwnerAccountId",
            "EdgeLocation",
            "ResourceArn",
            "CreatedAt",
            "Tags",
        ] {
            assert_eq!(
                attachment[member], at_create["Attachment"][member],
                "case {:?}: {member} must not have moved with the state, got {attachment}",
                case.name
            );
        }
        assert_eq!(
            read["TransitGatewayRouteTableAttachment"]["PeeringId"], at_create["PeeringId"],
            "case {:?}: the peering the attachment was created over cannot move, got {read}",
            case.name
        );
    }

    // A read never changes what the next caller sees.
    for (case, id, _) in &created {
        let (_, first) = emulator.get(&get_uri(id)).await;
        let (_, second) = emulator.get(&get_uri(id)).await;
        assert_eq!(
            first, second,
            "case {:?}: the reader must be a pure read, got {first} then {second}",
            case.name
        );
    }
}

#[tokio::test]
async fn an_attachment_the_store_does_not_hold_is_refused_with_the_modelled_not_found() {
    let emulator = Emulator::new();
    let (_, core_network_id) = core_network_at(&emulator, "one attachment", &["us-east-1"]).await;
    let case = RouteTableCase {
        name: "the live attachment",
        core_network: 0,
        transit_gateway_arn: transit_gateway_arn("aws", "us-east-1", "111122223333", "0000000a"),
        transit_gateway_route_table_arn: route_table_arn(
            "aws",
            "us-east-1",
            "111122223333",
            "0000000a",
        ),
        edge_location: "us-east-1".to_string(),
        tags: Value::Null,
        expected_tags: json!([]),
        routing_policy_label: None,
    };
    let (peering_id, _) = create_peering(&emulator, &case, &core_network_id).await;
    let (status, created) = emulator.post(CREATE_URI, create_body(&case, &peering_id)).await;
    assert_eq!(
        status, 200,
        "CreateTransitGatewayRouteTableAttachment: expected 200, got {status} with {created}"
    );
    let live = created["TransitGatewayRouteTableAttachment"]["Attachment"]["AttachmentId"]
        .as_str()
        .unwrap_or_else(|| panic!("no AttachmentId in {created}"))
        .to_string();

    // Each row is a different way of naming no stored transit gateway route table
    // attachment, every one satisfying the modelled `AttachmentId` pattern so it reaches
    // the handler: a never-minted id at each width the pattern allows, and the counter
    // value a DIFFERENT resource kind of this service was minted under, spelled with the
    // attachment prefix — an id no attachment ever carried.
    let borrowed_suffix = format!(
        "attachment-{}",
        peering_id.strip_prefix("peering-").unwrap_or_default()
    );
    for (name, attachment_id) in [
        ("never minted, 17 hex digits", "attachment-00000000000000099"),
        ("never minted, the 8-hex lower bound", "attachment-0000beef"),
        ("another counter value under this prefix", &borrowed_suffix),
    ] {
        let (status, body) = emulator.get(&get_uri(attachment_id)).await;
        assert_eq!(
            status, 404,
            "case {name} ({attachment_id:?}): expected 404, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {name} ({attachment_id:?}): wrong error code in {body}"
        );
        assert_eq!(
            body["Message"],
            json!("Resource not found."),
            "case {name} ({attachment_id:?}): wrong message in {body}"
        );
        assert_eq!(
            body["ResourceId"],
            json!(attachment_id),
            "case {name} ({attachment_id:?}): the refusal must name the id asked for, got {body}"
        );
        assert_eq!(
            body["ResourceType"],
            json!(ATTACHMENT),
            "case {name} ({attachment_id:?}): the refusal must name the resource type, got {body}"
        );
    }

    // The same holds for the peering reader, which answers under its own resource type.
    for (name, id) in [
        ("never minted, 17 hex digits", "peering-00000000000000099"),
        ("never minted, the 8-hex lower bound", "peering-0000beef"),
    ] {
        let (status, body) = emulator.get(&peering_uri(id)).await;
        assert_eq!(
            status, 404,
            "case {name} ({id:?}): expected 404, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {name} ({id:?}): wrong error code in {body}"
        );
        assert_eq!(
            body["ResourceType"],
            json!(PEERING),
            "case {name} ({id:?}): the refusal must name the peering resource type, got {body}"
        );
    }

    // An id that names an attachment of ANOTHER kind names no transit gateway route table
    // attachment either: the readers of the kinds share one id space, and each answers only
    // its own.
    let (status, vpc) = emulator
        .post(
            "/vpc-attachments",
            json!({
                "CoreNetworkId": core_network_id,
                "VpcArn": "arn:aws:ec2:us-east-1:111122223333:vpc/vpc-0000000a",
                "SubnetArns": ["arn:aws:ec2:us-east-1:111122223333:subnet/subnet-0000000a"],
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "CreateVpcAttachment: expected 200, got {status} with {vpc}"
    );
    let vpc_id = vpc["VpcAttachment"]["Attachment"]["AttachmentId"]
        .as_str()
        .unwrap_or_else(|| panic!("no AttachmentId in {vpc}"))
        .to_string();
    for (name, uri) in [
        ("a VPC attachment's id", get_uri(&vpc_id)),
        ("a route table attachment's id read as a VPC attachment", format!("/vpc-attachments/{live}")),
    ] {
        let (status, body) = emulator.get(&uri).await;
        assert_eq!(
            status, 404,
            "case {name}: expected 404, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {name}: wrong error code in {body}"
        );
    }

    // The refusals are the lookup and nothing wider: the stored attachment still reads.
    let (status, read) = emulator.get(&get_uri(&live)).await;
    assert_eq!(
        status, 200,
        "the live attachment: expected 200, got {status} with {read}"
    );
}

#[tokio::test]
async fn a_create_is_refused_when_its_peering_or_its_route_table_does_not_resolve() {
    let emulator = Emulator::new();
    let (_, live) = core_network_at(&emulator, "live", &["us-east-1"]).await;
    let (_, gone) = core_network_at(&emulator, "to be deleted", &["us-east-1"]).await;
    let (status, deleted) = emulator.delete(&format!("/core-networks/{gone}")).await;
    assert_eq!(
        status, 200,
        "DeleteCoreNetwork: expected 200, got {status} with {deleted}"
    );

    // Each row is a different WAY a peering create is invalid, and names the member at
    // fault: a core network that never existed, one created and then deleted, a transit
    // gateway ARN that names no Region, one that is not an ARN at all, one of another
    // service, one naming another resource type of EC2, and a gateway in a Region the core
    // network's policy configures no edge for. A `CoreNetworkId` outside
    // `^core-network-([0-9a-f]{8,17})$` is refused by the schema before the handler runs,
    // so it is not a row here.
    let peering_rows: Vec<(&str, Value, &str)> = vec![
        (
            "a core network that never existed",
            json!({
                "CoreNetworkId": "core-network-00000000000000099",
                "TransitGatewayArn": transit_gateway_arn("aws", "us-east-1", "111122223333", "0000000b"),
            }),
            "coreNetworkId",
        ),
        (
            "a deleted core network",
            json!({
                "CoreNetworkId": gone,
                "TransitGatewayArn": transit_gateway_arn("aws", "us-east-1", "111122223333", "0000000b"),
            }),
            "coreNetworkId",
        ),
        (
            "a transit gateway ARN naming no Region",
            json!({
                "CoreNetworkId": live,
                "TransitGatewayArn": transit_gateway_arn("aws", "", "111122223333", "0000000b"),
            }),
            "transitGatewayArn",
        ),
        (
            "a transit gateway ARN that is not an ARN",
            json!({ "CoreNetworkId": live, "TransitGatewayArn": "tgw-0000000b" }),
            "transitGatewayArn",
        ),
        (
            "an ARN of another service",
            json!({
                "CoreNetworkId": live,
                "TransitGatewayArn": "arn:aws:directconnect:us-east-1:111122223333:transit-gateway/tgw-0000000b",
            }),
            "transitGatewayArn",
        ),
        (
            "an EC2 ARN of another resource type",
            json!({
                "CoreNetworkId": live,
                "TransitGatewayArn": "arn:aws:ec2:us-east-1:111122223333:vpc/vpc-0000000b",
            }),
            "transitGatewayArn",
        ),
        (
            "a Region the core network configures no edge for",
            json!({
                "CoreNetworkId": live,
                "TransitGatewayArn": transit_gateway_arn("aws", "eu-west-3", "111122223333", "0000000b"),
            }),
            "transitGatewayArn",
        ),
    ];
    for (name, request, member) in peering_rows {
        let (status, body) = emulator.post(PEERING_URI, request).await;
        assert_eq!(
            status, 400,
            "case {name}: expected 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {name}: wrong error code in {body}"
        );
        assert_eq!(
            body["Fields"][0]["Name"],
            json!(member),
            "case {name}: the refusal must name {member}, got {body}"
        );
    }

    // A refused create stores nothing, so the gateway every row above named is still free
    // to peer at an edge the core network does configure.
    let case = RouteTableCase {
        name: "the live peering",
        core_network: 0,
        transit_gateway_arn: transit_gateway_arn("aws", "us-east-1", "111122223333", "0000000b"),
        transit_gateway_route_table_arn: route_table_arn(
            "aws",
            "us-east-1",
            "111122223333",
            "0000000b",
        ),
        edge_location: "us-east-1".to_string(),
        tags: Value::Null,
        expected_tags: json!([]),
        routing_policy_label: None,
    };
    let (peering_id, _) = create_peering(&emulator, &case, &live).await;

    // Each row is a different WAY an attachment create is invalid: a peering that names
    // nothing, a route table ARN naming no Region, one that is not an ARN, one of another
    // service, one naming the gateway itself rather than one of its route tables, and a
    // route table in a Region other than the one the peering peers in.
    let attachment_rows: Vec<(&str, Value, &str)> = vec![
        (
            "a peering that never existed",
            json!({
                "PeeringId": "peering-00000000000000099",
                "TransitGatewayRouteTableArn": case.transit_gateway_route_table_arn,
            }),
            "peeringId",
        ),
        (
            "a route table ARN naming no Region",
            json!({
                "PeeringId": peering_id,
                "TransitGatewayRouteTableArn": route_table_arn("aws", "", "111122223333", "0000000c"),
            }),
            "transitGatewayRouteTableArn",
        ),
        (
            "a route table ARN that is not an ARN",
            json!({ "PeeringId": peering_id, "TransitGatewayRouteTableArn": "tgw-rtb-0000000c" }),
            "transitGatewayRouteTableArn",
        ),
        (
            "an ARN of another service",
            json!({
                "PeeringId": peering_id,
                "TransitGatewayRouteTableArn": "arn:aws:networkmanager::111122223333:transit-gateway-route-table/tgw-rtb-0000000c",
            }),
            "transitGatewayRouteTableArn",
        ),
        (
            "an EC2 ARN of the gateway rather than a route table",
            json!({
                "PeeringId": peering_id,
                "TransitGatewayRouteTableArn": case.transit_gateway_arn,
            }),
            "transitGatewayRouteTableArn",
        ),
        (
            "a route table in another Region than the peering peers in",
            json!({
                "PeeringId": peering_id,
                "TransitGatewayRouteTableArn": route_table_arn("aws", "eu-west-3", "111122223333", "0000000c"),
            }),
            "transitGatewayRouteTableArn",
        ),
    ];
    for (name, request, member) in attachment_rows {
        let (status, body) = emulator.post(CREATE_URI, request).await;
        assert_eq!(
            status, 400,
            "case {name}: expected 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {name}: wrong error code in {body}"
        );
        assert_eq!(
            body["Fields"][0]["Name"],
            json!(member),
            "case {name}: the refusal must name {member}, got {body}"
        );
    }

    // A refused create stores nothing, so the route table the rows above named is still
    // free to attach over the peering.
    let (status, created) = emulator
        .post(CREATE_URI, create_body(&case, &peering_id))
        .await;
    assert_eq!(
        status, 200,
        "the route table over its own peering: expected 200, got {status} with {created}"
    );

    // A peering whose core network has since been deleted carries no attachment either:
    // the attachment hangs off that core network, so the create is refused on the member
    // that carried the reference and stores nothing.
    let (_, orphan) = core_network_at(&emulator, "to be emptied", &["us-east-2"]).await;
    let orphan_case = RouteTableCase {
        name: "an orphaned peering",
        core_network: 0,
        transit_gateway_arn: transit_gateway_arn("aws", "us-east-2", "111122223333", "0000000d"),
        transit_gateway_route_table_arn: route_table_arn(
            "aws",
            "us-east-2",
            "111122223333",
            "0000000d",
        ),
        edge_location: "us-east-2".to_string(),
        tags: Value::Null,
        expected_tags: json!([]),
        routing_policy_label: None,
    };
    let (orphan_peering, _) = create_peering(&emulator, &orphan_case, &orphan).await;
    let (status, emptied) = emulator.delete(&format!("/core-networks/{orphan}")).await;
    assert_eq!(
        status, 200,
        "DeleteCoreNetwork of the emptied core network: expected 200, got {status} with {emptied}"
    );
    let (status, body) = emulator
        .post(CREATE_URI, create_body(&orphan_case, &orphan_peering))
        .await;
    assert_eq!(
        status, 400,
        "case a peering whose core network is gone: expected 400, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "case a peering whose core network is gone: wrong error code in {body}"
    );
    assert_eq!(
        body["Fields"][0]["Name"],
        json!("peeringId"),
        "case a peering whose core network is gone: the refusal must name peeringId, got {body}"
    );
}

#[tokio::test]
async fn one_route_table_is_attached_once_and_a_retried_token_mints_nothing_new() {
    let emulator = Emulator::new();
    let (_, core_network_id) = core_network_at(&emulator, "idempotency", &["us-west-2"]).await;
    let case = RouteTableCase {
        name: "idempotency",
        core_network: 0,
        transit_gateway_arn: transit_gateway_arn("aws", "us-west-2", "111122223333", "0000cafe"),
        transit_gateway_route_table_arn: route_table_arn(
            "aws",
            "us-west-2",
            "111122223333",
            "0000cafe",
        ),
        edge_location: "us-west-2".to_string(),
        tags: Value::Null,
        expected_tags: json!([]),
        routing_policy_label: None,
    };
    let (peering_id, first_peering) = create_peering(&emulator, &case, &core_network_id).await;

    // The peering create is idempotent on its own token, and one transit gateway is peered
    // once.
    let token_body = json!({
        "CoreNetworkId": core_network_id,
        "TransitGatewayArn": case.transit_gateway_arn,
        "ClientToken": "peering-token",
    });
    let (status, tokened) = emulator.post(PEERING_URI, token_body.clone()).await;
    assert_eq!(
        status, 409,
        "case the same gateway with no earlier token: expected 409, got {status} with {tokened}"
    );
    assert_eq!(
        tokened["ResourceId"],
        json!(peering_id),
        "case the same gateway with no earlier token: the conflict must name the peering that holds it, got {tokened}"
    );
    assert_eq!(
        tokened["ResourceType"],
        json!(PEERING),
        "case the same gateway with no earlier token: the conflict must name the resource type, got {tokened}"
    );
    let other_gateway = transit_gateway_arn("aws", "us-west-2", "111122223333", "0000beef");
    let other_body = json!({
        "CoreNetworkId": core_network_id,
        "TransitGatewayArn": other_gateway,
        "ClientToken": "peering-token",
    });
    let (status, second) = emulator.post(PEERING_URI, other_body.clone()).await;
    assert_eq!(
        status, 200,
        "case a second gateway under a token: expected 200, got {status} with {second}"
    );
    let (status, retried) = emulator.post(PEERING_URI, other_body).await;
    assert_eq!(
        status, 200,
        "case the peering create retried with its token: expected 200, got {status} with {retried}"
    );
    assert_eq!(
        retried["TransitGatewayPeering"], second["TransitGatewayPeering"],
        "case the peering create retried with its token: expected the first peering, got {retried}"
    );
    assert_ne!(
        retried["TransitGatewayPeering"]["Peering"]["PeeringId"],
        first_peering["Peering"]["PeeringId"],
        "case the peering create retried with its token: a second gateway gets its own peering, got {retried}"
    );

    // The attachment create is idempotent the same way.
    let body = json!({
        "PeeringId": peering_id,
        "TransitGatewayRouteTableArn": case.transit_gateway_route_table_arn,
        "ClientToken": "token-one",
    });
    let (status, first) = emulator.post(CREATE_URI, body.clone()).await;
    assert_eq!(
        status, 200,
        "case first create: expected 200, got {status} with {first}"
    );
    let id = first["TransitGatewayRouteTableAttachment"]["Attachment"]["AttachmentId"]
        .as_str()
        .unwrap_or_else(|| panic!("case first create: no AttachmentId in {first}"))
        .to_string();
    let (status, retried) = emulator.post(CREATE_URI, body.clone()).await;
    assert_eq!(
        status, 200,
        "case retry of the same request: expected 200, got {status} with {retried}"
    );
    assert_eq!(
        retried["TransitGatewayRouteTableAttachment"]["Attachment"]["AttachmentId"],
        json!(id),
        "case retry of the same request: expected the first attachment, got {retried}"
    );

    // Each row is a create that must be refused as a conflict, and the attachment the
    // refusal has to name: the same token asking for something else, and the same route
    // table asked for again with and without a token of its own.
    let other_table = route_table_arn("aws", "us-west-2", "111122223333", "0000beef");
    for (name, request) in [
        (
            "the same token with a different route table",
            json!({
                "PeeringId": peering_id,
                "TransitGatewayRouteTableArn": other_table,
                "ClientToken": "token-one",
            }),
        ),
        (
            "the same route table under another token",
            json!({
                "PeeringId": peering_id,
                "TransitGatewayRouteTableArn": case.transit_gateway_route_table_arn,
                "ClientToken": "token-two",
            }),
        ),
        (
            "the same route table with no token at all",
            json!({
                "PeeringId": peering_id,
                "TransitGatewayRouteTableArn": case.transit_gateway_route_table_arn,
            }),
        ),
    ] {
        let (status, refused) = emulator.post(CREATE_URI, request).await;
        assert_eq!(
            status, 409,
            "case {name}: expected 409, got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ConflictException",
            "case {name}: wrong error code in {refused}"
        );
        assert_eq!(
            refused["ResourceId"],
            json!(id),
            "case {name}: the conflict must name the attachment that holds it, got {refused}"
        );
        assert_eq!(
            refused["ResourceType"],
            json!(ATTACHMENT),
            "case {name}: the conflict must name the resource type, got {refused}"
        );
    }

    // Nothing above minted a second attachment, and a different route table under a token
    // of its own still attaches.
    let (status, read) = emulator.get(&get_uri(&id)).await;
    assert_eq!(
        status, 200,
        "the reader after the refusals: expected 200, got {status} with {read}"
    );
    assert_eq!(
        read["TransitGatewayRouteTableAttachment"]["Attachment"]["ResourceArn"],
        json!(case.transit_gateway_route_table_arn),
        "the stored attachment must still hold the first route table, got {read}"
    );
    let (status, created) = emulator
        .post(
            CREATE_URI,
            json!({
                "PeeringId": peering_id,
                "TransitGatewayRouteTableArn": other_table,
                "ClientToken": "token-three",
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "a free route table under a fresh token: expected 200, got {status} with {created}"
    );
    assert_ne!(
        created["TransitGatewayRouteTableAttachment"]["Attachment"]["AttachmentId"],
        json!(id),
        "a second route table gets its own attachment, got {created}"
    );
}

#[tokio::test]
async fn the_claiming_attachment_policy_rule_decides_the_segment_and_the_acceptance() {
    let emulator = Emulator::new();
    const REGION: &str = "us-west-2";

    // Each row is one `attachment-policies` rule and what it decides about a TRANSIT
    // GATEWAY ROUTE TABLE attachment created under it: whether it claims the attachment at
    // all (so whether the attachment reports that rule's number and segment) and whether
    // that rule asks for the attachment to be accepted. Every key name is the published
    // Cloud WAN attachment policy schema's, the same source `crate::attachment_acceptance`
    // builds its rules from.
    let rows: Vec<(&str, Value, &str, Value, Value)> = vec![
        (
            "a rule matching this attachment type, asking for acceptance",
            json!([{
                "rule-number": 42,
                "conditions": [{
                    "type": "attachment-type",
                    "operator": "equals",
                    "value": TRANSIT_GATEWAY_ROUTE_TABLE,
                }],
                "action": {
                    "association-method": "constant",
                    "segment": "shared",
                    "require-attachment-acceptance": true,
                },
            }]),
            PENDING_ACCEPTANCE,
            json!(42),
            json!("shared"),
        ),
        (
            "a rule claiming this attachment without asking for acceptance",
            json!([{
                "rule-number": 7,
                "conditions": [{ "type": "any" }],
                "action": { "association-method": "constant", "segment": "prod" },
            }]),
            CREATING,
            json!(7),
            json!("prod"),
        ),
        (
            "a rule for another attachment type, which claims nothing here",
            json!([{
                "rule-number": 9,
                "conditions": [{ "type": "attachment-type", "operator": "equals", "value": "VPC" }],
                "action": {
                    "association-method": "constant",
                    "segment": "shared",
                    "require-attachment-acceptance": true,
                },
            }]),
            CREATING,
            Value::Null,
            Value::Null,
        ),
    ];

    for (index, (name, rules, state, rule_number, segment)) in rows.into_iter().enumerate() {
        let core_network = core_network_with(&emulator, name, REGION, &rules).await;
        let case = RouteTableCase {
            name: "claimed",
            core_network: 0,
            transit_gateway_arn: transit_gateway_arn(
                "aws",
                REGION,
                "111122223333",
                &format!("aa0000{index:02x}"),
            ),
            transit_gateway_route_table_arn: route_table_arn(
                "aws",
                REGION,
                "111122223333",
                &format!("bb0000{index:02x}"),
            ),
            edge_location: REGION.to_string(),
            tags: Value::Null,
            expected_tags: json!([]),
            routing_policy_label: None,
        };
        let (peering_id, _) = create_peering(&emulator, &case, &core_network).await;
        let (status, created) = emulator
            .post(CREATE_URI, create_body(&case, &peering_id))
            .await;
        assert_eq!(
            status, 200,
            "case {name:?}: CreateTransitGatewayRouteTableAttachment expected 200, got {status} with {created}"
        );
        let attachment = &created["TransitGatewayRouteTableAttachment"]["Attachment"];
        let id = attachment["AttachmentId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {name:?}: no AttachmentId in {created}"))
            .to_string();
        assert_eq!(
            attachment["State"],
            json!(state),
            "case {name:?}: expected state {state}, got {attachment}"
        );
        assert_eq!(
            attachment["AttachmentPolicyRuleNumber"], rule_number,
            "case {name:?}: expected the claiming rule's number {rule_number}, got {attachment}"
        );
        assert_eq!(
            attachment["SegmentName"], segment,
            "case {name:?}: expected the segment the rule names {segment}, got {attachment}"
        );

        // The reader answers the same decision from the stored record.
        let (status, read) = emulator.get(&get_uri(&id)).await;
        assert_eq!(
            status, 200,
            "case {name:?}: the reader expected 200, got {status} with {read}"
        );
        let read_attachment = &read["TransitGatewayRouteTableAttachment"]["Attachment"];
        assert_eq!(
            (
                read_attachment["State"].clone(),
                read_attachment["AttachmentPolicyRuleNumber"].clone(),
                read_attachment["SegmentName"].clone(),
            ),
            (json!(state), rule_number.clone(), segment.clone()),
            "case {name:?}: the reader must answer the stored decision, got {read_attachment}"
        );

        // An attachment waiting for acceptance is moved by `AcceptAttachment` and by
        // nothing else: the clock owes it no transition until then.
        if state == PENDING_ACCEPTANCE {
            tokio::time::sleep(PAST_SETTLE).await;
            let (_, still_waiting) = emulator.get(&get_uri(&id)).await;
            assert_eq!(
                still_waiting["TransitGatewayRouteTableAttachment"]["Attachment"]["State"],
                json!(PENDING_ACCEPTANCE),
                "case {name:?}: no span of the clock accepts the attachment, got {still_waiting}"
            );
            let (status, accepted) = accept(&emulator, &id).await;
            assert_eq!(
                status, 200,
                "case {name:?}: AcceptAttachment expected 200, got {status} with {accepted}"
            );
            let (_, read) = emulator.get(&get_uri(&id)).await;
            assert_eq!(
                read["TransitGatewayRouteTableAttachment"]["Attachment"]["State"],
                json!(CREATING),
                "case {name:?}: acceptance starts the settle span, got {read}"
            );
        }

        tokio::time::sleep(PAST_SETTLE).await;
        let (_, settled) = emulator.get(&get_uri(&id)).await;
        assert_eq!(
            settled["TransitGatewayRouteTableAttachment"]["Attachment"]["State"],
            json!(AVAILABLE),
            "case {name:?}: past the settle span the attachment is AVAILABLE, got {settled}"
        );
    }
}

#[tokio::test]
async fn a_core_network_holds_at_most_its_bound_of_route_table_attachments() {
    let emulator = Emulator::new();
    const REGION: &str = "us-west-2";
    let (_, full) = core_network_at(&emulator, "full", &[REGION]).await;
    let (_, spare) = core_network_at(&emulator, "spare", &[REGION]).await;
    let mut peerings: Vec<String> = Vec::new();
    for (index, core_network) in [&full, &spare].into_iter().enumerate() {
        let case = RouteTableCase {
            name: "bound",
            core_network: index,
            transit_gateway_arn: transit_gateway_arn(
                "aws",
                REGION,
                "111122223333",
                &format!("ee0000{index:02x}"),
            ),
            transit_gateway_route_table_arn: route_table_arn(
                "aws",
                REGION,
                "111122223333",
                &format!("ff0000{index:02x}"),
            ),
            edge_location: REGION.to_string(),
            tags: Value::Null,
            expected_tags: json!([]),
            routing_policy_label: None,
        };
        let (peering_id, _) = create_peering(&emulator, &case, core_network).await;
        peerings.push(peering_id);
    }

    for index in 0..MAX_ATTACHMENTS {
        let (status, created) = emulator
            .post(
                CREATE_URI,
                json!({
                    "PeeringId": peerings[0],
                    "TransitGatewayRouteTableArn": route_table_arn(
                        "aws",
                        REGION,
                        "111122223333",
                        &format!("{index:08x}"),
                    ),
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case attachment {index} of {MAX_ATTACHMENTS}: expected 200, got {status} with {created}"
        );
    }

    // One past the bound is refused, and this operation models no quota error, so the
    // refusal is the modelled `ValidationException` naming the peering whose core network
    // is full.
    let over = route_table_arn("aws", REGION, "111122223333", "ffffffff");
    let (status, refused) = emulator
        .post(
            CREATE_URI,
            json!({ "PeeringId": peerings[0], "TransitGatewayRouteTableArn": over }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case one past the bound: expected 400, got {status} with {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "ValidationException",
        "case one past the bound: wrong error code in {refused}"
    );
    assert_eq!(
        refused["Fields"][0]["Name"],
        json!("peeringId"),
        "case one past the bound: the refusal must name the peering whose core network is full, got {refused}"
    );

    // The bound is per core network: the same create over a peering of another core network
    // is accepted, which also shows the refused one stored nothing.
    let (status, created) = emulator
        .post(
            CREATE_URI,
            json!({ "PeeringId": peerings[1], "TransitGatewayRouteTableArn": over }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case the same route table on an empty core network: expected 200, got {status} with {created}"
    );
}

#[tokio::test]
async fn a_peering_and_its_route_table_attachment_are_in_the_network_resource_inventory() {
    let emulator = Emulator::new();
    const REGION: &str = "eu-west-1";
    let (global_network, core_network) =
        core_network_at(&emulator, "inventory", &[REGION]).await;
    let case = RouteTableCase {
        name: "inventory",
        core_network: 0,
        transit_gateway_arn: transit_gateway_arn("aws", REGION, "111122223333", "0000aa01"),
        transit_gateway_route_table_arn: route_table_arn("aws", REGION, "111122223333", "0000aa02"),
        edge_location: REGION.to_string(),
        tags: json!([{ "Key": "env", "Value": "inventory" }]),
        expected_tags: json!([{ "Key": "env", "Value": "inventory" }]),
        routing_policy_label: None,
    };
    let (peering_id, peering_shape) = create_peering(&emulator, &case, &core_network).await;
    let (status, created) = emulator
        .post(CREATE_URI, create_body(&case, &peering_id))
        .await;
    assert_eq!(
        status, 200,
        "CreateTransitGatewayRouteTableAttachment: expected 200, got {status} with {created}"
    );
    let attachment_id = created["TransitGatewayRouteTableAttachment"]["Attachment"]["AttachmentId"]
        .as_str()
        .unwrap_or_else(|| panic!("no AttachmentId in {created}"))
        .to_string();

    // Each row is a network-resource type the two creates above added one resource of, with
    // the id and ARN the inventory has to report for it. A type nothing was created under
    // stays at zero, which the last row checks.
    for (resource_type, expected) in [
        (PEERING, Some((peering_id.clone(), expected_arn(PEERING, &peering_id)))),
        (
            ATTACHMENT,
            Some((attachment_id.clone(), expected_arn(ATTACHMENT, &attachment_id))),
        ),
        ("connect-peer", None),
    ] {
        let (status, counts) = emulator
            .get(&format!(
                "/global-networks/{global_network}/network-resource-count?resourceType={resource_type}"
            ))
            .await;
        assert_eq!(
            status, 200,
            "case {resource_type}: GetNetworkResourceCounts expected 200, got {status} with {counts}"
        );
        assert_eq!(
            counts["NetworkResourceCounts"][0]["Count"],
            json!(i64::from(expected.is_some())),
            "case {resource_type}: the count must be what the store holds, got {counts}"
        );

        let (status, listed) = emulator
            .get(&format!(
                "/global-networks/{global_network}/network-resources?resourceType={resource_type}"
            ))
            .await;
        assert_eq!(
            status, 200,
            "case {resource_type}: GetNetworkResources expected 200, got {status} with {listed}"
        );
        let resources = listed["NetworkResources"]
            .as_array()
            .unwrap_or_else(|| panic!("case {resource_type}: no NetworkResources in {listed}"));
        match &expected {
            None => assert!(
                resources.is_empty(),
                "case {resource_type}: nothing of this type was created, got {listed}"
            ),
            Some((id, arn)) => {
                assert_eq!(
                    resources.len(),
                    1,
                    "case {resource_type}: expected the one resource created, got {listed}"
                );
                assert_eq!(
                    (
                        resources[0]["ResourceId"].clone(),
                        resources[0]["ResourceArn"].clone(),
                        resources[0]["CoreNetworkId"].clone(),
                        resources[0]["Tags"].clone(),
                    ),
                    (
                        json!(id),
                        json!(arn),
                        json!(core_network),
                        case.expected_tags.clone(),
                    ),
                    "case {resource_type}: the inventory must report the stored resource, got {listed}"
                );
            }
        }
    }

    // The peering's definition in the inventory is what its own reader answers, so the two
    // readers of one peering cannot disagree.
    let (_, listed) = emulator
        .get(&format!(
            "/global-networks/{global_network}/network-resources?resourceType={PEERING}"
        ))
        .await;
    let definition = listed["NetworkResources"][0]["Definition"]
        .as_str()
        .unwrap_or_else(|| panic!("no peering Definition in {listed}"));
    let definition: Value =
        serde_json::from_str(definition).unwrap_or_else(|error| panic!("{error}: {definition}"));
    assert_eq!(
        definition["PeeringId"], peering_shape["Peering"]["PeeringId"],
        "the peering definition must be the peering its own reader answers, got {definition}"
    );
    assert_eq!(
        definition["EdgeLocation"],
        json!(case.edge_location),
        "the peering definition must carry the stored edge location, got {definition}"
    );
}
