// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `GetDirectConnectGatewayAttachment` as a REPORTING operation,
//! driven over HTTP through the real router (the harness lives in
//! `crate::global_network_lifecycle`, the core-network helpers in
//! `crate::attachment_lifecycle` and `crate::attachment_acceptance`).
//!
//! The rule under test: `GetDirectConnectGatewayAttachment` answers the ONE stored
//! attachment of its own kind that its `AttachmentId` names — the shared `Attachment`
//! envelope as of the clock plus the Direct Connect gateway the record attaches — and
//! writes nothing, so the state it reports is the one the create, an acceptance and the
//! clock put the record in, and two reads in a row answer the same thing. An id that names
//! no stored `DIRECT_CONNECT_GATEWAY` attachment — never minted, naming an attachment of
//! another kind in the one id space the attachment readers share, or naming one that has
//! been removed — is the modelled `ResourceNotFoundException` (HTTP 404) naming the
//! resource type and the id asked for.
//!
//! The tables below span that class: refusals differing in HOW the id fails to name a
//! Direct Connect gateway attachment (never minted at each width the modelled pattern
//! allows, a counter value another resource kind of this service was minted under spelled
//! with the attachment prefix, the ids of the three other attachment kinds this core
//! network holds, and the id of a removed attachment); and a read followed through every
//! state a record of this kind passes through (`PENDING_ATTACHMENT_ACCEPTANCE`, `CREATING`,
//! `AVAILABLE`), each read asserted to leave the next caller's answer alone. The spanned
//! `EdgeLocations` and the attached gateway are asserted to survive every transition.
//! Nothing reaches into `crate::state`: every attachment is created, accepted, read back
//! and deleted through an operation.

use serde_json::{json, Value};

use crate::attachment_acceptance::{accept, core_network_with};
use crate::attachment_lifecycle::{
    attachment_id, core_network_at, create_connect_attachment, create_vpc_attachment, subnet_arn,
    vpc_arn,
};
use crate::direct_connect_gateway_attachment_lifecycle::gateway_arn;
use crate::global_network_lifecycle::{error_code, Emulator, PAST_SETTLE};
use crate::site_to_site_vpn_attachment_lifecycle::vpn_arn;

/// The ARN resource type (and id prefix) of an attachment: `^attachment-([0-9a-f]{8,17})$`
/// on `AttachmentId` (`smithy_query shape=AttachmentId`), which is also the `ResourceType`
/// a not-found refusal of an attachment names.
const ATTACHMENT: &str = "attachment";

/// The `AttachmentType` this reader's kind reports, one of the values the enum carries
/// (`smithy_query shape=AttachmentType`).
const DIRECT_CONNECT_GATEWAY: &str = "DIRECT_CONNECT_GATEWAY";

/// The `AttachmentState` values this suite observes (`smithy_query shape=AttachmentState`).
const PENDING_ACCEPTANCE: &str = "PENDING_ATTACHMENT_ACCEPTANCE";
/// See [`PENDING_ACCEPTANCE`].
const CREATING: &str = "CREATING";
/// See [`PENDING_ACCEPTANCE`].
const AVAILABLE: &str = "AVAILABLE";

/// The message every `ResourceNotFoundException` of this service carries: the model
/// declares the member required but states no text, and the identifying detail travels in
/// `ResourceId` / `ResourceType` instead (the shared refusal `crate::resource_not_found_message`
/// pins across operations).
const NOT_FOUND_MESSAGE: &str = "Resource not found.";

/// The `CreateDirectConnectGatewayAttachment` route
/// (`POST /direct-connect-gateway-attachments`).
const CREATE_URI: &str = "/direct-connect-gateway-attachments";

/// The `GetDirectConnectGatewayAttachment` URI of `attachment_id`
/// (`GET /direct-connect-gateway-attachments/{AttachmentId}`,
/// `smithy_query operation=GetDirectConnectGatewayAttachment`).
fn get_uri(attachment_id: &str) -> String {
    format!("{CREATE_URI}/{attachment_id}")
}

