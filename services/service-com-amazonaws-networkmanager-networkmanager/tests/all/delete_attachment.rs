// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `DeleteAttachment`, driven over HTTP through the real router (the
//! harness lives in `crate::global_network_lifecycle`).
//!
//! The rule under test: `DeleteAttachment` removes the stored attachment its id names —
//! whatever kind it is and whatever lifecycle state it is in, since the operation "supports
//! all attachment types" (`smithy_query query="doc" shape="DeleteAttachment"`) — answers
//! that removed record with `State` `DELETING` (an `AttachmentState` value, `smithy_query
//! shape=AttachmentState`) and the rest of its members as stored, and the removal is in the
//! shared store, so every other operation that reads attachments stops seeing it. An
//! attachment another attachment is built on — the transport a Connect attachment is
//! carried over — still has a dependent, so the delete is refused with the dependency
//! refusal this service answers for every parent that still owns children
//! (`ValidationException`, HTTP 400) and nothing is removed. An id that names no stored
//! attachment is `ResourceNotFoundException` (HTTP 404).
//!
//! The tables below span that class: rows differing in attachment kind (VPC, Connect,
//! site-to-site VPN, Direct Connect gateway), in Region and so in edge location, in the lifecycle state the
//! attachment is in when it is deleted (`PENDING_ATTACHMENT_ACCEPTANCE`, `CREATING`,
//! `AVAILABLE`), in whether it carries tags, and — for the refusal — in how many dependents
//! ride on the attachment being deleted. Nothing reaches into `crate::state`: every
//! attachment is created, read back and deleted through an operation.

use serde_json::{json, Value};

use crate::attachment_acceptance::{accept, core_network_with};
use crate::attachment_lifecycle::{
    attachment_id, core_network_at, create_connect_attachment, create_vpc_attachment, subnet_arn,
    vpc_arn,
};
use crate::direct_connect_gateway_attachment_lifecycle::gateway_arn;
use crate::global_network_lifecycle::{error_code, Emulator, PAST_SETTLE};
use crate::site_to_site_vpn_attachment_lifecycle::vpn_arn;

/// The ARN resource type an attachment refusal reports in `ResourceType`, which is also the
/// id prefix the model constrains (`^attachment-([0-9a-f]{8,17})$`, `smithy_query
/// shape=AttachmentId`).
const ATTACHMENT: &str = "attachment";

/// The message this service composes for a delete still blocked by a dependent: the parent
/// kind read as a title, then the dependent kind read as words — the same wording
/// `DeleteGlobalNetwork` answers, with the attachment as both the parent and the blocking
/// kind. The message rule makes this an EXAMPLE, not a requirement; it is asserted exactly
/// so the refusal cannot quietly start naming a different resource.
const DEPENDENCY_REFUSAL_MESSAGE: &str =
    "Attachment cannot be deleted due to an existing attachment associated with it.";

/// The four `AttachmentState` values this suite names (`smithy_query
/// shape=AttachmentState`).
const PENDING_ACCEPTANCE: &str = "PENDING_ATTACHMENT_ACCEPTANCE";
/// See [`PENDING_ACCEPTANCE`].
const CREATING: &str = "CREATING";
/// See [`PENDING_ACCEPTANCE`].
const AVAILABLE: &str = "AVAILABLE";
/// See [`PENDING_ACCEPTANCE`].
const DELETING: &str = "DELETING";

/// Which kind of attachment a row deletes: the create it goes through, the reader that must
/// stop finding it, and the `AttachmentType` the removed record reports.
///
/// The four kinds are the ones this service mints, each an `AttachmentType` value
/// (`smithy_query shape=AttachmentType`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Vpc,
    Connect,
    SiteToSiteVpn,
    DirectConnectGateway,
}

impl Kind {
    /// The `AttachmentType` an attachment of this kind reports.
    pub(crate) fn attachment_type(self) -> &'static str {
        match self {
            Kind::Vpc => "VPC",
            Kind::Connect => "CONNECT",
            Kind::SiteToSiteVpn => "SITE_TO_SITE_VPN",
            Kind::DirectConnectGateway => "DIRECT_CONNECT_GATEWAY",
        }
    }

    /// The `Get<Kind>Attachment` URI of `id`, the reader of this kind.
    pub(crate) fn get_uri(self, id: &str) -> String {
        match self {
            Kind::Vpc => format!("/vpc-attachments/{id}"),
            Kind::Connect => format!("/connect-attachments/{id}"),
            Kind::SiteToSiteVpn => format!("/site-to-site-vpn-attachments/{id}"),
            Kind::DirectConnectGateway => format!("/direct-connect-gateway-attachments/{id}"),
        }
    }

    /// The member of the `Get<Kind>Attachment` answer that wraps the shared `Attachment`
    /// envelope.
    pub(crate) fn wrapper(self) -> &'static str {
        match self {
            Kind::Vpc => "VpcAttachment",
            Kind::Connect => "ConnectAttachment",
            Kind::SiteToSiteVpn => "SiteToSiteVpnAttachment",
            Kind::DirectConnectGateway => "DirectConnectGatewayAttachment",
        }
    }
}

