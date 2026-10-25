// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `CreateTransitGatewayConnectPeer` and the
//! `DescribeTransitGatewayConnectPeers` it reads back through, driven over the wire the
//! service actually speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose
//! body carries `Action=CreateTransitGatewayConnectPeer`, answered with XML (README.md /
//! OPERATIONS.md "Wire protocol").
//!
//! The rule under test is that a Connect peer is COMPUTED FROM THE REQUEST AND THE STORE,
//! never echoed or guessed: the peering's interior addresses come from the inside CIDR block
//! the request names ("The first address from the range must be configured on the appliance
//! as the BGP IP address", the transit gateway taking the addresses after it), the transit
//! gateway address from that gateway's own CIDR blocks when the request names none, the
//! transit gateway ASN from the gateway's `AmazonSideAsn`, and the peer is stored `pending`
//! with the clock owing it `available` — all of it readable back through
//! `DescribeTransitGatewayConnectPeers`. Each table row is a distinct input along what that
//! generalises over: which inside range, which address family, how many inside blocks and in
//! which order, whether the transit gateway address and the peer ASN are named, and the ASN
//! at both published bounds. Every published constraint of the request has a refusal row
//! carrying EC2's own error code.

use std::sync::Arc;
use std::time::Duration;

use ec2::ops::types::{
    TransitGatewayConnect, TransitGatewayConnectOptions, TransitGatewayVpcAttachment,
};
use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, PROTOCOL_VALUE_GRE,
    TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE,
    TRANSIT_GATEWAY_ATTACHMENT_STATE_PENDING_ACCEPTANCE, TRANSITION_DELAY,
    TransitGatewayAttachmentView,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, elements, error_code, filters, indexed};
use super::wire::{Param, child, child_text, document, is_long_id, param};

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// The IPv4 CIDR block the transit gateway of the fixture carries, which is where an
/// unnamed `TransitGatewayAddress` is assigned from.
const GATEWAY_IPV4_BLOCK: &str = "10.100.0.0/24";

/// The IPv6 CIDR block the transit gateway of the fixture carries, for the peers whose two
/// GRE outer addresses are IPv6.
const GATEWAY_IPV6_BLOCK: &str = "2001:db8:1::/64";

/// The first host address of [`GATEWAY_IPV4_BLOCK`], which a peer that names no transit
/// gateway address on a gateway nothing else peers over is assigned.
const GATEWAY_IPV4_FIRST: &str = "10.100.0.1";

/// The first host address of [`GATEWAY_IPV6_BLOCK`].
const GATEWAY_IPV6_FIRST: &str = "2001:db8:1::1";

/// The id prefix a Connect peer is minted with.
const PEER_ID_PREFIX: &str = "tgw-connect-peer";

/// The HTTP status EC2 answers `DryRunOperation` with.
const DRY_RUN_STATUS: u16 = 412;

/// Long enough past [`TRANSITION_DELAY`] that a move the clock owes is certainly due.
fn past_the_transition() -> Duration {
    TRANSITION_DELAY + Duration::from_millis(250)
}

/// The emulator, a transit gateway with one CIDR block of each family, and a Connect
/// attachment on it in `attachment_state`.
///
/// `CreateTransitGatewayConnect` is still a scaffold, so the Connect attachment is seeded
/// straight into the store the way the attachment-acceptance suite seeds the attachments its
/// operations act on; the transit gateway itself is created over the wire, so the ASN and the
/// CIDR blocks the peer is derived from are the ones that operation really stored.
struct Fixture {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
    attachment_id: String,
    transit_gateway_asn: String,
}

impl Fixture {
    /// A fixture whose Connect attachment is in `attachment_state`.
    async fn new(attachment_state: &str) -> Self {
        let (router, engine) = super::ec2_query::emulator();
        let (status, body) = call(
            &router,
            REGION,
            "CreateTransitGateway",
            &[
                param("Options.TransitGatewayCidrBlocks.1", GATEWAY_IPV4_BLOCK),
                param("Options.TransitGatewayCidrBlocks.2", GATEWAY_IPV6_BLOCK),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "the fixture's transit gateway must be created, got {status} with body {body}"
        );
        let parsed = document("fixture", &body);
        let gateway = parsed
            .descendants()
            .find(|node| node.is_element() && child(*node, "transitGatewayId").is_some())
            .unwrap_or_else(|| panic!("no transit gateway in {body}"));
        let transit_gateway_id = child_text(gateway, "transitGatewayId")
            .unwrap_or_else(|| panic!("no transitGatewayId in {body}"));
        let transit_gateway_asn = child(gateway, "options")
            .and_then(|options| child_text(options, "amazonSideAsn"))
            .unwrap_or_else(|| panic!("no amazonSideAsn in {body}"));
        let attachment_id = seed_connect_attachment(&engine, &transit_gateway_id, attachment_state);
        Self {
            router,
            engine,
            attachment_id,
            transit_gateway_asn,
        }
    }
}

/// Seed one Connect attachment of `transit_gateway_id` in `state`, and answer the
/// `TransitGatewayAttachmentId` the store minted for it.
fn seed_connect_attachment(
    engine: &engine::InMemoryEngine,
    transit_gateway_id: &str,
    state: &str,
) -> String {
    engine
        .state::<Ec2State>()
        .lock()
        .insert_transit_gateway_attachment(TransitGatewayAttachmentView::Connect(
            TransitGatewayConnect {
                transport_transit_gateway_attachment_id: Some(
                    "tgw-attach-0123456789abcdef0".to_string(),
                ),
                transit_gateway_id: Some(transit_gateway_id.to_string()),
                state: Some(state.to_string()),
                options: Some(TransitGatewayConnectOptions {
                    protocol: Some(PROTOCOL_VALUE_GRE.to_string()),
                }),
                ..Default::default()
            },
        ))
        .expect("the attachment store has room for one Connect attachment")
}

/// One `CreateTransitGatewayConnectPeer` call carrying `params`.
async fn create_peer(router: &MultiServiceRouter, params: &[Param]) -> (u16, String) {
    call(router, REGION, "CreateTransitGatewayConnectPeer", params).await
}

/// One `DescribeTransitGatewayConnectPeers` call carrying `params`.
async fn describe_peers(router: &MultiServiceRouter, params: &[Param]) -> (u16, String) {
    call(
        router,
        REGION,
        "DescribeTransitGatewayConnectPeers",
        params,
    )
    .await
}

/// One reported `TransitGatewayAttachmentBgpConfiguration`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Bgp {
    transit_gateway_asn: String,
    peer_asn: String,
    transit_gateway_address: String,
    peer_address: String,
    bgp_status: String,
}

