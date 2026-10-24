// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of the Direct Connect gateway attachment slice —
//! `CreateDirectConnectGatewayAttachment`, `GetDirectConnectGatewayAttachment` and
//! `UpdateDirectConnectGatewayAttachment` — driven over HTTP through the real router (the
//! harness lives in `crate::global_network_lifecycle`, the core-network-with-edges helper
//! in `crate::attachment_lifecycle` and the acceptance-policy helper in
//! `crate::attachment_acceptance`).
//!
//! The rule under test: this is the ONE attachment kind that spans SEVERAL edge locations
//! of its core network, so the stored record holds the LIST its request named and the
//! modelled envelope reports it as `Attachment$EdgeLocations` with `Attachment$EdgeLocation`
//! absent ("This is returned only for Direct Connect gateway attachments. All other
//! attachment types retrun EdgeLocation", `smithy_query query="doc" shape="Attachment"
//! member="EdgeLocations"`); an update OVERWRITES that list with the one it names ("The
//! updated array of edge locations overwrites the previous array of locations",
//! `smithy_query query="doc" shape="UpdateDirectConnectGatewayAttachmentRequest"
//! member="EdgeLocations"`), keeping its order, leaving everything else as it was, and the
//! attachment then reads `UPDATING` until the transition the clock owes falls due and
//! `AVAILABLE` after it; every location named has to be one the core network's policy
//! configures an edge for, and an update that names no list at all changes nothing.
//!
//! The tables below span that class: lists differing in LENGTH (one, two and three
//! locations), in ORDER (the same set reordered), in direction of change (grown, shrunk,
//! replaced wholesale, left as it stands, not named at all) and in the width of an
//! `ExternalRegionCode` (an ordinary Region code and the 63-character bound the modelled
//! member publishes); and refusals differing in which value or reference is at fault and
//! in the state the attachment is in. Nothing reaches into `crate::state`: every
//! attachment is created, updated and read back through an operation.

use serde_json::{json, Value};

use crate::attachment_acceptance::core_network_with;
use crate::attachment_lifecycle::{core_network_at, create_vpc_attachment, subnet_arn, vpc_arn};
use crate::global_network_lifecycle::{
    error_code, expected_arn, is_minted_id, Emulator, PAST_SETTLE,
};
use crate::resource_tagging::tags_uri;

/// The ARN resource type (and id prefix) of an attachment:
/// `pattern ^attachment-([0-9a-f]{8,17})$` on `AttachmentId`
/// (`smithy_query shape=AttachmentId`).
const ATTACHMENT: &str = "attachment";

/// The `AttachmentType` a Direct Connect gateway attachment reports, one of the five the
/// enum carries (`smithy_query shape=AttachmentType`).
const DIRECT_CONNECT_GATEWAY: &str = "DIRECT_CONNECT_GATEWAY";

/// The `AttachmentState` values this suite observes (`smithy_query shape=AttachmentState`).
const CREATING: &str = "CREATING";
/// See [`CREATING`].
const AVAILABLE: &str = "AVAILABLE";
/// See [`CREATING`].
const UPDATING: &str = "UPDATING";
/// See [`CREATING`].
const PENDING_ACCEPTANCE: &str = "PENDING_ATTACHMENT_ACCEPTANCE";

/// The `CreateDirectConnectGatewayAttachment` route
/// (`POST /direct-connect-gateway-attachments`).
const CREATE_URI: &str = "/direct-connect-gateway-attachments";

/// The ordinary Region codes the core networks of this suite configure edges in.
const REGIONS: [&str; 4] = ["us-west-2", "eu-central-1", "ap-southeast-4", "sa-east-1"];

/// The `GetDirectConnectGatewayAttachment` / `UpdateDirectConnectGatewayAttachment` URI of
/// `attachment_id` (`GET` and `PATCH /direct-connect-gateway-attachments/{AttachmentId}`).
fn attachment_uri(attachment_id: &str) -> String {
    format!("{CREATE_URI}/{attachment_id}")
}

/// The widest `ExternalRegionCode` the model admits: `length 1..63` on the member of
/// `ExternalRegionCodeList` (`smithy_query shape=ExternalRegionCodeList`).
fn widest_region_code() -> String {
    "r".repeat(63)
}

/// How many edge locations one attachment may span: the bound
/// `state::MAX_EDGE_LOCATIONS_PER_ATTACHMENT` fixes, which is what keeps a caller from
/// growing one attachment's list without end.
const MAX_EDGE_LOCATIONS: usize = 50;

