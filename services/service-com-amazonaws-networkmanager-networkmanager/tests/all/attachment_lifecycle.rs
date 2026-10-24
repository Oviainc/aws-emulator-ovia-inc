// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of the Cloud WAN attachment lifecycle, driven over HTTP through the
//! real router (the harness lives in `crate::global_network_lifecycle`).
//!
//! The rule under test: an attachment is a stored resource of ONE edge location of ONE core
//! network — the edge location being one the core network's policy configures — and
//! everything an attachment operation answers is computed from that stored record: a VPC
//! attachment sits at the Region of the VPC its `VpcArn` names and reaches it through the
//! subnets of that same Region, a Connect attachment is carried over an existing VPC
//! attachment of the same core network and therefore sits at that transport's edge
//! location, both wait in `CREATING` until the transition the clock owes falls due, and each
//! reads back only through the reader of its own kind.
//!
//! The tables below span that class: attachments differing in Region, in how many subnets
//! they name, in the VPC options they carry, in tunnel protocol, in tags and in routing
//! policy label; and refusals differing in which reference or value is at fault. Nothing
//! reaches into `crate::state`: every attachment is created and read back through an
//! operation.

use serde_json::{json, Value};

use crate::core_network_lifecycle::create_core_network;
use crate::global_network_lifecycle::{error_code, expected_arn, is_minted_id, Emulator, PAST_SETTLE};
use crate::resource_tagging::{list_tags, tag, tags_uri};

/// The id prefix and ARN resource type of an attachment: the model's own
/// `^attachment-([0-9a-f]{8,17})$` (`smithy_query shape=AttachmentId`).
const ATTACHMENT: &str = "attachment";

/// How many attachments one core network may hold: the bound
/// `state::MAX_ATTACHMENTS_PER_CORE_NETWORK` fixes, which is what keeps a repeated create
/// from growing the store without end.
const MAX_ATTACHMENTS: usize = 100;

/// A core network policy document configuring one edge location per entry of `locations`.
///
/// The key names (`core-network-configuration`, `asn-ranges`, `edge-locations`,
/// `location`, `segments`, `name`) are the published Cloud WAN core network policy
/// schema's, the same source `crate::core_network_lifecycle` builds its documents from.
fn policy_document(locations: &[&str]) -> String {
    let edges: Vec<Value> = locations
        .iter()
        .map(|location| json!({ "location": location }))
        .collect();
    json!({
        "version": "2021.12",
        "core-network-configuration": {
            "asn-ranges": ["64512-64555"],
            "edge-locations": edges,
        },
        "segments": [{ "name": "shared" }],
    })
    .to_string()
}

/// Create a global network and, inside it, a core network whose policy configures an edge
/// in each of `locations`. Answers `(global network id, core network id)`.
async fn core_network_at(
    emulator: &Emulator,
    name: &str,
    locations: &[&str],
) -> (String, String) {
    let network = emulator.create_global_network(name).await;
    let (status, created) = create_core_network(
        emulator,
        json!({
            "GlobalNetworkId": network,
            "Description": name,
            "PolicyDocument": policy_document(locations),
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {name:?}: CreateCoreNetwork expected 200, got {status} with {created}"
    );
    let core_network = created["CoreNetwork"]["CoreNetworkId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name:?}: no CoreNetworkId in {created}"))
        .to_string();
    (network, core_network)
}

/// An EC2 VPC ARN in `region`, as the modelled `VpcArn` pattern spells one
/// (`^arn:[^:]{1,63}:ec2:[^:]{0,63}:[^:]{0,63}:vpc\/vpc-[0-9a-f]{8,17}$`, `smithy_query
/// shape=VpcArn`): the id is 8 hex digits, the shortest the pattern allows.
pub(crate) fn vpc_arn(region: &str, index: usize) -> String {
    format!(
        "arn:aws:ec2:{region}:{}:vpc/vpc-{index:08x}",
        engine::emulator_account_id()
    )
}

/// An EC2 subnet ARN in `region`. See [`vpc_arn`]; `SubnetArn` carries the matching
/// `subnet/subnet-<hex>` pattern.
pub(crate) fn subnet_arn(region: &str, index: usize) -> String {
    format!(
        "arn:aws:ec2:{region}:{}:subnet/subnet-{index:08x}",
        engine::emulator_account_id()
    )
}

/// `CreateVpcAttachment`.
pub(crate) async fn create_vpc_attachment(emulator: &Emulator, body: Value) -> (u16, Value) {
    emulator.post("/vpc-attachments", body).await
}

/// `CreateConnectAttachment`.
pub(crate) async fn create_connect_attachment(emulator: &Emulator, body: Value) -> (u16, Value) {
    emulator.post("/connect-attachments", body).await
}

/// The id member of a created attachment, or a panic naming what was there instead.
pub(crate) fn attachment_id(case: &str, wrapper: &Value) -> String {
    wrapper["Attachment"]["AttachmentId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no Attachment.AttachmentId in {wrapper}"))
        .to_string()
}