/// One reported `TransitGatewayConnectPeer`, read out of the XML by the document rather than
/// by string surgery — `peerAddress` and `transitGatewayAddress` appear both on the
/// configuration and inside every BGP configuration.
#[derive(Debug, Clone)]
struct Reported {
    peer_id: String,
    attachment_id: String,
    state: String,
    creation_time: String,
    transit_gateway_address: String,
    peer_address: String,
    inside_cidr_blocks: Vec<String>,
    protocol: String,
    bgp: Vec<Bgp>,
    tags: Vec<(String, String)>,
}

/// The `<item>` children of `parent`.
fn items<'a, 'i>(parent: roxmltree::Node<'a, 'i>) -> Vec<roxmltree::Node<'a, 'i>> {
    parent
        .children()
        .filter(|node| node.is_element() && node.tag_name().name() == "item")
        .collect()
}

/// The one Connect peer a response carries, or a panic naming the case.
fn reported(case: &str, body: &str) -> Reported {
    let parsed = document(case, body);
    let peer = parsed
        .descendants()
        .find(|node| {
            node.is_element() && child(*node, "transitGatewayConnectPeerId").is_some()
        })
        .unwrap_or_else(|| panic!("case {case:?}: no Connect peer in {body}"));
    let configuration = child(peer, "connectPeerConfiguration")
        .unwrap_or_else(|| panic!("case {case:?}: no connectPeerConfiguration in {body}"));
    let bgp = child(configuration, "bgpConfigurations")
        .map(|set| {
            items(set)
                .into_iter()
                .map(|item| Bgp {
                    transit_gateway_asn: child_text(item, "transitGatewayAsn")
                        .unwrap_or_default(),
                    peer_asn: child_text(item, "peerAsn").unwrap_or_default(),
                    transit_gateway_address: child_text(item, "transitGatewayAddress")
                        .unwrap_or_default(),
                    peer_address: child_text(item, "peerAddress").unwrap_or_default(),
                    bgp_status: child_text(item, "bgpStatus").unwrap_or_default(),
                })
                .collect()
        })
        .unwrap_or_default();
    Reported {
        peer_id: child_text(peer, "transitGatewayConnectPeerId").unwrap_or_default(),
        attachment_id: child_text(peer, "transitGatewayAttachmentId").unwrap_or_default(),
        state: child_text(peer, "state").unwrap_or_default(),
        creation_time: child_text(peer, "creationTime").unwrap_or_default(),
        transit_gateway_address: child_text(configuration, "transitGatewayAddress")
            .unwrap_or_default(),
        peer_address: child_text(configuration, "peerAddress").unwrap_or_default(),
        inside_cidr_blocks: child(configuration, "insideCidrBlocks")
            .map(|set| {
                items(set)
                    .into_iter()
                    .map(|item| item.text().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default(),
        protocol: child_text(configuration, "protocol").unwrap_or_default(),
        bgp,
        tags: child(peer, "tagSet")
            .map(|set| {
                items(set)
                    .into_iter()
                    .map(|item| {
                        (
                            child_text(item, "key").unwrap_or_default(),
                            child_text(item, "value").unwrap_or_default(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default(),
    }
}

/// One request this suite makes of a fixture's Connect attachment.
#[derive(Debug, Clone)]
struct Request<'a> {
    inside: &'a [&'a str],
    peer_address: &'a str,
    transit_gateway_address: Option<&'a str>,
    peer_asn: Option<&'a str>,
    tags: &'a [(&'a str, &'a str)],
}

impl Request<'_> {
    /// The form fields of this request against `attachment_id`.
    fn params(&self, attachment_id: &str) -> Vec<Param> {
        let mut params = vec![
            param("TransitGatewayAttachmentId", attachment_id),
            param("PeerAddress", self.peer_address),
        ];
        params.extend(indexed("InsideCidrBlocks", self.inside));
        if let Some(address) = self.transit_gateway_address {
            params.push(param("TransitGatewayAddress", address));
        }
        if let Some(peer_asn) = self.peer_asn {
            params.push(param("BgpOptions.PeerAsn", peer_asn));
        }
        if !self.tags.is_empty() {
            params.push(param(
                "TagSpecification.1.ResourceType",
                ec2::state::RESOURCE_TYPE_TRANSIT_GATEWAY_CONNECT_PEER,
            ));
            for (position, (key, value)) in self.tags.iter().enumerate() {
                let index = position + 1;
                params.push(param(&format!("TagSpecification.1.Tag.{index}.Key"), key));
                params.push(param(
                    &format!("TagSpecification.1.Tag.{index}.Value"),
                    value,
                ));
            }
        }
        params
    }
}

/// A request with nothing but the two required members.
fn request<'a>(inside: &'a [&'a str], peer_address: &'a str) -> Request<'a> {
    Request {
        inside,
        peer_address,
        transit_gateway_address: None,
        peer_asn: None,
        tags: &[],
    }
}

/// What one created peer must report.
#[derive(Debug, Clone)]
struct Expected<'a> {
    inside_cidr_blocks: &'a [&'a str],
    appliance_bgp_address: &'a str,
    gateway_bgp_addresses: &'a [&'a str],
    peer_asn: &'a str,
    transit_gateway_address: &'a str,
}

/// Hold one reported peer to [`Expected`], whichever response it came out of.
fn assert_reported(case: &str, source: &str, body: &str, expected: &Expected<'_>, peer: &Request<'_>, fixture: &Fixture) {
    let got = reported(case, body);
    assert_eq!(
        got.attachment_id, fixture.attachment_id,
        "case {case:?} ({source}): expected the Connect attachment {attachment:?}, got \
         {actual:?}",
        attachment = fixture.attachment_id,
        actual = got.attachment_id,
    );
    assert_eq!(
        got.inside_cidr_blocks, expected.inside_cidr_blocks,
        "case {case:?} ({source}): expected inside CIDR blocks {blocks:?}, got {actual:?}",
        blocks = expected.inside_cidr_blocks,
        actual = got.inside_cidr_blocks,
    );
    assert_eq!(
        got.peer_address, peer.peer_address,
        "case {case:?} ({source}): expected the requested peer address {address:?}, got \
         {actual:?}",
        address = peer.peer_address,
        actual = got.peer_address,
    );
    assert_eq!(
        got.transit_gateway_address, expected.transit_gateway_address,
        "case {case:?} ({source}): expected transit gateway address {address:?}, got \
         {actual:?}",
        address = expected.transit_gateway_address,
        actual = got.transit_gateway_address,
    );
    assert_eq!(
        got.protocol, PROTOCOL_VALUE_GRE,
        "case {case:?} ({source}): expected the GRE tunnel protocol, got {actual:?}",
        actual = got.protocol,
    );
    assert!(
        !got.creation_time.is_empty(),
        "case {case:?} ({source}): expected a creation time, got none in {body}"
    );
    let want: Vec<Bgp> = expected
        .gateway_bgp_addresses
        .iter()
        .map(|address| Bgp {
            transit_gateway_asn: fixture.transit_gateway_asn.clone(),
            peer_asn: expected.peer_asn.to_string(),
            transit_gateway_address: (*address).to_string(),
            peer_address: expected.appliance_bgp_address.to_string(),
            bgp_status: "down".to_string(),
        })
        .collect();
    assert_eq!(
        got.bgp, want,
        "case {case:?} ({source}): expected the BGP configurations {want:?}, got {actual:?}",
        actual = got.bgp,
    );
    let want_tags: Vec<(String, String)> = peer
        .tags
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect();
    assert_eq!(
        got.tags, want_tags,
        "case {case:?} ({source}): expected the tags {want_tags:?}, got {actual:?}",
        actual = got.tags,
    );
}

#[tokio::test]
async fn a_connect_peer_is_derived_from_the_inside_range_and_reads_back() {
    // Each row is a different input along what the rule generalises over: which inside
    // range, which family the GRE outer addresses are of, how many inside blocks and in
    // which order the request names them, whether the transit gateway address is named, and
    // the peer ASN (absent, and at each published bound).
    let cases: Vec<(&str, Request<'_>, Expected<'_>)> = vec![
        (
            "one IPv4 inside range, nothing else named",
            request(&["169.254.6.0/29"], "192.0.2.10"),
            Expected {
                inside_cidr_blocks: &["169.254.6.0/29"],
                appliance_bgp_address: "169.254.6.1",
                gateway_bgp_addresses: &["169.254.6.2", "169.254.6.3"],
                peer_asn: "64512",
                transit_gateway_address: GATEWAY_IPV4_FIRST,
            },
        ),
        (
            "an inside range further into 169.254.0.0/16",
            request(&["169.254.100.8/29"], "198.51.100.7"),
            Expected {
                inside_cidr_blocks: &["169.254.100.8/29"],
                appliance_bgp_address: "169.254.100.9",
                gateway_bgp_addresses: &["169.254.100.10", "169.254.100.11"],
                peer_asn: "64512",
                transit_gateway_address: GATEWAY_IPV4_FIRST,
            },
        ),
        (
            "the last inside range of 169.254.0.0/16",
            request(&["169.254.255.248/29"], "198.51.100.8"),
            Expected {
                inside_cidr_blocks: &["169.254.255.248/29"],
                appliance_bgp_address: "169.254.255.249",
                gateway_bgp_addresses: &["169.254.255.250", "169.254.255.251"],
                peer_asn: "64512",
                transit_gateway_address: GATEWAY_IPV4_FIRST,
            },
        ),
        (
            // One range has several spellings; the range is what the rule is about, so an
            // equivalent spelling of it is accepted and reported canonically.
            "the same ranges, the IPv6 one spelled with its zeroes written out",
            request(&["169.254.9.0/29", "fd00:0000:0000::/125"], "192.0.2.13"),
            Expected {
                inside_cidr_blocks: &["169.254.9.0/29", "fd00::/125"],
                appliance_bgp_address: "169.254.9.1",
                gateway_bgp_addresses: &["169.254.9.2", "169.254.9.3"],
                peer_asn: "64512",
                transit_gateway_address: GATEWAY_IPV4_FIRST,
            },
        ),
        (
            "an IPv6 inside range beside the IPv4 one",
            request(&["169.254.7.0/29", "fd00::/125"], "192.0.2.11"),
            Expected {
                inside_cidr_blocks: &["169.254.7.0/29", "fd00::/125"],
                appliance_bgp_address: "169.254.7.1",
                gateway_bgp_addresses: &["169.254.7.2", "169.254.7.3"],
                peer_asn: "64512",
                transit_gateway_address: GATEWAY_IPV4_FIRST,
            },
        ),
        (
            "the same two inside ranges, named IPv6 first",
            request(&["fd00:abcd::8/125", "169.254.8.0/29"], "192.0.2.12"),
            Expected {
                inside_cidr_blocks: &["169.254.8.0/29", "fd00:abcd::8/125"],
                appliance_bgp_address: "169.254.8.1",
                gateway_bgp_addresses: &["169.254.8.2", "169.254.8.3"],
                peer_asn: "64512",
                transit_gateway_address: GATEWAY_IPV4_FIRST,
            },
        ),
        (
            "a named transit gateway address out of the gateway's IPv4 block",
            Request {
                transit_gateway_address: Some("10.100.0.7"),
                ..request(&["169.254.9.0/29"], "192.0.2.13")
            },
            Expected {
                inside_cidr_blocks: &["169.254.9.0/29"],
                appliance_bgp_address: "169.254.9.1",
                gateway_bgp_addresses: &["169.254.9.2", "169.254.9.3"],
                peer_asn: "64512",
                transit_gateway_address: "10.100.0.7",
            },
        ),
        (
            "IPv6 GRE outer addresses, the transit gateway's assigned",
            request(&["169.254.10.0/29"], "2001:db8:9::1"),
            Expected {
                inside_cidr_blocks: &["169.254.10.0/29"],
                appliance_bgp_address: "169.254.10.1",
                gateway_bgp_addresses: &["169.254.10.2", "169.254.10.3"],
                peer_asn: "64512",
                transit_gateway_address: GATEWAY_IPV6_FIRST,
            },
        ),
        (
            "IPv6 GRE outer addresses, the transit gateway's named",
            Request {
                transit_gateway_address: Some("2001:db8:1::5"),
                ..request(&["169.254.11.0/29"], "2001:db8:9::2")
            },
            Expected {
                inside_cidr_blocks: &["169.254.11.0/29"],
                appliance_bgp_address: "169.254.11.1",
                gateway_bgp_addresses: &["169.254.11.2", "169.254.11.3"],
                peer_asn: "64512",
                transit_gateway_address: "2001:db8:1::5",
            },
        ),
        (
            "a peer ASN at the lowest number a BGP speaker can have",
            Request {
                peer_asn: Some("1"),
                ..request(&["169.254.12.0/29"], "192.0.2.14")
            },
            Expected {
                inside_cidr_blocks: &["169.254.12.0/29"],
                appliance_bgp_address: "169.254.12.1",
                gateway_bgp_addresses: &["169.254.12.2", "169.254.12.3"],
                peer_asn: "1",
                transit_gateway_address: GATEWAY_IPV4_FIRST,
            },
        ),
        (
            "a peer ASN at the highest number a BGP speaker can have",
            Request {
                peer_asn: Some("4294967294"),
                ..request(&["169.254.13.0/29"], "192.0.2.15")
            },
            Expected {
                inside_cidr_blocks: &["169.254.13.0/29"],
                appliance_bgp_address: "169.254.13.1",
                gateway_bgp_addresses: &["169.254.13.2", "169.254.13.3"],
                peer_asn: "4294967294",
                transit_gateway_address: GATEWAY_IPV4_FIRST,
            },
        ),
        (
            "a 16-bit private-use peer ASN and two tags",
            Request {
                peer_asn: Some("65001"),
                tags: &[("Name", "appliance"), ("Team", "network")],
                ..request(&["169.254.14.0/29"], "192.0.2.16")
            },
            Expected {
                inside_cidr_blocks: &["169.254.14.0/29"],
                appliance_bgp_address: "169.254.14.1",
                gateway_bgp_addresses: &["169.254.14.2", "169.254.14.3"],
                peer_asn: "65001",
                transit_gateway_address: GATEWAY_IPV4_FIRST,
            },
        ),
    ];

    for (case, peer, expected) in cases {
        let fixture = Fixture::new(TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE).await;
        let (status, body) = create_peer(&fixture.router, &peer.params(&fixture.attachment_id)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateTransitGatewayConnectPeer, got \
             {status} with body {body}"
        );
        let created = reported(case, &body);
        assert!(
            is_long_id(PEER_ID_PREFIX, &created.peer_id),
            "case {case:?}: expected a minted {PEER_ID_PREFIX}- id, got {actual:?}",
            actual = created.peer_id,
        );
        assert_reported(case, "create", &body, &expected, &peer, &fixture);

        // The round trip: the describe reports the record the create stored.
        let (status, read) = describe_peers(
            &fixture.router,
            &indexed("TransitGatewayConnectPeerIds", &[created.peer_id.as_str()]),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeTransitGatewayConnectPeers, got \
             {status} with body {read}"
        );
        assert_reported(case, "describe", &read, &expected, &peer, &fixture);
        let described = reported(case, &read);
        assert_eq!(
            described.peer_id, created.peer_id,
            "case {case:?}: the describe must report the created peer {expected_id:?}, got \
             {actual:?}",
            expected_id = created.peer_id,
            actual = described.peer_id,
        );
        assert_eq!(
            described.creation_time, created.creation_time,
            "case {case:?}: the creation time must be stable across reads, got \
             {actual:?} after {first:?}",
            actual = described.creation_time,
            first = created.creation_time,
        );
    }
}

#[tokio::test]
async fn a_created_peer_is_pending_until_the_clock_owes_it_available() {
    let case = "a freshly created Connect peer";
    let fixture = Fixture::new(TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE).await;
    let peer = request(&["169.254.20.0/29"], "192.0.2.20");
    let (status, body) = create_peer(&fixture.router, &peer.params(&fixture.attachment_id)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    let created = reported(case, &body);
    assert_eq!(
        created.state, "pending",
        "case {case:?}: expected the non-terminal state 'pending', got {actual:?}",
        actual = created.state,
    );

    // The move is OWED, not slept for: a caller acting straight after the create still sees
    // the transient state.
    let (_, read) = describe_peers(
        &fixture.router,
        &indexed("TransitGatewayConnectPeerIds", &[created.peer_id.as_str()]),
    )
    .await;
    assert_eq!(
        reported(case, &read).state,
        "pending",
        "case {case:?}: expected 'pending' before the transition falls due, got body {read}"
    );

    tokio::time::sleep(past_the_transition()).await;
    let (_, read) = describe_peers(
        &fixture.router,
        &indexed("TransitGatewayConnectPeerIds", &[created.peer_id.as_str()]),
    )
    .await;
    assert_eq!(
        reported(case, &read).state,
        "available",
        "case {case:?}: expected 'available' once the transition fell due, got body {read}"
    );
    assert_eq!(
        fixture
            .engine
            .state::<Ec2State>()
            .lock()
            .transit_gateway_connect_peers
            .get(&created.peer_id)
            .and_then(|record| record.view.state.clone())
            .unwrap_or_default(),
        "available",
        "case {case:?}: the stored peer must be the one the describe reported"
    );
}

#[tokio::test]
async fn a_request_outside_the_published_rules_is_refused() {
    struct Refusal<'a> {
        label: &'a str,
        attachment_state: &'a str,
        /// The attachment id the request names; `None` names the seeded Connect attachment.
        attachment_id: Option<&'a str>,
        peer: Request<'a>,
        status: u16,
        code: &'a str,
    }

    let cases = vec![
        Refusal {
            label: "an attachment id that is not an id of this kind",
            attachment_state: TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE,
            attachment_id: Some("tgw-attach-nope"),
            peer: request(&["169.254.6.0/29"], "192.0.2.10"),
            status: 400,
            code: "InvalidTransitGatewayAttachmentID.Malformed",
        },
        Refusal {
            label: "an id of another EC2 resource",
            attachment_state: TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE,
            attachment_id: Some("vpc-0123456789abcdef0"),
            peer: request(&["169.254.6.0/29"], "192.0.2.10"),
            status: 400,
            code: "InvalidTransitGatewayAttachmentID.Malformed",
        },
        Refusal {
            label: "an empty attachment id",
            attachment_state: TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE,
            attachment_id: Some(""),
            peer: request(&["169.254.6.0/29"], "192.0.2.10"),
            status: 400,
            code: "InvalidTransitGatewayAttachmentID.Malformed",
        },
        Refusal {
            label: "a well-formed attachment id the account holds no attachment for",
            attachment_state: TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE,
            attachment_id: Some("tgw-attach-0fedcba9876543210"),
            peer: request(&["169.254.6.0/29"], "192.0.2.10"),
            status: 400,
            code: "InvalidTransitGatewayAttachmentID.NotFound",
        },
        Refusal {
            label: "a Connect attachment that is not in service yet",
            attachment_state: TRANSIT_GATEWAY_ATTACHMENT_STATE_PENDING_ACCEPTANCE,
            attachment_id: None,
            peer: request(&["169.254.6.0/29"], "192.0.2.10"),
            status: 400,
            code: "IncorrectState",
        },
        Refusal {
            label: "an inside block larger than the published /29",
            attachment_state: TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE,
            attachment_id: None,
            peer: request(&["169.254.6.0/28"], "192.0.2.10"),
            status: 400,
            code: "InvalidParameterValue",
        },
        Refusal {
            label: "an inside block smaller than the published /29",
            attachment_state: TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE,
            attachment_id: None,
            peer: request(&["169.254.6.0/30"], "192.0.2.10"),
            status: 400,
            code: "InvalidParameterValue",
        },
        Refusal {
            label: "an inside block outside the published 169.254.0.0/16 range",
            attachment_state: TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE,
            attachment_id: None,
            peer: request(&["10.0.0.0/29"], "192.0.2.10"),
            status: 400,
            code: "InvalidParameterValue",
        },
        Refusal {
            label: "an inside block named by an address inside it rather than in canonical \
                    form",
            attachment_state: TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE,
            attachment_id: None,
            peer: request(&["169.254.6.1/29"], "192.0.2.10"),
            status: 400,
            code: "InvalidParameterValue",
        },
        Refusal {
            label: "two IPv4 inside blocks",
            attachment_state: TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE,
            attachment_id: None,
            peer: request(&["169.254.6.0/29", "169.254.7.0/29"], "192.0.2.10"),
            status: 400,
            code: "InvalidParameterValue",
        },
        Refusal {
            label: "an IPv6 inside block and no IPv4 one",
            attachment_state: TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE,
            attachment_id: None,
            peer: request(&["fd00::/125"], "192.0.2.10"),
            status: 400,
            code: "MissingParameter",
        },
        Refusal {
            label: "an IPv6 inside block of another size than the published /125",
            attachment_state: TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE,
            attachment_id: None,
            peer: request(&["169.254.6.0/29", "fd00::/126"], "192.0.2.10"),
            status: 400,
            code: "InvalidParameterValue",
        },
        Refusal {
            label: "an IPv6 inside block outside the published fd00::/8 range",
            attachment_state: TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE,
            attachment_id: None,
            peer: request(&["169.254.6.0/29", "2001:db8::/125"], "192.0.2.10"),
            status: 400,
            code: "InvalidParameterValue",
        },
        Refusal {
            label: "an IPv6 inside block named by an address inside it rather than the range",
            attachment_state: TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE,
            attachment_id: None,
            peer: request(&["169.254.6.0/29", "fd00::1/125"], "192.0.2.10"),
            status: 400,
            code: "InvalidParameterValue",
        },
        Refusal {
            label: "an inside block that is not a CIDR block at all",
            attachment_state: TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE,
            attachment_id: None,
            peer: request(&["one sixty nine"], "192.0.2.10"),
            status: 400,
            code: "InvalidParameterValue",
        },
        Refusal {
            label: "a peer address that is not an IP address",
            attachment_state: TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE,
            attachment_id: None,
            peer: request(&["169.254.6.0/29"], "the appliance"),
            status: 400,
            code: "InvalidParameterValue",
        },
        Refusal {
            label: "a transit gateway address that is not an IP address",
            attachment_state: TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE,
            attachment_id: None,
            peer: Request {
                transit_gateway_address: Some("the gateway"),
                ..request(&["169.254.6.0/29"], "192.0.2.10")
            },
            status: 400,
            code: "InvalidParameterValue",
        },
        Refusal {
            label: "a transit gateway address outside every CIDR block of the gateway",
            attachment_state: TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE,
            attachment_id: None,
            peer: Request {
                transit_gateway_address: Some("10.200.0.5"),
                ..request(&["169.254.6.0/29"], "192.0.2.10")
            },
            status: 400,
            code: "InvalidParameterValue",
        },
        Refusal {
            label: "two GRE outer addresses of different address families",
            attachment_state: TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE,
            attachment_id: None,
            peer: Request {
                transit_gateway_address: Some("10.100.0.5"),
                ..request(&["169.254.6.0/29"], "2001:db8:9::1")
            },
            status: 400,
            code: "InvalidParameterCombination",
        },
        Refusal {
            label: "a peer ASN below the lowest number a BGP speaker can have",
            attachment_state: TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE,
            attachment_id: None,
            peer: Request {
                peer_asn: Some("0"),
                ..request(&["169.254.6.0/29"], "192.0.2.10")
            },
            status: 400,
            code: "InvalidParameterValue",
        },
        Refusal {
            label: "a peer ASN above the highest number a BGP speaker can have",
            attachment_state: TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE,
            attachment_id: None,
            peer: Request {
                peer_asn: Some("4294967295"),
                ..request(&["169.254.6.0/29"], "192.0.2.10")
            },
            status: 400,
            code: "InvalidParameterValue",
        },
    ];

    for case in cases {
        let fixture = Fixture::new(case.attachment_state).await;
        let attachment_id = case.attachment_id.unwrap_or(&fixture.attachment_id);
        let (status, body) = create_peer(&fixture.router, &case.peer.params(attachment_id)).await;
        let label = case.label;
        assert_eq!(
            status, case.status,
            "case {label:?}: expected HTTP {expected}, got {status} with body {body}",
            expected = case.status,
        );
        assert_eq!(
            error_code(&body),
            case.code,
            "case {label:?}: expected the error code {expected:?}, got body {body}",
            expected = case.code,
        );
        // A refused request writes nothing.
        assert!(
            fixture
                .engine
                .state::<Ec2State>()
                .lock()
                .transit_gateway_connect_peers
                .is_empty(),
            "case {label:?}: a refused request must store no Connect peer"
        );
    }
}

#[tokio::test]
async fn a_tag_specification_for_another_resource_type_is_refused() {
    let case = "a tag specification naming a resource type this action does not tag";
    let fixture = Fixture::new(TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE).await;
    let mut params = request(&["169.254.6.0/29"], "192.0.2.10").params(&fixture.attachment_id);
    params.push(param(
        "TagSpecification.1.ResourceType",
        ec2::state::RESOURCE_TYPE_TRANSIT_GATEWAY,
    ));
    params.push(param("TagSpecification.1.Tag.1.Key", "Name"));
    params.push(param("TagSpecification.1.Tag.1.Value", "appliance"));
    let (status, body) = create_peer(&fixture.router, &params).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidParameterValue",
        "case {case:?}: expected InvalidParameterValue, got body {body}"
    );
}

#[tokio::test]
async fn an_attachment_of_another_kind_is_no_connect_attachment() {
    let case = "a VPC attachment named as the Connect attachment";
    let (router, engine) = super::ec2_query::emulator();
    let attachment_id = engine
        .state::<Ec2State>()
        .lock()
        .insert_transit_gateway_attachment(TransitGatewayAttachmentView::Vpc(
            TransitGatewayVpcAttachment {
                transit_gateway_id: Some("tgw-0123456789abcdef0".to_string()),
                vpc_id: Some("vpc-0123456789abcdef0".to_string()),
                state: Some(TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE.to_string()),
                ..Default::default()
            },
        ))
        .expect("the attachment store has room for one VPC attachment");
    let (status, body) = create_peer(
        &router,
        &request(&["169.254.6.0/29"], "192.0.2.10").params(&attachment_id),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidTransitGatewayAttachmentID.NotFound",
        "case {case:?}: expected the attachment to be unknown to this operation, got body \
         {body}"
    );
}

#[tokio::test]
async fn an_assigned_transit_gateway_address_is_the_first_the_gateway_has_free() {
    // "the first AVAILABLE IP address from the transit gateway CIDR block": each peer that
    // names no address of its own gets the next host address of the gateway's block, never
    // one another peer of that gateway already holds.
    let case = "three peers assigned addresses out of one gateway block";
    let fixture = Fixture::new(TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE).await;
    let expected = ["10.100.0.1", "10.100.0.2", "10.100.0.3"];
    for (index, want) in expected.iter().enumerate() {
        let inside = format!("169.254.{block}.0/29", block = 40 + index);
        let inside_blocks = [inside.as_str()];
        let peer = request(&inside_blocks, "192.0.2.60");
        let (status, body) =
            create_peer(&fixture.router, &peer.params(&fixture.attachment_id)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 for peer {index}, got {status} with body {body}"
        );
        let got = reported(case, &body).transit_gateway_address;
        assert_eq!(
            &got, want,
            "case {case:?}: expected peer {index} on {want:?}, got {got:?}"
        );
    }
}

#[tokio::test]
async fn a_second_peer_cannot_claim_an_allocation_the_first_holds() {
    // The interior range and the transit gateway's GRE outer address are allocations of the
    // gateway: the assignment path skips what is in use, and a request that names its own is
    // held to the same rule.
    let case = "a second peer on the same Connect attachment";
    let fixture = Fixture::new(TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE).await;
    let first = request(&["169.254.50.0/29"], "192.0.2.50");
    let (status, body) = create_peer(&fixture.router, &first.params(&fixture.attachment_id)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the first peer must be created, got {status} with body {body}"
    );
    let held_address = reported(case, &body).transit_gateway_address;

    let same_range = request(&["169.254.50.0/29"], "192.0.2.51");
    let (status, body) =
        create_peer(&fixture.router, &same_range.params(&fixture.attachment_id)).await;
    assert_eq!(
        status, 400,
        "case {case:?}: an inside range already peered over must be refused, got {status} \
         with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidParameterValue",
        "case {case:?}: expected InvalidParameterValue for the taken range, got body {body}"
    );

    let same_address = Request {
        transit_gateway_address: Some(held_address.as_str()),
        ..request(&["169.254.51.0/29"], "192.0.2.52")
    };
    let (status, body) =
        create_peer(&fixture.router, &same_address.params(&fixture.attachment_id)).await;
    assert_eq!(
        status, 400,
        "case {case:?}: a transit gateway address already in use must be refused, got \
         {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidParameterValue",
        "case {case:?}: expected InvalidParameterValue for the taken address {held_address:?}, \
         got body {body}"
    );

    // Only the first peer was stored.
    let (_, body) = describe_peers(&fixture.router, &[]).await;
    assert_eq!(
        elements(&body, "transitGatewayConnectPeerId").len(),
        1,
        "case {case:?}: only the first peer may be stored, got body {body}"
    );
}

#[tokio::test]
async fn a_dry_run_reports_the_outcome_and_stores_nothing() {
    let case = "DryRun on a request that would have succeeded";
    let fixture = Fixture::new(TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE).await;
    let mut params = request(&["169.254.6.0/29"], "192.0.2.10").params(&fixture.attachment_id);
    params.push(param("DryRun", "true"));
    let (status, body) = create_peer(&fixture.router, &params).await;
    assert_eq!(
        status, DRY_RUN_STATUS,
        "case {case:?}: expected HTTP {DRY_RUN_STATUS}, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "DryRunOperation",
        "case {case:?}: expected DryRunOperation, got body {body}"
    );
    assert!(
        fixture
            .engine
            .state::<Ec2State>()
            .lock()
            .transit_gateway_connect_peers
            .is_empty(),
        "case {case:?}: a dry run must store no Connect peer"
    );
}

/// Create `count` Connect peers on the fixture's attachment, each in its own inside range,
/// and answer their ids in the order they were created.
async fn create_peers(fixture: &Fixture, count: u8) -> Vec<String> {
    let mut ids = Vec::new();
    for index in 0..count {
        let inside = format!("169.254.{block}.0/29", block = 30 + u16::from(index));
        let peer_address = format!("192.0.2.{address}", address = 100 + u16::from(index));
        let inside_blocks = [inside.as_str()];
        let peer = request(&inside_blocks, peer_address.as_str());
        let (status, body) =
            create_peer(&fixture.router, &peer.params(&fixture.attachment_id)).await;
        assert_eq!(
            status, 200,
            "the {index}th Connect peer must be created, got {status} with body {body}"
        );
        ids.push(reported("seeding", &body).peer_id);
    }
    ids
}

#[tokio::test]
async fn the_describe_narrows_by_id_and_by_every_documented_filter() {
    let fixture = Fixture::new(TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE).await;
    let ids = create_peers(&fixture, 3).await;

    // Every peer, when the request narrows nothing.
    let (status, body) = describe_peers(&fixture.router, &[]).await;
    assert_eq!(
        status, 200,
        "expected HTTP 200 from an unnarrowed describe, got {status} with body {body}"
    );
    let mut reported_ids = elements(&body, "transitGatewayConnectPeerId");
    reported_ids.sort();
    let mut expected = ids.clone();
    expected.sort();
    assert_eq!(
        reported_ids, expected,
        "an unnarrowed describe must report every stored peer, got {reported_ids:?}"
    );

    // One id selects one peer; each documented filter selects what it names.
    struct Narrowing<'a> {
        label: &'a str,
        params: Vec<Param>,
        expected: Vec<String>,
    }
    let narrowings = vec![
        Narrowing {
            label: "one id",
            params: indexed("TransitGatewayConnectPeerIds", &[ids[1].as_str()]),
            expected: vec![ids[1].clone()],
        },
        Narrowing {
            label: "two ids",
            params: indexed(
                "TransitGatewayConnectPeerIds",
                &[ids[0].as_str(), ids[2].as_str()],
            ),
            expected: vec![ids[0].clone(), ids[2].clone()],
        },
        Narrowing {
            label: "the transit-gateway-connect-peer-id filter",
            params: filters(&[("transit-gateway-connect-peer-id", &[ids[2].as_str()])]),
            expected: vec![ids[2].clone()],
        },
        Narrowing {
            label: "the transit-gateway-attachment-id filter",
            params: filters(&[(
                "transit-gateway-attachment-id",
                &[fixture.attachment_id.as_str()],
            )]),
            expected: ids.clone(),
        },
        Narrowing {
            label: "the state filter on the state every peer is in",
            params: filters(&[("state", &["pending"])]),
            expected: ids.clone(),
        },
        Narrowing {
            label: "the state filter on a state no peer is in",
            params: filters(&[("state", &["deleted"])]),
            expected: Vec::new(),
        },
        Narrowing {
            label: "the transit-gateway-attachment-id filter on another attachment",
            params: filters(&[(
                "transit-gateway-attachment-id",
                &["tgw-attach-0fedcba9876543210"],
            )]),
            expected: Vec::new(),
        },
    ];
    for narrowing in narrowings {
        let label = narrowing.label;
        let (status, body) = describe_peers(&fixture.router, &narrowing.params).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected HTTP 200, got {status} with body {body}"
        );
        let mut got = elements(&body, "transitGatewayConnectPeerId");
        got.sort();
        let mut want = narrowing.expected.clone();
        want.sort();
        assert_eq!(
            got, want,
            "case {label:?}: expected the peers {want:?}, got {got:?}"
        );
    }
}

