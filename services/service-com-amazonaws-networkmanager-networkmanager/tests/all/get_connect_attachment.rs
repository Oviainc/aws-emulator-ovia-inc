// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `GetConnectAttachment`, driven over HTTP through the real router
//! (the harness lives in `crate::global_network_lifecycle`, the core-network helpers in
//! `crate::attachment_lifecycle` and `crate::attachment_acceptance`).
//!
//! The rule under test: `GetConnectAttachment` REPORTS the one stored attachment record its
//! `AttachmentId` names — the shared `Attachment` envelope as of the clock, the transport
//! attachment the Connect attachment is carried over, and its tunnel options — and writes
//! nothing: the state it answers is the one the create (and any acceptance) stored, the
//! `CREATING` -> `AVAILABLE` transition the clock owes shows up without any call having made
//! it, and two reads in a row answer the same thing. An id that names no stored CONNECT
//! attachment — never minted, or an attachment of another kind in the one id space the
//! attachment readers share — is the modelled `ResourceNotFoundException` (HTTP 404) naming
//! the resource type and the id asked for.
//!
//! The tables below span that class: Connect attachments differing in tunnel protocol (both
//! values `TunnelProtocol` enumerates, plus an `Options` naming none), in Region and so in
//! edge location, in how many subnets their transport reaches its VPC through, in tags, in
//! routing policy label, and in what the core network's `attachment-policies` decide about
//! them (claimed or not, acceptance required or not); several Connect attachments sharing
//! ONE transport; and refusals differing in how the id fails to name a Connect attachment.
//! Nothing reaches into `crate::state`: every attachment is created, accepted and read back
//! through an operation.

use serde_json::{json, Value};

use crate::attachment_acceptance::{accept, core_network_with};
use crate::attachment_lifecycle::{
    attachment_id, core_network_at, create_connect_attachment, create_vpc_attachment, subnet_arn,
    vpc_arn,
};
use crate::global_network_lifecycle::{
    error_code, expected_arn, is_minted_id, Emulator, PAST_SETTLE,
};
use crate::resource_tagging::list_tags;

/// The ARN resource type (and id prefix) of an attachment: `^attachment-([0-9a-f]{8,17})$`
/// on `AttachmentId` (`smithy_query shape=AttachmentId`).
const ATTACHMENT: &str = "attachment";

/// The `AttachmentType` a Connect attachment reports, one of the values the enum carries
/// (`smithy_query shape=AttachmentType`).
const CONNECT: &str = "CONNECT";

/// The three `AttachmentState` values a Connect attachment passes through here
/// (`smithy_query shape=AttachmentState`).
const PENDING_ACCEPTANCE: &str = "PENDING_ATTACHMENT_ACCEPTANCE";
/// See [`PENDING_ACCEPTANCE`].
const CREATING: &str = "CREATING";
/// See [`PENDING_ACCEPTANCE`].
const AVAILABLE: &str = "AVAILABLE";

/// The `GetConnectAttachment` URI of `attachment_id`
/// (`GET /connect-attachments/{AttachmentId}`, `smithy_query operation=GetConnectAttachment`).
fn get_uri(attachment_id: &str) -> String {
    format!("/connect-attachments/{attachment_id}")
}

/// `GetConnectAttachment`.
async fn read_connect(emulator: &Emulator, attachment_id: &str) -> (u16, Value) {
    emulator.get(&get_uri(attachment_id)).await
}

/// One row of the read table: the Connect attachment to create, and what the policy of its
/// core network therefore decides about it.
struct ReadCase {
    /// Names the case in every assertion message.
    name: &'static str,
    /// The Region of the core network's single edge location, and so of both attachments.
    region: &'static str,
    /// How many subnets the transport VPC attachment reaches its VPC through.
    subnets: usize,
    /// The `Options.Protocol` the Connect attachment create carries, or `None` for an
    /// `Options` naming no protocol. `GRE` and `NO_ENCAP` are the two values the enum
    /// carries (`smithy_query shape=TunnelProtocol`).
    protocol: Option<&'static str>,
    /// The `Tags` the create carries, or `Value::Null` for no `Tags` member.
    tags: Value,
    /// The `RoutingPolicyLabel` the create carries, or `None` for none.
    routing_policy_label: Option<&'static str>,
    /// The `attachment-policies` the core network's policy document carries, or
    /// `Value::Null` for a document carrying none.
    rules: Value,
    /// The state the created Connect attachment is in: [`PENDING_ACCEPTANCE`] exactly when
    /// the rule that claims it asks for acceptance.
    state: &'static str,
    /// The `AttachmentPolicyRuleNumber` the attachment reports — the claiming rule's number,
    /// or `Value::Null` where no rule claims it.
    rule_number: Value,
    /// The `SegmentName` the attachment reports, or `Value::Null` where the claiming rule
    /// names none.
    segment: Value,
}

