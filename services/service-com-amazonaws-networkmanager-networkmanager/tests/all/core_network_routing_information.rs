// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `ListCoreNetworkRoutingInformation`, driven over HTTP through the
//! real router (the harness lives in `crate::global_network_lifecycle`).
//!
//! The rule under test: the routing information a core network reports for ONE of its
//! segments at ONE of its edge locations is the set of routes that segment holds there,
//! computed from what this service stores — the static routes the policy in force declares
//! for the segment (each prefix of a `create-route` action, reached through each destination
//! it names, and reached through nothing where the destination names no attachment of that
//! segment at that edge) and the prefixes the segment's BGP peerings at that edge carry
//! (each inside CIDR block of a Connect peer, at the appliance's address and across the
//! appliance's AS) — reported in one stable order, narrowed by every match the request
//! carries, and paged by `MaxResults`/`NextToken`. A segment or an edge location the policy
//! in force does not declare addresses no route table, which is a field failure; an unknown
//! core network is not found.
//!
//! The tables below span that class: rows differing in the kind of route (a static route to
//! an attachment, a blackholed static route, a route learned over a BGP peering), in the
//! segment (one present at every edge, one restricted to a single edge, one holding no route
//! at all), in the edge location, in how many prefixes one action declares and how many
//! destinations it names, in whether a destination resolves to an attachment of that segment
//! at that edge, in whether the peering carries a peer ASN, in the address family of the
//! prefix, and — for the filters — in which attribute is matched, on how many values, and in
//! how many filters combine.
//!
//! The policy document's own key names (`segment-actions`, `action`, `create-route`,
//! `destination-cidr-blocks`, `destinations`, `blackhole`, `attachment-policies`,
//! `association-method`, `tag-value-of-key`) are the published Cloud WAN core network policy
//! schema's, the same source the other suites build documents from.

use std::collections::BTreeSet;
use std::time::SystemTime;

use serde_json::{json, Value};

use service_com_amazonaws_networkmanager_networkmanager::state;

use crate::attachment_lifecycle::{
    attachment_id, create_connect_attachment, create_vpc_attachment, subnet_arn, vpc_arn,
};
use crate::connect_peer_lifecycle::create_connect_peer;
use crate::core_network_lifecycle::create_core_network;
use crate::global_network_lifecycle::{error_code, Emulator, PAST_SETTLE};
use crate::resource_tagging::tag;

/// The two edge locations the subject's core network configures.
const WEST: &str = "us-west-2";
/// See [`WEST`].
const CENTRAL: &str = "eu-central-1";

/// The segments the subject's policy declares: one present at every edge, one restricted to
/// the western edge, and one no attachment joins and no action declares a route for.
const SHARED: &str = "shared";
/// See [`SHARED`].
const PROD: &str = "prod";
/// See [`SHARED`].
const QUIET: &str = "quiet";

/// The appliance addresses and ASN the subject's two BGP peerings carry.
const WEST_PEER_ADDRESS: &str = "203.0.113.1";
/// See [`WEST_PEER_ADDRESS`].
const CENTRAL_PEER_ADDRESS: &str = "198.51.100.7";
/// See [`WEST_PEER_ADDRESS`].
const WEST_PEER_ASN: i64 = 65000;

/// The inside IP addresses the western peering runs BGP over: an IPv4 and an IPv6 block, so
/// the prefixes a peering carries are exercised in both address families.
const WEST_INSIDE_CIDR_BLOCKS: [&str; 2] = ["169.254.10.0/29", "fd00::/125"];
/// See [`WEST_INSIDE_CIDR_BLOCKS`].
const CENTRAL_INSIDE_CIDR_BLOCKS: [&str; 1] = ["169.254.20.0/29"];

/// An attachment id of the shape the model fixes (`^attachment-([0-9a-f]{8,17})$`,
/// `smithy_query shape=AttachmentId`) that no attachment of the subject carries: what a
/// `create-route` destination naming nothing stored looks like.
const ABSENT_ATTACHMENT: &str = "attachment-0123456789abcdef0";

