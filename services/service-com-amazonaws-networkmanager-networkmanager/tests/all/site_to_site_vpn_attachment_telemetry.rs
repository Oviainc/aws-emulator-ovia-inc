// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of the telemetry a site-to-site VPN attachment registers, driven
//! over HTTP through the real router (the harness lives in
//! `crate::global_network_lifecycle`, the core-network-with-edges helper in
//! `crate::attachment_lifecycle`).
//!
//! The rule under test: the call that ties a connection whose session this service measures
//! into a global network registers that session with the global network, and the call that
//! unties it deregisters it — so attaching a VPN connection to a core network (or accepting
//! the attachment request its policy held) makes `GetNetworkTelemetry` report that
//! connection for the global network owning the core network, with the IPsec session health
//! of the attachment carrying it, until the attachment is deleted. An attachment that
//! attaches no such connection registers nothing.
//!
//! Nothing here reaches into `crate::state`: every registration is made by a request and
//! read back over the wire through `GetNetworkTelemetry`.
//!
//! The table below spans that class: VPN connection ARNs differing in partition, in the
//! Region segment (and so in the edge location the session lands at, two rows sharing one
//! Region so a Region filter selects a subset), in the account that owns the connection, and
//! in the width of the connection id at each bound the modelled pattern publishes. The
//! suites after it span the lifecycle: an attachment the policy holds for an acceptance, one
//! whose request is rejected, a deleted attachment, a re-attached connection, and the other
//! attachment kinds, whose attached resource has no session to measure.

use serde_json::{json, Value};

use crate::attachment_acceptance::{accept, policy_document as acceptance_policy_document};
use crate::attachment_lifecycle::{
    core_network_at, create_connect_attachment, create_vpc_attachment, subnet_arn, vpc_arn,
};
use crate::core_network_lifecycle::create_core_network;
use crate::direct_connect_gateway_attachment_lifecycle::gateway_arn;
use crate::global_network_lifecycle::{Emulator, PAST_SETTLE};
use crate::resource_tagging::escape;
use crate::site_to_site_vpn_attachment_lifecycle::vpn_arn;

/// The `CreateSiteToSiteVpnAttachment` route (`POST /site-to-site-vpn-attachments`).
const CREATE_URI: &str = "/site-to-site-vpn-attachments";

/// The `ResourceType` a telemetry entry of a VPN connection carries: one of the three the
/// `GetNetworkTelemetry` member documents (`smithy_query query="doc"
/// shape="GetNetworkTelemetryRequest" member="ResourceType"`, mirrored by
/// `state::TELEMETRY_RESOURCE_TYPES`).
const VPN_CONNECTION: &str = "vpn-connection";
/// Another of those three, used to show a filter that selects a kind this global network
/// holds none of answers nothing.
const CONNECT_PEER: &str = "connect-peer";

/// The two `ConnectionHealth$Status` values a registered session passes through
/// (`smithy_query shape=ConnectionHealth member="Status"`: `UP`, `DOWN`).
const DOWN: &str = "DOWN";
/// See [`DOWN`].
const UP: &str = "UP";

/// The `ConnectionHealth$Type` of a VPN connection's session: the IPsec value of that enum
/// (`smithy_query shape=ConnectionHealth member="Type"`: `BGP`, `IPSEC`).
const IPSEC: &str = "IPSEC";

/// The account that owns most of the VPN connections below, written out because a telemetry
/// entry reports the account the connection's own ARN carries rather than the one the
/// emulator runs as.
const OWNER: &str = "111122223333";
/// A second owning account, for a connection another account of the organization owns.
const OTHER_OWNER: &str = "444455556666";