/// The table: rows differing along everything the rule generalises over — the tunnel
/// protocol, the Region, the transport's subnet count, the tags, the routing policy label,
/// and what the `attachment-policies` of the core network decide (nothing at all, a claim
/// without acceptance, a claim requiring it).
///
/// Every policy key named here (`rule-number`, `conditions`, `type`, `operator`, `value`,
/// `action`, `association-method`, `segment`, `require-attachment-acceptance`) is the
/// published Cloud WAN core network policy schema's, the same source
/// `crate::attachment_acceptance` builds its documents from.
fn read_cases() -> Vec<ReadCase> {
    vec![
        ReadCase {
            name: "GRE over a one-subnet transport, no tags, a policy claiming nothing",
            region: "us-west-2",
            subnets: 1,
            protocol: Some("GRE"),
            tags: Value::Null,
            routing_policy_label: None,
            rules: Value::Null,
            state: CREATING,
            rule_number: Value::Null,
            segment: Value::Null,
        },
        ReadCase {
            name: "NO_ENCAP over a two-subnet transport, two tags, a label, claimed by `any`",
            region: "eu-west-1",
            subnets: 2,
            protocol: Some("NO_ENCAP"),
            tags: json!([{ "Key": "team", "Value": "net" }, { "Key": "tier", "Value": "edge" }]),
            routing_policy_label: Some("north"),
            rules: json!([{
                "rule-number": 50,
                "conditions": [{ "type": "any" }],
                "action": {
                    "association-method": "constant",
                    "segment": "shared",
                    "require-attachment-acceptance": false,
                },
            }]),
            state: CREATING,
            rule_number: json!(50),
            segment: json!("shared"),
        },
        ReadCase {
            name: "no protocol over a three-subnet transport, claimed by `attachment-type` CONNECT, acceptance required",
            region: "ap-south-1",
            subnets: 3,
            protocol: None,
            tags: json!([{ "Key": "only", "Value": "" }]),
            routing_policy_label: None,
            rules: json!([{
                "rule-number": 200,
                "conditions": [{
                    "type": "attachment-type",
                    "operator": "equals",
                    "value": "CONNECT",
                }],
                "action": {
                    "association-method": "constant",
                    "segment": "prod",
                    "require-attachment-acceptance": true,
                },
            }]),
            state: PENDING_ACCEPTANCE,
            rule_number: json!(200),
            segment: json!("prod"),
        },
        ReadCase {
            name: "GRE in a third Region, a label, claimed by `region`, acceptance required",
            region: "il-central-1",
            subnets: 1,
            protocol: Some("GRE"),
            tags: Value::Null,
            routing_policy_label: Some("south"),
            rules: json!([{
                "rule-number": 1,
                "conditions": [{ "type": "region", "operator": "equals", "value": "il-central-1" }],
                "action": {
                    "association-method": "constant",
                    "segment": "shared",
                    "require-attachment-acceptance": true,
                },
            }]),
            state: PENDING_ACCEPTANCE,
            rule_number: json!(1),
            segment: json!("shared"),
        },
        ReadCase {
            name: "NO_ENCAP claimed by a `tag-value` rule, segment from that tag",
            region: "us-east-1",
            subnets: 2,
            protocol: Some("NO_ENCAP"),
            tags: json!([{ "Key": "env", "Value": "prod" }]),
            routing_policy_label: None,
            rules: json!([{
                "rule-number": 10,
                "conditions": [{
                    "type": "tag-value",
                    "operator": "equals",
                    "key": "env",
                    "value": "prod",
                }],
                "action": {
                    "association-method": "tag",
                    "tag-value-of-key": "env",
                    "require-attachment-acceptance": false,
                },
            }]),
            state: CREATING,
            rule_number: json!(10),
            segment: json!("prod"),
        },
        ReadCase {
            name: "GRE under a rule whose condition does not claim it",
            region: "sa-east-1",
            subnets: 1,
            protocol: Some("GRE"),
            tags: Value::Null,
            routing_policy_label: None,
            rules: json!([{
                "rule-number": 300,
                "conditions": [{ "type": "tag-exists", "key": "env" }],
                "action": {
                    "association-method": "constant",
                    "segment": "prod",
                    "require-attachment-acceptance": true,
                },
            }]),
            state: CREATING,
            rule_number: Value::Null,
            segment: Value::Null,
        },
    ]
}