/// A Direct Connect gateway ARN carrying the gateway id `index`.
///
/// The form is the modelled one:
/// `^arn:[^:]{1,63}:directconnect::[^:]{0,63}:dx-gateway\/[0-9a-f]{8}-([0-9a-f]{4}-){3}[0-9a-f]{12}$`
/// (`smithy_query shape=DirectConnectGatewayArn`) — note the EMPTY region segment — so
/// every row below satisfies the pattern and reaches the handler.
pub(crate) fn gateway_arn(index: usize) -> String {
    format!(
        "arn:aws:directconnect::{}:dx-gateway/{index:08x}-0000-0000-0000-{index:012x}",
        engine::emulator_account_id()
    )
}

/// Every edge location the suite's core networks configure: the Region codes above plus
/// the widest one the model admits.
fn all_edges() -> Vec<String> {
    let mut edges: Vec<String> = REGIONS.iter().map(|region| region.to_string()).collect();
    edges.push(widest_region_code());
    edges
}

/// Create a core network configuring an edge in every location of [`all_edges`], and
/// answer its id.
async fn core_network_everywhere(emulator: &Emulator, name: &str) -> String {
    let edges = all_edges();
    let borrowed: Vec<&str> = edges.iter().map(String::as_str).collect();
    core_network_at(emulator, name, &borrowed).await.1
}