/// One row of the table: the VPN connection being attached and what its telemetry entry
/// must then report.
struct VpnCase {
    /// Names the case in every assertion message.
    name: &'static str,
    partition: &'static str,
    /// The Region of the connection, which is the edge location of its attachment and the
    /// `AwsRegion` its telemetry entry reports.
    region: &'static str,
    /// The account that owns the connection, which its ARN carries and its telemetry entry
    /// reports as `AccountId`.
    account: &'static str,
    /// The hex body of the connection id (`vpn-<8..17 hex>`, the bounds
    /// `^arn:[^:]{1,63}:ec2:[^:]{0,63}:[^:]{0,63}:vpn-connection\/vpn-[0-9a-f]{8,17}$`
    /// publishes, `smithy_query shape=VpnConnectionArn`).
    vpn_id: &'static str,
}

impl VpnCase {
    /// The ARN of this row's VPN connection.
    fn arn(&self) -> String {
        vpn_arn(self.partition, self.region, self.account, self.vpn_id)
    }

    /// The `ResourceId` a telemetry entry for this row must report: the connection id its
    /// ARN names.
    fn resource_id(&self) -> String {
        format!("vpn-{}", self.vpn_id)
    }
}

/// The table: rows differing along everything the rule generalises over — the partition, the
/// Region (two rows share one), the owning account and the width of the connection id at
/// each bound its pattern publishes.
fn vpn_cases() -> Vec<VpnCase> {
    vec![
        VpnCase {
            name: "the 8-hex id the pattern's lower bound allows",
            partition: "aws",
            region: "us-west-2",
            account: OWNER,
            vpn_id: "0123abcd",
        },
        VpnCase {
            name: "a second connection in the same Region, owned by another account",
            partition: "aws",
            region: "us-west-2",
            account: OTHER_OWNER,
            vpn_id: "00001111dead1",
        },
        VpnCase {
            name: "the 17-hex id at the pattern's upper bound, another Region",
            partition: "aws",
            region: "eu-west-1",
            account: OWNER,
            vpn_id: "0123456789abcdef0",
        },
        VpnCase {
            name: "another partition, a Region of that partition",
            partition: "aws-us-gov",
            region: "us-gov-west-1",
            account: OTHER_OWNER,
            vpn_id: "cafe0000beef",
        },
    ]
}

/// The `GetNetworkTelemetry` URI of `global_network`, with `query` (already escaped)
/// appended when it is not empty.
fn telemetry_uri(global_network: &str, query: &str) -> String {
    let path = format!("/global-networks/{global_network}/network-telemetry");
    if query.is_empty() {
        path
    } else {
        format!("{path}?{query}")
    }
}