/// One row of the attachment table: a VPC attachment and the Connect attachment carried
/// over it.
struct AttachmentCase {
    /// Names the case in every assertion message.
    name: &'static str,
    /// The Region the VPC is in, which is therefore the edge location both attachments sit
    /// at. Every one of these is an edge location of the core network the test builds.
    region: &'static str,
    /// How many subnets the VPC attachment names.
    subnets: usize,
    /// The `Options` the VPC attachment create carries, or `Value::Null` for none.
    vpc_options: Value,
    /// The `Options.Protocol` the Connect attachment create carries, or `None` for an
    /// `Options` that names no protocol.
    protocol: Option<&'static str>,
    /// The `Tags` both creates carry, or `Value::Null` for none.
    tags: Value,
    /// The `RoutingPolicyLabel` both creates carry, or `None` for none.
    routing_policy_label: Option<&'static str>,
}

/// The table: rows differing along everything the rule generalises over — the Region (and
/// so the edge location), the number of subnets, the VPC options, the tunnel protocol
/// (`GRE` and `NO_ENCAP` are the two values `TunnelProtocol` enumerates, `smithy_query
/// shape=TunnelProtocol`, plus a request that names none), the tags and the routing policy
/// label.
fn attachment_cases() -> Vec<AttachmentCase> {
    vec![
        AttachmentCase {
            name: "GRE over a one-subnet VPC, no options and no tags",
            region: "us-west-2",
            subnets: 1,
            vpc_options: Value::Null,
            protocol: Some("GRE"),
            tags: Value::Null,
            routing_policy_label: None,
        },
        AttachmentCase {
            name: "NO_ENCAP over a two-subnet VPC with every option on",
            region: "eu-west-1",
            subnets: 2,
            vpc_options: json!({
                "Ipv6Support": true,
                "ApplianceModeSupport": true,
                "DnsSupport": true,
                "SecurityGroupReferencingSupport": true,
            }),
            protocol: Some("NO_ENCAP"),
            tags: json!([{ "Key": "team", "Value": "net" }]),
            routing_policy_label: Some("north"),
        },
        AttachmentCase {
            name: "no protocol over a three-subnet VPC with mixed options",
            region: "ap-south-1",
            subnets: 3,
            vpc_options: json!({
                "Ipv6Support": true,
                "ApplianceModeSupport": false,
                "DnsSupport": true,
                "SecurityGroupReferencingSupport": false,
            }),
            protocol: None,
            tags: json!([{ "Key": "team", "Value": "net" }, { "Key": "tier", "Value": "edge" }]),
            routing_policy_label: None,
        },
        AttachmentCase {
            name: "GRE in a third Region, options all off",
            region: "il-central-1",
            subnets: 1,
            vpc_options: json!({
                "Ipv6Support": false,
                "ApplianceModeSupport": false,
                "DnsSupport": false,
                "SecurityGroupReferencingSupport": false,
            }),
            protocol: Some("GRE"),
            tags: json!([{ "Key": "only", "Value": "" }]),
            routing_policy_label: Some("south"),
        },
    ]
}

/// `body` with `Tags` / `RoutingPolicyLabel` added when the row carries them.
fn with_common(mut body: Value, case: &AttachmentCase) -> Value {
    let object = body
        .as_object_mut()
        .unwrap_or_else(|| panic!("case {:?}: the request body must be an object", case.name));
    if !case.tags.is_null() {
        object.insert("Tags".to_string(), case.tags.clone());
    }
    if let Some(label) = case.routing_policy_label {
        object.insert("RoutingPolicyLabel".to_string(), json!(label));
    }
    body
}

/// The `Tags` an attachment of this row reports: what the row asked for, or the empty list
/// when it asked for none.
fn expected_tags(case: &AttachmentCase) -> Value {
    if case.tags.is_null() {
        json!([])
    } else {
        case.tags.clone()
    }
}