/// The `attachment-policies` of a policy that holds every new attachment for an acceptance:
/// one rule claiming every attachment (`any`) whose action asks for one. The key names are
/// the published Cloud WAN core network policy schema's, the same source
/// `crate::attachment_acceptance` builds its documents from.
pub(crate) fn requires_acceptance() -> Value {
    json!([{
        "rule-number": 100,
        "conditions": [{ "type": "any" }],
        "action": {
            "association-method": "constant",
            "segment": "shared",
            "require-attachment-acceptance": true,
        },
    }])
}

/// `DeleteAttachment` (`DELETE /attachments/{AttachmentId}`).
async fn delete_attachment(emulator: &Emulator, id: &str) -> (u16, Value) {
    emulator.delete(&format!("/attachments/{id}")).await
}

/// One row of the delete table.
struct DeleteCase {
    /// Names the case in every assertion message.
    name: &'static str,
    kind: Kind,
    /// The Region of the core network's single edge location, and so of the attachment.
    region: &'static str,
    /// Whether the core network's policy holds every new attachment for an acceptance, which
    /// is what puts the attachment in [`PENDING_ACCEPTANCE`].
    require_acceptance: bool,
    /// Whether to let the settle span the clock owes elapse before the delete, which is what
    /// moves a started attachment from [`CREATING`] to [`AVAILABLE`].
    settle: bool,
    /// The `Tags` the create carries (`Value::Null` for no `Tags` member) and the list the
    /// removed record must then report.
    tags: Value,
    expected_tags: Value,
    /// The state the attachment must be in when the delete is issued.
    state_before: &'static str,
}

/// The table: rows differ along the attachment kind, the Region (and so the edge location),
/// the state the attachment is in when it is deleted, and whether it carries tags —
/// everything the rule generalises over.
fn delete_cases() -> Vec<DeleteCase> {
    vec![
        DeleteCase {
            name: "a VPC attachment deleted while still CREATING",
            kind: Kind::Vpc,
            region: "us-west-2",
            require_acceptance: false,
            settle: false,
            tags: Value::Null,
            expected_tags: json!([]),
            state_before: CREATING,
        },
        DeleteCase {
            name: "a VPC attachment deleted once AVAILABLE, with tags",
            kind: Kind::Vpc,
            region: "eu-central-1",
            require_acceptance: false,
            settle: true,
            tags: json!([{ "Key": "env", "Value": "prod" }, { "Key": "team", "Value": "net" }]),
            expected_tags: json!([
                { "Key": "env", "Value": "prod" },
                { "Key": "team", "Value": "net" },
            ]),
            state_before: AVAILABLE,
        },
        DeleteCase {
            name: "a VPC attachment deleted while waiting for its acceptance",
            kind: Kind::Vpc,
            region: "ap-south-1",
            require_acceptance: true,
            settle: false,
            tags: json!([{ "Key": "only", "Value": "" }]),
            expected_tags: json!([{ "Key": "only", "Value": "" }]),
            state_before: PENDING_ACCEPTANCE,
        },
        DeleteCase {
            name: "a Connect attachment deleted while still CREATING",
            kind: Kind::Connect,
            region: "us-east-1",
            require_acceptance: false,
            settle: false,
            tags: Value::Null,
            expected_tags: json!([]),
            state_before: CREATING,
        },
        DeleteCase {
            name: "a Connect attachment deleted while waiting for its acceptance",
            kind: Kind::Connect,
            region: "sa-east-1",
            require_acceptance: true,
            settle: false,
            tags: json!([{ "Key": "tier", "Value": "edge" }]),
            expected_tags: json!([{ "Key": "tier", "Value": "edge" }]),
            state_before: PENDING_ACCEPTANCE,
        },
        DeleteCase {
            name: "a site-to-site VPN attachment deleted once AVAILABLE",
            kind: Kind::SiteToSiteVpn,
            region: "eu-west-1",
            require_acceptance: false,
            settle: true,
            tags: json!([{ "Key": "env", "Value": "test" }]),
            expected_tags: json!([{ "Key": "env", "Value": "test" }]),
            state_before: AVAILABLE,
        },
        DeleteCase {
            name: "a site-to-site VPN attachment deleted while still CREATING",
            kind: Kind::SiteToSiteVpn,
            region: "ca-central-1",
            require_acceptance: false,
            settle: false,
            tags: Value::Null,
            expected_tags: json!([]),
            state_before: CREATING,
        },
        DeleteCase {
            name: "a Direct Connect gateway attachment deleted once AVAILABLE",
            kind: Kind::DirectConnectGateway,
            region: "ap-southeast-4",
            require_acceptance: false,
            settle: true,
            tags: json!([{ "Key": "env", "Value": "prod" }]),
            expected_tags: json!([{ "Key": "env", "Value": "prod" }]),
            state_before: AVAILABLE,
        },
        DeleteCase {
            name: "a Direct Connect gateway attachment deleted while waiting for its acceptance",
            kind: Kind::DirectConnectGateway,
            region: "eu-west-2",
            require_acceptance: true,
            settle: false,
            tags: Value::Null,
            expected_tags: json!([]),
            state_before: PENDING_ACCEPTANCE,
        },
    ]
}