/// The `NetworkTelemetry` entries `global_network` reports under `query`, or a panic naming
/// what came back instead.
async fn telemetry(
    emulator: &Emulator,
    case: &str,
    global_network: &str,
    query: &str,
) -> Vec<Value> {
    let (status, body) = emulator.get(&telemetry_uri(global_network, query)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetNetworkTelemetry expected 200, got {status} with {body}"
    );
    body["NetworkTelemetry"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no NetworkTelemetry array in {body}"))
        .clone()
}

/// The one entry `global_network` reports for the connection `resource_arn`, or `None` where
/// it reports none.
async fn entry_for(
    emulator: &Emulator,
    case: &str,
    global_network: &str,
    resource_arn: &str,
) -> Option<Value> {
    let query = format!("resourceArn={}", escape(resource_arn));
    let entries = telemetry(emulator, case, global_network, &query).await;
    assert!(
        entries.len() <= 1,
        "case {case:?}: one connection has at most one registration, got {entries:?}"
    );
    entries.into_iter().next()
}

/// `CreateSiteToSiteVpnAttachment` of `vpn_connection_arn` on `core_network`, answering the
/// minted attachment id.
async fn attach(
    emulator: &Emulator,
    case: &str,
    core_network: &str,
    vpn_connection_arn: &str,
) -> String {
    let (status, created) = emulator
        .post(
            CREATE_URI,
            json!({
                "CoreNetworkId": core_network,
                "VpnConnectionArn": vpn_connection_arn,
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateSiteToSiteVpnAttachment expected 200, got {status} with {created}"
    );
    created["SiteToSiteVpnAttachment"]["Attachment"]["AttachmentId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no AttachmentId in {created}"))
        .to_string()
}

/// Assert the members a telemetry entry of `case` reports, with the health `status`
/// expected at the instant it was read.
fn assert_entry(case: &VpnCase, entry: &Value, core_network: &str, status: &str) {
    let name = case.name;
    assert_eq!(
        entry["ResourceType"].as_str(),
        Some(VPN_CONNECTION),
        "case {name:?}: expected a {VPN_CONNECTION} entry, got {entry}"
    );
    assert_eq!(
        entry["ResourceArn"].as_str(),
        Some(case.arn().as_str()),
        "case {name:?}: expected the attached connection's ARN, got {entry}"
    );
    assert_eq!(
        entry["ResourceId"].as_str(),
        Some(case.resource_id().as_str()),
        "case {name:?}: expected the connection id its ARN names, got {entry}"
    );
    assert_eq!(
        entry["AwsRegion"].as_str(),
        Some(case.region),
        "case {name:?}: expected the edge location the session lands at, got {entry}"
    );
    assert_eq!(
        entry["AccountId"].as_str(),
        Some(case.account),
        "case {name:?}: expected the account owning the connection, got {entry}"
    );
    assert_eq!(
        entry["CoreNetworkId"].as_str(),
        Some(core_network),
        "case {name:?}: expected the core network the connection reaches, got {entry}"
    );
    assert_eq!(
        entry["RegisteredGatewayArn"],
        Value::Null,
        "case {name:?}: a connection reaching a core network edge is registered through no \
         gateway, got {entry}"
    );
    assert_eq!(
        entry["Health"]["Type"].as_str(),
        Some(IPSEC),
        "case {name:?}: a VPN connection's session is IPsec, got {entry}"
    );
    assert_eq!(
        entry["Health"]["Status"].as_str(),
        Some(status),
        "case {name:?}: expected the session {status}, got {entry}"
    );
    assert!(
        entry["Health"]["Timestamp"].is_number(),
        "case {name:?}: the health reports the instant the status took effect, got {entry}"
    );
}

#[tokio::test]
async fn attaching_a_vpn_connection_registers_its_ipsec_session_with_the_global_network() {
    let emulator = Emulator::new();
    let cases = vpn_cases();
    let regions: Vec<&str> = cases.iter().map(|case| case.region).collect();
    // One core network with an edge in every Region the table uses, so every row shares a
    // global network and a registration that leaked between them would show up.
    let (global_network, core_network) =
        core_network_at(&emulator, "vpn telemetry", &regions).await;

    // Before any attachment the global network knows of no session at all.
    let before = telemetry(&emulator, "before any attachment", &global_network, "").await;
    assert!(
        before.is_empty(),
        "case before any attachment: expected no telemetry, got {before:?}"
    );

    for case in &cases {
        let arn = case.arn();
        attach(&emulator, case.name, &core_network, &arn).await;
        // The session starts DOWN and comes up when the attachment carrying it reaches
        // AVAILABLE, so a caller reading straight after the create still sees it DOWN.
        let entry = entry_for(&emulator, case.name, &global_network, &arn)
            .await
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: the attached connection must be registered with {global_network}",
                    case.name
                )
            });
        assert_entry(case, &entry, &core_network, DOWN);
    }

    tokio::time::sleep(PAST_SETTLE).await;

    let settled = telemetry(&emulator, "after the settle span", &global_network, "").await;
    assert_eq!(
        settled.len(),
        cases.len(),
        "case after the settle span: expected one entry per attached connection, got {settled:?}"
    );
    for case in &cases {
        let arn = case.arn();
        let entry = settled
            .iter()
            .find(|entry| entry["ResourceArn"].as_str() == Some(arn.as_str()))
            .unwrap_or_else(|| {
                panic!("case {:?}: no entry for {arn} in {settled:?}", case.name)
            });
        assert_entry(case, entry, &core_network, UP);
    }

    // Every filter the request carries narrows those same registrations.
    let all_vpn = telemetry(
        &emulator,
        "resourceType=vpn-connection",
        &global_network,
        &format!("resourceType={}", escape(VPN_CONNECTION)),
    )
    .await;
    assert_eq!(
        all_vpn.len(),
        cases.len(),
        "case resourceType=vpn-connection: expected every registration, got {all_vpn:?}"
    );
    let other_kind = telemetry(
        &emulator,
        "resourceType=connect-peer",
        &global_network,
        &format!("resourceType={}", escape(CONNECT_PEER)),
    )
    .await;
    assert!(
        other_kind.is_empty(),
        "case resourceType=connect-peer: this global network holds none, got {other_kind:?}"
    );
    let by_core_network = telemetry(
        &emulator,
        "coreNetworkId",
        &global_network,
        &format!("coreNetworkId={}", escape(&core_network)),
    )
    .await;
    assert_eq!(
        by_core_network.len(),
        cases.len(),
        "case coreNetworkId: every connection reaches that core network, got {by_core_network:?}"
    );
    for case in &cases {
        let by_region = telemetry(
            &emulator,
            case.name,
            &global_network,
            &format!("awsRegion={}", escape(case.region)),
        )
        .await;
        let expected = cases.iter().filter(|row| row.region == case.region).count();
        assert_eq!(
            by_region.len(),
            expected,
            "case {:?}: awsRegion={} expected {expected} entries, got {by_region:?}",
            case.name,
            case.region
        );
        let by_account = telemetry(
            &emulator,
            case.name,
            &global_network,
            &format!("accountId={}", escape(case.account)),
        )
        .await;
        let expected = cases
            .iter()
            .filter(|row| row.account == case.account)
            .count();
        assert_eq!(
            by_account.len(),
            expected,
            "case {:?}: accountId={} expected {expected} entries, got {by_account:?}",
            case.name,
            case.account
        );
    }

    // Reading the telemetry changes nothing a later caller sees.
    let first = telemetry(&emulator, "a repeated read", &global_network, "").await;
    let second = telemetry(&emulator, "a repeated read", &global_network, "").await;
    assert_eq!(
        first, second,
        "case a repeated read: expected the identical page, got {second:?} after {first:?}"
    );
}

