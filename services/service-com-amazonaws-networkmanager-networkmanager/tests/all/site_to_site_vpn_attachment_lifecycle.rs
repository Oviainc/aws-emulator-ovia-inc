// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of the site-to-site VPN attachment slice —
//! `CreateSiteToSiteVpnAttachment` and `GetSiteToSiteVpnAttachment` — driven over HTTP
//! through the real router (the harness lives in `crate::global_network_lifecycle`, and the
//! core-network-with-edges helper in `crate::attachment_lifecycle`).
//!
//! The rule under test: `CreateSiteToSiteVpnAttachment` MINTS an attachment of the
//! `SITE_TO_SITE_VPN` type on ONE edge location of the core network it names — the edge
//! location being the Region of the VPN connection the request attaches, which the core
//! network's policy has to configure an edge for — stores it with an
//! `attachment-<17 hex>` id and this service's ARN for that id, reports the attached VPN
//! connection as both `ResourceArn` and `VpnConnectionArn`, waits in `CREATING` until the
//! transition the clock owes falls due and is `AVAILABLE` after it; and
//! `GetSiteToSiteVpnAttachment` reports that one stored record, answering an id of any
//! other attachment kind as not found, without changing what the next caller sees.
//! Nothing reaches into `crate::state`: every attachment is created and read back through
//! an operation.
//!
//! The tables below span that class: VPN connection ARNs differing in their Region
//! segment (several Regions and the 63-character bound the modelled pattern allows), in
//! partition, in whether they carry an account segment, and in the width of the VPN
//! connection id at each bound the pattern publishes; attachments carrying tags and a
//! routing policy label and attachments carrying neither; attachments on two different
//! core networks; and refusals differing in which reference or value is at fault.

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

/// The `AttachmentType` a site-to-site VPN attachment reports, one of the five the enum
/// carries (`smithy_query shape=AttachmentType`).
const SITE_TO_SITE_VPN: &str = "SITE_TO_SITE_VPN";

/// The two `AttachmentState` values a freshly created attachment passes through
/// (`smithy_query shape=AttachmentState`).
const CREATING: &str = "CREATING";
/// See [`CREATING`].
const AVAILABLE: &str = "AVAILABLE";

/// How many attachments one core network holds before a create is refused: the bound
/// `state::MAX_ATTACHMENTS_PER_CORE_NETWORK` fixes.
const MAX_ATTACHMENTS: usize = 100;

/// The `CreateSiteToSiteVpnAttachment` route (`POST /site-to-site-vpn-attachments`).
const CREATE_URI: &str = "/site-to-site-vpn-attachments";

/// The `GetSiteToSiteVpnAttachment` URI of `attachment_id`
/// (`GET /site-to-site-vpn-attachments/{AttachmentId}`).
fn get_uri(attachment_id: &str) -> String {
    format!("{CREATE_URI}/{attachment_id}")
}

/// A VPN connection ARN in `region`, carrying `account` and the connection id `vpn_id`.
///
/// The form is the modelled one:
/// `^arn:[^:]{1,63}:ec2:[^:]{0,63}:[^:]{0,63}:vpn-connection\/vpn-[0-9a-f]{8,17}$`
/// (`smithy_query shape=VpnConnectionArn`), so every row below satisfies the pattern and
/// reaches the handler.
pub(crate) fn vpn_arn(partition: &str, region: &str, account: &str, vpn_id: &str) -> String {
    format!("arn:{partition}:ec2:{region}:{account}:vpn-connection/vpn-{vpn_id}")
}

/// One row of the create table: the VPN connection ARN being attached, the edge location
/// its Region names, and the optional members the request carries.
struct VpnCase {
    /// Names the case in every assertion message.
    name: &'static str,
    /// Which core network of the two the attachment goes on (index into the pair).
    core_network: usize,
    vpn_connection_arn: String,
    /// The `EdgeLocation` the answer must carry: the Region of the ARN above, which the
    /// row's core network configures an edge for.
    edge_location: String,
    /// The `Tags` the request carries (`Value::Null` for no `Tags` member) and the list
    /// the attachment must then report.
    tags: Value,
    expected_tags: Value,
    routing_policy_label: Option<&'static str>,
}