/// The policy document the subject's core network holds, carrying `routes` as its
/// `segment-actions`.
///
/// Every attachment joins the segment its own `segment` tag names, which is the
/// `association-method: tag` form of an attachment-policy rule's action, so one document
/// serves attachments of several segments.
fn policy_document(routes: &Value) -> String {
    json!({
        "version": "2021.12",
        "core-network-configuration": {
            "asn-ranges": ["64512-64555"],
            "edge-locations": [
                { "location": WEST, "asn": 64512 },
                { "location": CENTRAL, "asn": 64513 },
            ],
        },
        "segments": [
            { "name": SHARED },
            { "name": PROD, "edge-locations": [WEST] },
            { "name": QUIET },
        ],
        "attachment-policies": [{
            "rule-number": 100,
            "conditions": [{ "type": "any" }],
            "action": { "association-method": "tag", "tag-value-of-key": "segment" },
        }],
        "segment-actions": routes,
    })
    .to_string()
}

/// The subject: a core network with two edges and three segments, the attachments of two of
/// those segments, and the BGP peering each of its two Connect attachments carries.
struct Subject {
    core_network: String,
    /// The VPC attachment of [`SHARED`] at [`WEST`], and the VPC it attaches.
    west_vpc: String,
    west_vpc_arn: String,
    /// The VPC attachment of [`SHARED`] at [`CENTRAL`], and the VPC it attaches.
    central_vpc: String,
    central_vpc_arn: String,
    /// The VPC attachment of [`PROD`] at [`WEST`], and the VPC it attaches.
    prod_vpc: String,
    prod_vpc_arn: String,
    /// The Connect attachments the two peerings are carried over.
    west_connect: String,
    central_connect: String,
    /// The Connect peers: the western one peers across [`WEST_PEER_ASN`], the central one
    /// names no peer ASN at all.
    west_peer: String,
    central_peer: String,
}

