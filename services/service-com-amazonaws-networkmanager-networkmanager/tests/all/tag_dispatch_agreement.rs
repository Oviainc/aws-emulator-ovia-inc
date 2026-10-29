// SPDX-License-Identifier: CC0-1.0

//! The ARN -> stored-tag-list resolution of this service, driven through an operation on
//! EVERY side of it: the writer (`TagResource` / `UntagResource`), the reader
//! (`ListTagsForResource`) and the "does this ARN still name a stored resource" probe that
//! `UpdateNetworkResourceMetadata` prunes deleted resources' metadata through.
//!
//! The rule under test: one ARN of this service names at most one stored record, so a
//! WRITE and a READ of that record's tags resolve the SAME record for the SAME ARN — for
//! every taggable kind this service holds, and alike for an ARN that names nothing (an
//! unknown id, a stored id named under a parent it is not stored under, or a resource type
//! that carries no tags at all), which every one of those operations refuses the same way.
//!
//! The table spans that class: one row per taggable kind the service holds — the kinds
//! named under a global network (a site, a link, a device, a connection) and the ones
//! identified account-wide (a global network, a core network, an attachment, a peering, a
//! Connect peer) — each driven through the writer, the reader and, where the kind is part
//! of a global network's network-resource inventory, the metadata probe. Nothing reaches
//! into `crate::state`: every resource is created, tagged and read back through an
//! operation.

use serde_json::{json, Value};

use crate::attachment_lifecycle::{
    attachment_id, core_network_at, create_connect_attachment, create_vpc_attachment, subnet_arn,
    vpc_arn,
};
use crate::connect_peer_lifecycle::create_connect_peer;
use crate::device_lifecycle::create_device_ok;
use crate::global_network_lifecycle::{
    error_code, expected_arn, expected_scoped_arn, Emulator,
};
use crate::link_lifecycle::{create_link_ok, plain_link};
use crate::network_resources::{create_ok, listed, minted};
use crate::resource_tagging::{escape, tag, tags_uri, untag_uri};
use crate::transit_gateway_peering::{create_peering, own_account, transit_gateway_arn};

/// The Region the fixture's core network configures an edge in, which is therefore the
/// Region of the VPC it attaches and of the transit gateway it peers with.
const EDGE_REGION: &str = "us-west-2";

/// Resource types of this service that carry NO tags: the model gives `RouteAnalysis`,
/// `CoreNetworkPolicy` and `LinkAssociation` no `Tags` member at all (`smithy_query
/// shape=RouteAnalysis,CoreNetworkPolicy,LinkAssociation`), so a well-formed ARN naming
/// one of them resolves to no tag list — in the writer exactly as in the reader.
const UNTAGGABLE_KINDS: [&str; 3] = ["route-analysis", "core-network-policy", "link-association"];

/// One minted resource: its kind (which is also the resource type its ARN carries), its
/// own id, the ARN the naming rule gives it, the global network its name is scoped under
/// (`None` for a kind identified account-wide) and the resource type a global network's
/// network-resource inventory reports it under (`None` for a kind the inventory does not
/// hold).
struct Row {
    kind: &'static str,
    id: String,
    arn: String,
    scope: Option<String>,
    inventory_type: Option<&'static str>,
}

impl Row {
    /// The ARN of a resource of this row's kind carrying `id` instead of the stored id, at
    /// the same naming depth as the stored resource.
    fn sibling(&self, id: &str) -> String {
        match &self.scope {
            Some(scope) => expected_scoped_arn(self.kind, scope, id),
            None => expected_arn(self.kind, id),
        }
    }

    /// The stored id of this row named under `parent`: for a scoped kind that is another
    /// global network than the one the resource lives in, and for an account-wide kind a
    /// parent its name never carries. Either way the ARN names no stored resource.
    fn misnamed(&self, parent: &str) -> String {
        expected_scoped_arn(self.kind, parent, &self.id)
    }
}

/// An id of `kind` that nothing minted: the `<kind>-<17 hex digits>` shape this service
/// mints ids in (`crate::global_network_lifecycle::is_minted_id`), with a body no counter
/// reaches.
fn unknown_id(kind: &str) -> String {
    format!("{kind}-{:017x}", 0xf0f0f0f0f0f0fu64)
}

