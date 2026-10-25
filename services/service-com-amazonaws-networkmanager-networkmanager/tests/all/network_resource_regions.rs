// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of the Region a `GetNetworkResources` entry reports, and of the
//! `awsRegion` filter that selects on it, driven over HTTP through the real router (the
//! harness lives in `crate::global_network_lifecycle`).
//!
//! The rule under test: a network-resource entry reports `AwsRegion` as the ONE Region the
//! resource's own reader names it at — the edge location that reader answers as a single
//! `ExternalRegionCode`, the very shape `NetworkResource$AwsRegion` targets (`smithy_query
//! shape="NetworkResource,Attachment,Peering,ConnectPeer"`) — and omits it only where that
//! reader names no single Region: a resource held under the global network itself, or one
//! that spans a LIST of edges (a core network's `Edges`, a Direct Connect gateway
//! attachment's `EdgeLocations`), which a single-valued member cannot carry. The
//! `awsRegion` query filter then selects exactly the entries reporting that Region.
//!
//! The table below spans that class: one row per kind the store holds — the five
//! attachment kinds, a transit gateway peering, a Connect peer, a core network, a site, a
//! link, a device (carrying an AWS location of its own) and a connection — differing in how
//! many Regions the record places the resource in (one, several, a one-element list, none
//! at all), in which Region that is (four Region codes, plus the 63-character
//! `ExternalRegionCode` the model's bound publishes), and in how the resource came to sit
//! there (the Region of an attached VPC, of a VPN connection, of a peered transit gateway,
//! of a transport attachment, or an explicitly named list of edges). Nothing reaches into
//! `crate::state`: every resource is created through its own operation and every expected
//! Region is read back from that resource's own reader.

use serde_json::{json, Value};

use crate::attachment_lifecycle::{
    attachment_id, core_network_at, create_connect_attachment, subnet_arn, vpc_arn,
};
// `widest_region_code` is the 63-character `ExternalRegionCode` bound, kept with the suite
// that drives the attachment kind spanning a list of edges.
use crate::direct_connect_gateway_attachment_lifecycle::{gateway_arn, widest_region_code};
use crate::global_network_lifecycle::Emulator;
// The inventory's own reader, its key spelling, its `NextToken`-following collector and the
// must-succeed create wrapper live with the suite that covers `GetNetworkResources` itself.
use crate::network_resources::{create_ok, key_of, keys, listed, minted};
use crate::resource_tagging::escape;
use crate::site_to_site_vpn_attachment_lifecycle::vpn_arn;

/// The Region the VPC attachment, the Connect attachment carried over it and that
/// attachment's Connect peer all sit at.
const REGION_VPC: &str = "us-west-2";

/// The Region of the VPN connection the site-to-site VPN attachment attaches, and so of
/// that attachment's edge.
const REGION_VPN: &str = "eu-central-1";

/// The Region of the peered transit gateway, and so of the peering and of the route table
/// attachment created over it.
const REGION_PEERING: &str = "ap-southeast-4";

/// The Region the two Direct Connect gateway attachments span — the one edge location of
/// the core network that NO entry reports, because the kind sitting there reports a list of
/// edges instead.
const REGION_GATEWAY: &str = "sa-east-1";

/// A Region the core network configures no edge in at all, so no resource can sit there.
const REGION_NOWHERE: &str = "us-east-1";

/// One resource of the global network under test: how a `GetNetworkResources` entry names
/// it, and how to read it back through its own reader.
struct Inventoried {
    /// Names the case in every assertion message.
    name: &'static str,
    /// The `ResourceType` the inventory reports it under.
    resource_type: &'static str,
    id: String,
    /// The URI of the operation that describes this one resource.
    read_uri: String,
    /// The members to walk, in order, from that answer down to the resource's own shape —
    /// the shape whose `EdgeLocation` (if it has one) is the Region it sits at.
    shape_path: &'static [&'static str],
    /// Whether the last member of the path is a LIST of one resource (`GetSites` /
    /// `GetLinks` / `GetDevices` / `GetConnections`) rather than the resource itself.
    shape_is_list: bool,
}

impl Inventoried {
    /// The `<ResourceType>/<ResourceId>` key the inventory lists this resource under, which
    /// is also the order it pages in.
    fn key(&self) -> String {
        format!("{}/{}", self.resource_type, self.id)
    }