/// The table: one row per form the attached VPN connection's ARN takes, and per
/// combination of the optional members a create carries.
fn vpn_cases() -> Vec<VpnCase> {
    vec![
        VpnCase {
            name: "the 8-hex vpn id the pattern's lower bound allows, no optional members",
            core_network: 0,
            vpn_connection_arn: vpn_arn("aws", "us-west-2", "111122223333", "0123abcd"),
            edge_location: "us-west-2".to_string(),
            tags: Value::Null,
            expected_tags: json!([]),
            routing_policy_label: None,
        },
        VpnCase {
            name: "the 17-hex vpn id at the pattern's upper bound, with tags and a label",
            core_network: 0,
            vpn_connection_arn: vpn_arn("aws", "eu-central-1", "111122223333", "0123456789abcdef0"),
            edge_location: "eu-central-1".to_string(),
            tags: json!([{ "Key": "env", "Value": "test" }, { "Key": "team", "Value": "net" }]),
            expected_tags: json!([
                { "Key": "env", "Value": "test" },
                { "Key": "team", "Value": "net" },
            ]),
            routing_policy_label: Some("east-west"),
        },
        VpnCase {
            name: "another partition and one of its own Regions",
            core_network: 0,
            vpn_connection_arn: vpn_arn("aws-cn", "cn-north-1", "111122223333", "abcdef0123456789a"),
            edge_location: "cn-north-1".to_string(),
            tags: json!([]),
            expected_tags: json!([]),
            routing_policy_label: Some("china"),
        },
        VpnCase {
            name: "an ARN with no account segment, which the pattern allows",
            core_network: 0,
            vpn_connection_arn: vpn_arn("aws", "ap-southeast-4", "", "11111111"),
            edge_location: "ap-southeast-4".to_string(),
            tags: json!([{ "Key": "only", "Value": "" }]),
            expected_tags: json!([{ "Key": "only", "Value": "" }]),
            routing_policy_label: None,
        },
        VpnCase {
            name: "a 63-character Region, the bound the pattern publishes",
            core_network: 1,
            vpn_connection_arn: vpn_arn("aws", &"r".repeat(63), "111122223333", "deadbeef"),
            edge_location: "r".repeat(63),
            tags: Value::Null,
            expected_tags: json!([]),
            routing_policy_label: None,
        },
        VpnCase {
            name: "a second attachment, on the other core network",
            core_network: 1,
            vpn_connection_arn: vpn_arn("aws", "sa-east-1", "111122223333", "cafebabe0"),
            edge_location: "sa-east-1".to_string(),
            tags: json!([{ "Key": "env", "Value": "prod" }]),
            expected_tags: json!([{ "Key": "env", "Value": "prod" }]),
            routing_policy_label: None,
        },
    ]
}