/// Assert the `Attachment` envelope of `wrapper` is the one this row describes, at `state`.
fn assert_envelope(
    case: &AttachmentCase,
    wrapper: &Value,
    core_network: &str,
    attachment_type: &str,
    resource_arn: &str,
    state: Option<&str>,
) {
    let name = case.name;
    let attachment = &wrapper["Attachment"];
    let id = attachment_id(name, wrapper);
    assert!(
        is_minted_id(&id, ATTACHMENT),
        "case {name:?}: expected an attachment-<17 hex> id, got {id:?}"
    );
    assert_eq!(
        attachment["CoreNetworkId"],
        json!(core_network),
        "case {name:?}: the attachment must back-reference its core network, got {attachment}"
    );
    assert_eq!(
        attachment["CoreNetworkArn"],
        json!(expected_arn("core-network", core_network)),
        "case {name:?}: expected the core network's ARN, got {attachment}"
    );
    assert_eq!(
        attachment["AttachmentType"],
        json!(attachment_type),
        "case {name:?}: expected a {attachment_type} attachment, got {attachment}"
    );
    assert_eq!(
        attachment["EdgeLocation"],
        json!(case.region),
        "case {name:?}: the attachment sits at the edge location of its resource, got {attachment}"
    );
    assert_eq!(
        attachment["ResourceArn"],
        json!(resource_arn),
        "case {name:?}: expected the ARN of the attached resource, got {attachment}"
    );
    assert_eq!(
        attachment["OwnerAccountId"],
        json!(engine::emulator_account_id()),
        "case {name:?}: expected the emulator's account id, got {attachment}"
    );
    assert_eq!(
        attachment["Tags"],
        expected_tags(case),
        "case {name:?}: expected the tags the create carried, got {attachment}"
    );
    assert_eq!(
        attachment["LastModificationErrors"],
        json!([]),
        "case {name:?}: no modification has failed, got {attachment}"
    );
    assert!(
        attachment["CreatedAt"].as_f64().is_some_and(|at| at > 0.0),
        "case {name:?}: CreatedAt must be a timestamp, got {attachment}"
    );
    if let Some(state) = state {
        assert_eq!(
            attachment["State"],
            json!(state),
            "case {name:?}: expected state {state}, got {attachment}"
        );
    }
}

#[tokio::test]
async fn attachments_are_created_at_an_edge_of_their_core_network_and_read_back_from_state() {
    let emulator = Emulator::new();
    let cases = attachment_cases();
    let regions: Vec<&str> = cases.iter().map(|case| case.region).collect();
    // One core network with an edge in every Region the table uses, so the rows share a
    // parent and a count that leaked between them would show up.
    let (_, core_network) = core_network_at(&emulator, "attachments", &regions).await;

    // Each created pair, kept so the settled state can be asserted after ONE wait.
    let mut created: Vec<(&AttachmentCase, String, String, String)> = Vec::new();

    for (index, case) in cases.iter().enumerate() {
        let name = case.name;
        let vpc = vpc_arn(case.region, index + 1);
        let subnets: Vec<String> = (0..case.subnets)
            .map(|subnet| subnet_arn(case.region, (index + 1) * 100 + subnet))
            .collect();
        let mut request = json!({
            "CoreNetworkId": core_network,
            "VpcArn": vpc,
            "SubnetArns": subnets,
        });
        if !case.vpc_options.is_null() {
            request["Options"] = case.vpc_options.clone();
        }
        let (status, created_vpc) =
            create_vpc_attachment(&emulator, with_common(request, case)).await;
        assert_eq!(
            status, 200,
            "case {name:?}: CreateVpcAttachment expected 200, got {status} with {created_vpc}"
        );
        let vpc_attachment = &created_vpc["VpcAttachment"];
        assert_envelope(
            case,
            vpc_attachment,
            &core_network,
            "VPC",
            &vpc,
            Some("CREATING"),
        );
        assert_eq!(
            vpc_attachment["SubnetArns"],
            json!(subnets),
            "case {name:?}: the subnets are stored in the order given, got {vpc_attachment}"
        );
        assert_eq!(
            vpc_attachment["Options"],
            case.vpc_options,
            "case {name:?}: expected the VPC options the create carried, got {vpc_attachment}"
        );
        let transport = attachment_id(name, vpc_attachment);
        let transport_arn = expected_arn(ATTACHMENT, &transport);

        let mut options = json!({});
        if let Some(protocol) = case.protocol {
            options["Protocol"] = json!(protocol);
        }
        let request = json!({
            "CoreNetworkId": core_network,
            "EdgeLocation": case.region,
            "TransportAttachmentId": transport,
            "Options": options,
        });
        let (status, created_connect) =
            create_connect_attachment(&emulator, with_common(request, case)).await;
        assert_eq!(
            status, 200,
            "case {name:?}: CreateConnectAttachment expected 200, got {status} with {created_connect}"
        );
        let connect_attachment = &created_connect["ConnectAttachment"];
        // A Connect attachment attaches no resource of its own: it is carried over its
        // transport, which is the resource its ARN names.
        assert_envelope(
            case,
            connect_attachment,
            &core_network,
            "CONNECT",
            &transport_arn,
            Some("CREATING"),
        );
        assert_eq!(
            connect_attachment["TransportAttachmentId"],
            json!(transport),
            "case {name:?}: expected the transport the create named, got {connect_attachment}"
        );
        assert_eq!(
            connect_attachment["Options"]["Protocol"],
            case.protocol.map_or(Value::Null, |protocol| json!(protocol)),
            "case {name:?}: expected the tunnel protocol the create carried, got {connect_attachment}"
        );
        let connect = attachment_id(name, connect_attachment);
        assert_ne!(
            connect, transport,
            "case {name:?}: each attachment gets its own id, got {connect:?} twice"
        );

        // The create persisted the record: each reader answers it from the store.
        let (status, read) = emulator
            .get(&format!("/vpc-attachments/{transport}"))
            .await;
        assert_eq!(
            status, 200,
            "case {name:?}: GetVpcAttachment expected 200, got {status} with {read}"
        );
        assert_eq!(
            read["VpcAttachment"]["SubnetArns"],
            json!(subnets),
            "case {name:?}: GetVpcAttachment must answer the stored subnets, got {read}"
        );
        assert_eq!(
            attachment_id(name, &read["VpcAttachment"]),
            transport,
            "case {name:?}: GetVpcAttachment must answer the attachment asked for, got {read}"
        );
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
            "case {name:?}: GetConnectAttachment must answer the stored transport, got {read}"
        );

        created.push((case, transport, connect, vpc));
    }

    tokio::time::sleep(PAST_SETTLE).await;

    for (case, transport, connect, vpc) in &created {
        let name = case.name;
        let transport_arn = expected_arn(ATTACHMENT, transport);
        for (reader, wrapper_member, id, attachment_type, resource_arn) in [
            ("vpc-attachments", "VpcAttachment", transport, "VPC", vpc),
            (
                "connect-attachments",
                "ConnectAttachment",
                connect,
                "CONNECT",
                &transport_arn,
            ),
        ] {
            let (status, body) = emulator.get(&format!("/{reader}/{id}")).await;
            assert_eq!(
                status, 200,
                "case {name:?}/{attachment_type}: expected 200, got {status} with {body}"
            );
            let wrapper = &body[wrapper_member];
            assert_envelope(
                case,
                wrapper,
                &core_network,
                attachment_type,
                resource_arn,
                Some("AVAILABLE"),
            );
            let attachment = &wrapper["Attachment"];
            let created_at = attachment["CreatedAt"].as_f64().unwrap_or_default();
            let updated_at = attachment["UpdatedAt"].as_f64().unwrap_or_default();
            assert!(
                updated_at > created_at,
                "case {name:?}/{attachment_type}: the settle transition is a change, so UpdatedAt ({updated_at}) must be past CreatedAt ({created_at}), got {attachment}"
            );
        }
    }
}