#[tokio::test]
async fn a_vpn_attachment_registers_its_session_only_once_its_request_is_accepted() {
    let emulator = Emulator::new();
    const REGION: &str = "us-west-2";
    const PENDING_ACCEPTANCE: &str = "PENDING_ATTACHMENT_ACCEPTANCE";

    // A policy whose single `attachment-policies` rule claims every attachment and asks for
    // its acceptance, so the create below leaves the attachment waiting for one.
    let rules = json!([{
        "rule-number": 100,
        "conditions": [{ "type": "any" }],
        "action": {
            "association-method": "constant",
            "segment": "shared",
            "require-attachment-acceptance": true,
        },
    }]);
    let global_network = emulator.create_global_network("vpn acceptance telemetry").await;
    let (status, created) = create_core_network(
        &emulator,
        json!({
            "GlobalNetworkId": global_network,
            "Description": "vpn acceptance telemetry",
            "PolicyDocument": acceptance_policy_document(REGION, &rules),
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case the core network: CreateCoreNetwork expected 200, got {status} with {created}"
    );
    let core_network = created["CoreNetwork"]["CoreNetworkId"]
        .as_str()
        .unwrap_or_else(|| panic!("case the core network: no CoreNetworkId in {created}"))
        .to_string();

    let accepted_case = VpnCase {
        name: "an attachment request that is accepted",
        partition: "aws",
        region: REGION,
        account: OWNER,
        vpn_id: "0000acce",
    };
    let rejected_case = VpnCase {
        name: "an attachment request that is rejected",
        partition: "aws",
        region: REGION,
        account: OTHER_OWNER,
        vpn_id: "0000bad0",
    };

    let mut ids: Vec<(&VpnCase, String)> = Vec::new();
    for case in [&accepted_case, &rejected_case] {
        let id = attach(&emulator, case.name, &core_network, &case.arn()).await;
        let (_, read) = emulator.get(&format!("{CREATE_URI}/{id}")).await;
        assert_eq!(
            read["SiteToSiteVpnAttachment"]["Attachment"]["State"].as_str(),
            Some(PENDING_ACCEPTANCE),
            "case {:?}: the policy holds the attachment for an acceptance, got {read}",
            case.name
        );
        // Nothing is created for an attachment waiting for an acceptance, so there is no
        // session to report — not now and not after the span the clock owes a created one.
        let waiting = entry_for(&emulator, case.name, &global_network, &case.arn()).await;
        assert!(
            waiting.is_none(),
            "case {:?}: an attachment waiting for its acceptance registers nothing, got {waiting:?}",
            case.name
        );
        ids.push((case, id));
    }
    tokio::time::sleep(PAST_SETTLE).await;
    for (case, _) in &ids {
        let waiting = entry_for(&emulator, case.name, &global_network, &case.arn()).await;
        assert!(
            waiting.is_none(),
            "case {:?}: no span of the clock creates an attachment nothing accepted, got \
             {waiting:?}",
            case.name
        );
    }

    // The acceptance starts attaching, so it is what registers the session: DOWN until the
    // attachment it rides reaches AVAILABLE, UP after.
    let accepted_id = ids[0].1.clone();
    let rejected_id = ids[1].1.clone();
    let (status, accepted) = accept(&emulator, &accepted_id).await;
    assert_eq!(
        status, 200,
        "case the acceptance: AcceptAttachment expected 200, got {status} with {accepted}"
    );
    let entry = entry_for(
        &emulator,
        accepted_case.name,
        &global_network,
        &accepted_case.arn(),
    )
    .await
    .unwrap_or_else(|| {
        panic!(
            "case {:?}: the acceptance must register the session",
            accepted_case.name
        )
    });
    assert_entry(&accepted_case, &entry, &core_network, DOWN);

    // The rejected request is turned down, so nothing is ever created for it and no session
    // is registered for its connection.
    let (status, rejected) = emulator
        .call("POST", &format!("/attachments/{rejected_id}/reject"), None)
        .await;
    assert_eq!(
        status, 200,
        "case the rejection: RejectAttachment expected 200, got {status} with {rejected}"
    );

    tokio::time::sleep(PAST_SETTLE).await;
    let entry = entry_for(
        &emulator,
        accepted_case.name,
        &global_network,
        &accepted_case.arn(),
    )
    .await
    .unwrap_or_else(|| panic!("case {:?}: the session stays registered", accepted_case.name));
    assert_entry(&accepted_case, &entry, &core_network, UP);
    let entries = telemetry(&emulator, "after both decisions", &global_network, "").await;
    assert_eq!(
        entries.len(),
        1,
        "case after both decisions: only the accepted attachment registered a session, got \
         {entries:?}"
    );
}

#[tokio::test]
async fn deleting_the_attachment_deregisters_the_connection_it_attached() {
    let emulator = Emulator::new();
    const REGION: &str = "eu-central-1";
    let (global_network, core_network) =
        core_network_at(&emulator, "vpn telemetry teardown", &[REGION]).await;

    let kept = VpnCase {
        name: "the connection that stays attached",
        partition: "aws",
        region: REGION,
        account: OWNER,
        vpn_id: "0000cafe",
    };
    let removed = VpnCase {
        name: "the connection whose attachment is deleted",
        partition: "aws",
        region: REGION,
        account: OTHER_OWNER,
        vpn_id: "0000dead",
    };

    let kept_id = attach(&emulator, kept.name, &core_network, &kept.arn()).await;
    let removed_id = attach(&emulator, removed.name, &core_network, &removed.arn()).await;
    tokio::time::sleep(PAST_SETTLE).await;
    let entries = telemetry(&emulator, "both attached", &global_network, "").await;
    assert_eq!(
        entries.len(),
        2,
        "case both attached: expected one entry per attached connection, got {entries:?}"
    );

    let (status, deleted) = emulator
        .delete(&format!("/attachments/{removed_id}"))
        .await;
    assert_eq!(
        status, 200,
        "case {:?}: DeleteAttachment expected 200, got {status} with {deleted}",
        removed.name
    );
    let gone = entry_for(&emulator, removed.name, &global_network, &removed.arn()).await;
    assert!(
        gone.is_none(),
        "case {:?}: the delete unties the connection, got {gone:?}",
        removed.name
    );
    let left = telemetry(&emulator, "after the delete", &global_network, "").await;
    assert_eq!(
        left.len(),
        1,
        "case after the delete: the other registration is untouched, got {left:?}"
    );
    assert_entry(&kept, &left[0], &core_network, UP);
    // The delete untied one attachment and nothing else: the other is still stored.
    let (status, still_there) = emulator.get(&format!("{CREATE_URI}/{kept_id}")).await;
    assert_eq!(
        status, 200,
        "case {:?}: the kept attachment must still read, got {status} with {still_there}",
        kept.name
    );

    // The same connection attached again is registered again: the registration follows the
    // attachment rather than being minted once per connection.
    attach(&emulator, removed.name, &core_network, &removed.arn()).await;
    let again = entry_for(&emulator, removed.name, &global_network, &removed.arn())
        .await
        .unwrap_or_else(|| {
            panic!(
                "case {:?}: re-attaching the connection must register it again",
                removed.name
            )
        });
    assert_entry(&removed, &again, &core_network, DOWN);
}

#[tokio::test]
async fn an_attachment_whose_resource_has_no_session_registers_nothing() {
    let emulator = Emulator::new();
    const REGION: &str = "us-east-1";
    let (global_network, core_network) =
        core_network_at(&emulator, "other attachment kinds", &[REGION]).await;

    // A VPC attachment, a Connect attachment carried over it and a Direct Connect gateway
    // attachment: each attaches a resource of the inventory rather than a connection whose
    // session this service measures.
    let (status, vpc) = create_vpc_attachment(
        &emulator,
        json!({
            "CoreNetworkId": core_network,
            "VpcArn": vpc_arn(REGION, 1),
            "SubnetArns": [subnet_arn(REGION, 1)],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case the VPC attachment: expected 200, got {status} with {vpc}"
    );
    let transport = vpc["VpcAttachment"]["Attachment"]["AttachmentId"]
        .as_str()
        .unwrap_or_else(|| panic!("case the VPC attachment: no AttachmentId in {vpc}"))
        .to_string();
    let (status, connect) = create_connect_attachment(
        &emulator,
        json!({
            "CoreNetworkId": core_network,
            "EdgeLocation": REGION,
            "TransportAttachmentId": transport,
            "Options": { "Protocol": "GRE" },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case the Connect attachment: expected 200, got {status} with {connect}"
    );
    let (status, gateway) = emulator
        .post(
            "/direct-connect-gateway-attachments",
            json!({
                "CoreNetworkId": core_network,
                "DirectConnectGatewayArn": gateway_arn(1),
                "EdgeLocations": [REGION],
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case the Direct Connect gateway attachment: expected 200, got {status} with {gateway}"
    );

    tokio::time::sleep(PAST_SETTLE).await;
    let entries = telemetry(&emulator, "the other kinds", &global_network, "").await;
    assert!(
        entries.is_empty(),
        "case the other kinds: none of them attaches a connection with a session, got \
         {entries:?}"
    );

    // One site-to-site VPN attachment on the same core network is the one registration.
    let vpn = VpnCase {
        name: "the only connection with a session",
        partition: "aws",
        region: REGION,
        account: OWNER,
        vpn_id: "0000f1ee",
    };
    attach(&emulator, vpn.name, &core_network, &vpn.arn()).await;
    let entries = telemetry(&emulator, "with the VPN attachment", &global_network, "").await;
    assert_eq!(
        entries.len(),
        1,
        "case with the VPN attachment: expected exactly its registration, got {entries:?}"
    );
    assert_entry(&vpn, &entries[0], &core_network, DOWN);
}