/// `CreateVpcAttachment` of the VPC `index` in `region`, joining the segment `segment`
/// names, asserting the create succeeded.
async fn vpc_attachment(
    emulator: &Emulator,
    core_network: &str,
    region: &str,
    index: usize,
    segment: &str,
) -> (String, String) {
    let arn = vpc_arn(region, index);
    let (status, created) = create_vpc_attachment(
        emulator,
        json!({
            "CoreNetworkId": core_network,
            "VpcArn": arn,
            "SubnetArns": [subnet_arn(region, index)],
            "Tags": [tag("segment", segment)],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {segment:?} at {region}: CreateVpcAttachment expected 200, got {status} with {created}"
    );
    (
        attachment_id(segment, &created["VpcAttachment"]),
        arn,
    )
}

/// `CreateConnectAttachment` over `transport` at `region`, joining the segment `segment`
/// names, asserting the create succeeded.
async fn connect_attachment(
    emulator: &Emulator,
    core_network: &str,
    region: &str,
    transport: &str,
    segment: &str,
) -> String {
    let (status, created) = create_connect_attachment(
        emulator,
        json!({
            "CoreNetworkId": core_network,
            "EdgeLocation": region,
            "TransportAttachmentId": transport,
            "Options": { "Protocol": "GRE" },
            "Tags": [tag("segment", segment)],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {segment:?} at {region}: CreateConnectAttachment expected 200, got {status} with {created}"
    );
    attachment_id(segment, &created["ConnectAttachment"])
}

/// `CreateConnectPeer` on `attachment`, asserting the create succeeded and answering the
/// minted peer id.
async fn connect_peer(
    emulator: &Emulator,
    attachment: &str,
    address: &str,
    inside_cidr_blocks: &[&str],
    peer_asn: Option<i64>,
) -> String {
    let mut body = json!({
        "ConnectAttachmentId": attachment,
        "PeerAddress": address,
        "CoreNetworkAddress": "203.0.113.254",
        "InsideCidrBlocks": inside_cidr_blocks,
    });
    if let Some(asn) = peer_asn {
        body["BgpOptions"] = json!({ "PeerAsn": asn });
    }
    let (status, created) = create_connect_peer(emulator, body).await;
    assert_eq!(
        status, 200,
        "case {address:?}: CreateConnectPeer expected 200, got {status} with {created}"
    );
    created["ConnectPeer"]["ConnectPeerId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {address:?}: no ConnectPeer.ConnectPeerId in {created}"))
        .to_string()
}

/// Put `document` in force on `core_network`, as its next executed policy version.
///
/// A policy document is only put IN FORCE by the create of a core network and by
/// `ExecuteCoreNetworkChangeSet`, which is still a scaffold stub with its own issue
/// (`networkmanager ExecuteCoreNetworkChangeSet`), so the one thing this suite cannot drive
/// through an operation is a SECOND policy version taking effect — and a `create-route`
/// action names the attachments it reaches by id, which exist only after the core network
/// does. Everything else here is created and read back through an operation.
fn put_policy_in_force(engine: &engine::InMemoryEngine, core_network: &str, document: String) {
    let store = state::service_state(engine).expect("the networkmanager store must be reachable");
    let mut guard = store.lock();
    let record = guard
        .core_networks
        .get_mut(core_network)
        .unwrap_or_else(|| panic!("the subject core network {core_network} must be stored"));
    record.topology = state::policy_topology(&document);
    record.add_policy_version(document, None, None, SystemTime::now(), true);
}

/// Build the subject, then put in force a policy whose `create-route` actions name the
/// attachments it has just created.
async fn subject(emulator: &Emulator, engine: &engine::InMemoryEngine) -> Subject {
    let global_network = emulator.create_global_network("routing information").await;
    let (status, created) = create_core_network(
        emulator,
        json!({
            "GlobalNetworkId": global_network,
            "Description": "routing information",
            "PolicyDocument": policy_document(&json!([])),
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateCoreNetwork expected 200, got {status} with {created}"
    );
    let core_network = created["CoreNetwork"]["CoreNetworkId"]
        .as_str()
        .unwrap_or_else(|| panic!("no CoreNetworkId in {created}"))
        .to_string();

    let (west_vpc, west_vpc_arn) = vpc_attachment(emulator, &core_network, WEST, 1, SHARED).await;
    let (central_vpc, central_vpc_arn) =
        vpc_attachment(emulator, &core_network, CENTRAL, 2, SHARED).await;
    let (prod_vpc, prod_vpc_arn) = vpc_attachment(emulator, &core_network, WEST, 3, PROD).await;
    let west_connect = connect_attachment(emulator, &core_network, WEST, &west_vpc, SHARED).await;
    let central_connect =
        connect_attachment(emulator, &core_network, CENTRAL, &central_vpc, SHARED).await;
    let west_peer = connect_peer(
        emulator,
        &west_connect,
        WEST_PEER_ADDRESS,
        &WEST_INSIDE_CIDR_BLOCKS,
        Some(WEST_PEER_ASN),
    )
    .await;
    let central_peer = connect_peer(
        emulator,
        &central_connect,
        CENTRAL_PEER_ADDRESS,
        &CENTRAL_INSIDE_CIDR_BLOCKS,
        None,
    )
    .await;

    // The static routes: two prefixes through one attachment, one prefix through an
    // attachment of another edge AND through nothing (`blackhole`), one prefix of the
    // western-only segment, one prefix whose destination names no stored attachment, and one
    // action acting on a segment the document never declares.
    put_policy_in_force(
        engine,
        &core_network,
        policy_document(&json!([
            {
                "action": "create-route",
                "segment": SHARED,
                "destination-cidr-blocks": ["10.0.0.0/16", "10.1.0.0/16"],
                "destinations": [west_vpc],
            },
            {
                "action": "create-route",
                "segment": SHARED,
                "destination-cidr-blocks": ["10.2.0.0/16"],
                "destinations": [central_vpc, "blackhole"],
            },
            {
                "action": "create-route",
                "segment": PROD,
                "destination-cidr-blocks": ["172.16.0.0/16"],
                "destinations": [prod_vpc],
            },
            {
                "action": "create-route",
                "segment": SHARED,
                "destination-cidr-blocks": ["192.168.0.0/24"],
                "destinations": [ABSENT_ATTACHMENT],
            },
            {
                "action": "create-route",
                "segment": "undeclared",
                "destination-cidr-blocks": ["203.0.113.0/24"],
                "destinations": ["blackhole"],
            },
        ])),
    );

    // Every attachment and every peering owes the clock its `CREATING` -> `AVAILABLE`
    // transition, and a route is held only through a next hop that is available, so the
    // listing is read once that transition has fallen due.
    std::thread::sleep(PAST_SETTLE);

    Subject {
        core_network,
        west_vpc,
        west_vpc_arn,
        central_vpc,
        central_vpc_arn,
        prod_vpc,
        prod_vpc_arn,
        west_connect,
        central_connect,
        west_peer,
        central_peer,
    }
}

/// The URI `ListCoreNetworkRoutingInformation` is addressed at, carrying `query` (already
/// escaped) when it is not empty.
fn list_uri(core_network: &str, query: &str) -> String {
    let base = format!("/core-networks/{core_network}/core-network-routing-information");
    if query.is_empty() {
        base
    } else {
        format!("{base}?{query}")
    }
}

/// `ListCoreNetworkRoutingInformation` for one segment at one edge location, plus whatever
/// `extra` members the request carries.
async fn list(
    emulator: &Emulator,
    core_network: &str,
    segment: &str,
    edge: &str,
    extra: &Value,
    query: &str,
) -> (u16, Value) {
    let mut body = json!({ "SegmentName": segment, "EdgeLocation": edge });
    if let Some(members) = extra.as_object() {
        for (name, value) in members {
            body[name] = value.clone();
        }
    }
    emulator
        .post(&list_uri(core_network, query), body)
        .await
}

/// The entries one page reports, in the order returned.
fn entries(case: &str, body: &Value) -> Vec<Value> {
    body["CoreNetworkRoutingInformation"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected a CoreNetworkRoutingInformation array in {body}")
        })
        .clone()
}

/// A successful listing, asserting the call answered 200.
async fn page(
    emulator: &Emulator,
    case: &str,
    core_network: &str,
    segment: &str,
    edge: &str,
    extra: &Value,
    query: &str,
) -> Value {
    let (status, body) = list(emulator, core_network, segment, edge, extra, query).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListCoreNetworkRoutingInformation expected 200, got {status} with {body}"
    );
    body
}

/// The next hop of a static route reached through an attachment: the attachment, and the
/// resource it attaches as that resource's own ARN names it.
fn attachment_hop(attachment: &str, resource_arn: &str, segment: &str, edge: &str) -> Value {
    json!({
        "CoreNetworkAttachmentId": attachment,
        "ResourceId": state::arn_resource_id(resource_arn),
        "ResourceType": state::arn_resource_kind(resource_arn)
            .expect("the ARN of an attached resource names its type"),
        "SegmentName": segment,
        "EdgeLocation": edge,
    })
}

/// The next hop of a route learned over a BGP peering: the appliance's address, the Connect
/// attachment carrying the peering, and the peering itself.
fn peer_hop(address: &str, attachment: &str, peer: &str, segment: &str, edge: &str) -> Value {
    json!({
        "IpAddress": address,
        "CoreNetworkAttachmentId": attachment,
        "ResourceId": peer,
        "ResourceType": "connect-peer",
        "SegmentName": segment,
        "EdgeLocation": edge,
    })
}

/// One reported route: its prefix, its next hop (`Value::Null` for a blackholed route, which
/// is reached through nothing) and its AS path.
fn route(prefix: &str, next_hop: Value, as_path: &[&str]) -> Value {
    let mut entry = json!({ "Prefix": prefix, "AsPath": as_path });
    if !next_hop.is_null() {
        entry["NextHop"] = next_hop;
    }
    entry
}

/// The routes [`SHARED`] holds at [`WEST`], in the order the listing reports them: ascending
/// prefix, and ascending destination for the one prefix reached several ways.
fn west_shared(subject: &Subject) -> Vec<Value> {
    let hop = || attachment_hop(&subject.west_vpc, &subject.west_vpc_arn, SHARED, WEST);
    let peer = || {
        peer_hop(
            WEST_PEER_ADDRESS,
            &subject.west_connect,
            &subject.west_peer,
            SHARED,
            WEST,
        )
    };
    vec![
        // The two prefixes one action declares through the attachment of this segment at
        // this edge. A static route is originated by the core network, so its AS path is
        // empty.
        route("10.0.0.0/16", hop(), &[]),
        route("10.1.0.0/16", hop(), &[]),
        // The action naming the attachment of the OTHER edge and `blackhole`: only the
        // blackholed route is held here, reached through nothing.
        route("10.2.0.0/16", Value::Null, &[]),
        // The prefixes the peering carries, across the appliance's AS.
        route(
            WEST_INSIDE_CIDR_BLOCKS[0],
            peer(),
            &[&WEST_PEER_ASN.to_string()],
        ),
        route(
            WEST_INSIDE_CIDR_BLOCKS[1],
            peer(),
            &[&WEST_PEER_ASN.to_string()],
        ),
    ]
}

/// One row of the listing table: a segment, an edge location and the routes the segment
/// holds there.
struct ListingCase {
    name: &'static str,
    segment: &'static str,
    edge: &'static str,
    expected: Vec<Value>,
}

/// The table: one row per (segment, edge) the subject can be asked about.
fn listing_cases(subject: &Subject) -> Vec<ListingCase> {
    vec![
        ListingCase {
            name: "the segment at every edge, at the western one",
            segment: SHARED,
            edge: WEST,
            expected: west_shared(subject),
        },
        ListingCase {
            name: "the same segment at the central edge",
            segment: SHARED,
            edge: CENTRAL,
            expected: vec![
                // One prefix reached two ways at this edge: through the attachment this
                // edge holds, and through nothing.
                route(
                    "10.2.0.0/16",
                    attachment_hop(
                        &subject.central_vpc,
                        &subject.central_vpc_arn,
                        SHARED,
                        CENTRAL,
                    ),
                    &[],
                ),
                route("10.2.0.0/16", Value::Null, &[]),
                // The central peering names no peer ASN, so the route it carries traverses
                // no AS this service knows and its path is empty.
                route(
                    CENTRAL_INSIDE_CIDR_BLOCKS[0],
                    peer_hop(
                        CENTRAL_PEER_ADDRESS,
                        &subject.central_connect,
                        &subject.central_peer,
                        SHARED,
                        CENTRAL,
                    ),
                    &[],
                ),
            ],
        },
        ListingCase {
            name: "the segment restricted to the western edge, at that edge",
            segment: PROD,
            edge: WEST,
            expected: vec![route(
                "172.16.0.0/16",
                attachment_hop(&subject.prod_vpc, &subject.prod_vpc_arn, PROD, WEST),
                &[],
            )],
        },
        ListingCase {
            name: "the same segment at an edge it is not present at",
            segment: PROD,
            edge: CENTRAL,
            expected: Vec::new(),
        },
        ListingCase {
            name: "a declared segment no route and no attachment names",
            segment: QUIET,
            edge: WEST,
            expected: Vec::new(),
        },
        ListingCase {
            name: "that same segment at the other edge",
            segment: QUIET,
            edge: CENTRAL,
            expected: Vec::new(),
        },
    ]
}

#[tokio::test]
async fn every_route_a_segment_holds_at_an_edge_is_reported_from_the_stored_policy_and_peerings() {
    let (emulator, engine) = Emulator::with_engine();
    let subject = subject(&emulator, &engine).await;

    for case in listing_cases(&subject) {
        let name = case.name;
        let body = page(
            &emulator,
            name,
            &subject.core_network,
            case.segment,
            case.edge,
            &Value::Null,
            "",
        )
        .await;
        let reported = entries(name, &body);
        assert_eq!(
            reported, case.expected,
            "case {name:?}: expected {:?}, got {reported:?}",
            case.expected
        );
        assert!(
            body["NextToken"].is_null(),
            "case {name:?}: a listing that fits one page carries no token, got {body}"
        );

        // Every route is reported once, whatever it is reached through: one prefix reached
        // two ways is two entries and one prefix declared twice is one, which the table
        // above fixes in order. The order itself is the stable key order the page token
        // names an offset into, asserted by the token walk below.
        let prefixes: Vec<String> = reported
            .iter()
            .map(|entry| {
                entry["Prefix"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {name:?}: no Prefix in {entry}"))
                    .to_string()
            })
            .collect();
        assert_eq!(
            prefixes.len(),
            case.expected.len(),
            "case {name:?}: expected one entry per route held, got {prefixes:?}"
        );

        // Reading reports the same answer twice: nothing about this listing writes.
        let again = entries(
            name,
            &page(
                &emulator,
                name,
                &subject.core_network,
                case.segment,
                case.edge,
                &Value::Null,
                "",
            )
            .await,
        );
        assert_eq!(
            again, reported,
            "case {name:?}: a second read must report the same routes, got {again:?}"
        );
    }
}

/// One row of the filter table: the extra request members, and which of the routes
/// [`west_shared`] holds they select (by index).
struct FilterCase {
    name: &'static str,
    extra: Value,
    selects: Vec<usize>,
}

/// The table: one row per attribute a request may match on, and per way the matches combine.
fn filter_cases(subject: &Subject) -> Vec<FilterCase> {
    let asn = WEST_PEER_ASN.to_string();
    vec![
        FilterCase {
            name: "no match at all",
            extra: json!({}),
            selects: vec![0, 1, 2, 3, 4],
        },
        FilterCase {
            name: "the attachment a static route is reached through",
            extra: json!({ "NextHopFilters": { "core-network-attachment-id": [subject.west_vpc] } }),
            selects: vec![0, 1],
        },
        FilterCase {
            name: "two attachments at once",
            extra: json!({
                "NextHopFilters": {
                    "core-network-attachment-id": [subject.west_vpc, subject.west_connect],
                },
            }),
            selects: vec![0, 1, 3, 4],
        },
        FilterCase {
            name: "the appliance address a peering is reached at",
            extra: json!({ "NextHopFilters": { "ip-address": [WEST_PEER_ADDRESS] } }),
            selects: vec![3, 4],
        },
        FilterCase {
            name: "the type of the resource the hop reaches",
            extra: json!({ "NextHopFilters": { "resource-type": ["vpc"] } }),
            selects: vec![0, 1],
        },
        FilterCase {
            name: "the peering as the resource the hop reaches",
            extra: json!({ "NextHopFilters": { "resource-type": ["connect-peer"] } }),
            selects: vec![3, 4],
        },
        FilterCase {
            name: "the id of the resource the hop reaches",
            extra: json!({
                "NextHopFilters": { "resource-id": [state::arn_resource_id(&subject.west_vpc_arn)] },
            }),
            selects: vec![0, 1],
        },
        FilterCase {
            name: "the segment of the hop, which the blackholed route has no hop to carry",
            extra: json!({ "NextHopFilters": { "segment-name": [SHARED] } }),
            selects: vec![0, 1, 3, 4],
        },
        FilterCase {
            name: "the edge location of the hop",
            extra: json!({ "NextHopFilters": { "edge-location": [WEST] } }),
            selects: vec![0, 1, 3, 4],
        },
        FilterCase {
            name: "two filters that both hold",
            extra: json!({
                "NextHopFilters": {
                    "resource-type": ["vpc"],
                    "core-network-attachment-id": [subject.west_vpc],
                },
            }),
            selects: vec![0, 1],
        },
        FilterCase {
            name: "two filters no one route satisfies together",
            extra: json!({
                "NextHopFilters": {
                    "resource-type": ["vpc"],
                    "ip-address": [WEST_PEER_ADDRESS],
                },
            }),
            selects: Vec::new(),
        },
        FilterCase {
            name: "a value no hop carries",
            extra: json!({ "NextHopFilters": { "ip-address": ["192.0.2.99"] } }),
            selects: Vec::new(),
        },
        FilterCase {
            name: "a filter naming no value at all",
            extra: json!({ "NextHopFilters": { "resource-type": [] } }),
            selects: Vec::new(),
        },
        FilterCase {
            name: "the exact AS path a learned route traverses",
            extra: json!({ "ExactAsPathMatches": [asn] }),
            selects: vec![3, 4],
        },
        FilterCase {
            name: "the empty AS path a statically originated route carries",
            extra: json!({ "ExactAsPathMatches": [""] }),
            selects: vec![0, 1, 2],
        },
        FilterCase {
            name: "both AS paths at once",
            extra: json!({ "ExactAsPathMatches": ["", asn] }),
            selects: vec![0, 1, 2, 3, 4],
        },
        FilterCase {
            name: "an AS path no route traverses",
            extra: json!({ "ExactAsPathMatches": ["64999"] }),
            selects: Vec::new(),
        },
        // Nothing stored assigns a route a local preference, a MED or a community, so a
        // request matching on one of them selects no route rather than being ignored.
        FilterCase {
            name: "a local preference, which no stored route carries",
            extra: json!({ "LocalPreferenceMatches": ["100"] }),
            selects: Vec::new(),
        },
        FilterCase {
            name: "a MED, which no stored route carries",
            extra: json!({ "MedMatches": ["0"] }),
            selects: Vec::new(),
        },
        FilterCase {
            name: "a community, which no stored route carries",
            extra: json!({ "CommunityMatches": ["64512:100"] }),
            selects: Vec::new(),
        },
        FilterCase {
            name: "a next-hop filter and an AS path together",
            extra: json!({
                "NextHopFilters": { "resource-type": ["connect-peer"] },
                "ExactAsPathMatches": [asn],
            }),
            selects: vec![3, 4],
        },
    ]
}

#[tokio::test]
async fn every_match_narrows_the_listing_to_the_routes_whose_attribute_it_names() {
    let (emulator, engine) = Emulator::with_engine();
    let subject = subject(&emulator, &engine).await;
    let held = west_shared(&subject);

    for case in filter_cases(&subject) {
        let name = case.name;
        let body = page(
            &emulator,
            name,
            &subject.core_network,
            SHARED,
            WEST,
            &case.extra,
            "",
        )
        .await;
        let expected: Vec<Value> = case
            .selects
            .iter()
            .map(|index| held[*index].clone())
            .collect();
        let reported = entries(name, &body);
        assert_eq!(
            reported, expected,
            "case {name:?}: expected {expected:?}, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn a_page_reports_at_most_max_results_routes_and_a_token_resumes_after_the_last_of_them() {
    let (emulator, engine) = Emulator::with_engine();
    let subject = subject(&emulator, &engine).await;
    let held = west_shared(&subject);

    // The largest page a caller may ask for is 500 (`MaxResults` is modelled `range 1..500`,
    // `smithy_query query="operation" operation="ListCoreNetworkRoutingInformation"`), so
    // every page size from one route at a time up to the whole listing is walked.
    for size in [1, 2, 3, held.len(), 500] {
        let name = format!("{size} route(s) per page");
        let mut walked: Vec<Value> = Vec::new();
        let mut tokens: BTreeSet<String> = BTreeSet::new();
        let mut query = format!("maxResults={size}");
        loop {
            let body = page(
                &emulator,
                &name,
                &subject.core_network,
                SHARED,
                WEST,
                &Value::Null,
                &query,
            )
            .await;
            let reported = entries(&name, &body);
            assert!(
                reported.len() <= size,
                "case {name:?}: a page must hold at most {size} routes, got {}",
                reported.len()
            );
            walked.extend(reported);
            match body["NextToken"].as_str() {
                Some(token) => {
                    assert!(
                        tokens.insert(token.to_string()),
                        "case {name:?}: the token {token} was issued twice, so the walk does not advance"
                    );
                    query = format!("maxResults={size}&nextToken={token}");
                }
                None => break,
            }
        }
        assert_eq!(
            walked, held,
            "case {name:?}: a token walk must report every route exactly once, in order, got {walked:?}"
        );
    }
}

/// One row of the refusal table: a request that addresses no route table, and the status and
/// error code it is answered with.
struct RefusalCase {
    name: &'static str,
    core_network: Option<&'static str>,
    segment: &'static str,
    edge: &'static str,
    extra: Value,
    query: &'static str,
    status: u16,
    code: &'static str,
}

/// The table: one row per way a request can fail to address a route table of the subject.
fn refusal_cases() -> Vec<RefusalCase> {
    vec![
        RefusalCase {
            name: "a core network id nothing is stored under",
            core_network: Some("core-network-0123456789abcdef0"),
            segment: SHARED,
            edge: WEST,
            extra: Value::Null,
            query: "",
            status: 404,
            code: "ResourceNotFoundException",
        },
        RefusalCase {
            name: "a segment the policy in force does not declare",
            core_network: None,
            segment: "undeclared",
            edge: WEST,
            extra: Value::Null,
            query: "",
            status: 400,
            code: "ValidationException",
        },
        RefusalCase {
            name: "an edge location the policy in force configures no edge in",
            core_network: None,
            segment: SHARED,
            edge: "ap-southeast-4",
            extra: Value::Null,
            query: "",
            status: 400,
            code: "ValidationException",
        },
        RefusalCase {
            name: "a next-hop filter naming no part of a next hop",
            core_network: None,
            segment: SHARED,
            edge: WEST,
            extra: json!({ "NextHopFilters": { "attachment" : ["anything"] } }),
            query: "",
            status: 400,
            code: "ValidationException",
        },
        RefusalCase {
            name: "a token this service did not mint",
            core_network: None,
            segment: SHARED,
            edge: WEST,
            extra: Value::Null,
            query: "nextToken=not-a-token-this-service-minted",
            status: 400,
            code: "ValidationException",
        },
    ]
}

#[tokio::test]
async fn a_request_that_addresses_no_route_table_is_refused_on_the_member_that_carried_it() {
    let (emulator, engine) = Emulator::with_engine();
    let subject = subject(&emulator, &engine).await;

    for case in refusal_cases() {
        let name = case.name;
        let core_network = case.core_network.unwrap_or(&subject.core_network);
        let (status, body) = list(
            &emulator,
            core_network,
            case.segment,
            case.edge,
            &case.extra,
            case.query,
        )
        .await;
        assert_eq!(
            status, case.status,
            "case {name:?}: expected {}, got {status} with {body}",
            case.status
        );
        assert_eq!(
            error_code(&body),
            case.code,
            "case {name:?}: expected {}, got {body}",
            case.code
        );
    }
}

/// How many values one match filter may carry: the bound `state::MAX_FILTER_IDS` fixes,
/// which is the maximum every filter of every reporting operation of this service allows —
/// what keeps the work of selecting a listing from being the product of the routes held and
/// a list the caller can make as long as it likes.
const MAX_FILTER_VALUES: usize = 10;

/// `count` values for a match filter, each a value nothing stored carries, so what the
/// request is answered depends on the LENGTH of the list and not on what is in it.
fn filter_values(count: usize) -> Vec<String> {
    (0..count).map(|index| format!("value-{index}")).collect()
}

/// The request member `member` carrying `values`, spelled the way the wire names it: one
/// row of the match-filter table below.
fn match_filter(member: &str, values: Vec<String>) -> Value {
    match member {
        "localPreferenceMatches" => json!({ "LocalPreferenceMatches": values }),
        "medMatches" => json!({ "MedMatches": values }),
        "exactAsPathMatches" => json!({ "ExactAsPathMatches": values }),
        "communityMatches" => json!({ "CommunityMatches": values }),
        "nextHopFilters" => json!({ "NextHopFilters": { "ip-address": values } }),
        other => panic!("case {other:?}: the table names no such match filter"),
    }
}

#[tokio::test]
async fn a_match_filter_carrying_more_values_than_the_bound_allows_is_refused_on_its_member() {
    let (emulator, engine) = Emulator::with_engine();
    let subject = subject(&emulator, &engine).await;

    // One row per list a request matches on: the four BGP-attribute lists, and one value
    // list of the next-hop filter map, each named by the member a refusal reports.
    for member in [
        "localPreferenceMatches",
        "medMatches",
        "exactAsPathMatches",
        "communityMatches",
        "nextHopFilters",
    ] {
        let at_bound = match_filter(member, filter_values(MAX_FILTER_VALUES));
        let (status, body) = list(
            &emulator,
            &subject.core_network,
            SHARED,
            WEST,
            &at_bound,
            "",
        )
        .await;
        assert_eq!(
            status, 200,
            "case {member:?}: {MAX_FILTER_VALUES} values is at the bound and accepted, got {status} with {body}"
        );
        assert_eq!(
            entries(member, &body),
            Vec::<Value>::new(),
            "case {member:?}: values nothing carries select no route, got {body}"
        );

        let over = match_filter(member, filter_values(MAX_FILTER_VALUES + 1));
        let (status, refused) =
            list(&emulator, &subject.core_network, SHARED, WEST, &over, "").await;
        assert_eq!(
            status, 400,
            "case {member:?}: one value past the bound is refused, got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {member:?}: expected ValidationException, got {refused}"
        );
        assert_eq!(
            refused["Fields"][0]["Name"],
            json!(member),
            "case {member:?}: the refusal must name the member that carried the list, got {refused}"
        );
    }
}