/// `GetDirectConnectGatewayAttachment`.
async fn read_gateway(emulator: &Emulator, attachment_id: &str) -> (u16, Value) {
    emulator.get(&get_uri(attachment_id)).await
}

/// `CreateDirectConnectGatewayAttachment` of `gateway` on `core_network_id` spanning
/// `edge_locations`, and the shape it answered.
async fn create_gateway_attachment(
    emulator: &Emulator,
    case: &str,
    core_network_id: &str,
    gateway: &str,
    edge_locations: &[&str],
) -> Value {
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
    created["DirectConnectGatewayAttachment"].clone()
}

/// `DeleteAttachment` (`DELETE /attachments/{AttachmentId}`), the one operation that
/// removes an attachment of any kind.
async fn delete_attachment(emulator: &Emulator, attachment_id: &str) -> (u16, Value) {
    emulator
        .delete(&format!("/attachments/{attachment_id}"))
        .await
}

/// How many attachments the global network `global_network_id` holds, read through
/// `GetNetworkResourceCounts` — the one way a client can see that a refused read stored
/// nothing.
async fn attachment_count(emulator: &Emulator, global_network_id: &str) -> Value {
    let (status, counts) = emulator
        .get(&format!(
            "/global-networks/{global_network_id}/network-resource-count?resourceType=attachment"
        ))
        .await;
    assert_eq!(
        status, 200,
        "GetNetworkResourceCounts expected 200, got {status} with {counts}"
    );
    // Pick the entry by its `ResourceType` rather than by position, so the assertion names
    // the count it means even if the list ever carries more than one entry.
    counts["NetworkResourceCounts"]
        .as_array()
        .unwrap_or_else(|| panic!("a count list is an array, got {counts}"))
        .iter()
        .find(|entry| entry["ResourceType"] == json!(ATTACHMENT))
        .unwrap_or_else(|| panic!("the counts must carry an {ATTACHMENT} entry, got {counts}"))
        .get("Count")
        .cloned()
        .unwrap_or(Value::Null)
}