/// The `CreateSiteToSiteVpnAttachment` body for `case`, on `core_network_id`.
fn create_body(case: &VpnCase, core_network_id: &str) -> Value {
    let mut body = json!({
        "CoreNetworkId": core_network_id,
        "VpnConnectionArn": case.vpn_connection_arn,
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
async fn a_site_to_site_vpn_attachment_round_trips_through_stored_state() {
    let emulator = Emulator::new();
    let cases = vpn_cases();
    // Two core networks, each configuring an edge in exactly the Regions its own rows
    // attach in, so a row that landed on the wrong parent would show up.
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
        core_network_at(&emulator, "vpn attachments", &first_edges)
            .await
            .1,
        core_network_at(&emulator, "other vpn attachments", &second_edges)
            .await
            .1,
    ];
    let mut created: Vec<(&VpnCase, String, Value)> = Vec::new();

    for case in &cases {
        let core_network_id = &core_networks[case.core_network];
        let (status, response) = emulator
            .post(CREATE_URI, create_body(case, core_network_id))
            .await;
        assert_eq!(
            status, 200,
            "case {:?}: CreateSiteToSiteVpnAttachment expected 200, got {status} with {response}",
            case.name
        );
        let created_shape = &response["SiteToSiteVpnAttachment"];
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
            json!(expected_arn("core-network", core_network_id)),
            "case {:?}: the core network ARN must name that core network, got {attachment}",
            case.name
        );
        assert_eq!(
            attachment["AttachmentType"],
            json!(SITE_TO_SITE_VPN),
            "case {:?}: expected the site-to-site VPN attachment type, got {attachment}",
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
            "case {:?}: the edge location is the Region of the attached VPN connection, got {attachment}",
            case.name
        );
        assert!(
            attachment["EdgeLocations"].is_null(),
            "case {:?}: a single-edge attachment reports EdgeLocation, not EdgeLocations, got {attachment}",
            case.name
        );
        assert_eq!(
            attachment["ResourceArn"],
            json!(case.vpn_connection_arn),
            "case {:?}: the attached resource is the VPN connection, got {attachment}",
            case.name
        );
        assert_eq!(
            created_shape["VpnConnectionArn"],
            json!(case.vpn_connection_arn),
            "case {:?}: the shape must report the attached VPN connection, got {created_shape}",
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
            "case {:?}: GetSiteToSiteVpnAttachment expected 200, got {status} with {read}",
            case.name
        );
        assert_eq!(
            read["SiteToSiteVpnAttachment"], *created_shape,
            "case {:?}: the reader must answer the stored attachment, got {read}",
            case.name
        );

        // The attachment's own ARN is not a member of the shape, so it is read back the
        // one way a client can: through the tagging slice, which resolves the ARN to this
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
        let (status, read) = emulator.get(&get_uri(id)).await;
        assert_eq!(
            status, 200,
            "case {:?}: GetSiteToSiteVpnAttachment after the settle span expected 200, got {status} with {read}",
            case.name
        );
        let attachment = &read["SiteToSiteVpnAttachment"]["Attachment"];
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
    }

    // A read never changes what the next caller sees.
    for (case, id, _) in &created {
        let (_, first) = emulator.get(&get_uri(id)).await;
        let (_, second) = emulator.get(&get_uri(id)).await;
        assert_eq!(
            first, second,
            "case {:?}: GetSiteToSiteVpnAttachment must be a pure read, got {first} then {second}",
            case.name
        );
    }
}

#[tokio::test]
async fn an_attachment_the_store_does_not_hold_is_refused_with_the_modelled_not_found() {
    let emulator = Emulator::new();
    let (_, core_network_id) = core_network_at(&emulator, "one attachment", &["us-east-1"]).await;
    let (status, created) = emulator
        .post(
            CREATE_URI,
            json!({
                "CoreNetworkId": core_network_id,
                "VpnConnectionArn": vpn_arn("aws", "us-east-1", "111122223333", "0000000a"),
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "CreateSiteToSiteVpnAttachment: expected 200, got {status} with {created}"
    );
    let live = created["SiteToSiteVpnAttachment"]["Attachment"]["AttachmentId"]
        .as_str()
        .unwrap_or_else(|| panic!("no AttachmentId in {created}"))
        .to_string();

    // Each row is a different way of naming no stored site-to-site VPN attachment, every
    // one satisfying the modelled `AttachmentId` pattern so it reaches the handler: a
    // never-minted id at each width the pattern allows, and the counter value a DIFFERENT
    // resource kind of this service was minted under, spelled with the attachment prefix —
    // an id no attachment ever carried.
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
        let (status, body) = emulator.get(&get_uri(attachment_id)).await;
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

    // An id that names an attachment of ANOTHER kind names no site-to-site VPN attachment
    // either: the readers of the two kinds share one id space, and each answers only its
    // own.
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
    let (status, body) = emulator.get(&get_uri(&vpc_id)).await;
    assert_eq!(
        status, 404,
        "case a VPC attachment's id: expected 404, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "case a VPC attachment's id: wrong error code in {body}"
    );

    // The refusals are the lookup and nothing wider: the stored attachment still reads.
    let (status, read) = emulator.get(&get_uri(&live)).await;
    assert_eq!(
        status, 200,
        "GetSiteToSiteVpnAttachment on the live attachment: expected 200, got {status} with {read}"
    );
}

#[tokio::test]
async fn a_create_is_refused_when_its_core_network_or_edge_location_does_not_resolve() {
    let emulator = Emulator::new();
    let (_, live) = core_network_at(&emulator, "live", &["us-east-1"]).await;
    let (_, gone) = core_network_at(&emulator, "to be deleted", &["us-east-1"]).await;
    let (status, deleted) = emulator.delete(&format!("/core-networks/{gone}")).await;
    assert_eq!(
        status, 200,
        "DeleteCoreNetwork: expected 200, got {status} with {deleted}"
    );

    // Each row is a different WAY of being invalid, and names the member at fault: a core
    // network that never existed, one created and then deleted, a VPN connection ARN that
    // names no Region at all (the modelled pattern allows an empty Region segment), and a
    // VPN connection in a Region the core network's policy configures no edge for. A
    // `CoreNetworkId` outside `^core-network-([0-9a-f]{8,17})$` is refused by the schema
    // before the handler runs, so it is not a row here.
    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "a core network that never existed",
            json!({
                "CoreNetworkId": "core-network-00000000000000099",
                "VpnConnectionArn": vpn_arn("aws", "us-east-1", "111122223333", "0000000b"),
            }),
            "coreNetworkId",
        ),
        (
            "a deleted core network",
            json!({
                "CoreNetworkId": gone,
                "VpnConnectionArn": vpn_arn("aws", "us-east-1", "111122223333", "0000000b"),
            }),
            "coreNetworkId",
        ),
        (
            "a VPN connection ARN naming no Region",
            json!({
                "CoreNetworkId": live,
                "VpnConnectionArn": vpn_arn("aws", "", "111122223333", "0000000b"),
            }),
            "vpnConnectionArn",
        ),
        (
            "a Region the core network configures no edge for",
            json!({
                "CoreNetworkId": live,
                "VpnConnectionArn": vpn_arn("aws", "eu-west-3", "111122223333", "0000000b"),
            }),
            "vpnConnectionArn",
        ),
    ];

    for (case, request, member) in rows {
        let (status, body) = emulator.post(CREATE_URI, request).await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case}: wrong error code in {body}"
        );
        assert_eq!(
            body["Fields"][0]["Name"],
            json!(member),
            "case {case}: the refusal must name {member}, got {body}"
        );
    }

    // A refused create stores nothing, so the VPN connection every row above named is
    // still free to attach at an edge the core network does configure.
    let (status, created) = emulator
        .post(
            CREATE_URI,
            json!({
                "CoreNetworkId": live,
                "VpnConnectionArn": vpn_arn("aws", "us-east-1", "111122223333", "0000000b"),
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "CreateSiteToSiteVpnAttachment at a configured edge: expected 200, got {status} with {created}"
    );
}

#[tokio::test]
async fn one_vpn_connection_is_attached_once_and_a_retried_token_mints_nothing_new() {
    let emulator = Emulator::new();
    let (_, core_network_id) = core_network_at(&emulator, "idempotency", &["us-west-2"]).await;
    let vpn = vpn_arn("aws", "us-west-2", "111122223333", "0000cafe");
    let other_vpn = vpn_arn("aws", "us-west-2", "111122223333", "0000beef");
    let body = json!({
        "CoreNetworkId": core_network_id,
        "VpnConnectionArn": vpn,
        "ClientToken": "token-one",
    });

    let (status, first) = emulator.post(CREATE_URI, body.clone()).await;
    assert_eq!(
        status, 200,
        "case first create: expected 200, got {status} with {first}"
    );
    let id = first["SiteToSiteVpnAttachment"]["Attachment"]["AttachmentId"]
        .as_str()
        .unwrap_or_else(|| panic!("case first create: no AttachmentId in {first}"))
        .to_string();

    // The same request with the same token answers the record the first call minted.
    let (status, retried) = emulator.post(CREATE_URI, body.clone()).await;
    assert_eq!(
        status, 200,
        "case retry of the same request: expected 200, got {status} with {retried}"
    );
    assert_eq!(
        retried["SiteToSiteVpnAttachment"]["Attachment"]["AttachmentId"],
        json!(id),
        "case retry of the same request: expected the first attachment, got {retried}"
    );

    // Each row is a create that must be refused as a conflict, and the attachment the
    // refusal has to name: the same token asking for something else, and the same VPN
    // connection asked for again with and without a token of its own.
    for (case, request) in [
        (
            "the same token with a different VPN connection",
            json!({
                "CoreNetworkId": core_network_id,
                "VpnConnectionArn": other_vpn,
                "ClientToken": "token-one",
            }),
        ),
        (
            "the same VPN connection under another token",
            json!({
                "CoreNetworkId": core_network_id,
                "VpnConnectionArn": vpn,
                "ClientToken": "token-two",
            }),
        ),
        (
            "the same VPN connection with no token at all",
            json!({ "CoreNetworkId": core_network_id, "VpnConnectionArn": vpn }),
        ),
    ] {
        let (status, refused) = emulator.post(CREATE_URI, request).await;
        assert_eq!(
            status, 409,
            "case {case}: expected 409, got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ConflictException",
            "case {case}: wrong error code in {refused}"
        );
        assert_eq!(
            refused["ResourceId"],
            json!(id),
            "case {case}: the conflict must name the attachment that holds it, got {refused}"
        );
        assert_eq!(
            refused["ResourceType"],
            json!(ATTACHMENT),
            "case {case}: the conflict must name the resource type, got {refused}"
        );
    }

    // Nothing above minted a second attachment, and a different VPN connection under a
    // token of its own still attaches.
    let (status, read) = emulator.get(&get_uri(&id)).await;
    assert_eq!(
        status, 200,
        "GetSiteToSiteVpnAttachment after the refusals: expected 200, got {status} with {read}"
    );
    assert_eq!(
        read["SiteToSiteVpnAttachment"]["Attachment"]["ResourceArn"],
        json!(vpn),
        "the stored attachment must still hold the first VPN connection, got {read}"
    );
    let (status, created) = emulator
        .post(
            CREATE_URI,
            json!({
                "CoreNetworkId": core_network_id,
                "VpnConnectionArn": other_vpn,
                "ClientToken": "token-three",
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "a free VPN connection under a fresh token: expected 200, got {status} with {created}"
    );
    assert_ne!(
        created["SiteToSiteVpnAttachment"]["Attachment"]["AttachmentId"],
        json!(id),
        "a second VPN connection gets its own attachment, got {created}"
    );
}

#[tokio::test]
async fn the_claiming_attachment_policy_rule_decides_the_segment_and_the_acceptance() {
    let emulator = Emulator::new();
    const PENDING_ACCEPTANCE: &str = "PENDING_ATTACHMENT_ACCEPTANCE";

    // Each row is one `attachment-policies` rule and what it decides about a SITE-TO-SITE
    // VPN attachment created under it: whether it claims the attachment at all (so whether
    // the attachment reports that rule's number and segment) and whether that rule asks for
    // the attachment to be accepted. Every key name is the published Cloud WAN attachment
    // policy schema's, the same source `crate::attachment_acceptance` builds its rules from.
    let rows: Vec<(&str, Value, &str, Value, Value)> = vec![
        (
            "a rule matching this attachment type, asking for acceptance",
            json!([{
                "rule-number": 42,
                "conditions": [{
                    "type": "attachment-type",
                    "operator": "equals",
                    "value": SITE_TO_SITE_VPN,
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

    for (index, (case, rules, state, rule_number, segment)) in rows.into_iter().enumerate() {
        let core_network = core_network_with(&emulator, case, "us-west-2", &rules).await;
        let (status, created) = emulator
            .post(
                CREATE_URI,
                json!({
                    "CoreNetworkId": core_network,
                    "VpnConnectionArn": vpn_arn(
                        "aws",
                        "us-west-2",
                        "111122223333",
                        &format!("aa0000{index:02x}"),
                    ),
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateSiteToSiteVpnAttachment expected 200, got {status} with {created}"
        );
        let attachment = &created["SiteToSiteVpnAttachment"]["Attachment"];
        let id = attachment["AttachmentId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no AttachmentId in {created}"))
            .to_string();
        assert_eq!(
            attachment["State"],
            json!(state),
            "case {case:?}: expected state {state}, got {attachment}"
        );
        assert_eq!(
            attachment["AttachmentPolicyRuleNumber"], rule_number,
            "case {case:?}: expected the claiming rule's number {rule_number}, got {attachment}"
        );
        assert_eq!(
            attachment["SegmentName"], segment,
            "case {case:?}: expected the segment the rule names {segment}, got {attachment}"
        );

        // The reader answers the same decision from the stored record.
        let (status, read) = emulator.get(&get_uri(&id)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetSiteToSiteVpnAttachment expected 200, got {status} with {read}"
        );
        let read_attachment = &read["SiteToSiteVpnAttachment"]["Attachment"];
        assert_eq!(
            (
                read_attachment["State"].clone(),
                read_attachment["AttachmentPolicyRuleNumber"].clone(),
                read_attachment["SegmentName"].clone(),
            ),
            (json!(state), rule_number.clone(), segment.clone()),
            "case {case:?}: the reader must answer the stored decision, got {read_attachment}"
        );

        // An attachment waiting for acceptance is moved by `AcceptAttachment` and by nothing
        // else: the clock owes it no transition until then.
        if state == PENDING_ACCEPTANCE {
            tokio::time::sleep(PAST_SETTLE).await;
            let (_, still_waiting) = emulator.get(&get_uri(&id)).await;
            assert_eq!(
                still_waiting["SiteToSiteVpnAttachment"]["Attachment"]["State"],
                json!(PENDING_ACCEPTANCE),
                "case {case:?}: no span of the clock accepts the attachment, got {still_waiting}"
            );
            let (status, accepted) = accept(&emulator, &id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: AcceptAttachment expected 200, got {status} with {accepted}"
            );
            let (_, read) = emulator.get(&get_uri(&id)).await;
            assert_eq!(
                read["SiteToSiteVpnAttachment"]["Attachment"]["State"],
                json!(CREATING),
                "case {case:?}: acceptance starts the settle span, got {read}"
            );
        }

        tokio::time::sleep(PAST_SETTLE).await;
        let (_, settled) = emulator.get(&get_uri(&id)).await;
        assert_eq!(
            settled["SiteToSiteVpnAttachment"]["Attachment"]["State"],
            json!(AVAILABLE),
            "case {case:?}: past the settle span the attachment is AVAILABLE, got {settled}"
        );
    }
}

#[tokio::test]
async fn a_core_network_holds_at_most_its_bound_of_site_to_site_vpn_attachments() {
    let emulator = Emulator::new();
    let (_, full) = core_network_at(&emulator, "full", &["us-west-2"]).await;
    let (_, spare) = core_network_at(&emulator, "spare", &["us-west-2"]).await;

    for index in 0..MAX_ATTACHMENTS {
        let (status, created) = emulator
            .post(
                CREATE_URI,
                json!({
                    "CoreNetworkId": full,
                    "VpnConnectionArn": vpn_arn(
                        "aws",
                        "us-west-2",
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
    // refusal is the modelled `ValidationException` naming the core network that is full.
    let over = vpn_arn("aws", "us-west-2", "111122223333", "ffffffff");
    let (status, refused) = emulator
        .post(
            CREATE_URI,
            json!({ "CoreNetworkId": full, "VpnConnectionArn": over }),
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
        json!("coreNetworkId"),
        "case one past the bound: the refusal must name the core network that is full, got {refused}"
    );

    // The bound is per core network: the same create on another core network is accepted,
    // which also shows the refused one stored nothing.
    let (status, created) = emulator
        .post(
            CREATE_URI,
            json!({ "CoreNetworkId": spare, "VpnConnectionArn": over }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case the same VPN connection on an empty core network: expected 200, got {status} with {created}"
    );
}