/// Create the core network a row needs, answering its id.
async fn core_network_for(emulator: &Emulator, case: &DeleteCase) -> String {
    if case.require_acceptance {
        core_network_with(emulator, case.name, case.region, &requires_acceptance()).await
    } else {
        core_network_at(emulator, case.name, &[case.region]).await.1
    }
}

/// Add `tags` to a create body, where the row carries any.
fn with_tags(mut body: Value, tags: &Value) -> Value {
    if !tags.is_null() {
        body["Tags"] = tags.clone();
    }
    body
}

/// Create the attachment a row deletes, answering `(its id, the ids that must be removed
/// before the core network can be deleted)`.
async fn create_for(emulator: &Emulator, index: usize, case: &DeleteCase) -> (String, Vec<String>) {
    let core_network = core_network_for(emulator, case).await;
    create_attachment_of_kind(
        emulator,
        case.name,
        case.kind,
        &core_network,
        case.region,
        index,
        &case.tags,
    )
    .await
}

/// Create one attachment of `kind` on `core_network`, at edge location `region`, carrying
/// `tags` (`Value::Null` for a create with no `Tags` member). `index` distinguishes the
/// resource it attaches from every other row's, so one emulator can hold several.
///
/// Answers `(its id, the ids that must be removed before the core network can be deleted)`.
/// A Connect attachment needs a VPC attachment of the same core network as its transport, so
/// a Connect row leaves that transport behind: it is the second entry of that list. Shared
/// with the suites that need an attachment of a chosen kind (`crate::attachment_rejection`).
pub(crate) async fn create_attachment_of_kind(
    emulator: &Emulator,
    name: &str,
    kind: Kind,
    core_network: &str,
    region: &str,
    index: usize,
    tags: &Value,
) -> (String, Vec<String>) {
    match kind {
        Kind::Vpc => {
            let body = with_tags(
                json!({
                    "CoreNetworkId": core_network,
                    "VpcArn": vpc_arn(region, index + 1),
                    "SubnetArns": [subnet_arn(region, index + 1)],
                }),
                tags,
            );
            let (status, created) = create_vpc_attachment(emulator, body).await;
            assert_eq!(
                status, 200,
                "case {name:?}: CreateVpcAttachment expected 200, got {status} with {created}"
            );
            let id = attachment_id(name, &created["VpcAttachment"]);
            (id.clone(), vec![id])
        }
        Kind::Connect => {
            let (status, transport) = create_vpc_attachment(
                emulator,
                json!({
                    "CoreNetworkId": core_network,
                    "VpcArn": vpc_arn(region, index + 1),
                    "SubnetArns": [subnet_arn(region, index + 1)],
                }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {name:?}: the transport create expected 200, got {status} with {transport}"
            );
            let transport_state = transport["VpcAttachment"]["Attachment"]["State"].clone();
            let transport = attachment_id(name, &transport["VpcAttachment"]);
            // A Connect attachment is built on an attachment that EXISTS, and a transport
            // the core network policy held for an acceptance is not one until that
            // acceptance is granted, so a policy that claimed the transport too is answered
            // here: the transport is accepted first, which is what starts it.
            if transport_state == json!(PENDING_ACCEPTANCE) {
                let (status, accepted) = accept(emulator, &transport).await;
                assert_eq!(
                    status, 200,
                    "case {name:?}: AcceptAttachment on the transport expected 200, got {status} with {accepted}"
                );
            }
            let body = with_tags(
                json!({
                    "CoreNetworkId": core_network,
                    "EdgeLocation": region,
                    "TransportAttachmentId": transport,
                    "Options": { "Protocol": "GRE" },
                }),
                tags,
            );
            let (status, created) = create_connect_attachment(emulator, body).await;
            assert_eq!(
                status, 200,
                "case {name:?}: CreateConnectAttachment expected 200, got {status} with {created}"
            );
            let id = attachment_id(name, &created["ConnectAttachment"]);
            (id.clone(), vec![id, transport])
        }
        Kind::SiteToSiteVpn => {
            let body = with_tags(
                json!({
                    "CoreNetworkId": core_network,
                    "VpnConnectionArn": vpn_arn(
                        "aws",
                        region,
                        engine::emulator_account_id(),
                        &format!("{:08x}", index + 1),
                    ),
                }),
                tags,
            );
            let (status, created) = emulator.post("/site-to-site-vpn-attachments", body).await;
            assert_eq!(
                status, 200,
                "case {name:?}: CreateSiteToSiteVpnAttachment expected 200, got {status} with {created}"
            );
            let id = attachment_id(name, &created["SiteToSiteVpnAttachment"]);
            (id.clone(), vec![id])
        }
        Kind::DirectConnectGateway => {
            // A Direct Connect gateway attachment spans a LIST of edge locations rather
            // than sitting at one, so the row's Region is its single-entry list.
            let body = with_tags(
                json!({
                    "CoreNetworkId": core_network,
                    "DirectConnectGatewayArn": gateway_arn(index + 1),
                    "EdgeLocations": [region],
                }),
                tags,
            );
            let (status, created) = emulator
                .post("/direct-connect-gateway-attachments", body)
                .await;
            assert_eq!(
                status, 200,
                "case {name:?}: CreateDirectConnectGatewayAttachment expected 200, got {status} with {created}"
            );
            let id = attachment_id(name, &created["DirectConnectGatewayAttachment"]);
            (id.clone(), vec![id])
        }
    }
}

#[tokio::test]
async fn delete_attachment_removes_the_record_of_any_kind_and_reports_it_deleting() {
    for (index, case) in delete_cases().iter().enumerate() {
        let emulator = Emulator::new();
        let name = case.name;
        let (id, teardown) = create_for(&emulator, index, case).await;
        if case.settle {
            tokio::time::sleep(PAST_SETTLE).await;
        }

        // The state the row says the attachment is in, read back through its own reader, so
        // the delete below is issued against a known state rather than an assumed one.
        let (status, read) = emulator.get(&case.kind.get_uri(&id)).await;
        assert_eq!(
            status, 200,
            "case {name:?}: the reader of this kind expected 200 before the delete, got {status} with {read}"
        );
        let before = &read[case.kind.wrapper()]["Attachment"];
        assert_eq!(
            before["State"],
            json!(case.state_before),
            "case {name:?}: expected the attachment to be {} before the delete, got {before}",
            case.state_before
        );

        let (status, deleted) = delete_attachment(&emulator, &id).await;
        assert_eq!(
            status, 200,
            "case {name:?}: DeleteAttachment expected 200, got {status} with {deleted}"
        );
        let answered = &deleted["Attachment"];
        assert_eq!(
            answered["AttachmentId"],
            json!(id),
            "case {name:?}: DeleteAttachment must answer the record it removed, got {answered}"
        );
        assert_eq!(
            answered["State"],
            json!(DELETING),
            "case {name:?}: expected the removed record to report {DELETING}, got {answered}"
        );
        assert_eq!(
            answered["AttachmentType"],
            json!(case.kind.attachment_type()),
            "case {name:?}: expected AttachmentType {}, got {answered}",
            case.kind.attachment_type()
        );
        assert_eq!(
            answered["CoreNetworkId"], before["CoreNetworkId"],
            "case {name:?}: the removed record keeps the core network it belonged to, got {answered}"
        );
        // Exactly ONE of the two edge members is reported, and which one is a function of
        // the kind: a Direct Connect gateway attachment spans a list of edges and reports
        // `EdgeLocations`, every other kind sits at one and reports `EdgeLocation`.
        let (expected_edge, expected_edges) = match case.kind {
            Kind::DirectConnectGateway => (Value::Null, json!([case.region])),
            Kind::Vpc | Kind::Connect | Kind::SiteToSiteVpn => (json!(case.region), Value::Null),
        };
        assert_eq!(
            answered["EdgeLocation"], expected_edge,
            "case {name:?}: expected EdgeLocation {expected_edge}, got {answered}"
        );
        assert_eq!(
            answered["EdgeLocations"], expected_edges,
            "case {name:?}: expected EdgeLocations {expected_edges}, got {answered}"
        );
        assert_eq!(
            answered["Tags"], case.expected_tags,
            "case {name:?}: expected the tags the create carried, got {answered}"
        );
        assert_eq!(
            answered["ResourceArn"], before["ResourceArn"],
            "case {name:?}: the removed record keeps the resource it attached, got {answered}"
        );

        // The removal is in the shared store: the reader of this kind no longer finds it,
        // and neither does a second delete.
        let (status, read) = emulator.get(&case.kind.get_uri(&id)).await;
        assert_eq!(
            status, 404,
            "case {name:?}: the reader after the delete expected 404, got {status} with {read}"
        );
        assert_eq!(
            error_code(&read),
            "ResourceNotFoundException",
            "case {name:?}: expected ResourceNotFoundException from the reader, got {read}"
        );
        let (status, again) = delete_attachment(&emulator, &id).await;
        assert_eq!(
            status, 404,
            "case {name:?}: a second DeleteAttachment expected 404, got {status} with {again}"
        );
        assert_eq!(
            error_code(&again),
            "ResourceNotFoundException",
            "case {name:?}: expected ResourceNotFoundException, got {again}"
        );

        // A third operation reads the same store: `DeleteCoreNetwork` is refused while the
        // core network still holds an attachment, so it succeeding once every attachment of
        // this row is gone is the count dropping to zero, not just this reader's answer.
        let core_network = before["CoreNetworkId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {name:?}: no CoreNetworkId in {before}"))
            .to_string();
        for remaining in teardown.iter().skip(1) {
            let (status, removed) = delete_attachment(&emulator, remaining).await;
            assert_eq!(
                status, 200,
                "case {name:?}: deleting the leftover {remaining} expected 200, got {status} with {removed}"
            );
        }
        let (status, removed) = emulator.delete(&format!("/core-networks/{core_network}")).await;
        assert_eq!(
            status, 200,
            "case {name:?}: with every attachment gone DeleteCoreNetwork must succeed, got {status} with {removed}"
        );
    }
}