#[tokio::test]
async fn the_describe_pages_in_id_order() {
    let case = "seven peers, five to a page";
    let fixture = Fixture::new(TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE).await;
    let mut all = create_peers(&fixture, 7).await;
    all.sort();

    let (status, body) =
        describe_peers(&fixture.router, &[param("MaxResults", "5")]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    let first_page = elements(&body, "transitGatewayConnectPeerId");
    assert_eq!(
        first_page,
        all[..5].to_vec(),
        "case {case:?}: expected the first five ids in id order, got {first_page:?}"
    );
    let token = super::ec2_query::element(&body, "nextToken")
        .unwrap_or_else(|| panic!("case {case:?}: expected a nextToken in {body}"));

    let (status, body) = describe_peers(
        &fixture.router,
        &[param("MaxResults", "5"), param("NextToken", &token)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 for the second page, got {status} with body {body}"
    );
    let second_page = elements(&body, "transitGatewayConnectPeerId");
    assert_eq!(
        second_page,
        all[5..].to_vec(),
        "case {case:?}: expected the remaining ids, got {second_page:?}"
    );
    assert!(
        super::ec2_query::element(&body, "nextToken").is_none(),
        "case {case:?}: the last page must carry no token, got body {body}"
    );
}

#[tokio::test]
async fn the_describe_refuses_what_it_cannot_answer() {
    struct Case<'a> {
        label: &'a str,
        params: Vec<Param>,
        status: u16,
        code: &'a str,
    }
    let fixture = Fixture::new(TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE).await;
    let ids = create_peers(&fixture, 1).await;
    let cases = vec![
        Case {
            label: "an id that is not a Connect peer id",
            params: indexed("TransitGatewayConnectPeerIds", &["tgw-connect-peer-nope"]),
            status: 400,
            code: "InvalidTransitGatewayConnectPeerID.Malformed",
        },
        Case {
            label: "a well-formed id the account holds no peer for",
            params: indexed(
                "TransitGatewayConnectPeerIds",
                &["tgw-connect-peer-0fedcba9876543210"],
            ),
            status: 400,
            code: "InvalidTransitGatewayConnectPeerID.NotFound",
        },
        Case {
            label: "a filter this operation does not document",
            params: filters(&[("tag:Name", &["appliance"])]),
            status: 400,
            code: "InvalidParameterValue",
        },
        Case {
            label: "a pagination token this service did not mint",
            params: vec![param("NextToken", "not-a-token")],
            status: 400,
            code: "InvalidPaginationToken",
        },
        Case {
            label: "a dry run",
            params: vec![param("DryRun", "true")],
            status: DRY_RUN_STATUS,
            code: "DryRunOperation",
        },
    ];
    for case in cases {
        let label = case.label;
        let (status, body) = describe_peers(&fixture.router, &case.params).await;
        assert_eq!(
            status, case.status,
            "case {label:?}: expected HTTP {expected}, got {status} with body {body}",
            expected = case.status,
        );
        assert_eq!(
            error_code(&body),
            case.code,
            "case {label:?}: expected the error code {expected:?}, got body {body}",
            expected = case.code,
        );
    }
    // The refusals above changed nothing: the stored peer is still reported.
    let (_, body) = describe_peers(&fixture.router, &[]).await;
    assert_eq!(
        elements(&body, "transitGatewayConnectPeerId"),
        ids,
        "a refused describe must leave the store as it was, got body {body}"
    );
}
