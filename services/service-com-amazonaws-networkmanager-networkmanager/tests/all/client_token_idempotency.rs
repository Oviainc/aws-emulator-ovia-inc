// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of the ONE idempotency-token rule every operation of this service
//! that models `ClientToken` follows, driven over HTTP through the real router (the harness
//! lives in `crate::global_network_lifecycle`).
//!
//! The rule under test: a create whose request carries a non-empty idempotency token is
//! resolved against the records the relevant store already holds — a record under that token
//! whose request is the same one is ANSWERED instead of a second record being produced, the
//! same token asking for anything else is refused `ConflictException` (HTTP 409) naming that
//! record and storing nothing, and an EMPTY token is no token at all, so a request carrying
//! one behaves exactly as a request carrying no `ClientToken` member.
//!
//! The table below spans that class: one row per operation that models the token — the nine
//! creates and `PutCoreNetworkPolicy` — differing in the store the lookup walks, in which
//! member the second call changes (a list, a tunnel protocol, a tag set, an address, a
//! description, an alias), and in whether the record the refusal names has an id of its own
//! or is keyed by the core network that holds it. Nothing reaches into `crate::state` for
//! data: every record is produced and observed through an operation.

use serde_json::{json, Value};

use service_com_amazonaws_networkmanager_networkmanager::state::{
    ATTACHMENT_KIND, CONNECT_PEER_KIND, CORE_NETWORK_KIND, CORE_NETWORK_POLICY_KIND, PEERING_KIND,
    PREFIX_LIST_ASSOCIATION_KIND,
};

use crate::attachment_lifecycle::{
    attachment_id, core_network_at, create_connect_attachment, create_vpc_attachment,
    policy_document, subnet_arn, vpc_arn,
};
use crate::direct_connect_gateway_attachment_lifecycle::gateway_arn;
use crate::global_network_lifecycle::{error_code, Emulator};
use crate::site_to_site_vpn_attachment_lifecycle::vpn_arn;
use crate::transit_gateway_peering::{create_peering, own_account, peering_id, transit_gateway_arn};

/// The edge locations the core network under test configures, so a row can ask for one edge
/// or for both.
const EDGES: [&str; 2] = ["us-west-2", "eu-west-1"];

/// A transit gateway route table ARN in `region`.
///
/// The form is the skeleton every EC2 ARN this model types spells — `VpcArn` carries
/// `^arn:[^:]{1,63}:ec2:[^:]{0,63}:[^:]{0,63}:vpc\/vpc-[0-9a-f]{8,17}$` (`smithy_query
/// shape=VpcArn`) — with the `transit-gateway-route-table` resource type, since
/// `TransitGatewayRouteTableArn` itself carries only a length and `^[\s\S]*$`.
fn route_table_arn(region: &str, index: usize) -> String {
    format!(
        "arn:aws:ec2:{region}:{}:transit-gateway-route-table/tgw-rtb-{index:08x}",
        own_account()
    )
}

/// A managed prefix list ARN: the same skeleton as [`route_table_arn`], with the
/// `prefix-list` resource type and a `pl-` id of 8 hex digits, the shortest that skeleton
/// admits.
fn prefix_list_arn(index: usize) -> String {
    format!(
        "arn:aws:ec2:{}:{}:prefix-list/pl-{index:08x}",
        EDGES[0],
        own_account()
    )
}

/// The resources the rows hang their creates off: everything a tokened create needs to
/// reach a handler, none of it carrying a token of its own.
struct Fixture {
    /// The core network most rows create on.
    core_network: String,
    /// A global network holding no core network yet, which the `CreateCoreNetwork` row
    /// creates in: a global network holds a single core network.
    spare_global_network: String,
    /// A VPC attachment the Connect attachment row is carried over.
    transport: String,
    /// A Connect attachment the Connect peer row peers on.
    connect_attachment: String,
    /// A peering the transit gateway route table attachment row attaches over.
    peering: String,
}