/// The `Options` a row's create carries: the protocol it names, or an object naming none.
fn options_of(case: &ReadCase) -> Value {
    match case.protocol {
        Some(protocol) => json!({ "Protocol": protocol }),
        None => json!({}),
    }
}

/// The `Tags` a row's attachment reports: what the create carried, or the empty list where
/// it carried no `Tags` member.
fn expected_tags(case: &ReadCase) -> Value {
    if case.tags.is_null() {
        json!([])
    } else {
        case.tags.clone()
    }
}

/// `body` with the row's optional `Tags` / `RoutingPolicyLabel` members added.
fn with_optional(mut body: Value, case: &ReadCase) -> Value {
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

/// Assert the `ConnectAttachment` in `shape` is the one `case` describes, at `state`.
fn assert_connect_attachment(
    case: &ReadCase,
    shape: &Value,
    core_network: &str,
    transport: &str,
    state: &str,
) {
    let name = case.name;
    let attachment = &shape["Attachment"];
    let id = attachment_id(name, shape);
    assert!(
        is_minted_id(&id, ATTACHMENT),
        "case {name:?}: expected an attachment-<17 hex> id, got {id:?}"
    );
    assert_eq!(
        shape["TransportAttachmentId"],
        json!(transport),
        "case {name:?}: expected the stored transport attachment, got {shape}"
    );
    // The whole `Options` object, not just its member: a row whose create named no protocol
    // must still report an `Options` that is PRESENT and empty, not an absent one.
    assert_eq!(
        shape["Options"],
        options_of(case),
        "case {name:?}: expected the stored tunnel options, got {shape}"
    );
    assert_eq!(
        shape["Options"]["Protocol"],
        case.protocol.map_or(Value::Null, |protocol| json!(protocol)),
        "case {name:?}: expected the stored tunnel protocol, got {shape}"
    );
    assert_eq!(
        attachment["CoreNetworkId"],
        json!(core_network),
        "case {name:?}: expected the core network the attachment hangs off, got {attachment}"
    );
    assert_eq!(
        attachment["CoreNetworkArn"],
        json!(expected_arn("core-network", core_network)),
        "case {name:?}: expected the core network's ARN, got {attachment}"
    );
    assert_eq!(
        attachment["AttachmentType"],
        json!(CONNECT),
        "case {name:?}: expected a {CONNECT} attachment, got {attachment}"
    );
    assert_eq!(
        attachment["EdgeLocation"],
        json!(case.region),
        "case {name:?}: expected the edge location of the transport, got {attachment}"
    );
    // A Connect attachment attaches no resource of its own: it is carried over its
    // transport, so that attachment's ARN is the resource its record names.
    assert_eq!(
        attachment["ResourceArn"],
        json!(expected_arn(ATTACHMENT, transport)),
        "case {name:?}: expected the transport's ARN as the attached resource, got {attachment}"
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
        "case {name:?}: no modification of the attachment has failed, got {attachment}"
    );
    assert_eq!(
        attachment["AttachmentPolicyRuleNumber"], case.rule_number,
        "case {name:?}: expected the claiming rule's number {}, got {attachment}",
        case.rule_number
    );
    assert_eq!(
        attachment["SegmentName"], case.segment,
        "case {name:?}: expected segment {}, got {attachment}",
        case.segment
    );
    assert_eq!(
        attachment["State"],
        json!(state),
        "case {name:?}: expected state {state}, got {attachment}"
    );
    assert!(
        attachment["CreatedAt"].as_f64().is_some_and(|at| at > 0.0),
        "case {name:?}: CreatedAt must be a timestamp, got {attachment}"
    );
}

#[tokio::test]
async fn get_connect_attachment_reports_the_stored_record_of_every_kind_of_connect_attachment() {
    let emulator = Emulator::new();
    let cases = read_cases();
    // What each row created, kept so the transition the clock owes can be asserted after ONE
    // wait: the row, its Connect attachment id, its transport id and the shape the create
    // answered.
    let mut created: Vec<(&ReadCase, String, String, Value)> = Vec::new();

    for (index, case) in cases.iter().enumerate() {
        let name = case.name;
        let core_network = core_network_with(&emulator, name, case.region, &case.rules).await;

        // The transport: "a core network Connect attachment uses an existing VPC attachment
        // as the underlying transport mechanism" (`smithy_query query="doc"
        // shape="CreateConnectAttachment"`).
        let subnets: Vec<String> = (0..case.subnets)
            .map(|subnet| subnet_arn(case.region, (index + 1) * 100 + subnet))
            .collect();
        let (status, created_transport) = create_vpc_attachment(
            &emulator,
            json!({
                "CoreNetworkId": core_network,
                "VpcArn": vpc_arn(case.region, index + 1),
                "SubnetArns": subnets,
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?}: CreateVpcAttachment expected 200, got {status} with {created_transport}"
        );
        let transport = attachment_id(name, &created_transport["VpcAttachment"]);

        let request = json!({
            "CoreNetworkId": core_network,
            "EdgeLocation": case.region,
            "TransportAttachmentId": transport,
            "Options": options_of(case),
        });
        let (status, created_connect) =
            create_connect_attachment(&emulator, with_optional(request, case)).await;
        assert_eq!(
            status, 200,
            "case {name:?}: CreateConnectAttachment expected 200, got {status} with {created_connect}"
        );
        let at_create = created_connect["ConnectAttachment"].clone();
        assert_connect_attachment(case, &at_create, &core_network, &transport, case.state);
        let id = attachment_id(name, &at_create);

        // The read answers the record the create stored, member for member: the state it
        // reports is the stored one, not one the read moved.
        let (status, read) = read_connect(&emulator, &id).await;
        assert_eq!(
            status, 200,
            "case {name:?}: GetConnectAttachment expected 200, got {status} with {read}"
        );
        assert_eq!(
            read["ConnectAttachment"], at_create,
            "case {name:?}: the reader must answer the stored attachment, got {read}"
        );

        // A read never changes what the next caller sees.
        let (_, again) = read_connect(&emulator, &id).await;
        assert_eq!(
            again, read,
            "case {name:?}: GetConnectAttachment must be a pure read, got {read} then {again}"
        );

        // The attachment's own ARN is not a member of the shape, so it is read back the one
        // way a client can: through the tagging slice, which resolves that ARN to this
        // stored attachment and the tags it was created with.
        let (status, listed) = list_tags(&emulator, &expected_arn(ATTACHMENT, &id)).await;
        assert_eq!(
            status, 200,
            "case {name:?}: ListTagsForResource expected 200, got {status} with {listed}"
        );
        assert_eq!(
            listed,
            expected_tags(case),
            "case {name:?}: the attachment ARN must resolve to the stored attachment's tags, got {listed}"
        );

        // An attachment waiting for the acceptance its policy asked for is moved by nothing
        // but that acceptance: the clock owes it no transition, and `AcceptAttachment` is
        // what starts the one it then owes.
        if case.state == PENDING_ACCEPTANCE {
            let (status, accepted) = accept(&emulator, &id).await;
            assert_eq!(
                status, 200,
                "case {name:?}: AcceptAttachment expected 200, got {status} with {accepted}"
            );
            let (status, read) = read_connect(&emulator, &id).await;
            assert_eq!(
                status, 200,
                "case {name:?}: GetConnectAttachment after the acceptance expected 200, got {status} with {read}"
            );
            assert_connect_attachment(
                case,
                &read["ConnectAttachment"],
                &core_network,
                &transport,
                CREATING,
            );
        }

        created.push((case, id, transport, at_create));
    }

    // Past the settle span every row's `CREATING` -> `AVAILABLE` transition has fallen due,
    // and the read reports it although no call made it.
    tokio::time::sleep(PAST_SETTLE).await;
    for (case, id, transport, at_create) in &created {
        let name = case.name;
        let (status, read) = read_connect(&emulator, id).await;
        assert_eq!(
            status, 200,
            "case {name:?}: GetConnectAttachment past the settle span expected 200, got {status} with {read}"
        );
        let shape = &read["ConnectAttachment"];
        let attachment = &shape["Attachment"];
        assert_eq!(
            attachment["State"],
            json!(AVAILABLE),
            "case {name:?}: past the settle span the attachment is {AVAILABLE}, got {attachment}"
        );
        let created_at = attachment["CreatedAt"].as_f64().unwrap_or_default();
        let updated_at = attachment["UpdatedAt"].as_f64().unwrap_or_default();
        assert!(
            updated_at > created_at,
            "case {name:?}: the settle transition is a change, so UpdatedAt ({updated_at}) must be past CreatedAt ({created_at}), got {attachment}"
        );
        assert_eq!(
            shape["TransportAttachmentId"],
            json!(transport),
            "case {name:?}: the transport must not have moved with the state, got {shape}"
        );
        assert_eq!(
            shape["Options"], at_create["Options"],
            "case {name:?}: the tunnel options must not have moved with the state, got {shape}"
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
            "AttachmentPolicyRuleNumber",
            "SegmentName",
        ] {
            assert_eq!(
                attachment[member], at_create["Attachment"][member],
                "case {name:?}: {member} must not have moved with the state, got {attachment}"
            );
        }

        // The settled state is a function of the stored record and the clock, not of the
        // read: once the transition has fallen due, `UpdatedAt` stays where it fell rather
        // than following the clock of each read.
        let (_, again) = read_connect(&emulator, id).await;
        assert_eq!(
            again, read,
            "case {name:?}: a settled attachment reads the same twice, got {read} then {again}"
        );
    }
}

#[tokio::test]
async fn several_connect_attachments_over_one_transport_each_read_back_their_own_record() {
    let emulator = Emulator::new();
    let (_, core_network) =
        core_network_at(&emulator, "one transport, many tunnels", &["us-west-2"]).await;
    let (status, created_transport) = create_vpc_attachment(
        &emulator,
        json!({
            "CoreNetworkId": core_network,
            "VpcArn": vpc_arn("us-west-2", 1),
            "SubnetArns": [subnet_arn("us-west-2", 11), subnet_arn("us-west-2", 12)],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateVpcAttachment expected 200, got {status} with {created_transport}"
    );
    let transport = attachment_id("the transport", &created_transport["VpcAttachment"]);

    // One row per Connect attachment carried over that one transport: the protocols the
    // enum carries and a request naming none, so the count of tunnels sharing a transport
    // is a dimension of the table too.
    let mut tunnels: Vec<(Option<&str>, String)> = Vec::new();
    for protocol in [Some("GRE"), Some("NO_ENCAP"), None] {
        let mut request = json!({
            "CoreNetworkId": core_network,
            "EdgeLocation": "us-west-2",
            "TransportAttachmentId": transport,
            "Options": {},
        });
        if let Some(protocol) = protocol {
            request["Options"] = json!({ "Protocol": protocol });
        }
        let (status, created) = create_connect_attachment(&emulator, request).await;
        assert_eq!(
            status, 200,
            "case {protocol:?}: CreateConnectAttachment expected 200, got {status} with {created}"
        );
        let id = attachment_id("a tunnel", &created["ConnectAttachment"]);
        assert!(
            tunnels.iter().all(|(_, minted)| *minted != id),
            "case {protocol:?}: each Connect attachment gets its own id, got {id:?} twice"
        );
        tunnels.push((protocol, id));
    }

    for (protocol, id) in &tunnels {
        let (status, read) = read_connect(&emulator, id).await;
        assert_eq!(
            status, 200,
            "case {protocol:?}: GetConnectAttachment expected 200, got {status} with {read}"
        );
        let shape = &read["ConnectAttachment"];
        assert_eq!(
            shape["Attachment"]["AttachmentId"],
            json!(id),
            "case {protocol:?}: the reader must answer the attachment asked for, got {shape}"
        );
        assert_eq!(
            shape["TransportAttachmentId"],
            json!(transport),
            "case {protocol:?}: every tunnel reports the transport it was created over, got {shape}"
        );
        assert_eq!(
            shape["Options"]["Protocol"],
            protocol.map_or(Value::Null, |protocol| json!(protocol)),
            "case {protocol:?}: each tunnel reports its OWN tunnel options, got {shape}"
        );
    }
}

#[tokio::test]
async fn an_id_that_names_no_stored_connect_attachment_is_the_modelled_not_found() {
    let emulator = Emulator::new();
    let (global_network, core_network) =
        core_network_at(&emulator, "not found", &["us-east-1"]).await;

    let (status, created_transport) = create_vpc_attachment(
        &emulator,
        json!({
            "CoreNetworkId": core_network,
            "VpcArn": vpc_arn("us-east-1", 1),
            "SubnetArns": [subnet_arn("us-east-1", 11)],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateVpcAttachment expected 200, got {status} with {created_transport}"
    );
    let vpc = attachment_id("the transport", &created_transport["VpcAttachment"]);

    let (status, created_vpn) = emulator
        .post(
            "/site-to-site-vpn-attachments",
            json!({
                "CoreNetworkId": core_network,
                "VpnConnectionArn": format!(
                    "arn:aws:ec2:us-east-1:{}:vpn-connection/vpn-0000000a",
                    engine::emulator_account_id()
                ),
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "CreateSiteToSiteVpnAttachment expected 200, got {status} with {created_vpn}"
    );
    let vpn = attachment_id("the VPN attachment", &created_vpn["SiteToSiteVpnAttachment"]);

    let (status, created_connect) = create_connect_attachment(
        &emulator,
        json!({
            "CoreNetworkId": core_network,
            "EdgeLocation": "us-east-1",
            "TransportAttachmentId": vpc,
            "Options": { "Protocol": "GRE" },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateConnectAttachment expected 200, got {status} with {created_connect}"
    );
    let connect = attachment_id("the Connect attachment", &created_connect["ConnectAttachment"]);

    // Each row is a different way of naming no stored CONNECT attachment, every one
    // satisfying the modelled `AttachmentId` pattern so it reaches the handler: a
    // never-minted id at each width the pattern allows, the counter value a DIFFERENT
    // resource kind of this service was minted under spelled with the attachment prefix, and
    // the ids of the two attachments of OTHER kinds the setup stored in the one id space the
    // attachment readers share.
    let borrowed_suffix = format!(
        "attachment-{}",
        core_network.strip_prefix("core-network-").unwrap_or_else(|| panic!(
            "a minted core network id carries the core-network- prefix, got {core_network:?}"
        ))
    );
    for (case, asked_for) in [
        ("never minted, 17 hex digits", "attachment-00000000000000099"),
        ("never minted, the 8-hex lower bound", "attachment-0000beef"),
        ("another counter value under this prefix", &borrowed_suffix),
        ("a VPC attachment's id", &vpc),
        ("a site-to-site VPN attachment's id", &vpn),
    ] {
        let (status, body) = read_connect(&emulator, asked_for).await;
        assert_eq!(
            status, 404,
            "case {case} ({asked_for:?}): expected 404, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case} ({asked_for:?}): wrong error code in {body}"
        );
        assert_eq!(
            body["ResourceType"],
            json!(ATTACHMENT),
            "case {case} ({asked_for:?}): the refusal must name the resource type, got {body}"
        );
        assert_eq!(
            body["ResourceId"],
            json!(asked_for),
            "case {case} ({asked_for:?}): the refusal must name the id asked for, got {body}"
        );
    }

    // The refusals are the lookup and nothing wider: the stored Connect attachment still
    // reads, and the reads and refusals stored nothing — the global network still holds
    // exactly the three attachments the setup created in it.
    let (status, read) = read_connect(&emulator, &connect).await;
    assert_eq!(
        status, 200,
        "GetConnectAttachment on the live attachment expected 200, got {status} with {read}"
    );
    let (status, counts) = emulator
        .get(&format!(
            "/global-networks/{global_network}/network-resource-count?resourceType=attachment"
        ))
        .await;
    assert_eq!(
        status, 200,
        "GetNetworkResourceCounts expected 200, got {status} with {counts}"
    );
    // Pick the entry by its `ResourceType` rather than by position, so the assertion names
    // the count it means even if the list ever carries more than one entry.
    let attachment_count = counts["NetworkResourceCounts"]
        .as_array()
        .unwrap_or_else(|| panic!("a count list is an array, got {counts}"))
        .iter()
        .find(|entry| entry["ResourceType"] == json!(ATTACHMENT))
        .unwrap_or_else(|| panic!("the counts must carry an {ATTACHMENT} entry, got {counts}"))
        .clone();
    assert_eq!(
        attachment_count["Count"],
        json!(3),
        "the reads stored nothing, so the global network still holds its three attachments, got {counts}"
    );
}