/// One row of the transport-dependency table: how many Connect attachments ride on the VPC
/// attachment the delete names.
struct TransportCase {
    name: &'static str,
    carried: usize,
}

/// The rows differ in how many dependents the attachment still has, including the control
/// row that has none.
fn transport_cases() -> Vec<TransportCase> {
    vec![
        TransportCase {
            name: "no Connect attachment rides on it",
            carried: 0,
        },
        TransportCase {
            name: "one Connect attachment rides on it",
            carried: 1,
        },
        TransportCase {
            name: "three Connect attachments ride on it",
            carried: 3,
        },
    ]
}

#[tokio::test]
async fn deleting_the_transport_of_a_connect_attachment_is_refused_until_it_is_gone() {
    for case in transport_cases() {
        let emulator = Emulator::new();
        let name = case.name;
        let (_, core_network) = core_network_at(&emulator, name, &["us-west-2"]).await;
        let (status, transport) = create_vpc_attachment(
            &emulator,
            json!({
                "CoreNetworkId": core_network,
                "VpcArn": vpc_arn("us-west-2", 1),
                "SubnetArns": [subnet_arn("us-west-2", 1)],
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?}: the transport create expected 200, got {status} with {transport}"
        );
        let transport = attachment_id(name, &transport["VpcAttachment"]);

        let mut carried: Vec<String> = Vec::new();
        for index in 0..case.carried {
            let (status, connect) = create_connect_attachment(
                &emulator,
                json!({
                    "CoreNetworkId": core_network,
                    "EdgeLocation": "us-west-2",
                    "TransportAttachmentId": transport,
                    "Options": { "Protocol": if index % 2 == 0 { "GRE" } else { "NO_ENCAP" } },
                }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {name:?}: CreateConnectAttachment expected 200, got {status} with {connect}"
            );
            carried.push(attachment_id(name, &connect["ConnectAttachment"]));
        }

        let (status, body) = delete_attachment(&emulator, &transport).await;

        if case.carried == 0 {
            assert_eq!(
                status, 200,
                "case {name:?}: an attachment nothing rides on deletes, got {status} with {body}"
            );
            assert_eq!(
                body["Attachment"]["State"],
                json!(DELETING),
                "case {name:?}: the removed record reports {DELETING}, got {body}"
            );
            continue;
        }

        assert_eq!(
            status, 400,
            "case {name:?}: a dependency refusal is HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {name:?}: expected ValidationException, got {body}"
        );
        assert_eq!(
            body["Reason"],
            json!("Other"),
            "case {name:?}: no input field is at fault, so the reason is the modelled Other, got {body}"
        );
        assert_eq!(
            body["Fields"],
            Value::Null,
            "case {name:?}: a dependency refusal blames no field, got {body}"
        );
        let message = body["Message"]
            .as_str()
            .unwrap_or_else(|| panic!("case {name:?}: no Message in {body}"));
        assert_eq!(
            message, DEPENDENCY_REFUSAL_MESSAGE,
            "case {name:?}: expected the dependency refusal message this service composes, got {message:?}"
        );

        // The refusal changed nothing: the transport and every attachment riding on it are
        // still readable.
        let (status, still) = emulator.get(&format!("/vpc-attachments/{transport}")).await;
        assert_eq!(
            status, 200,
            "case {name:?}: a refused delete must leave the transport stored, got {status} with {still}"
        );
        for connect in &carried {
            let (status, still) = emulator.get(&format!("/connect-attachments/{connect}")).await;
            assert_eq!(
                status, 200,
                "case {name:?}: a refused delete must leave {connect} alone, got {status} with {still}"
            );
        }

        // Removing the dependents one at a time: the refusal stands while any remains and
        // the identical call succeeds once the last is gone, so it tracked stored state.
        for (removed, connect) in carried.iter().enumerate() {
            let (status, gone) = delete_attachment(&emulator, connect).await;
            assert_eq!(
                status, 200,
                "case {name:?}: deleting {connect} expected 200, got {status} with {gone}"
            );
            let (status, retried) = delete_attachment(&emulator, &transport).await;
            if removed + 1 == carried.len() {
                assert_eq!(
                    status, 200,
                    "case {name:?}: with the last dependent gone the same delete must succeed, got {status} with {retried}"
                );
                assert_eq!(
                    retried["Attachment"]["AttachmentId"],
                    json!(transport),
                    "case {name:?}: the delete answers the transport it removed, got {retried}"
                );
            } else {
                assert_eq!(
                    status, 400,
                    "case {name:?}: {} dependents still block the delete, got {status} with {retried}",
                    carried.len() - removed - 1
                );
            }
        }
    }
}

#[tokio::test]
async fn deleting_an_attachment_id_that_names_nothing_is_the_services_not_found_refusal() {
    let emulator = Emulator::new();

    // Ids this service never minted, differing in length across the span the model's own
    // `^attachment-([0-9a-f]{8,17})$` allows (`smithy_query shape=AttachmentId`): the 8-hex
    // lower bound, the 17-hex upper bound, and one in between.
    let unknown: Vec<(&str, String)> = vec![
        ("the 8-hex lower bound of the pattern", "attachment-0123abcd".to_string()),
        ("a 12-hex id inside the pattern", "attachment-0123456789ab".to_string()),
        (
            "the 17-hex upper bound of the pattern",
            "attachment-0123456789abcdef0".to_string(),
        ),
    ];

    for (case, id) in unknown {
        let (status, body) = delete_attachment(&emulator, &id).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404 for an id naming nothing, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {body}"
        );
        assert_eq!(
            body["ResourceId"],
            json!(id),
            "case {case:?}: the refusal names the id asked for, got {body}"
        );
        assert_eq!(
            body["ResourceType"],
            json!(ATTACHMENT),
            "case {case:?}: the refusal names the resource type {ATTACHMENT}, got {body}"
        );
    }

    // An id outside the modelled pattern never reaches the handler: the constraint walk
    // refuses it first, with the modelled `ValidationException` (HTTP 400).
    let (status, refused) = delete_attachment(&emulator, "attachment-zzzz").await;
    assert_eq!(
        status, 400,
        "an id outside the modelled pattern is HTTP 400, got {status} with {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "ValidationException",
        "expected ValidationException, got {refused}"
    );
}