/// One VPC attachment of the fixture, attaching the VPC `index` at the first edge.
async fn fixture_transport(
    emulator: &Emulator,
    name: &str,
    core_network: &str,
    index: usize,
) -> String {
    let (status, created) = create_vpc_attachment(
        emulator,
        json!({
            "CoreNetworkId": core_network,
            "VpcArn": vpc_arn(EDGES[0], index),
            "SubnetArns": [subnet_arn(EDGES[0], index)],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {name:?}: the fixture's CreateVpcAttachment expected 200, got {status} with {created}"
    );
    attachment_id(name, &created["VpcAttachment"])
}

/// Build the fixture in a fresh emulator.
async fn fixture(emulator: &Emulator, name: &str) -> Fixture {
    let (_, core_network) = core_network_at(emulator, name, &EDGES).await;
    let spare_global_network = emulator
        .create_global_network(&format!("{name}-spare"))
        .await;
    let transport = fixture_transport(emulator, name, &core_network, 90).await;
    let peer_transport = fixture_transport(emulator, name, &core_network, 91).await;

    let (status, created) = create_connect_attachment(
        emulator,
        json!({
            "CoreNetworkId": &core_network,
            "EdgeLocation": EDGES[0],
            "TransportAttachmentId": peer_transport,
            "Options": { "Protocol": "GRE" },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {name:?}: the fixture's CreateConnectAttachment expected 200, got {status} with {created}"
    );
    let connect_attachment = attachment_id(name, &created["ConnectAttachment"]);

    let (status, created) = create_peering(
        emulator,
        json!({
            "CoreNetworkId": &core_network,
            "TransitGatewayArn": transit_gateway_arn("aws", EDGES[0], &own_account(), 90, 8),
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {name:?}: the fixture's CreateTransitGatewayPeering expected 200, got {status} with {created}"
    );
    let peering = peering_id(name, &created["TransitGatewayPeering"]);

    Fixture {
        core_network,
        spare_global_network,
        transport,
        connect_attachment,
        peering,
    }
}

/// One row: the create of one operation that models the token, and what makes a second call
/// under the same token a DIFFERENT request.
struct TokenRow {
    /// Names the case in every assertion message.
    name: &'static str,
    /// The route the create is addressed to.
    uri: String,
    /// The request, carrying no `ClientToken` member: the harness adds the row's token.
    request: Value,
    /// The same create asking for something else, one compared member changed.
    different: Value,
    /// Where the success answer carries the identity of the record the create produced.
    identity: &'static [&'static str],
    /// The `ResourceType` the refusal must name.
    resource_type: &'static str,
    /// What the refusal must say the first request under the token DID, given the identity
    /// it reported: the one part of the message each handler still owns.
    effect: Box<dyn Fn(&str) -> String>,
    /// The core network the record is keyed under where it has no id of its own, so the
    /// refusal names the composite `<CoreNetworkId>/<identity>` key the store holds it
    /// under (`state::prefix_list_association_key`,
    /// `state::policy_version_resource_id`); `None` where the record has its own id.
    key_scope: Option<String>,
}

/// The table: one row per tokened operation of this service.
fn rows(fixture: &Fixture) -> Vec<TokenRow> {
    let core_network = fixture.core_network.clone();
    vec![
        TokenRow {
            name: "CreateVpcAttachment, the subnet list changed",
            uri: "/vpc-attachments".to_string(),
            request: json!({
                "CoreNetworkId": &core_network,
                "VpcArn": vpc_arn(EDGES[0], 1),
                "SubnetArns": [subnet_arn(EDGES[0], 1)],
            }),
            different: json!({
                "CoreNetworkId": &core_network,
                "VpcArn": vpc_arn(EDGES[0], 1),
                "SubnetArns": [subnet_arn(EDGES[0], 1), subnet_arn(EDGES[0], 2)],
            }),
            identity: &["VpcAttachment", "Attachment", "AttachmentId"],
            resource_type: ATTACHMENT_KIND,
            effect: Box::new(|id| format!("created attachment {id}")),
            key_scope: None,
        },
        TokenRow {
            name: "CreateConnectAttachment, the tunnel protocol changed",
            uri: "/connect-attachments".to_string(),
            request: json!({
                "CoreNetworkId": &core_network,
                "EdgeLocation": EDGES[0],
                "TransportAttachmentId": &fixture.transport,
                "Options": { "Protocol": "GRE" },
            }),
            different: json!({
                "CoreNetworkId": &core_network,
                "EdgeLocation": EDGES[0],
                "TransportAttachmentId": &fixture.transport,
                "Options": { "Protocol": "NO_ENCAP" },
            }),
            identity: &["ConnectAttachment", "Attachment", "AttachmentId"],
            resource_type: ATTACHMENT_KIND,
            effect: Box::new(|id| format!("created attachment {id}")),
            key_scope: None,
        },
        TokenRow {
            name: "CreateSiteToSiteVpnAttachment, a tag added",
            uri: "/site-to-site-vpn-attachments".to_string(),
            request: json!({
                "CoreNetworkId": &core_network,
                "VpnConnectionArn": vpn_arn("aws", EDGES[1], &own_account(), "00000011"),
            }),
            different: json!({
                "CoreNetworkId": &core_network,
                "VpnConnectionArn": vpn_arn("aws", EDGES[1], &own_account(), "00000011"),
                "Tags": [{ "Key": "env", "Value": "test" }],
            }),
            identity: &["SiteToSiteVpnAttachment", "Attachment", "AttachmentId"],
            resource_type: ATTACHMENT_KIND,
            effect: Box::new(|id| format!("created attachment {id}")),
            key_scope: None,
        },
        TokenRow {
            name: "CreateDirectConnectGatewayAttachment, a second edge location",
            uri: "/direct-connect-gateway-attachments".to_string(),
            request: json!({
                "CoreNetworkId": &core_network,
                "DirectConnectGatewayArn": gateway_arn(21),
                "EdgeLocations": [EDGES[0]],
            }),
            different: json!({
                "CoreNetworkId": &core_network,
                "DirectConnectGatewayArn": gateway_arn(21),
                "EdgeLocations": EDGES,
            }),
            identity: &["DirectConnectGatewayAttachment", "Attachment", "AttachmentId"],
            resource_type: ATTACHMENT_KIND,
            effect: Box::new(|id| format!("created attachment {id}")),
            key_scope: None,
        },
        TokenRow {
            name: "CreateTransitGatewayRouteTableAttachment, another route table",
            uri: "/transit-gateway-route-table-attachments".to_string(),
            request: json!({
                "PeeringId": &fixture.peering,
                "TransitGatewayRouteTableArn": route_table_arn(EDGES[0], 31),
            }),
            different: json!({
                "PeeringId": &fixture.peering,
                "TransitGatewayRouteTableArn": route_table_arn(EDGES[0], 32),
            }),
            identity: &["TransitGatewayRouteTableAttachment", "Attachment", "AttachmentId"],
            resource_type: ATTACHMENT_KIND,
            effect: Box::new(|id| format!("created attachment {id}")),
            key_scope: None,
        },
        TokenRow {
            name: "CreateTransitGatewayPeering, another transit gateway",
            uri: "/transit-gateway-peerings".to_string(),
            request: json!({
                "CoreNetworkId": &core_network,
                "TransitGatewayArn": transit_gateway_arn("aws", EDGES[1], &own_account(), 41, 12),
            }),
            different: json!({
                "CoreNetworkId": &core_network,
                "TransitGatewayArn": transit_gateway_arn("aws", EDGES[1], &own_account(), 42, 12),
            }),
            identity: &["TransitGatewayPeering", "Peering", "PeeringId"],
            resource_type: PEERING_KIND,
            effect: Box::new(|id| format!("created peering {id}")),
            key_scope: None,
        },
        TokenRow {
            name: "CreateConnectPeer, another peer address",
            uri: "/connect-peers".to_string(),
            request: json!({
                "ConnectAttachmentId": &fixture.connect_attachment,
                "PeerAddress": "203.0.113.50",
                "InsideCidrBlocks": ["169.254.50.0/29"],
                "BgpOptions": { "PeerAsn": 65050 },
            }),
            different: json!({
                "ConnectAttachmentId": &fixture.connect_attachment,
                "PeerAddress": "203.0.113.51",
                "InsideCidrBlocks": ["169.254.50.0/29"],
                "BgpOptions": { "PeerAsn": 65050 },
            }),
            identity: &["ConnectPeer", "ConnectPeerId"],
            resource_type: CONNECT_PEER_KIND,
            effect: Box::new(|id| format!("created Connect peer {id}")),
            key_scope: None,
        },
        TokenRow {
            name: "CreateCoreNetwork, another description",
            uri: "/core-networks".to_string(),
            request: json!({
                "GlobalNetworkId": &fixture.spare_global_network,
                "Description": "the core network the token created",
            }),
            different: json!({
                "GlobalNetworkId": &fixture.spare_global_network,
                "Description": "a core network of another description",
            }),
            identity: &["CoreNetwork", "CoreNetworkId"],
            resource_type: CORE_NETWORK_KIND,
            effect: Box::new(|id| format!("created core network {id}")),
            key_scope: None,
        },
        TokenRow {
            name: "CreateCoreNetworkPrefixListAssociation, another alias",
            uri: "/prefix-list".to_string(),
            request: json!({
                "CoreNetworkId": &core_network,
                "PrefixListArn": prefix_list_arn(61),
                "PrefixListAlias": "alpha",
            }),
            different: json!({
                "CoreNetworkId": &core_network,
                "PrefixListArn": prefix_list_arn(61),
                "PrefixListAlias": "beta",
            }),
            identity: &["PrefixListArn"],
            resource_type: PREFIX_LIST_ASSOCIATION_KIND,
            effect: {
                let core_network = core_network.clone();
                Box::new(move |arn| {
                    format!("associated prefix list {arn} with core network {core_network}")
                })
            },
            key_scope: Some(core_network.clone()),
        },
        TokenRow {
            name: "PutCoreNetworkPolicy, another description",
            uri: format!("/core-networks/{core_network}/core-network-policy"),
            request: json!({
                "PolicyDocument": policy_document(&[EDGES[0], EDGES[1], "ap-south-1"]),
                "Description": "the version the token created",
            }),
            different: json!({
                "PolicyDocument": policy_document(&[EDGES[0], EDGES[1], "ap-south-1"]),
                "Description": "a version of another description",
            }),
            identity: &["CoreNetworkPolicy", "PolicyVersionId"],
            resource_type: CORE_NETWORK_POLICY_KIND,
            effect: {
                let core_network = core_network.clone();
                Box::new(move |version| {
                    format!("created policy version {version} of core network {core_network}")
                })
            },
            key_scope: Some(core_network.clone()),
        },
    ]
}

/// `request` with the `ClientToken` member `token` (nothing added for `None`).
fn with_token(request: &Value, token: Option<&str>) -> Value {
    let mut body = request.clone();
    if let Some(token) = token {
        body["ClientToken"] = json!(token);
    }
    body
}

/// The identity the success answer `body` reports for the record `row` produced.
fn identity(row: &TokenRow, body: &Value) -> Value {
    let mut found = body;
    for member in row.identity {
        found = &found[*member];
    }
    assert!(
        !found.is_null(),
        "case {:?}: expected {:?} in {body}",
        row.name,
        row.identity
    );
    found.clone()
}

/// A reported identity as a string, whether the member is modelled a string or an integer.
fn scalar(row: &TokenRow, value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Number(number) => number.to_string(),
        other => panic!("case {:?}: expected a scalar identity, got {other}", row.name),
    }
}

/// The message of a refusal: `ConflictException` carries it in `Message`, the service's
/// other refusals in `message`.
fn message_of(body: &Value) -> String {
    body["Message"]
        .as_str()
        .or_else(|| body["message"].as_str())
        .unwrap_or_default()
        .to_string()
}

#[tokio::test]
async fn every_tokened_create_replays_its_token_and_refuses_a_different_request() {
    let emulator = Emulator::new();
    let fixture = fixture(&emulator, "idempotency").await;

    for (index, row) in rows(&fixture).iter().enumerate() {
        let case = row.name;
        // One token per row: the attachment rows share a single store, so a token already
        // spent on another kind would be a different request rather than a fresh one.
        let token = format!("token-{index}");
        let request = with_token(&row.request, Some(&token));

        let (status, first) = emulator.post(&row.uri, request.clone()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the first create expected 200, got {status} with {first}"
        );
        let produced = identity(row, &first);

        let (status, retried) = emulator.post(&row.uri, request.clone()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a retry of the identical request expected 200, got {status} with {retried}"
        );
        assert_eq!(
            identity(row, &retried),
            produced,
            "case {case:?}: a retry must answer the record the first call produced ({produced}), got {retried}"
        );

        let (status, refused) = emulator
            .post(&row.uri, with_token(&row.different, Some(&token)))
            .await;
        assert_eq!(
            status, 409,
            "case {case:?}: the same token asking for something else expected 409, got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {refused}"
        );
        assert_eq!(
            refused["ResourceType"],
            json!(row.resource_type),
            "case {case:?}: the refusal must name the resource type, got {refused}"
        );
        let expected_id = match &row.key_scope {
            Some(scope) => format!("{scope}/{}", scalar(row, &produced)),
            None => scalar(row, &produced),
        };

        assert_eq!(
            refused["ResourceId"],
            json!(expected_id),
            "case {case:?}: the refusal must name {expected_id:?}, got {refused}"
        );
        let reported = scalar(row, &produced);
        let expected_message = format!(
            "client token {token:?} already {}, with a different request",
            (row.effect)(&reported)
        );
        assert_eq!(
            message_of(&refused),
            expected_message,
            "case {case:?}: expected the refusal to read {expected_message:?}, got {refused}"
        );

        // The refusal stored nothing, so the token still answers the record it first
        // produced.
        let (status, again) = emulator.post(&row.uri, request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the token after the refusal expected 200, got {status} with {again}"
        );
        assert_eq!(
            identity(row, &again),
            produced,
            "case {case:?}: the refusal must have stored nothing, got {again}"
        );
    }
}

/// Drive every row's create, a retry of it and the changed request under `token`, answering
/// the `(case, status, error code)` of each call — an empty error code for a success.
async fn sequence(
    emulator: &Emulator,
    rows: &[TokenRow],
    token: Option<&str>,
) -> Vec<(String, u16, String)> {
    let mut answers = Vec::new();
    for row in rows {
        for (step, request) in [
            ("the create", &row.request),
            ("the identical retry", &row.request),
            ("the changed request", &row.different),
        ] {
            let (status, body) = emulator.post(&row.uri, with_token(request, token)).await;
            let code = if (200..300).contains(&status) {
                String::new()
            } else {
                error_code(&body).to_string()
            };
            if token == Some("") {
                let message = message_of(&body);
                assert!(
                    !message.contains("client token"),
                    "case {:?} / {step}: an empty token is no token, so no answer may refuse one, got {message:?}",
                    row.name
                );
            }
            answers.push((format!("{} / {step}", row.name), status, code));
        }
    }
    answers
}

#[tokio::test]
async fn an_empty_client_token_answers_exactly_what_no_client_token_answers() {
    let empty_emulator = Emulator::new();
    let empty_fixture = fixture(&empty_emulator, "empty-token").await;
    let empty = sequence(&empty_emulator, &rows(&empty_fixture), Some("")).await;

    let absent_emulator = Emulator::new();
    let absent_fixture = fixture(&absent_emulator, "absent-token").await;
    let absent = sequence(&absent_emulator, &rows(&absent_fixture), None).await;

    assert_eq!(
        empty, absent,
        "an empty ClientToken must answer exactly what no ClientToken member answers"
    );
}