/// Assert `body` is a field-validation refusal naming `member`.
fn assert_field_refusal(case: &str, member: &str, status: u16, body: &Value) {
    assert_eq!(
        status, 400,
        "case {case:?}: an invalid attachment request is HTTP 400, got {status} with {body}"
    );
    assert_eq!(
        error_code(body),
        "ValidationException",
        "case {case:?}: expected ValidationException, got {body}"
    );
    assert_eq!(
        body["Fields"][0]["Name"],
        json!(member),
        "case {case:?}: the refusal must name {member}, got {body}"
    );
}

#[tokio::test]
async fn a_connect_attachment_needs_a_vpc_transport_of_its_own_core_network_and_edge() {
    let emulator = Emulator::new();
    let (_, core_network) = core_network_at(&emulator, "connect refusals", &["us-west-2", "eu-west-1"]).await;
    let (_, other_core_network) =
        core_network_at(&emulator, "another core network", &["us-west-2"]).await;

    // The transport the valid request uses, the one belonging to the other core network,
    // and a Connect attachment — which is an attachment of the wrong KIND to carry one.
    let (status, transport) = create_vpc_attachment(
        &emulator,
        json!({
            "CoreNetworkId": core_network,
            "VpcArn": vpc_arn("us-west-2", 1),
            "SubnetArns": [subnet_arn("us-west-2", 11)],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the transport attachment must be created, got {status} with {transport}"
    );
    let transport = attachment_id("transport", &transport["VpcAttachment"]);

    let (status, foreign) = create_vpc_attachment(
        &emulator,
        json!({
            "CoreNetworkId": other_core_network,
            "VpcArn": vpc_arn("us-west-2", 2),
            "SubnetArns": [subnet_arn("us-west-2", 21)],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the foreign attachment must be created, got {status} with {foreign}"
    );
    let foreign = attachment_id("foreign transport", &foreign["VpcAttachment"]);

    let (status, connect) = create_connect_attachment(
        &emulator,
        json!({
            "CoreNetworkId": core_network,
            "EdgeLocation": "us-west-2",
            "TransportAttachmentId": transport,
            "Options": { "Protocol": "GRE" },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the first Connect attachment must be created, got {status} with {connect}"
    );
    let connect = attachment_id("connect", &connect["ConnectAttachment"]);

    // Each row is a different WAY of being invalid, and names the member at fault: an
    // unresolvable core network, and three unresolvable transports (never existed, in
    // another core network, of the wrong kind), plus an edge location that is not the
    // transport's.
    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "an unknown core network",
            json!({
                "CoreNetworkId": "core-network-0000000000000ffff",
                "EdgeLocation": "us-west-2",
                "TransportAttachmentId": transport,
                "Options": { "Protocol": "GRE" },
            }),
            "coreNetworkId",
        ),
        (
            "a transport id that names nothing",
            json!({
                "CoreNetworkId": core_network,
                "EdgeLocation": "us-west-2",
                "TransportAttachmentId": "attachment-0000000000000ffff",
                "Options": { "Protocol": "GRE" },
            }),
            "transportAttachmentId",
        ),
        (
            "a transport of another core network",
            json!({
                "CoreNetworkId": core_network,
                "EdgeLocation": "us-west-2",
                "TransportAttachmentId": foreign,
                "Options": { "Protocol": "GRE" },
            }),
            "transportAttachmentId",
        ),
        (
            "a Connect attachment as the transport",
            json!({
                "CoreNetworkId": core_network,
                "EdgeLocation": "us-west-2",
                "TransportAttachmentId": connect,
                "Options": { "Protocol": "GRE" },
            }),
            "transportAttachmentId",
        ),
        (
            "an edge location that is not the transport's",
            json!({
                "CoreNetworkId": core_network,
                "EdgeLocation": "eu-west-1",
                "TransportAttachmentId": transport,
                "Options": { "Protocol": "GRE" },
            }),
            "edgeLocation",
        ),
    ];

    for (case, request, member) in rows {
        let (status, body) = create_connect_attachment(&emulator, request).await;
        assert_field_refusal(case, member, status, &body);
    }

    // `Options.Protocol` is a modelled enumeration (GRE, NO_ENCAP: `smithy_query
    // shape=TunnelProtocol`), so a value outside it is refused by the constraint walk that
    // runs before the handler — the same modelled `ValidationException` (HTTP 400), naming
    // the member it rejected and the values it accepts.
    let (status, refused) = create_connect_attachment(
        &emulator,
        json!({
            "CoreNetworkId": core_network,
            "EdgeLocation": "us-west-2",
            "TransportAttachmentId": transport,
            "Options": { "Protocol": "IPSEC" },
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "a protocol outside the modelled enumeration is HTTP 400, got {status} with {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "ValidationException",
        "expected ValidationException, got {refused}"
    );
    let message = refused["message"]
        .as_str()
        .or_else(|| refused["Message"].as_str())
        .unwrap_or_default();
    for expected in ["options.protocol", "GRE", "NO_ENCAP"] {
        assert!(
            message.contains(expected),
            "the refusal must name {expected}, got {message:?}"
        );
    }

    // Every refusal stored nothing: the core network still holds exactly the two
    // attachments the setup created in it (the third belongs to the other core network,
    // and so to the other global network).
    let (status, counts) = emulator
        .get(&format!(
            "/global-networks/{}/network-resource-count?resourceType=attachment",
            core_network_owner(&emulator, &core_network).await
        ))
        .await;
    assert_eq!(
        status, 200,
        "GetNetworkResourceCounts expected 200, got {status} with {counts}"
    );
    assert_eq!(
        counts["NetworkResourceCounts"][0]["Count"],
        json!(2),
        "the refusals stored nothing, so the global network still holds its two attachments, got {counts}"
    );
}

/// The `GlobalNetworkId` the core network `core_network` belongs to, read back through
/// `GetCoreNetwork` rather than remembered, so the inventory assertion addresses the parent
/// the store itself reports.
async fn core_network_owner(emulator: &Emulator, core_network: &str) -> String {
    let (status, body) = emulator
        .get(&format!("/core-networks/{core_network}"))
        .await;
    assert_eq!(
        status, 200,
        "GetCoreNetwork expected 200, got {status} with {body}"
    );
    body["CoreNetwork"]["GlobalNetworkId"]
        .as_str()
        .unwrap_or_else(|| panic!("no GlobalNetworkId in {body}"))
        .to_string()
}

#[tokio::test]
async fn a_vpc_attachment_needs_a_vpc_and_subnets_at_an_edge_of_its_core_network() {
    let emulator = Emulator::new();
    let (_, core_network) = core_network_at(&emulator, "vpc refusals", &["us-west-2"]).await;

    // Each row is a different WAY of being invalid: an unresolvable core network, a VPC ARN
    // naming no Region at all (the modelled pattern allows an empty Region segment), a VPC
    // in a Region the policy configures no edge for, no subnets, and a subnet outside the
    // VPC's Region.
    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "an unknown core network",
            json!({
                "CoreNetworkId": "core-network-0000000000000ffff",
                "VpcArn": vpc_arn("us-west-2", 1),
                "SubnetArns": [subnet_arn("us-west-2", 11)],
            }),
            "coreNetworkId",
        ),
        (
            "a VPC ARN with an empty Region segment",
            json!({
                "CoreNetworkId": core_network,
                "VpcArn": vpc_arn("", 1),
                "SubnetArns": [subnet_arn("us-west-2", 11)],
            }),
            "vpcArn",
        ),
        (
            "a VPC in a Region the policy configures no edge for",
            json!({
                "CoreNetworkId": core_network,
                "VpcArn": vpc_arn("us-east-2", 1),
                "SubnetArns": [subnet_arn("us-east-2", 11)],
            }),
            "vpcArn",
        ),
        (
            "no subnets at all",
            json!({
                "CoreNetworkId": core_network,
                "VpcArn": vpc_arn("us-west-2", 1),
                "SubnetArns": [],
            }),
            "subnetArns",
        ),
        (
            "a subnet outside the VPC's Region",
            json!({
                "CoreNetworkId": core_network,
                "VpcArn": vpc_arn("us-west-2", 1),
                "SubnetArns": [subnet_arn("us-west-2", 11), subnet_arn("eu-west-1", 12)],
            }),
            "subnetArns",
        ),
    ];

    for (case, request, member) in rows {
        let (status, body) = create_vpc_attachment(&emulator, request).await;
        assert_field_refusal(case, member, status, &body);
    }

    // The first VPC attachment of a VPC is accepted; a second one of the SAME VPC is the
    // service's concurrent-state conflict, because a VPC can be attached only once.
    let request = json!({
        "CoreNetworkId": core_network,
        "VpcArn": vpc_arn("us-west-2", 1),
        "SubnetArns": [subnet_arn("us-west-2", 11)],
    });
    let (status, created) = create_vpc_attachment(&emulator, request.clone()).await;
    assert_eq!(
        status, 200,
        "the first attachment of the VPC must be created, got {status} with {created}"
    );
    let first = attachment_id("the first attachment", &created["VpcAttachment"]);
    let (status, conflict) = create_vpc_attachment(&emulator, request).await;
    assert_eq!(
        status, 409,
        "a second attachment of the same VPC is HTTP 409, got {status} with {conflict}"
    );
    assert_eq!(
        error_code(&conflict),
        "ConflictException",
        "expected ConflictException, got {conflict}"
    );
    assert_eq!(
        conflict["ResourceId"],
        json!(first),
        "the conflict must name the attachment already holding the VPC, got {conflict}"
    );
    assert_eq!(
        conflict["ResourceType"],
        json!(ATTACHMENT),
        "the conflict must name the resource type, got {conflict}"
    );
}

#[tokio::test]
async fn a_repeated_client_token_answers_the_attachment_it_already_created() {
    let emulator = Emulator::new();
    let (_, core_network) = core_network_at(&emulator, "idempotency", &["us-west-2"]).await;

    let vpc_request = json!({
        "CoreNetworkId": core_network,
        "VpcArn": vpc_arn("us-west-2", 1),
        "SubnetArns": [subnet_arn("us-west-2", 11)],
        "ClientToken": "vpc-token",
    });
    let (status, first) = create_vpc_attachment(&emulator, vpc_request.clone()).await;
    assert_eq!(
        status, 200,
        "CreateVpcAttachment expected 200, got {status} with {first}"
    );
    let transport = attachment_id("the first VPC attachment", &first["VpcAttachment"]);
    let (status, again) = create_vpc_attachment(&emulator, vpc_request.clone()).await;
    assert_eq!(
        status, 200,
        "a retry of the identical request expected 200, got {status} with {again}"
    );
    assert_eq!(
        attachment_id("the retry", &again["VpcAttachment"]),
        transport,
        "a retry of the identical request answers the attachment already minted, got {again}"
    );

    let mut different = vpc_request;
    different["SubnetArns"] = json!([subnet_arn("us-west-2", 12)]);
    let (status, conflict) = create_vpc_attachment(&emulator, different).await;
    assert_eq!(
        status, 409,
        "the same token asking for something else is HTTP 409, got {status} with {conflict}"
    );
    assert_eq!(
        error_code(&conflict),
        "ConflictException",
        "expected ConflictException, got {conflict}"
    );

    let connect_request = json!({
        "CoreNetworkId": core_network,
        "EdgeLocation": "us-west-2",
        "TransportAttachmentId": transport,
        "Options": { "Protocol": "GRE" },
        "ClientToken": "connect-token",
    });
    let (status, first) = create_connect_attachment(&emulator, connect_request.clone()).await;
    assert_eq!(
        status, 200,
        "CreateConnectAttachment expected 200, got {status} with {first}"
    );
    let connect = attachment_id("the first Connect attachment", &first["ConnectAttachment"]);
    let (status, again) = create_connect_attachment(&emulator, connect_request.clone()).await;
    assert_eq!(
        status, 200,
        "a retry of the identical request expected 200, got {status} with {again}"
    );
    assert_eq!(
        attachment_id("the retry", &again["ConnectAttachment"]),
        connect,
        "a retry of the identical request answers the attachment already minted, got {again}"
    );

    let mut different = connect_request;
    different["Options"] = json!({ "Protocol": "NO_ENCAP" });
    let (status, conflict) = create_connect_attachment(&emulator, different).await;
    assert_eq!(
        status, 409,
        "the same token asking for a different protocol is HTTP 409, got {status} with {conflict}"
    );
    assert_eq!(
        error_code(&conflict),
        "ConflictException",
        "expected ConflictException, got {conflict}"
    );
}

#[tokio::test]
async fn an_attachment_reads_back_only_through_the_reader_of_its_own_kind() {
    let emulator = Emulator::new();
    let (_, core_network) = core_network_at(&emulator, "readers", &["us-west-2"]).await;
    let (status, vpc) = create_vpc_attachment(
        &emulator,
        json!({
            "CoreNetworkId": core_network,
            "VpcArn": vpc_arn("us-west-2", 1),
            "SubnetArns": [subnet_arn("us-west-2", 11)],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateVpcAttachment expected 200, got {status} with {vpc}"
    );
    let vpc = attachment_id("the VPC attachment", &vpc["VpcAttachment"]);
    let (status, connect) = create_connect_attachment(
        &emulator,
        json!({
            "CoreNetworkId": core_network,
            "EdgeLocation": "us-west-2",
            "TransportAttachmentId": vpc,
            "Options": { "Protocol": "GRE" },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateConnectAttachment expected 200, got {status} with {connect}"
    );
    let connect = attachment_id("the Connect attachment", &connect["ConnectAttachment"]);

    // Each row: the reader, the id, and whether that reader reports an attachment of that
    // kind. An id of another kind names no attachment of the kind asked for, which is the
    // same answer as an id that names nothing.
    for (case, reader, id, found) in [
        ("its own reader", "vpc-attachments", vpc.clone(), true),
        ("its own reader", "connect-attachments", connect.clone(), true),
        ("the other kind's reader", "vpc-attachments", connect, false),
        ("the other kind's reader", "connect-attachments", vpc, false),
        (
            "an id that names nothing",
            "vpc-attachments",
            "attachment-0000000000000ffff".to_string(),
            false,
        ),
        (
            "an id that names nothing",
            "connect-attachments",
            "attachment-0000000000000ffff".to_string(),
            false,
        ),
    ] {
        let (status, body) = emulator.get(&format!("/{reader}/{id}")).await;
        if found {
            assert_eq!(
                status, 200,
                "case {case:?} ({reader}/{id}): expected 200, got {status} with {body}"
            );
            continue;
        }
        assert_eq!(
            status, 404,
            "case {case:?} ({reader}/{id}): expected 404, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?} ({reader}/{id}): expected ResourceNotFoundException, got {body}"
        );
        assert_eq!(
            body["ResourceType"],
            json!(ATTACHMENT),
            "case {case:?} ({reader}/{id}): the refusal must name the resource type, got {body}"
        );
        assert_eq!(
            body["ResourceId"],
            json!(id),
            "case {case:?} ({reader}/{id}): the refusal must name the id asked for, got {body}"
        );
    }
}

#[tokio::test]
async fn a_core_network_holding_attachments_is_counted_tagged_and_blocked_from_deletion() {
    let emulator = Emulator::new();
    let (network, core_network) = core_network_at(&emulator, "inventory", &["us-west-2"]).await;

    // Each row adds one more attachment to the same core network, so the inventory count
    // and the delete refusal are asserted against several holdings, not just one.
    let mut held = 0;
    let mut arns: Vec<String> = Vec::new();
    for index in 1..=3 {
        let (status, created) = create_vpc_attachment(
            &emulator,
            json!({
                "CoreNetworkId": core_network,
                "VpcArn": vpc_arn("us-west-2", index),
                "SubnetArns": [subnet_arn("us-west-2", index * 10)],
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "attachment {index}: CreateVpcAttachment expected 200, got {status} with {created}"
        );
        held += 1;
        arns.push(expected_arn(
            ATTACHMENT,
            &attachment_id("the attachment", &created["VpcAttachment"]),
        ));

        let (status, counts) = emulator
            .get(&format!(
                "/global-networks/{network}/network-resource-count?resourceType=attachment"
            ))
            .await;
        assert_eq!(
            status, 200,
            "attachment {index}: GetNetworkResourceCounts expected 200, got {status} with {counts}"
        );
        assert_eq!(
            counts["NetworkResourceCounts"][0]["Count"],
            json!(held),
            "attachment {index}: the inventory must report the {held} attachments held, got {counts}"
        );

        // "This can only be done if there are no attachments on a core network"
        // (`smithy_query query="doc" shape="DeleteCoreNetwork"`): the delete is refused
        // while any is held, and the refusal is a validation failure against stored state.
        let (status, refusal) = emulator
            .delete(&format!("/core-networks/{core_network}"))
            .await;
        assert_eq!(
            status, 400,
            "attachment {index}: a core network holding attachments cannot be deleted, got {status} with {refusal}"
        );
        assert_eq!(
            error_code(&refusal),
            "ValidationException",
            "attachment {index}: expected ValidationException, got {refusal}"
        );
        assert_eq!(
            refusal["Reason"],
            json!("Other"),
            "attachment {index}: no input field is at fault, got {refusal}"
        );
        let message = refusal["Message"].as_str().unwrap_or_default();
        assert!(
            message.contains(ATTACHMENT),
            "attachment {index}: the refusal must name the blocking attachment, got {message:?}"
        );
    }

    // An attachment is a taggable resource of this service, so its ARN reaches the one tag
    // store every tagging operation shares.
    let arn = arns
        .first()
        .unwrap_or_else(|| panic!("the loop above created attachments"));
    let (status, tagged) = emulator
        .post(&tags_uri(arn), json!({ "Tags": [tag("owner", "net")] }))
        .await;
    assert_eq!(
        status, 200,
        "TagResource on an attachment expected 200, got {status} with {tagged}"
    );
    let (status, listed) = list_tags(&emulator, arn).await;
    assert_eq!(
        status, 200,
        "ListTagsForResource expected 200, got {status} with {listed}"
    );
    assert_eq!(
        listed,
        json!([tag("owner", "net")]),
        "the attachment must report the tag that was put on it, got {listed}"
    );
}

#[tokio::test]
async fn a_core_network_holds_at_most_the_bounded_number_of_attachments() {
    let emulator = Emulator::new();
    let (network, core_network) = core_network_at(&emulator, "quota", &["us-west-2"]).await;

    // Filling the core network to the bound: every create up to it is accepted, so the
    // refusal below is about the COUNT held and not about the request.
    let mut transport = String::new();
    for index in 1..=MAX_ATTACHMENTS {
        let (status, created) = create_vpc_attachment(
            &emulator,
            json!({
                "CoreNetworkId": core_network,
                "VpcArn": vpc_arn("us-west-2", index),
                "SubnetArns": [subnet_arn("us-west-2", index)],
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "attachment {index} of {MAX_ATTACHMENTS} is within the bound, got {status} with {created}"
        );
        if transport.is_empty() {
            // The first of them serves as the transport of the refused Connect create.
            transport = attachment_id("the transport", &created["VpcAttachment"]);
        }
    }

    let (status, body) = emulator
        .get(&format!(
            "/global-networks/{network}/network-resource-count?resourceType=attachment"
        ))
        .await;
    assert_eq!(
        status, 200,
        "GetNetworkResourceCounts expected 200, got {status} with {body}"
    );
    assert_eq!(
        body["NetworkResourceCounts"][0]["Count"],
        json!(MAX_ATTACHMENTS),
        "the core network must hold exactly the bound, got {body}"
    );

    // Each row: the create that is refused at the cap. Neither attachment create models a
    // `ServiceQuotaExceededException` (`smithy_query query="operation"
    // operation="CreateVpcAttachment,CreateConnectAttachment"`), so the refusal is the
    // modelled `ValidationException` with the `Other` reason — no input member is at fault.
    for (case, request) in [
        (
            "one VPC attachment past the bound",
            json!({
                "CoreNetworkId": core_network,
                "VpcArn": vpc_arn("us-west-2", MAX_ATTACHMENTS + 1),
                "SubnetArns": [subnet_arn("us-west-2", MAX_ATTACHMENTS + 1)],
            }),
        ),
        (
            "one Connect attachment past the bound",
            json!({
                "CoreNetworkId": core_network,
                "EdgeLocation": "us-west-2",
                "TransportAttachmentId": transport,
                "Options": { "Protocol": "GRE" },
            }),
        ),
    ] {
        let is_connect = request.get("TransportAttachmentId").is_some();
        let (status, refusal) = if is_connect {
            create_connect_attachment(&emulator, request).await
        } else {
            create_vpc_attachment(&emulator, request).await
        };
        assert_eq!(
            status, 400,
            "case {case:?}: a create past the bound is HTTP 400, got {status} with {refusal}"
        );
        assert_eq!(
            error_code(&refusal),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {refusal}"
        );
        assert_eq!(
            refusal["Reason"],
            json!("Other"),
            "case {case:?}: the count held is at fault, not an input member, got {refusal}"
        );
    }

    // The refusals stored nothing: the core network still holds exactly the bound.
    let (status, body) = emulator
        .get(&format!(
            "/global-networks/{network}/network-resource-count?resourceType=attachment"
        ))
        .await;
    assert_eq!(
        status, 200,
        "GetNetworkResourceCounts expected 200, got {status} with {body}"
    );
    assert_eq!(
        body["NetworkResourceCounts"][0]["Count"],
        json!(MAX_ATTACHMENTS),
        "a refused create must store nothing, got {body}"
    );
}