/// The string member `member` of `value`, or a panic naming what was there instead.
fn string_member(value: &Value, member: &str) -> String {
    value[member]
        .as_str()
        .unwrap_or_else(|| panic!("expected a string {member} in {value}"))
        .to_string()
}

/// Build one resource of every taggable kind this service holds, inside one global
/// network, and report them as the rows of the table.
///
/// `base` offsets the VPC, subnet and transit gateway ARNs the fixture attaches and peers
/// with, so two fixtures of one emulator never ask for the same external resource twice.
async fn taggable_rows(
    emulator: &Emulator,
    label: &'static str,
    base: usize,
) -> (String, Vec<Row>) {
    let (network, core_network) = core_network_at(emulator, label, &[EDGE_REGION]).await;
    let site = emulator.create_site(&network, label).await;
    let link = create_link_ok(emulator, &network, &site, &plain_link()).await;
    let device = create_device_ok(emulator, &network, json!({ "SiteId": site })).await;
    let peer_device = create_device_ok(emulator, &network, json!({})).await;
    let connection = minted(
        &create_ok(
            emulator,
            &format!("/global-networks/{network}/connections"),
            json!({ "DeviceId": device, "ConnectedDeviceId": peer_device }),
        )
        .await,
        "Connection",
        "ConnectionId",
    );

    let (status, attached) = create_vpc_attachment(
        emulator,
        json!({
            "CoreNetworkId": core_network,
            "VpcArn": vpc_arn(EDGE_REGION, base),
            "SubnetArns": [subnet_arn(EDGE_REGION, base)],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {label}: CreateVpcAttachment expected 200, got {status} with {attached}"
    );
    let attachment = attachment_id(label, &attached["VpcAttachment"]);

    // A Connect attachment is carried over a VPC attachment of the same core network, and
    // one VPC is attached by at most one attachment, so the Connect peer below hangs off
    // its own transport.
    let (status, transport) = create_vpc_attachment(
        emulator,
        json!({
            "CoreNetworkId": core_network,
            "VpcArn": vpc_arn(EDGE_REGION, base + 1),
            "SubnetArns": [subnet_arn(EDGE_REGION, base + 1)],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {label}: the transport CreateVpcAttachment expected 200, got {status} with {transport}"
    );
    let transport_id = attachment_id(label, &transport["VpcAttachment"]);
    let (status, connect) = create_connect_attachment(
        emulator,
        json!({
            "CoreNetworkId": core_network,
            "EdgeLocation": EDGE_REGION,
            "TransportAttachmentId": transport_id,
            "Options": { "Protocol": "GRE" },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {label}: CreateConnectAttachment expected 200, got {status} with {connect}"
    );
    let connect_attachment = attachment_id(label, &connect["ConnectAttachment"]);

    let (status, peer) = create_connect_peer(
        emulator,
        json!({
            "ConnectAttachmentId": connect_attachment,
            "PeerAddress": "203.0.113.1",
            "CoreNetworkAddress": "203.0.113.2",
            "InsideCidrBlocks": [format!("169.254.{}.0/29", base)],
            "BgpOptions": { "PeerAsn": 65000 },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {label}: CreateConnectPeer expected 200, got {status} with {peer}"
    );
    let connect_peer = string_member(&peer["ConnectPeer"], "ConnectPeerId");

    let (status, peered) = create_peering(
        emulator,
        json!({
            "CoreNetworkId": core_network,
            "TransitGatewayArn": transit_gateway_arn("aws", EDGE_REGION, &own_account(), base, 17),
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {label}: CreateTransitGatewayPeering expected 200, got {status} with {peered}"
    );
    let peering = string_member(&peered["TransitGatewayPeering"]["Peering"], "PeeringId");

    let rows = vec![
        Row {
            kind: "global-network",
            arn: expected_arn("global-network", &network),
            id: network.clone(),
            scope: None,
            inventory_type: None,
        },
        Row {
            kind: "site",
            arn: expected_scoped_arn("site", &network, &site),
            id: site.clone(),
            scope: Some(network.clone()),
            inventory_type: Some("site"),
        },
        Row {
            kind: "link",
            arn: expected_scoped_arn("link", &network, &link),
            id: link,
            scope: Some(network.clone()),
            inventory_type: Some("link"),
        },
        Row {
            kind: "device",
            arn: expected_scoped_arn("device", &network, &device),
            id: device,
            scope: Some(network.clone()),
            inventory_type: Some("device"),
        },
        Row {
            kind: "connection",
            arn: expected_scoped_arn("connection", &network, &connection),
            id: connection,
            scope: Some(network.clone()),
            inventory_type: Some("connection"),
        },
        Row {
            kind: "core-network",
            arn: expected_arn("core-network", &core_network),
            id: core_network,
            scope: None,
            inventory_type: Some("core-network"),
        },
        Row {
            kind: "attachment",
            arn: expected_arn("attachment", &attachment),
            id: attachment,
            scope: None,
            inventory_type: Some("attachment"),
        },
        Row {
            kind: "peering",
            arn: expected_arn("peering", &peering),
            id: peering,
            scope: None,
            inventory_type: None,
        },
        Row {
            kind: "connect-peer",
            arn: expected_arn("connect-peer", &connect_peer),
            id: connect_peer,
            scope: None,
            inventory_type: None,
        },
    ];
    (network, rows)
}

/// The `TagList` `ListTagsForResource` answers for `arn`, with the status.
async fn read_tags(emulator: &Emulator, arn: &str) -> (u16, Value) {
    let (status, body) = emulator.get(&tags_uri(arn)).await;
    let tags = if status == 200 {
        body["TagList"].clone()
    } else {
        body
    };
    (status, tags)
}

/// The `UpdateNetworkResourceMetadata` URI of one network resource: the ARN travels as a
/// non-greedy `@httpLabel`, so a client sends its `:` and `/` encoded.
fn metadata_uri(network: &str, arn: &str) -> String {
    format!(
        "/global-networks/{network}/network-resources/{}/metadata",
        escape(arn)
    )
}

/// The `Metadata` the inventory reports for the one resource `arn` names.
async fn reported_metadata(emulator: &Emulator, network: &str, case: &str, arn: &str) -> Value {
    let (entries, _) = listed(emulator, network, &format!("resourceArn={}", escape(arn))).await;
    assert_eq!(
        entries.len(),
        1,
        "case {case}: expected exactly one inventory entry for {arn}, got {entries:?}"
    );
    entries[0]["Metadata"].clone()
}

#[tokio::test]
async fn a_tag_written_through_an_arn_is_the_tag_read_back_through_it_for_every_taggable_kind() {
    let emulator = Emulator::new();
    let (network, rows) = taggable_rows(&emulator, "the tag dispatch", 1).await;
    // A second global network, so a scoped row can be mis-named under a parent that
    // EXISTS and still names no stored resource.
    let other_network = emulator.create_global_network("a sibling global network").await;

    for row in &rows {
        let kind = row.kind;

        // The writer resolves the record the ARN names...
        let (status, body) = emulator
            .post(&tags_uri(&row.arn), json!({ "Tags": [tag("dispatch", kind)] }))
            .await;
        assert_eq!(
            status, 200,
            "case {kind}: TagResource({}) expected 200, got {status} with {body}",
            row.arn
        );

        // ... and the reader answers the tag that write stored, so both resolved the one
        // record the ARN names.
        let (status, tags) = read_tags(&emulator, &row.arn).await;
        assert_eq!(
            status, 200,
            "case {kind}: ListTagsForResource({}) expected 200, got {status} with {tags}",
            row.arn
        );
        assert_eq!(
            tags,
            json!([tag("dispatch", kind)]),
            "case {kind}: expected the stored tag list through {}, got {tags}",
            row.arn
        );

        // The removal resolves the same record once more: the reader sees the key gone.
        let (status, body) = emulator.delete(&untag_uri(&row.arn, &["dispatch"])).await;
        assert_eq!(
            status, 200,
            "case {kind}: UntagResource({}) expected 200, got {status} with {body}",
            row.arn
        );
        let (status, tags) = read_tags(&emulator, &row.arn).await;
        assert_eq!(
            status, 200,
            "case {kind}: ListTagsForResource after the removal expected 200, got {status} with {tags}"
        );
        assert_eq!(
            tags,
            json!([]),
            "case {kind}: the removed key must be gone from the stored list, got {tags}"
        );
    }

    // Every way of naming nothing, for every kind: the writer and the reader must answer
    // it alike, or a tag could be stored where no read can ever find it.
    let mut unresolved: Vec<(String, String)> = Vec::new();
    for row in &rows {
        unresolved.push((
            format!("{}: an id of this kind that nothing minted", row.kind),
            row.sibling(&unknown_id(row.kind)),
        ));
        let parent = match &row.scope {
            Some(_) => other_network.clone(),
            None => network.clone(),
        };
        unresolved.push((
            format!("{}: the stored id under a parent it is not stored under", row.kind),
            row.misnamed(&parent),
        ));
    }
    for kind in UNTAGGABLE_KINDS {
        unresolved.push((
            format!("{kind}: a resource type that carries no tags"),
            expected_arn(kind, &unknown_id(kind)),
        ));
    }

    for (case, arn) in &unresolved {
        let (status, body) = emulator
            .post(&tags_uri(arn), json!({ "Tags": [tag("dispatch", "unresolved")] }))
            .await;
        assert_eq!(
            status, 404,
            "case {case}: TagResource({arn}) expected 404, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case}: TagResource({arn}) answered the wrong error code in {body}"
        );

        let (status, body) = emulator.delete(&untag_uri(arn, &["dispatch"])).await;
        assert_eq!(
            status, 404,
            "case {case}: UntagResource({arn}) expected 404, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case}: UntagResource({arn}) answered the wrong error code in {body}"
        );

        let (status, body) = read_tags(&emulator, arn).await;
        assert_eq!(
            status, 404,
            "case {case}: ListTagsForResource({arn}) expected 404, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case}: ListTagsForResource({arn}) answered the wrong error code in {body}"
        );
    }

    // Nothing the refusals touched changed a stored tag list: every row still reads empty
    // through its own ARN.
    for row in &rows {
        let (status, tags) = read_tags(&emulator, &row.arn).await;
        assert_eq!(
            status, 200,
            "case {}: the stored resource must still read back, got {status} with {tags}",
            row.kind
        );
        assert_eq!(
            tags,
            json!([]),
            "case {}: no refused call may have stored a tag, got {tags}",
            row.kind
        );
    }
}

#[tokio::test]
async fn the_metadata_prune_keeps_the_entries_of_every_resource_the_same_dispatch_still_resolves() {
    let emulator = Emulator::new();
    let (network, rows) = taggable_rows(&emulator, "the metadata probe", 10).await;
    let inventory: Vec<&Row> = rows
        .iter()
        .filter(|row| row.inventory_type.is_some())
        .collect();
    assert!(
        !inventory.is_empty(),
        "the fixture must hold network resources to store metadata on, got {} rows",
        rows.len()
    );

    // Each resource of the inventory is given its own metadata map. Every write first
    // drops the entries of resources the ARN dispatch no longer resolves, so the LAST
    // write prunes against every ARN written before it.
    for (index, row) in inventory.iter().enumerate() {
        let metadata = json!({ "owner": row.kind, "order": index.to_string() });
        let (status, body) = emulator
            .patch(
                &metadata_uri(&network, &row.arn),
                json!({ "Metadata": metadata }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {}: UpdateNetworkResourceMetadata expected 200, got {status} with {body}",
            row.kind
        );
        assert_eq!(
            body["Metadata"], metadata,
            "case {}: expected the stored metadata, got {body}",
            row.kind
        );
    }

    // A resource the dispatch resolves is NOT deleted, so no write may have pruned its
    // metadata: the inventory still reports every map stored above.
    for (index, row) in inventory.iter().enumerate() {
        let reported = reported_metadata(&emulator, &network, row.kind, &row.arn).await;
        assert_eq!(
            reported,
            json!({ "owner": row.kind, "order": index.to_string() }),
            "case {}: expected the metadata stored for {} to survive the prune, got {reported}",
            row.kind,
            row.arn
        );
    }
}