/// `CreateDirectConnectGatewayAttachment` of `gateway` on `core_network_id` at
/// `edge_locations`, with no optional member, and the minted attachment id.
async fn create_attachment(
    emulator: &Emulator,
    case: &str,
    core_network_id: &str,
    gateway: &str,
    edge_locations: &[String],
) -> String {
    let (status, created) = emulator
        .post(
            CREATE_URI,
            json!({
                "CoreNetworkId": core_network_id,
                "DirectConnectGatewayArn": gateway,
                "EdgeLocations": edge_locations,
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateDirectConnectGatewayAttachment expected 200, got {status} with {created}"
    );
    created["DirectConnectGatewayAttachment"]["Attachment"]["AttachmentId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no AttachmentId in {created}"))
        .to_string()
}

/// One row of the create table: the edge locations the attachment spans and the optional
/// members the request carries.
struct CreateCase {
    /// Names the case in every assertion message.
    name: &'static str,
    edge_locations: Vec<String>,
    /// The `Tags` the request carries (`Value::Null` for no `Tags` member) and the list the
    /// attachment must then report.
    tags: Value,
    expected_tags: Value,
    routing_policy_label: Option<&'static str>,
}

/// The table: one row per form the spanned list of edge locations takes, and per
/// combination of the optional members a create carries.
fn create_cases() -> Vec<CreateCase> {
    vec![
        CreateCase {
            name: "a single edge location, no optional members",
            edge_locations: vec![REGIONS[0].to_string()],
            tags: Value::Null,
            expected_tags: json!([]),
            routing_policy_label: None,
        },
        CreateCase {
            name: "three edge locations, in an order of their own, with tags and a label",
            edge_locations: vec![
                REGIONS[3].to_string(),
                REGIONS[1].to_string(),
                REGIONS[2].to_string(),
            ],
            tags: json!([{ "Key": "env", "Value": "test" }, { "Key": "team", "Value": "net" }]),
            expected_tags: json!([
                { "Key": "env", "Value": "test" },
                { "Key": "team", "Value": "net" },
            ]),
            routing_policy_label: Some("east-west"),
        },
        CreateCase {
            name: "two edge locations, one of them the 63-character code the bound allows",
            edge_locations: vec![widest_region_code(), REGIONS[1].to_string()],
            tags: json!([]),
            expected_tags: json!([]),
            routing_policy_label: Some("widest"),
        },
    ]
}

/// The `CreateDirectConnectGatewayAttachment` body for `case`, on `core_network_id`.
fn create_body(case: &CreateCase, core_network_id: &str, gateway: &str) -> Value {
    let mut body = json!({
        "CoreNetworkId": core_network_id,
        "DirectConnectGatewayArn": gateway,
        "EdgeLocations": case.edge_locations,
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

#[tokio::test]
async fn a_direct_connect_gateway_attachment_round_trips_through_stored_state() {
    let emulator = Emulator::new();
    let core_network_id = core_network_everywhere(&emulator, "gateway attachments").await;
    let cases = create_cases();
    let mut created: Vec<(&CreateCase, String, Value)> = Vec::new();

    for (index, case) in cases.iter().enumerate() {
        let gateway = gateway_arn(index + 1);
        let (status, response) = emulator
            .post(CREATE_URI, create_body(case, &core_network_id, &gateway))
            .await;
        assert_eq!(
            status, 200,
            "case {:?}: CreateDirectConnectGatewayAttachment expected 200, got {status} with {response}",
            case.name
        );
        let created_shape = &response["DirectConnectGatewayAttachment"];
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
            "case {:?}: the attachment must back-reference its core network, got {attachment}",
            case.name
        );
        assert_eq!(
            attachment["CoreNetworkArn"],
            json!(expected_arn("core-network", &core_network_id)),
            "case {:?}: the core network ARN must name that core network, got {attachment}",
            case.name
        );
        assert_eq!(
            attachment["AttachmentType"],
            json!(DIRECT_CONNECT_GATEWAY),
            "case {:?}: expected the Direct Connect gateway attachment type, got {attachment}",
            case.name
        );
        assert_eq!(
            attachment["OwnerAccountId"],
            json!(engine::emulator_account_id()),
            "case {:?}: the owner is the emulator account, got {attachment}",
            case.name
        );
        assert_eq!(
            attachment["EdgeLocations"],
            json!(case.edge_locations),
            "case {:?}: the edge locations are the ones named, in that order, got {attachment}",
            case.name
        );
        assert!(
            attachment["EdgeLocation"].is_null(),
            "case {:?}: this kind reports EdgeLocations, never EdgeLocation, got {attachment}",
            case.name
        );
        assert_eq!(
            attachment["ResourceArn"],
            json!(gateway),
            "case {:?}: the attached resource is the Direct Connect gateway, got {attachment}",
            case.name
        );
        assert_eq!(
            created_shape["DirectConnectGatewayArn"],
            json!(gateway),
            "case {:?}: the shape must report the attached gateway, got {created_shape}",
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
        let (status, read) = emulator.get(&attachment_uri(&id)).await;
        assert_eq!(
            status, 200,
            "case {:?}: GetDirectConnectGatewayAttachment expected 200, got {status} with {read}",
            case.name
        );
        assert_eq!(
            read["DirectConnectGatewayAttachment"], *created_shape,
            "case {:?}: the reader must answer the stored attachment, got {read}",
            case.name
        );

        // The attachment's own ARN is not a member of the shape, so it is read back the one
        // way a client can: through the tagging slice, which resolves the ARN to this
        // stored attachment and its tags.
        let (status, listed) = emulator
            .get(&tags_uri(&expected_arn(ATTACHMENT, &id)))
            .await;
        assert_eq!(
            status, 200,
            "case {:?}: ListTagsForResource expected 200, got {status} with {listed}",
            case.name
        );
        assert_eq!(
            listed["TagList"], case.expected_tags,
            "case {:?}: the attachment ARN must resolve to the stored attachment's tags, got {listed}",
            case.name
        );

        created.push((case, id, created_shape.clone()));
    }

    // Past the settle span the transition the clock owed has fallen due, and nothing the
    // request supplied moved with it.
    tokio::time::sleep(PAST_SETTLE).await;
    for (case, id, at_create) in &created {
        let (status, read) = emulator.get(&attachment_uri(id)).await;
        assert_eq!(
            status, 200,
            "case {:?}: GetDirectConnectGatewayAttachment after the settle span expected 200, got {status} with {read}",
            case.name
        );
        let attachment = &read["DirectConnectGatewayAttachment"]["Attachment"];
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
            "EdgeLocations",
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

        // A read never changes what the next caller sees.
        let (_, again) = emulator.get(&attachment_uri(id)).await;
        assert_eq!(
            again, read,
            "case {:?}: GetDirectConnectGatewayAttachment must be a pure read, got {read} then {again}",
            case.name
        );
    }
}

/// One row of the update table: the `EdgeLocations` the update names (`None` for a request
/// that carries no such member) and the list the attachment must hold afterwards.
struct UpdateCase {
    /// Names the case in every assertion message.
    name: &'static str,
    requested: Option<Vec<String>>,
    expected: Vec<String>,
    /// Whether the row really changes the stored list, which is what owes the
    /// `UPDATING` -> `AVAILABLE` transition.
    changes: bool,
}

/// The table: one row per way of overwriting the stored list, applied in order to one
/// attachment, so each row starts from what the row before it left stored.
fn update_cases() -> Vec<UpdateCase> {
    vec![
        UpdateCase {
            name: "one location replaced by another",
            requested: Some(vec![REGIONS[1].to_string()]),
            expected: vec![REGIONS[1].to_string()],
            changes: true,
        },
        UpdateCase {
            name: "grown to three, kept in the order named",
            requested: Some(vec![
                REGIONS[3].to_string(),
                REGIONS[0].to_string(),
                REGIONS[2].to_string(),
            ]),
            expected: vec![
                REGIONS[3].to_string(),
                REGIONS[0].to_string(),
                REGIONS[2].to_string(),
            ],
            changes: true,
        },
        UpdateCase {
            name: "the same three locations, reordered",
            requested: Some(vec![
                REGIONS[2].to_string(),
                REGIONS[3].to_string(),
                REGIONS[0].to_string(),
            ]),
            expected: vec![
                REGIONS[2].to_string(),
                REGIONS[3].to_string(),
                REGIONS[0].to_string(),
            ],
            changes: true,
        },
        UpdateCase {
            name: "shrunk back to a single location",
            requested: Some(vec![REGIONS[0].to_string()]),
            expected: vec![REGIONS[0].to_string()],
            changes: true,
        },
        UpdateCase {
            name: "the 63-character edge location the length bound allows",
            requested: Some(vec![widest_region_code()]),
            expected: vec![widest_region_code()],
            changes: true,
        },
        UpdateCase {
            name: "the list the attachment already holds, which changes nothing",
            requested: Some(vec![widest_region_code()]),
            expected: vec![widest_region_code()],
            changes: false,
        },
        UpdateCase {
            name: "no EdgeLocations member at all, which asks for no change",
            requested: None,
            expected: vec![widest_region_code()],
            changes: false,
        },
    ]
}

#[tokio::test]
async fn an_update_overwrites_the_edge_locations_the_attachment_spans() {
    let emulator = Emulator::new();
    let core_network_id = core_network_everywhere(&emulator, "updated gateway attachment").await;
    let id = create_attachment(
        &emulator,
        "the attachment under update",
        &core_network_id,
        &gateway_arn(1),
        &[REGIONS[0].to_string()],
    )
    .await;
    // An update is refused while the attachment is still being created, so the table starts
    // from an AVAILABLE attachment.
    tokio::time::sleep(PAST_SETTLE).await;

    let (status, before) = emulator.get(&attachment_uri(&id)).await;
    assert_eq!(
        status, 200,
        "GetDirectConnectGatewayAttachment before the table: expected 200, got {status} with {before}"
    );
    let created_at = before["DirectConnectGatewayAttachment"]["Attachment"]["CreatedAt"]
        .as_f64()
        .unwrap_or_default();
    let mut previous_updated_at = before["DirectConnectGatewayAttachment"]["Attachment"]
        ["UpdatedAt"]
        .as_f64()
        .unwrap_or_default();

    for case in update_cases() {
        let body = match &case.requested {
            Some(locations) => json!({ "EdgeLocations": locations }),
            None => json!({}),
        };
        let (status, answer) = emulator.patch(&attachment_uri(&id), body).await;
        assert_eq!(
            status, 200,
            "case {:?}: UpdateDirectConnectGatewayAttachment expected 200, got {status} with {answer}",
            case.name
        );
        let shape = &answer["DirectConnectGatewayAttachment"];
        let attachment = &shape["Attachment"];
        assert_eq!(
            attachment["EdgeLocations"],
            json!(case.expected),
            "case {:?}: the update overwrites the stored list, got {attachment}",
            case.name
        );
        assert!(
            attachment["EdgeLocation"].is_null(),
            "case {:?}: this kind reports EdgeLocations, never EdgeLocation, got {attachment}",
            case.name
        );
        assert_eq!(
            attachment["State"],
            json!(if case.changes { UPDATING } else { AVAILABLE }),
            "case {:?}: a change owes the UPDATING transition and no change owes none, got {attachment}",
            case.name
        );
        assert_eq!(
            attachment["AttachmentId"],
            json!(id),
            "case {:?}: the update answers the attachment addressed, got {attachment}",
            case.name
        );
        assert_eq!(
            attachment["AttachmentType"],
            json!(DIRECT_CONNECT_GATEWAY),
            "case {:?}: the kind cannot change under an update, got {attachment}",
            case.name
        );
        assert_eq!(
            shape["DirectConnectGatewayArn"],
            json!(gateway_arn(1)),
            "case {:?}: the attached gateway cannot change under an update, got {shape}",
            case.name
        );
        assert_eq!(
            attachment["CreatedAt"].as_f64().unwrap_or_default(),
            created_at,
            "case {:?}: CreatedAt cannot move under an update, got {attachment}",
            case.name
        );

        // The reader answers the list the update stored, not the one the request named.
        let (status, read) = emulator.get(&attachment_uri(&id)).await;
        assert_eq!(
            status, 200,
            "case {:?}: GetDirectConnectGatewayAttachment expected 200, got {status} with {read}",
            case.name
        );
        assert_eq!(
            read["DirectConnectGatewayAttachment"]["Attachment"]["EdgeLocations"],
            json!(case.expected),
            "case {:?}: the reader must answer the stored list, got {read}",
            case.name
        );

        // Past the settle span the transition the update owed has fallen due, and the list
        // it stored is still the stored one.
        tokio::time::sleep(PAST_SETTLE).await;
        let (_, settled) = emulator.get(&attachment_uri(&id)).await;
        let settled_attachment = &settled["DirectConnectGatewayAttachment"]["Attachment"];
        assert_eq!(
            settled_attachment["State"],
            json!(AVAILABLE),
            "case {:?}: past the settle span the attachment is AVAILABLE again, got {settled_attachment}",
            case.name
        );
        assert_eq!(
            settled_attachment["EdgeLocations"],
            json!(case.expected),
            "case {:?}: the stored list outlives the transition, got {settled_attachment}",
            case.name
        );
        let updated_at = settled_attachment["UpdatedAt"].as_f64().unwrap_or_default();
        if case.changes {
            assert!(
                updated_at > previous_updated_at,
                "case {:?}: a change moves UpdatedAt past {previous_updated_at}, got {updated_at}",
                case.name
            );
        } else {
            assert!(
                (updated_at - previous_updated_at).abs() < f64::EPSILON,
                "case {:?}: no change leaves UpdatedAt at {previous_updated_at}, got {updated_at}",
                case.name
            );
        }
        previous_updated_at = updated_at;
    }
}

#[tokio::test]
async fn an_update_is_refused_when_the_attachment_or_a_location_does_not_resolve() {
    let emulator = Emulator::new();
    let core_network_id = core_network_everywhere(&emulator, "refused updates").await;
    let id = create_attachment(
        &emulator,
        "the attachment under update",
        &core_network_id,
        &gateway_arn(1),
        &[REGIONS[0].to_string()],
    )
    .await;

    // While the attachment is still being created, a modification of it is the modelled
    // conflict: it would leave the resource in an inconsistent state.
    let (status, body) = emulator
        .patch(
            &attachment_uri(&id),
            json!({ "EdgeLocations": [REGIONS[1]] }),
        )
        .await;
    assert_eq!(
        status, 409,
        "case an attachment still CREATING: expected 409, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "ConflictException",
        "case an attachment still CREATING: wrong error code in {body}"
    );
    assert_eq!(
        body["ResourceId"],
        json!(id),
        "case an attachment still CREATING: the refusal must name the attachment, got {body}"
    );
    assert_eq!(
        body["ResourceType"],
        json!(ATTACHMENT),
        "case an attachment still CREATING: the refusal must name the resource type, got {body}"
    );
    tokio::time::sleep(PAST_SETTLE).await;

    // Each row is a different way of naming no stored Direct Connect gateway attachment,
    // every one satisfying the modelled `AttachmentId` pattern so it reaches the handler: a
    // never-minted id at each width the pattern allows, and the counter value a DIFFERENT
    // resource kind of this service was minted under, spelled with the attachment prefix.
    let borrowed_suffix = format!(
        "attachment-{}",
        core_network_id
            .strip_prefix("core-network-")
            .unwrap_or_default()
    );
    for (case, attachment_id) in [
        ("never minted, 17 hex digits", "attachment-00000000000000099"),
        ("never minted, the 8-hex lower bound", "attachment-0000beef"),
        ("another counter value under this prefix", &borrowed_suffix),
    ] {
        let (status, body) = emulator
            .patch(
                &attachment_uri(attachment_id),
                json!({ "EdgeLocations": [REGIONS[1]] }),
            )
            .await;
        assert_eq!(
            status, 404,
            "case {case} ({attachment_id:?}): expected 404, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case} ({attachment_id:?}): wrong error code in {body}"
        );
        assert_eq!(
            body["Message"],
            json!("Resource not found."),
            "case {case} ({attachment_id:?}): wrong message in {body}"
        );
        assert_eq!(
            body["ResourceId"],
            json!(attachment_id),
            "case {case} ({attachment_id:?}): the refusal must name the id asked for, got {body}"
        );
        assert_eq!(
            body["ResourceType"],
            json!(ATTACHMENT),
            "case {case} ({attachment_id:?}): the refusal must name the resource type, got {body}"
        );
    }

    // An id that names an attachment of ANOTHER kind names no Direct Connect gateway
    // attachment either: the readers and writers of the kinds share one id space, and each
    // answers only its own.
    let (status, vpc) = create_vpc_attachment(
        &emulator,
        json!({
            "CoreNetworkId": core_network_id,
            "VpcArn": vpc_arn(REGIONS[0], 1),
            "SubnetArns": [subnet_arn(REGIONS[0], 1)],
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
    let (status, body) = emulator
        .patch(
            &attachment_uri(&vpc_id),
            json!({ "EdgeLocations": [REGIONS[1]] }),
        )
        .await;
    assert_eq!(
        status, 404,
        "case a VPC attachment's id: expected 404, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "case a VPC attachment's id: wrong error code in {body}"
    );

    // Each row is a different WAY of naming a list no attachment can span, and every one
    // names `edgeLocations` as the member at fault: the empty list the modelled
    // `ExternalRegionCodeList` admits, a Region the core network's policy configures no
    // edge for, a one-character code (the length bound's lower end) that configures none
    // either, and a list whose last entry alone is unconfigured.
    let rows: Vec<(&str, Value, &str)> = vec![
        ("the empty list", json!([]), "FieldValidationFailed"),
        (
            "a Region the policy configures no edge for",
            json!(["us-east-1"]),
            "Other",
        ),
        (
            "the one-character lower bound of the code length",
            json!(["r"]),
            "Other",
        ),
        (
            "a list whose last entry alone is unconfigured",
            json!([REGIONS[1], REGIONS[2], "ca-central-1"]),
            "Other",
        ),
        (
            "one configured location named twice",
            json!([REGIONS[1], REGIONS[1]]),
            "Other",
        ),
    ];
    for (case, locations, reason) in rows {
        let (status, body) = emulator
            .patch(
                &attachment_uri(&id),
                json!({ "EdgeLocations": locations.clone() }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case} ({locations}): expected 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case} ({locations}): wrong error code in {body}"
        );
        assert_eq!(
            body["Reason"],
            json!(reason),
            "case {case} ({locations}): wrong Reason in {body}"
        );
        assert_eq!(
            body["Fields"][0]["Name"],
            json!("edgeLocations"),
            "case {case} ({locations}): the refusal must name the member at fault, got {body}"
        );
    }

    // Every refusal above stored NOTHING: the attachment still spans the list it was
    // created with, and is still AVAILABLE rather than applying a change.
    let (status, read) = emulator.get(&attachment_uri(&id)).await;
    assert_eq!(
        status, 200,
        "GetDirectConnectGatewayAttachment after the refusals: expected 200, got {status} with {read}"
    );
    let attachment = &read["DirectConnectGatewayAttachment"]["Attachment"];
    assert_eq!(
        attachment["EdgeLocations"],
        json!([REGIONS[0]]),
        "a refused update must leave the stored list alone, got {attachment}"
    );
    assert_eq!(
        attachment["State"],
        json!(AVAILABLE),
        "a refused update must leave the attachment AVAILABLE, got {attachment}"
    );

    // The third state an update is refused in: one still applying an EARLIER change. The
    // accepted update below leaves the attachment UPDATING for the settle span, so a second
    // one inside that span is the same conflict, and the list the accepted one stored is
    // what the attachment still spans afterwards.
    let (status, accepted) = emulator
        .patch(
            &attachment_uri(&id),
            json!({ "EdgeLocations": [REGIONS[1], REGIONS[2]] }),
        )
        .await;
    assert_eq!(
        status, 200,
        "the accepted update: expected 200, got {status} with {accepted}"
    );
    assert_eq!(
        accepted["DirectConnectGatewayAttachment"]["Attachment"]["State"],
        json!(UPDATING),
        "the accepted update leaves the attachment UPDATING, got {accepted}"
    );
    let (status, body) = emulator
        .patch(
            &attachment_uri(&id),
            json!({ "EdgeLocations": [REGIONS[3]] }),
        )
        .await;
    assert_eq!(
        status, 409,
        "case an attachment still UPDATING: expected 409, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "ConflictException",
        "case an attachment still UPDATING: wrong error code in {body}"
    );
    assert_eq!(
        body["ResourceId"],
        json!(id),
        "case an attachment still UPDATING: the refusal must name the attachment, got {body}"
    );
    tokio::time::sleep(PAST_SETTLE).await;
    let (_, settled) = emulator.get(&attachment_uri(&id)).await;
    let settled_attachment = &settled["DirectConnectGatewayAttachment"]["Attachment"];
    assert_eq!(
        settled_attachment["EdgeLocations"],
        json!([REGIONS[1], REGIONS[2]]),
        "the refused second update must leave the first one's list stored, got {settled_attachment}"
    );
    assert_eq!(
        settled_attachment["State"],
        json!(AVAILABLE),
        "past the settle span the attachment is AVAILABLE again, got {settled_attachment}"
    );
}

#[tokio::test]
async fn the_number_of_edge_locations_one_attachment_spans_is_bounded() {
    let emulator = Emulator::new();
    // A core network configuring exactly as many edges as one attachment may span, so a
    // list AT the bound is a list of distinct configured locations and the bound itself is
    // what the row past it runs into.
    let edges: Vec<String> = (1..=MAX_EDGE_LOCATIONS)
        .map(|index| format!("edge-{index:02}"))
        .collect();
    let borrowed: Vec<&str> = edges.iter().map(String::as_str).collect();
    let core_network_id = core_network_at(&emulator, "every edge", &borrowed).await.1;

    // AT the bound: accepted, and the whole list is what the attachment spans.
    let (status, created) = emulator
        .post(
            CREATE_URI,
            json!({
                "CoreNetworkId": core_network_id,
                "DirectConnectGatewayArn": gateway_arn(1),
                "EdgeLocations": edges,
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case a list at the bound ({MAX_EDGE_LOCATIONS}): expected 200, got {status} with {created}"
    );
    let attachment = &created["DirectConnectGatewayAttachment"]["Attachment"];
    assert_eq!(
        attachment["EdgeLocations"],
        json!(edges),
        "case a list at the bound: every location named must be stored, got {attachment}"
    );
    let id = attachment["AttachmentId"]
        .as_str()
        .unwrap_or_else(|| panic!("no AttachmentId in {created}"))
        .to_string();

    // One past the bound: refused, on the length alone — the extra entry below is a
    // location the core network does configure, so nothing but the count is at fault.
    let mut past_bound = edges.clone();
    past_bound.push(edges[0].clone());
    let (status, body) = emulator
        .post(
            CREATE_URI,
            json!({
                "CoreNetworkId": core_network_id,
                "DirectConnectGatewayArn": gateway_arn(2),
                "EdgeLocations": past_bound,
            }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case a create one past the bound: expected 400, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "case a create one past the bound: wrong error code in {body}"
    );
    assert_eq!(
        body["Reason"],
        json!("FieldValidationFailed"),
        "case a create one past the bound: wrong Reason in {body}"
    );
    assert_eq!(
        body["Fields"][0]["Name"],
        json!("edgeLocations"),
        "case a create one past the bound: the refusal must name the member at fault, got {body}"
    );

    // The update is held to the same bound, and the refusal stores nothing.
    tokio::time::sleep(PAST_SETTLE).await;
    let (status, body) = emulator
        .patch(
            &attachment_uri(&id),
            json!({ "EdgeLocations": past_bound }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case an update one past the bound: expected 400, got {status} with {body}"
    );
    assert_eq!(
        body["Reason"],
        json!("FieldValidationFailed"),
        "case an update one past the bound: wrong Reason in {body}"
    );
    let (status, read) = emulator.get(&attachment_uri(&id)).await;
    assert_eq!(
        status, 200,
        "GetDirectConnectGatewayAttachment after the refusals: expected 200, got {status} with {read}"
    );
    let read_attachment = &read["DirectConnectGatewayAttachment"]["Attachment"];
    assert_eq!(
        read_attachment["EdgeLocations"],
        json!(edges),
        "a refused update must leave the stored list alone, got {read_attachment}"
    );
    assert_eq!(
        read_attachment["State"],
        json!(AVAILABLE),
        "a refused update must leave the attachment AVAILABLE, got {read_attachment}"
    );
}

#[tokio::test]
async fn an_attachment_waiting_for_acceptance_cannot_have_its_locations_updated() {
    let emulator = Emulator::new();
    // A policy whose one `attachment-policies` rule claims every attachment and asks for
    // the attachment request to be accepted, so the created attachment waits in
    // PENDING_ATTACHMENT_ACCEPTANCE instead of being created.
    let core_network_id = core_network_with(
        &emulator,
        "acceptance before update",
        REGIONS[0],
        &json!([{
            "rule-number": 100,
            "conditions": [{ "type": "any" }],
            "action": {
                "association-method": "constant",
                "segment": "shared",
                "require-attachment-acceptance": true,
            },
        }]),
    )
    .await;
    let (status, created) = emulator
        .post(
            CREATE_URI,
            json!({
                "CoreNetworkId": core_network_id,
                "DirectConnectGatewayArn": gateway_arn(1),
                "EdgeLocations": [REGIONS[0]],
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "CreateDirectConnectGatewayAttachment: expected 200, got {status} with {created}"
    );
    let attachment = &created["DirectConnectGatewayAttachment"]["Attachment"];
    assert_eq!(
        attachment["State"],
        json!(PENDING_ACCEPTANCE),
        "the claiming rule asks for acceptance, so the attachment waits for it, got {attachment}"
    );
    let id = attachment["AttachmentId"]
        .as_str()
        .unwrap_or_else(|| panic!("no AttachmentId in {created}"))
        .to_string();

    let (status, body) = emulator
        .patch(&attachment_uri(&id), json!({ "EdgeLocations": [REGIONS[0]] }))
        .await;
    assert_eq!(
        status, 409,
        "case an attachment waiting for acceptance: expected 409, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "ConflictException",
        "case an attachment waiting for acceptance: wrong error code in {body}"
    );
    assert_eq!(
        body["ResourceId"],
        json!(id),
        "case an attachment waiting for acceptance: the refusal must name the attachment, got {body}"
    );

    // The refusal stored nothing: the attachment is still waiting for its acceptance.
    let (status, read) = emulator.get(&attachment_uri(&id)).await;
    assert_eq!(
        status, 200,
        "GetDirectConnectGatewayAttachment: expected 200, got {status} with {read}"
    );
    assert_eq!(
        read["DirectConnectGatewayAttachment"]["Attachment"]["State"],
        json!(PENDING_ACCEPTANCE),
        "a refused update must leave the attachment waiting, got {read}"
    );
}

#[tokio::test]
async fn a_create_is_refused_when_its_core_network_or_an_edge_location_does_not_resolve() {
    let emulator = Emulator::new();
    let core_network_id = core_network_everywhere(&emulator, "refused creates").await;

    // Each row is a different WAY of being invalid, and names the member at fault: a core
    // network that never existed, the empty edge-location list the modelled list shape
    // admits, and a location the core network's policy configures no edge for. A
    // `CoreNetworkId` or `DirectConnectGatewayArn` outside its modelled pattern is refused
    // by the schema before the handler runs, so neither is a row here.
    let rows: Vec<(&str, Value, &str, &str)> = vec![
        (
            "a core network that never existed",
            json!({
                "CoreNetworkId": "core-network-00000000000000099",
                "DirectConnectGatewayArn": gateway_arn(9),
                "EdgeLocations": [REGIONS[0]],
            }),
            "coreNetworkId",
            "FieldValidationFailed",
        ),
        (
            "no edge location at all",
            json!({
                "CoreNetworkId": core_network_id,
                "DirectConnectGatewayArn": gateway_arn(10),
                "EdgeLocations": [],
            }),
            "edgeLocations",
            "FieldValidationFailed",
        ),
        (
            "an edge location the policy configures none for",
            json!({
                "CoreNetworkId": core_network_id,
                "DirectConnectGatewayArn": gateway_arn(11),
                "EdgeLocations": [REGIONS[0], "us-east-1"],
            }),
            "edgeLocations",
            "Other",
        ),
    ];
    for (case, body, field, reason) in rows {
        let (status, answer) = emulator.post(CREATE_URI, body).await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400, got {status} with {answer}"
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case {case}: wrong error code in {answer}"
        );
        assert_eq!(
            answer["Reason"],
            json!(reason),
            "case {case}: wrong Reason in {answer}"
        );
        assert_eq!(
            answer["Fields"][0]["Name"],
            json!(field),
            "case {case}: the refusal must name the member at fault, got {answer}"
        );
    }

    // A gateway is attached by at most one attachment, so attaching the same one twice is
    // the service's concurrent-state conflict and the second call stores nothing.
    let gateway = gateway_arn(1);
    let id = create_attachment(
        &emulator,
        "the first attachment of the gateway",
        &core_network_id,
        &gateway,
        &[REGIONS[0].to_string()],
    )
    .await;
    let (status, body) = emulator
        .post(
            CREATE_URI,
            json!({
                "CoreNetworkId": core_network_id,
                "DirectConnectGatewayArn": gateway,
                "EdgeLocations": [REGIONS[1]],
            }),
        )
        .await;
    assert_eq!(
        status, 409,
        "case the same gateway twice: expected 409, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "ConflictException",
        "case the same gateway twice: wrong error code in {body}"
    );
    assert_eq!(
        body["ResourceId"],
        json!(id),
        "case the same gateway twice: the refusal must name the attachment holding it, got {body}"
    );
}

#[tokio::test]
async fn a_repeated_create_with_one_client_token_mints_one_attachment() {
    let emulator = Emulator::new();
    let core_network_id = core_network_everywhere(&emulator, "idempotent creates").await;
    let body = json!({
        "CoreNetworkId": core_network_id,
        "DirectConnectGatewayArn": gateway_arn(1),
        "EdgeLocations": [REGIONS[0], REGIONS[1]],
        "ClientToken": "one-token",
    });

    let (status, first) = emulator.post(CREATE_URI, body.clone()).await;
    assert_eq!(
        status, 200,
        "the first create: expected 200, got {status} with {first}"
    );
    let (status, retried) = emulator.post(CREATE_URI, body).await;
    assert_eq!(
        status, 200,
        "the retry: expected 200, got {status} with {retried}"
    );
    assert_eq!(
        retried["DirectConnectGatewayAttachment"]["Attachment"]["AttachmentId"],
        first["DirectConnectGatewayAttachment"]["Attachment"]["AttachmentId"],
        "a retry of one request answers the attachment it minted, got {retried}"
    );

    // The same token asking for a DIFFERENT request is the modelled conflict.
    let (status, conflict) = emulator
        .post(
            CREATE_URI,
            json!({
                "CoreNetworkId": core_network_id,
                "DirectConnectGatewayArn": gateway_arn(1),
                "EdgeLocations": [REGIONS[1], REGIONS[0]],
                "ClientToken": "one-token",
            }),
        )
        .await;
    assert_eq!(
        status, 409,
        "case the same token, another list: expected 409, got {status} with {conflict}"
    );
    assert_eq!(
        error_code(&conflict),
        "ConflictException",
        "case the same token, another list: wrong error code in {conflict}"
    );
}