/// Assert the reader refused `asked_for` as the modelled not-found, naming that id and the
/// attachment resource type.
fn assert_not_found(case: &str, asked_for: &str, status: u16, body: &Value) {
    assert_eq!(
        status, 404,
        "case {case} ({asked_for:?}): expected 404, got {status} with {body}"
    );
    assert_eq!(
        error_code(body),
        "ResourceNotFoundException",
        "case {case} ({asked_for:?}): wrong error code in {body}"
    );
    assert_eq!(
        body["Message"],
        json!(NOT_FOUND_MESSAGE),
        "case {case} ({asked_for:?}): wrong message in {body}"
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

#[tokio::test]
async fn an_id_that_names_no_stored_direct_connect_gateway_attachment_is_the_modelled_not_found() {
    let emulator = Emulator::new();
    let region = "us-east-1";
    let (global_network, core_network) =
        core_network_at(&emulator, "gateway reader refusals", &[region]).await;

    // One attachment of every other kind this service mints in the SAME core network, so
    // each of their ids is a live id of the one id space the attachment readers share.
    let (status, created_vpc) = create_vpc_attachment(
        &emulator,
        json!({
            "CoreNetworkId": core_network,
            "VpcArn": vpc_arn(region, 1),
            "SubnetArns": [subnet_arn(region, 11)],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateVpcAttachment expected 200, got {status} with {created_vpc}"
    );
    let vpc = attachment_id("the VPC attachment", &created_vpc["VpcAttachment"]);

    let (status, created_connect) = create_connect_attachment(
        &emulator,
        json!({
            "CoreNetworkId": core_network,
            "EdgeLocation": region,
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

    let (status, created_vpn) = emulator
        .post(
            "/site-to-site-vpn-attachments",
            json!({
                "CoreNetworkId": core_network,
                "VpnConnectionArn": vpn_arn(
                    "aws",
                    region,
                    engine::emulator_account_id(),
                    "0000000a",
                ),
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "CreateSiteToSiteVpnAttachment expected 200, got {status} with {created_vpn}"
    );
    let vpn = attachment_id(
        "the site-to-site VPN attachment",
        &created_vpn["SiteToSiteVpnAttachment"],
    );

    let gateway = gateway_arn(1);
    let live_shape = create_gateway_attachment(
        &emulator,
        "the live gateway attachment",
        &core_network,
        &gateway,
        &[region],
    )
    .await;
    let live = attachment_id("the live gateway attachment", &live_shape);

    // Each row is a different way of naming no stored Direct Connect gateway attachment,
    // every one satisfying the modelled `AttachmentId` pattern so it reaches the handler: a
    // never-minted id at each width the pattern allows, the counter value a DIFFERENT
    // resource kind of this service was minted under spelled with the attachment prefix, and
    // the ids of the three attachments of OTHER kinds the setup stored beside it.
    let borrowed_suffix = format!(
        "attachment-{}",
        core_network
            .strip_prefix("core-network-")
            .unwrap_or_else(|| panic!(
                "a minted core network id carries the core-network- prefix, got {core_network:?}"
            ))
    );
    for (case, asked_for) in [
        ("never minted, 17 hex digits", "attachment-00000000000000099"),
        ("never minted, the 8-hex lower bound", "attachment-0000beef"),
        ("another counter value under this prefix", &borrowed_suffix),
        ("a VPC attachment's id", &vpc),
        ("a Connect attachment's id", &connect),
        ("a site-to-site VPN attachment's id", &vpn),
    ] {
        let (status, body) = read_gateway(&emulator, asked_for).await;
        assert_not_found(case, asked_for, status, &body);
    }

    // The refusals are the lookup and nothing wider: the stored gateway attachment still
    // reads, reporting the gateway it attaches, and nothing was written — the global network
    // still holds exactly the four attachments the setup created in it.
    let (status, read) = read_gateway(&emulator, &live).await;
    assert_eq!(
        status, 200,
        "GetDirectConnectGatewayAttachment on the live attachment expected 200, got {status} with {read}"
    );
    assert_eq!(
        read["DirectConnectGatewayAttachment"]["DirectConnectGatewayArn"],
        json!(gateway),
        "the reader must report the attached Direct Connect gateway, got {read}"
    );
    assert_eq!(
        read["DirectConnectGatewayAttachment"]["Attachment"]["AttachmentType"],
        json!(DIRECT_CONNECT_GATEWAY),
        "the reader must answer an attachment of its own kind, got {read}"
    );
    let count = attachment_count(&emulator, &global_network).await;
    assert_eq!(
        count,
        json!(4),
        "the reads stored nothing, so the global network still holds its four attachments, got {count}"
    );

    // An attachment that has been REMOVED names no stored attachment either, so the reader
    // refuses its id exactly as it refuses one never minted.
    let (status, deleted) = delete_attachment(&emulator, &live).await;
    assert_eq!(
        status, 200,
        "DeleteAttachment expected 200, got {status} with {deleted}"
    );
    let (status, body) = read_gateway(&emulator, &live).await;
    assert_not_found("a removed attachment's id", &live, status, &body);
    let count = attachment_count(&emulator, &global_network).await;
    assert_eq!(
        count,
        json!(3),
        "the delete removed one of the four attachments, got {count}"
    );
}

#[tokio::test]
async fn the_reader_reports_the_state_the_record_and_the_clock_put_the_attachment_in() {
    let emulator = Emulator::new();
    let region = "eu-central-1";
    // A policy whose one `attachment-policies` rule claims every attachment and asks for the
    // attachment request to be accepted, so the created attachment waits in
    // PENDING_ATTACHMENT_ACCEPTANCE until `AcceptAttachment` starts it. The key names are
    // the published Cloud WAN core network policy schema's, the same source
    // `crate::attachment_acceptance` builds its documents from.
    let core_network = core_network_with(
        &emulator,
        "the gateway reader through every state",
        region,
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

    let gateway = gateway_arn(7);
    let created = create_gateway_attachment(
        &emulator,
        "held for acceptance",
        &core_network,
        &gateway,
        &[region],
    )
    .await;
    let id = attachment_id("held for acceptance", &created);

    // Every row is one state the record passes through, in the order it does: what the read
    // must answer, and what has to have happened before that read.
    let (status, waiting) = read_gateway(&emulator, &id).await;
    assert_eq!(
        status, 200,
        "case waiting for acceptance: expected 200, got {status} with {waiting}"
    );
    assert_eq!(
        waiting["DirectConnectGatewayAttachment"], created,
        "case waiting for acceptance: the reader must answer the stored attachment, got {waiting}"
    );
    assert_eq!(
        waiting["DirectConnectGatewayAttachment"]["Attachment"]["State"],
        json!(PENDING_ACCEPTANCE),
        "case waiting for acceptance: expected {PENDING_ACCEPTANCE}, got {waiting}"
    );

    // The clock owes an attachment waiting for an acceptance NO transition: past the span a
    // started attachment settles in, this one is still waiting, and the read that observed
    // it changed nothing.
    tokio::time::sleep(PAST_SETTLE).await;
    let (status, still_waiting) = read_gateway(&emulator, &id).await;
    assert_eq!(
        status, 200,
        "case still waiting past the settle span: expected 200, got {status} with {still_waiting}"
    );
    assert_eq!(
        still_waiting, waiting,
        "case still waiting past the settle span: only an acceptance moves it, got {waiting} then {still_waiting}"
    );

    // The acceptance starts the attachment, so the reader reports CREATING until the
    // transition the clock then owes falls due.
    let (status, accepted) = accept(&emulator, &id).await;
    assert_eq!(
        status, 200,
        "AcceptAttachment expected 200, got {status} with {accepted}"
    );
    let (status, creating) = read_gateway(&emulator, &id).await;
    assert_eq!(
        status, 200,
        "case started by the acceptance: expected 200, got {status} with {creating}"
    );
    let shape = &creating["DirectConnectGatewayAttachment"];
    assert_eq!(
        shape["Attachment"]["State"],
        json!(CREATING),
        "case started by the acceptance: expected {CREATING}, got {shape}"
    );
    let created_at = shape["Attachment"]["CreatedAt"].as_f64().unwrap_or_default();
    let updated_at = shape["Attachment"]["UpdatedAt"].as_f64().unwrap_or_default();
    assert!(
        updated_at >= created_at,
        "case started by the acceptance: the acceptance is a change, so UpdatedAt ({updated_at}) cannot precede CreatedAt ({created_at}), got {shape}"
    );
    let (_, again) = read_gateway(&emulator, &id).await;
    assert_eq!(
        again, creating,
        "case started by the acceptance: the read must not move the state, got {creating} then {again}"
    );

    // Past the settle span the transition the clock owed has fallen due, although no call
    // made it, and nothing the request supplied moved with the state.
    tokio::time::sleep(PAST_SETTLE).await;
    let (status, settled) = read_gateway(&emulator, &id).await;
    assert_eq!(
        status, 200,
        "case settled: expected 200, got {status} with {settled}"
    );
    let shape = &settled["DirectConnectGatewayAttachment"];
    assert_eq!(
        shape["Attachment"]["State"],
        json!(AVAILABLE),
        "case settled: expected {AVAILABLE}, got {shape}"
    );
    assert_eq!(
        shape["DirectConnectGatewayArn"],
        json!(gateway),
        "case settled: the attached gateway must not have moved with the state, got {shape}"
    );
    assert_eq!(
        shape["Attachment"]["EdgeLocations"],
        json!([region]),
        "case settled: the spanned edge locations must not have moved with the state, got {shape}"
    );
    for member in [
        "AttachmentId",
        "CoreNetworkId",
        "CoreNetworkArn",
        "AttachmentType",
        "OwnerAccountId",
        "ResourceArn",
        "CreatedAt",
        "Tags",
        "SegmentName",
        "AttachmentPolicyRuleNumber",
    ] {
        assert_eq!(
            shape["Attachment"][member], created["Attachment"][member],
            "case settled: {member} must not have moved with the state, got {shape}"
        );
    }

    // Once the transition has fallen due the settled state stays where it fell rather than
    // following the clock of each read.
    let (_, again) = read_gateway(&emulator, &id).await;
    assert_eq!(
        again, settled,
        "case settled: a settled attachment reads the same twice, got {settled} then {again}"
    );
}