    /// The resource's own shape, as its own reader answers it.
    async fn shape(&self, emulator: &Emulator) -> Value {
        let (status, body) = emulator.get(&self.read_uri).await;
        assert_eq!(
            status,
            200,
            "case {:?}: GET {} expected 200, got {status} with {body}",
            self.name,
            self.read_uri
        );
        let mut shape = &body;
        for member in self.shape_path {
            shape = &shape[member];
        }
        if self.shape_is_list {
            shape = &shape[0];
        }
        assert!(
            shape.is_object(),
            "case {:?}: expected the resource's own shape at {:?} in {body}",
            self.name,
            self.shape_path
        );
        shape.clone()
    }

    /// The Region this resource's own reader names it at: the single `EdgeLocation` that
    /// reader answers, or `None` where it answers none — which is what the entry's
    /// `AwsRegion` must be.
    async fn expected_region(&self, emulator: &Emulator) -> Option<String> {
        self.shape(emulator).await["EdgeLocation"]
            .as_str()
            .map(str::to_string)
    }
}

/// The inventory every test below runs against: ONE global network whose core network
/// configures an edge in each Region above, holding one resource of every kind the store
/// keeps — including the kinds that sit at one edge, the kind that spans several, and the
/// kinds that sit at none.
async fn inventory(emulator: &Emulator) -> (String, Vec<Inventoried>) {
    let wide = widest_region_code();
    let edges = [
        REGION_VPC,
        REGION_VPN,
        REGION_PEERING,
        REGION_GATEWAY,
        wide.as_str(),
    ];
    let (network, core_network) =
        core_network_at(emulator, "network resource regions", &edges).await;
    let mut held: Vec<Inventoried> = vec![Inventoried {
        name: "a core network, which spans the edges its policy configures",
        resource_type: "core-network",
        id: core_network.clone(),
        read_uri: format!("/core-networks/{core_network}"),
        shape_path: &["CoreNetwork"],
        shape_is_list: false,
    }];

    // A VPC attachment sits at the Region of the VPC it attaches.
    let created = create_ok(
        emulator,
        "/vpc-attachments",
        json!({
            "CoreNetworkId": core_network,
            "VpcArn": vpc_arn(REGION_VPC, 1),
            "SubnetArns": [subnet_arn(REGION_VPC, 1)],
        }),
    )
    .await;
    let transport = attachment_id("vpc attachment", &created["VpcAttachment"]);
    held.push(Inventoried {
        name: "a VPC attachment, at the Region of its VPC",
        resource_type: "attachment",
        id: transport.clone(),
        read_uri: format!("/vpc-attachments/{transport}"),
        shape_path: &["VpcAttachment", "Attachment"],
        shape_is_list: false,
    });

    // A second VPC attachment, at the widest Region code the model admits.
    let created = create_ok(
        emulator,
        "/vpc-attachments",
        json!({
            "CoreNetworkId": core_network,
            "VpcArn": vpc_arn(&wide, 2),
            "SubnetArns": [subnet_arn(&wide, 2)],
        }),
    )
    .await;
    let widest = attachment_id("widest-region VPC attachment", &created["VpcAttachment"]);
    held.push(Inventoried {
        name: "a VPC attachment at the 63-character Region code the bound allows",
        resource_type: "attachment",
        id: widest.clone(),
        read_uri: format!("/vpc-attachments/{widest}"),
        shape_path: &["VpcAttachment", "Attachment"],
        shape_is_list: false,
    });

    // A Connect attachment takes the edge of the transport it is carried over, and a
    // Connect peer takes the edge of that Connect attachment.
    let (status, created) = create_connect_attachment(
        emulator,
        json!({
            "CoreNetworkId": core_network,
            "EdgeLocation": REGION_VPC,
            "TransportAttachmentId": transport,
            "Options": { "Protocol": "GRE" },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"connect attachment\": expected 200, got {status} with {created}"
    );
    let connect = attachment_id("connect attachment", &created["ConnectAttachment"]);
    held.push(Inventoried {
        name: "a Connect attachment, at the edge of its transport",
        resource_type: "attachment",
        id: connect.clone(),
        read_uri: format!("/connect-attachments/{connect}"),
        shape_path: &["ConnectAttachment", "Attachment"],
        shape_is_list: false,
    });

    let created = create_ok(
        emulator,
        "/connect-peers",
        json!({
            "ConnectAttachmentId": connect,
            "PeerAddress": "203.0.113.1",
            "CoreNetworkAddress": "203.0.113.2",
            "InsideCidrBlocks": ["169.254.10.0/29"],
        }),
    )
    .await;
    let peer = minted(&created, "ConnectPeer", "ConnectPeerId");
    held.push(Inventoried {
        name: "a Connect peer, at the edge of its Connect attachment",
        resource_type: "connect-peer",
        id: peer.clone(),
        read_uri: format!("/connect-peers/{peer}"),
        shape_path: &["ConnectPeer"],
        shape_is_list: false,
    });

    // A site-to-site VPN attachment sits at the Region of the VPN connection it attaches.
    let created = create_ok(
        emulator,
        "/site-to-site-vpn-attachments",
        json!({
            "CoreNetworkId": core_network,
            "VpnConnectionArn": vpn_arn("aws", REGION_VPN, "111122223333", "0123abcd"),
        }),
    )
    .await;
    let vpn = attachment_id(
        "site-to-site VPN attachment",
        &created["SiteToSiteVpnAttachment"],
    );
    held.push(Inventoried {
        name: "a site-to-site VPN attachment, at the Region of its VPN connection",
        resource_type: "attachment",
        id: vpn.clone(),
        read_uri: format!("/site-to-site-vpn-attachments/{vpn}"),
        shape_path: &["SiteToSiteVpnAttachment", "Attachment"],
        shape_is_list: false,
    });

    // A transit gateway peering sits at the Region of the gateway it peers with, and the
    // route table attachment created over it takes that same edge.
    let created = create_ok(
        emulator,
        "/transit-gateway-peerings",
        json!({
            "CoreNetworkId": core_network,
            "TransitGatewayArn":
                format!("arn:aws:ec2:{REGION_PEERING}:111122223333:transit-gateway/tgw-0123abcd"),
        }),
    )
    .await;
    let peering = created["TransitGatewayPeering"]["Peering"]["PeeringId"]
        .as_str()
        .unwrap_or_else(|| panic!("case \"transit gateway peering\": no PeeringId in {created}"))
        .to_string();
    held.push(Inventoried {
        name: "a transit gateway peering, at the Region of its gateway",
        resource_type: "peering",
        id: peering.clone(),
        read_uri: format!("/transit-gateway-peerings/{peering}"),
        shape_path: &["TransitGatewayPeering", "Peering"],
        shape_is_list: false,
    });

    let created = create_ok(
        emulator,
        "/transit-gateway-route-table-attachments",
        json!({
            "PeeringId": peering,
            "TransitGatewayRouteTableArn":
                format!("arn:aws:ec2:{REGION_PEERING}:111122223333:transit-gateway-route-table/tgw-rtb-0123abcd"),
        }),
    )
    .await;
    let route_table = attachment_id(
        "route table attachment",
        &created["TransitGatewayRouteTableAttachment"],
    );
    held.push(Inventoried {
        name: "a transit gateway route table attachment, at the edge of its peering",
        resource_type: "attachment",
        id: route_table.clone(),
        read_uri: format!("/transit-gateway-route-table-attachments/{route_table}"),
        shape_path: &["TransitGatewayRouteTableAttachment", "Attachment"],
        shape_is_list: false,
    });

    // The one kind that spans SEVERAL edges, and so reports a list of them rather than a
    // single Region: once over two edges, once over a list of exactly one.
    for (index, (name, spanned)) in [
        (
            "a Direct Connect gateway attachment spanning two edges",
            vec![REGION_GATEWAY.to_string(), REGION_VPC.to_string()],
        ),
        (
            "a Direct Connect gateway attachment spanning a one-element list of edges",
            vec![REGION_GATEWAY.to_string()],
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let created = create_ok(
            emulator,
            "/direct-connect-gateway-attachments",
            json!({
                "CoreNetworkId": core_network,
                "DirectConnectGatewayArn": gateway_arn(index + 1),
                "EdgeLocations": spanned,
            }),
        )
        .await;
        let gateway = attachment_id(name, &created["DirectConnectGatewayAttachment"]);
        held.push(Inventoried {
            name,
            resource_type: "attachment",
            id: gateway.clone(),
            read_uri: format!("/direct-connect-gateway-attachments/{gateway}"),
            shape_path: &["DirectConnectGatewayAttachment", "Attachment"],
            shape_is_list: false,
        });
    }

    // The kinds held under the global network itself: none of them sits at an edge, and the
    // device carries an AWS location of its own, which is not a Region code.
    let site = emulator.create_site(&network, "regions").await;
    held.push(Inventoried {
        name: "a site, held under the global network",
        resource_type: "site",
        id: site.clone(),
        read_uri: format!("/global-networks/{network}/sites?siteIds={site}"),
        shape_path: &["Sites"],
        shape_is_list: true,
    });

    let created = create_ok(
        emulator,
        &format!("/global-networks/{network}/links"),
        json!({
            "SiteId": site,
            "Bandwidth": { "UploadSpeed": 100, "DownloadSpeed": 200 },
        }),
    )
    .await;
    let link = minted(&created, "Link", "LinkId");
    held.push(Inventoried {
        name: "a link, held under the global network",
        resource_type: "link",
        id: link.clone(),
        read_uri: format!("/global-networks/{network}/links?linkIds={link}"),
        shape_path: &["Links"],
        shape_is_list: true,
    });

    let devices_uri = format!("/global-networks/{network}/devices");
    let created = create_ok(
        emulator,
        &devices_uri,
        json!({
            "SiteId": site,
            "AWSLocation": { "Zone": format!("{REGION_VPC}a"), "SubnetArn": subnet_arn(REGION_VPC, 7) },
        }),
    )
    .await;
    let device = minted(&created, "Device", "DeviceId");
    held.push(Inventoried {
        name: "a device carrying an AWS location of its own",
        resource_type: "device",
        id: device.clone(),
        read_uri: format!("{devices_uri}?deviceIds={device}"),
        shape_path: &["Devices"],
        shape_is_list: true,
    });

    let created = create_ok(emulator, &devices_uri, json!({})).await;
    let other_device = minted(&created, "Device", "DeviceId");
    held.push(Inventoried {
        name: "a device with no AWS location",
        resource_type: "device",
        id: other_device.clone(),
        read_uri: format!("{devices_uri}?deviceIds={other_device}"),
        shape_path: &["Devices"],
        shape_is_list: true,
    });

    let connections_uri = format!("/global-networks/{network}/connections");
    let created = create_ok(
        emulator,
        &connections_uri,
        json!({ "DeviceId": device, "ConnectedDeviceId": other_device }),
    )
    .await;
    let connection = minted(&created, "Connection", "ConnectionId");
    held.push(Inventoried {
        name: "a connection between two devices of the global network",
        resource_type: "connection",
        id: connection.clone(),
        read_uri: format!("{connections_uri}?connectionIds={connection}"),
        shape_path: &["Connections"],
        shape_is_list: true,
    });

    held.sort_by_key(Inventoried::key);
    (network, held)
}

/// The Region each resource's own reader names it at, in the inventory's own order.
async fn expected_regions(emulator: &Emulator, held: &[Inventoried]) -> Vec<Option<String>> {
    let mut regions = Vec::new();
    for resource in held {
        regions.push(resource.expected_region(emulator).await);
    }
    regions
}

#[tokio::test]
async fn every_entry_reports_the_one_region_its_own_reader_names_it_at() {
    let emulator = Emulator::new();
    let (network, held) = inventory(&emulator).await;
    let regions = expected_regions(&emulator, &held).await;
    let (reported_entries, _) = listed(&emulator, &network, "").await;

    let reported: Vec<String> = reported_entries.iter().map(key_of).collect();
    let expected_keys: Vec<String> = held.iter().map(Inventoried::key).collect();
    assert_eq!(
        reported, expected_keys,
        "expected every stored resource once, ordered by type then id, got {reported:?}"
    );

    // At least one row of each form, so a pass cannot come from an inventory that holds
    // only Region-less kinds.
    let with_region = regions.iter().filter(|region| region.is_some()).count();
    assert!(
        with_region >= 5 && with_region < regions.len(),
        "the fixture must hold both kinds of row, got {regions:?}"
    );

    for ((resource, expected), entry) in held.iter().zip(&regions).zip(&reported_entries) {
        let case = resource.name;
        let wanted = expected.as_ref().map_or(Value::Null, |region| json!(region));
        assert_eq!(
            entry["AwsRegion"], wanted,
            "case {case:?}: expected AwsRegion {wanted}, got {}",
            entry["AwsRegion"]
        );
    }

    // The absolute values, anchored against the Regions the fixture placed its resources
    // in: the comparison above holds both sides to the same stored edge, so a record
    // holding some OTHER Region would still pass it. The Region only the list-of-edges
    // kind sits at is absent here, because no entry reports it.
    let wide = widest_region_code();
    let mut named: Vec<&str> = regions.iter().flatten().map(String::as_str).collect();
    named.sort_unstable();
    named.dedup();
    let mut placed = vec![REGION_VPC, REGION_VPN, REGION_PEERING, wide.as_str()];
    placed.sort_unstable();
    assert_eq!(
        named, placed,
        "expected exactly the Regions the fixture placed single-edge resources in, got {regions:?}"
    );
    // And each kind that sits at one edge reports the Region the fixture put IT at, while
    // the kinds that sit at none report nothing.
    for (resource_type, expected) in [
        ("peering", Some(REGION_PEERING)),
        ("connect-peer", Some(REGION_VPC)),
        ("core-network", None),
        ("site", None),
    ] {
        let reported: Vec<Option<String>> = held
            .iter()
            .zip(&regions)
            .filter(|(resource, _)| resource.resource_type == resource_type)
            .map(|(_, region)| region.clone())
            .collect();
        let wanted = vec![expected.map(str::to_string)];
        assert_eq!(
            reported, wanted,
            "case {resource_type:?}: expected {wanted:?}, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn the_aws_region_filter_selects_exactly_the_entries_reporting_that_region() {
    let emulator = Emulator::new();
    let (network, held) = inventory(&emulator).await;
    let regions = expected_regions(&emulator, &held).await;
    let wide = widest_region_code();

    // The keys whose entry reports `region`, in the inventory's order.
    let selecting = |region: &str| -> Vec<String> {
        held.iter()
            .zip(&regions)
            .filter(|(_, reported)| reported.as_deref() == Some(region))
            .map(|(resource, _)| resource.key())
            .collect()
    };
    // The keys of the resources of `resource_type` whose entry reports `region`.
    let selecting_type = |resource_type: &str, region: &str| -> Vec<String> {
        held.iter()
            .zip(&regions)
            .filter(|(resource, reported)| {
                resource.resource_type == resource_type && reported.as_deref() == Some(region)
            })
            .map(|(resource, _)| resource.key())
            .collect()
    };

    let mut rows: Vec<(String, String, Vec<String>)> = Vec::new();
    // Every Region the fixture touches is a row: the three holding single-edge resources,
    // the one where only the list-of-edges kind sits (so no entry reports it), the
    // 63-character bound, and a Region the core network configures no edge in.
    for region in [
        REGION_VPC,
        REGION_VPN,
        REGION_PEERING,
        REGION_GATEWAY,
        wide.as_str(),
        REGION_NOWHERE,
    ] {
        rows.push((
            format!("awsRegion={region}"),
            format!("awsRegion={}", escape(region)),
            selecting(region),
        ));
    }
    // The filter composes with the others: a Region and a type, a Region and the account
    // that holds the inventory, and a Region with a type that sits nowhere near it.
    rows.push((
        format!("attachments in {REGION_VPC}"),
        format!("resourceType=attachment&awsRegion={REGION_VPC}"),
        selecting_type("attachment", REGION_VPC),
    ));
    rows.push((
        format!("the Connect peer in {REGION_VPC}"),
        format!("resourceType=connect-peer&awsRegion={REGION_VPC}"),
        selecting_type("connect-peer", REGION_VPC),
    ));
    rows.push((
        format!("the peering in {REGION_PEERING}"),
        format!("resourceType=peering&awsRegion={REGION_PEERING}"),
        selecting_type("peering", REGION_PEERING),
    ));
    rows.push((
        format!("sites in {REGION_VPC}"),
        format!("resourceType=site&awsRegion={REGION_VPC}"),
        selecting_type("site", REGION_VPC),
    ));
    rows.push((
        format!("the account's resources in {REGION_VPN}"),
        format!(
            "accountId={}&awsRegion={REGION_VPN}",
            engine::emulator_account_id()
        ),
        selecting(REGION_VPN),
    ));

    for (case, query, expected) in rows {
        let selected = keys(&emulator, &network, &query).await;
        assert_eq!(
            selected, expected,
            "case {case:?} ({query:?}): expected {expected:?}, got {selected:?}"
        );
    }
}
