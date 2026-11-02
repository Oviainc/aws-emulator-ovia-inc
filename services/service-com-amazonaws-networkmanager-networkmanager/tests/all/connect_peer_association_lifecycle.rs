// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of the Connect peer association slice (`AssociateConnectPeer`,
//! `GetConnectPeerAssociations`), driven over HTTP.
//!
//! Every call goes through the real `MultiServiceRouter`, so the restJson1 routes, the
//! label/query bindings and the error envelopes are exercised as a client hits them.
//! Nothing reaches into `crate::state`: an association is only ever read back through
//! `GetConnectPeerAssociations` (and the telemetry it registers through
//! `GetNetworkTelemetry`).
//!
//! The rule under test: an association ties ONE stored Connect peer of a core network of
//! the addressed global network to ONE device of that global network, optionally over a
//! link that device is already associated with; the peer is the association's identity, so
//! a repeat of the same request reports the stored association and a request naming
//! another device for the same peer conflicts. `GetConnectPeerAssociations` then reports
//! one association per stored tie of the addressed global network, narrowed to the
//! `connectPeerIds` the query names — an id naming nothing selects none — paged by
//! `MaxResults`/`NextToken` in the stable peer-id order, and reading changes nothing a
//! later caller sees.
//!
//! The table below spans that class: rows differing in the Connect attachment (and so the
//! edge location) the peer hangs off, in whether the device sits at a site, in whether a
//! link is named, and in how the link member is spelled (named, omitted, empty).
//!
//! The harness is the one `global_network_lifecycle` defines — one shared engine, this
//! service, the production dispatch.

use serde_json::{json, Value};

use crate::attachment_lifecycle::{
    attachment_id, create_connect_attachment, create_vpc_attachment, policy_document, subnet_arn,
    vpc_arn,
};
use crate::core_network_lifecycle::create_core_network;
use crate::device_lifecycle::create_device_ok;
use crate::global_network_lifecycle::{error_code, Emulator, PAST_SETTLE};
use crate::link_lifecycle::{create_link_ok, plain_link};

/// The edge locations the fixture's core network configures, one Connect attachment each,
/// so the rows below span more than one edge.
pub(crate) const EDGES: [&str; 2] = ["us-west-2", "eu-central-1"];

/// The largest page a caller may ask for: `MaxResults` is modelled `range 1..500`
/// (`smithy_query query="operation" operation="GetConnectPeerAssociations"`).
const MAX_PAGE: usize = 500;

/// `AssociateConnectPeer` on `network`, with the request body as given.
pub(crate) async fn associate(emulator: &Emulator, network: &str, body: Value) -> (u16, Value) {
    emulator
        .post(
            &format!("/global-networks/{network}/connect-peer-associations"),
            body,
        )
        .await
}

/// `GetConnectPeerAssociations` on `network` with `query` appended to the URI.
pub(crate) async fn get_associations(emulator: &Emulator, network: &str, query: &str) -> (u16, Value) {
    let uri = if query.is_empty() {
        format!("/global-networks/{network}/connect-peer-associations")
    } else {
        format!("/global-networks/{network}/connect-peer-associations?{query}")
    };
    emulator.get(&uri).await
}

/// `GetConnectPeerAssociations` that must succeed, answering the whole response.
pub(crate) async fn get_associations_ok(emulator: &Emulator, case: &str, network: &str, query: &str) -> Value {
    let (status, body) = get_associations(emulator, network, query).await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetConnectPeerAssociations expected 200, got {status} with {body}"
    );
    body
}

/// The `ConnectPeerId`s one page reports, in the order returned.
pub(crate) fn reported(case: &str, body: &Value) -> Vec<String> {
    body["ConnectPeerAssociations"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected a ConnectPeerAssociations array in {body}")
        })
        .iter()
        .map(|association| {
            association["ConnectPeerId"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: no ConnectPeerId in {association}"))
                .to_string()
        })
        .collect()
}

/// The one association a page reports for `peer`, or `None` when it reports none.
pub(crate) fn association_of(case: &str, body: &Value, peer: &str) -> Option<Value> {
    body["ConnectPeerAssociations"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected a ConnectPeerAssociations array in {body}")
        })
        .iter()
        .find(|association| association["ConnectPeerId"].as_str() == Some(peer))
        .cloned()
}

/// A built fixture: a global network, its core network, one Connect attachment per entry
/// of [`EDGES`], two devices (one at a site, one with none), the site's link, and one
/// Connect peer per row of [`CASES`].
pub(crate) struct Fixture {
    pub(crate) global_network: String,
    pub(crate) core_network: String,
    /// The device at a site, which is the only device a link can be associated with.
    pub(crate) sited_device: String,
    /// A device created with no site at all.
    pub(crate) siteless_device: String,
    /// The link associated with [`Self::sited_device`].
    pub(crate) link: String,
    /// A link of the same site that NO device is associated with.
    pub(crate) unassociated_link: String,
    /// The Connect peers, one per row of [`CASES`], in row order.
    pub(crate) peers: Vec<String>,
}

/// One row of the table: the association one `AssociateConnectPeer` asks for.
pub(crate) struct Case {
    /// Names the case in every assertion message.
    pub(crate) name: &'static str,
    /// Which entry of [`EDGES`] the row's peer hangs off.
    pub(crate) edge: usize,
    /// Whether the row associates the device that sits at a site.
    sited_device: bool,
    /// How the row spells the optional `LinkId`.
    link: LinkMember,
}

/// How a row spells `AssociateConnectPeerRequest$LinkId`: the modelled member is optional
/// and its pattern `^[\s\S]*$` admits the empty string, which names no link
/// (`smithy_query query="operation" operation="AssociateConnectPeer"`).
enum LinkMember {
    /// The link the device is associated with.
    Associated,
    /// The member is absent from the request.
    Absent,
    /// The member is present and empty.
    Empty,
}

/// The rows, spanning what the rule generalises over: both edges, a device at a site and
/// one with none, and all three spellings of the optional link member.
pub(crate) const CASES: [Case; 4] = [
    Case {
        name: "a peer of the first edge, over a link of the device's site",
        edge: 0,
        sited_device: true,
        link: LinkMember::Associated,
    },
    Case {
        name: "a second peer of the first edge, naming no link at all",
        edge: 0,
        sited_device: true,
        link: LinkMember::Absent,
    },
    Case {
        name: "a peer of the second edge, tied to a device that sits at no site",
        edge: 1,
        sited_device: false,
        link: LinkMember::Absent,
    },
    Case {
        name: "a peer of the second edge, spelling the link member empty",
        edge: 1,
        sited_device: false,
        link: LinkMember::Empty,
    },
];

impl Case {
    /// The device this row associates.
    pub(crate) fn device(&self, fixture: &Fixture) -> String {
        if self.sited_device {
            fixture.sited_device.clone()
        } else {
            fixture.siteless_device.clone()
        }
    }

    /// The link this row expects the stored association to carry, or `None` where it
    /// named none.
    pub(crate) fn expected_link(&self, fixture: &Fixture) -> Option<String> {
        match self.link {
            LinkMember::Associated => Some(fixture.link.clone()),
            LinkMember::Absent | LinkMember::Empty => None,
        }
    }

    /// The `AssociateConnectPeer` body this row sends for `peer`.
    pub(crate) fn request(&self, fixture: &Fixture, peer: &str) -> Value {
        let mut body = json!({
            "ConnectPeerId": peer,
            "DeviceId": self.device(fixture),
        });
        let object = body
            .as_object_mut()
            .unwrap_or_else(|| panic!("case {:?}: the request body must be an object", self.name));
        match self.link {
            LinkMember::Associated => {
                object.insert("LinkId".to_string(), json!(fixture.link));
            }
            LinkMember::Empty => {
                object.insert("LinkId".to_string(), json!(""));
            }
            LinkMember::Absent => {}
        }
        body
    }
}

/// `CreateConnectPeer` on the Connect attachment `attachment`, answering the minted id.
/// `index` offsets the addresses so two peers of one attachment differ.
///
/// `pub(crate)` because `tests/all/disassociate_link_tie_scope.rs` and
/// `tests/all/associate_link_member_agreement.rs` need a peer of their own global network's
/// core network, so every suite mints one through this helper.
pub(crate) async fn create_connect_peer(emulator: &Emulator, attachment: &str, index: usize) -> String {
    let (status, created) = emulator
        .post(
            "/connect-peers",
            json!({
                "ConnectAttachmentId": attachment,
                "PeerAddress": format!("203.0.113.{}", index + 1),
                "CoreNetworkAddress": format!("198.51.100.{}", index + 1),
                "InsideCidrBlocks": [format!("169.254.{}.0/29", index + 10)],
                "BgpOptions": { "PeerAsn": 65000 + index as i64 },
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "CreateConnectPeer on {attachment}: expected 200, got {status} with {created}"
    );
    created["ConnectPeer"]["ConnectPeerId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateConnectPeer on {attachment}: no id in {created}"))
        .to_string()
}

/// Build a fixture named `name`. `base` offsets the VPC and subnet ARNs its Connect
/// attachments are carried over, so two fixtures of one emulator attach different VPCs.
pub(crate) async fn fixture(emulator: &Emulator, name: &str, base: usize) -> Fixture {
    let global_network = emulator.create_global_network(name).await;
    let (status, created) = create_core_network(
        emulator,
        json!({
            "GlobalNetworkId": global_network,
            "Description": name,
            "PolicyDocument": policy_document(&EDGES),
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

    // One Connect attachment per edge, each carried over its own VPC attachment: a VPC is
    // attached by at most one attachment.
    let mut attachments: Vec<String> = Vec::new();
    for (index, edge) in EDGES.iter().enumerate() {
        let position = base + index;
        let (status, created) = create_vpc_attachment(
            emulator,
            json!({
                "CoreNetworkId": core_network,
                "VpcArn": vpc_arn(edge, position),
                "SubnetArns": [subnet_arn(edge, position)],
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?}: CreateVpcAttachment expected 200, got {status} with {created}"
        );
        let transport = attachment_id(name, &created["VpcAttachment"]);

        let (status, created) = create_connect_attachment(
            emulator,
            json!({
                "CoreNetworkId": core_network,
                "EdgeLocation": edge,
                "TransportAttachmentId": transport,
                "Options": { "Protocol": "GRE" },
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?}: CreateConnectAttachment expected 200, got {status} with {created}"
        );
        attachments.push(attachment_id(name, &created["ConnectAttachment"]));
    }

    let site = emulator.create_site(&global_network, name).await;
    let sited_device = create_device_ok(emulator, &global_network, json!({ "SiteId": site })).await;
    let siteless_device =
        create_device_ok(emulator, &global_network, json!({ "Description": name })).await;
    let link = create_link_ok(emulator, &global_network, &site, &plain_link()).await;
    let unassociated_link = create_link_ok(emulator, &global_network, &site, &plain_link()).await;

    // The link a row names has to be associated with the device it names, which is what
    // `AssociateLink` records.
    let (status, body) = emulator
        .post(
            &format!("/global-networks/{global_network}/link-associations"),
            json!({ "DeviceId": sited_device, "LinkId": link }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {name:?}: AssociateLink expected 200, got {status} with {body}"
    );

    let mut peers: Vec<String> = Vec::new();
    for (index, case) in CASES.iter().enumerate() {
        peers.push(create_connect_peer(emulator, &attachments[case.edge], base + index).await);
    }

    Fixture {
        global_network,
        core_network,
        sited_device,
        siteless_device,
        link,
        unassociated_link,
        peers,
    }
}

/// Assert one reported association against the row that made it.
pub(crate) fn assert_association(case: &Case, fixture: &Fixture, peer: &str, association: &Value, state: &str) {
    let name = case.name;
    assert_eq!(
        association["ConnectPeerId"].as_str(),
        Some(peer),
        "case {name:?}: expected the association to name peer {peer}, got {association}"
    );
    assert_eq!(
        association["GlobalNetworkId"].as_str(),
        Some(fixture.global_network.as_str()),
        "case {name:?}: expected global network {}, got {association}",
        fixture.global_network
    );
    assert_eq!(
        association["DeviceId"].as_str(),
        Some(case.device(fixture).as_str()),
        "case {name:?}: expected device {}, got {association}",
        case.device(fixture)
    );
    match case.expected_link(fixture) {
        Some(link) => assert_eq!(
            association["LinkId"].as_str(),
            Some(link.as_str()),
            "case {name:?}: expected link {link}, got {association}"
        ),
        None => assert!(
            association["LinkId"].is_null(),
            "case {name:?}: a row naming no link must report none, got {association}"
        ),
    }
    assert_eq!(
        association["State"].as_str(),
        Some(state),
        "case {name:?}: expected state {state}, got {association}"
    );
}

/// Associate every row of [`CASES`], asserting each call succeeded, and answer the
/// associations in row order.
pub(crate) async fn associate_all(emulator: &Emulator, fixture: &Fixture) -> Vec<Value> {
    let mut made: Vec<Value> = Vec::new();
    for (index, case) in CASES.iter().enumerate() {
        let peer = &fixture.peers[index];
        let (status, body) = associate(emulator, &fixture.global_network, case.request(fixture, peer))
            .await;
        assert_eq!(
            status, 200,
            "case {:?}: AssociateConnectPeer expected 200, got {status} with {body}",
            case.name
        );
        made.push(body["ConnectPeerAssociation"].clone());
    }
    made
}

#[tokio::test]
async fn every_associated_peer_reads_back_through_get_connect_peer_associations() {
    let emulator = Emulator::new();
    let fixture = fixture(&emulator, "associations", 0).await;

    // The association is PENDING while the transition the clock owes is still due, which
    // is the state both the making call and a read straight after it report.
    let made = associate_all(&emulator, &fixture).await;
    let page = get_associations_ok(&emulator, "every association", &fixture.global_network, "").await;
    for (index, case) in CASES.iter().enumerate() {
        let peer = &fixture.peers[index];
        assert_association(case, &fixture, peer, &made[index], "PENDING");
        let reported = association_of(case.name, &page, peer).unwrap_or_else(|| {
            panic!("case {:?}: peer {peer} is not reported in {page}", case.name)
        });
        assert_association(case, &fixture, peer, &reported, "PENDING");
    }

    // Every association of the global network is reported, in the stable peer-id order of
    // the store, and nothing else is.
    let mut expected = fixture.peers.clone();
    expected.sort();
    assert_eq!(
        reported("every association", &page),
        expected,
        "case every association: expected exactly {expected:?}, got {page}"
    );
    assert!(
        page["NextToken"].is_null(),
        "case every association: an unpaged read must carry no NextToken, got {page}"
    );

    // Once the span the clock owes has elapsed every association is AVAILABLE, computed
    // from the stored instant rather than written by anything.
    tokio::time::sleep(PAST_SETTLE).await;
    let settled =
        get_associations_ok(&emulator, "after the settle span", &fixture.global_network, "").await;
    for (index, case) in CASES.iter().enumerate() {
        let peer = &fixture.peers[index];
        let reported = association_of(case.name, &settled, peer).unwrap_or_else(|| {
            panic!("case {:?}: peer {peer} is not reported in {settled}", case.name)
        });
        assert_association(case, &fixture, peer, &reported, "AVAILABLE");
    }
}

#[tokio::test]
async fn the_connect_peer_ids_filter_narrows_the_listing_and_an_unknown_id_selects_nothing() {
    let emulator = Emulator::new();
    let fixture = fixture(&emulator, "filters", 0).await;
    associate_all(&emulator, &fixture).await;

    // An id of the minted shape (`connect-peer-<17 lowercase hex>`) that names nothing:
    // the leading `f` puts it above anything the monotonic id counter can mint.
    let unknown = "connect-peer-f0000000000000001".to_string();
    let all: Vec<String> = {
        let mut ids = fixture.peers.clone();
        ids.sort();
        ids
    };
    let sorted = |ids: &[&String]| {
        let mut selected: Vec<String> = ids.iter().map(|id| (*id).clone()).collect();
        selected.sort();
        selected
    };

    let rows: Vec<(&str, Vec<String>, Vec<String>)> = vec![
        ("the filter absent", Vec::new(), all.clone()),
        (
            "one id",
            vec![fixture.peers[0].clone()],
            vec![fixture.peers[0].clone()],
        ),
        (
            "two ids",
            vec![fixture.peers[1].clone(), fixture.peers[3].clone()],
            sorted(&[&fixture.peers[1], &fixture.peers[3]]),
        ),
        (
            "every id",
            fixture.peers.clone(),
            all.clone(),
        ),
        ("one unknown id", vec![unknown.clone()], Vec::new()),
        (
            "a known and an unknown id",
            vec![fixture.peers[2].clone(), unknown.clone()],
            vec![fixture.peers[2].clone()],
        ),
        (
            "the same id twice",
            vec![fixture.peers[0].clone(), fixture.peers[0].clone()],
            vec![fixture.peers[0].clone()],
        ),
        (
            // `ConnectPeerIds` documents none of the "The maximum is 10." bound the
            // id-list filters of the other listings carry (`smithy_query query="doc"
            // shape="GetConnectPeerAssociationsRequest" member="ConnectPeerIds"`), so a
            // selector well past ten ids is answered rather than refused.
            "far more ids than the bounded filters of this service allow",
            {
                let mut ids: Vec<String> = (0..26)
                    .map(|index| format!("connect-peer-f{index:016x}"))
                    .collect();
                ids.extend(fixture.peers.iter().cloned());
                ids
            },
            all.clone(),
        ),
    ];

    for (case, ids, expected) in rows {
        let query = ids
            .iter()
            .map(|id| format!("connectPeerIds={id}"))
            .collect::<Vec<String>>()
            .join("&");
        let page = get_associations_ok(&emulator, case, &fixture.global_network, &query).await;
        assert_eq!(
            reported(case, &page),
            expected,
            "case {case:?}: expected exactly {expected:?}, got {page}"
        );
    }
}

#[tokio::test]
async fn one_global_networks_associations_are_not_reported_by_another() {
    let emulator = Emulator::new();
    let first = fixture(&emulator, "first", 0).await;
    let second = fixture(&emulator, "second", 100).await;
    associate_all(&emulator, &first).await;
    associate_all(&emulator, &second).await;

    for (case, fixture) in [("first", &first), ("second", &second)] {
        let page = get_associations_ok(&emulator, case, &fixture.global_network, "").await;
        let mut expected = fixture.peers.clone();
        expected.sort();
        assert_eq!(
            reported(case, &page),
            expected,
            "case {case:?}: expected only its own associations {expected:?}, got {page}"
        );
    }

    // A peer of the OTHER global network cannot be tied here either: the peers this global
    // network can associate are the peers of its own core networks.
    let (status, body) = associate(
        &emulator,
        &first.global_network,
        json!({
            "ConnectPeerId": second.peers[0],
            "DeviceId": first.sited_device,
        }),
    )
    .await;
    assert_eq!(
        status, 404,
        "case a peer of another global network: expected 404, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "case a peer of another global network: expected ResourceNotFoundException, got {body}"
    );
}

#[tokio::test]
async fn every_page_size_walks_the_listing_exactly_once() {
    let emulator = Emulator::new();
    let fixture = fixture(&emulator, "pagination", 0).await;
    associate_all(&emulator, &fixture).await;
    let mut expected = fixture.peers.clone();
    expected.sort();

    // Every page size from one item to past the whole listing, plus each published bound
    // of `MaxResults` (1..500) and the absent page size.
    let sizes = [
        None,
        Some(1),
        Some(2),
        Some(3),
        Some(CASES.len()),
        Some(CASES.len() + 1),
        Some(MAX_PAGE),
    ];
    for size in sizes {
        let case = size.map_or_else(|| "no page size".to_string(), |size| format!("{size} per page"));
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let mut query: Vec<String> = Vec::new();
            if let Some(size) = size {
                query.push(format!("maxResults={size}"));
            }
            if let Some(token) = token.as_deref() {
                query.push(format!("nextToken={token}"));
            }
            let page =
                get_associations_ok(&emulator, &case, &fixture.global_network, &query.join("&"))
                    .await;
            let items = reported(&case, &page);
            if let Some(size) = size {
                assert!(
                    items.len() <= size,
                    "case {case:?}: a page carried {} items, more than {size}: {page}",
                    items.len()
                );
            }
            walked.extend(items);
            pages += 1;
            assert!(
                pages <= CASES.len() + 1,
                "case {case:?}: the walk did not terminate after {pages} pages, last {page}"
            );
            match page["NextToken"].as_str() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            walked, expected,
            "case {case:?}: the walk must report {expected:?} exactly once, got {walked:?}"
        );
    }

    // A token this service did not issue is a field failure, not a page.
    let (status, body) =
        get_associations(&emulator, &fixture.global_network, "nextToken=not-a-token").await;
    assert_eq!(
        status, 400,
        "case a foreign token: expected 400, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "case a foreign token: expected ValidationException, got {body}"
    );
    assert_eq!(
        body["Fields"][0]["Name"],
        json!("NextToken"),
        "case a foreign token: the refusal must name NextToken, got {body}"
    );
}

#[tokio::test]
async fn a_request_the_store_cannot_serve_is_refused_and_changes_nothing() {
    let emulator = Emulator::new();
    let fixture = fixture(&emulator, "refusals", 0).await;
    let peer = fixture.peers[0].clone();

    // Ids of the minted shape that name nothing stored.
    let unknown_network = "global-network-f0000000000000001";
    let unknown_peer = "connect-peer-f0000000000000001";
    let unknown_device = "device-f0000000000000001";
    let unknown_link = "link-f0000000000000001";

    // Each row: the request, and the status and error code the store's answer must carry.
    let rows: Vec<(&str, String, Value, u16, &str)> = vec![
        (
            // The association is a new record stored under the global network the path
            // addresses, so that parent reference is a field failure rather than the
            // missing-resource answer the resources inside it get.
            "an unknown global network",
            unknown_network.to_string(),
            json!({ "ConnectPeerId": peer, "DeviceId": fixture.sited_device }),
            400,
            "ValidationException",
        ),
        (
            "an unknown Connect peer",
            fixture.global_network.clone(),
            json!({ "ConnectPeerId": unknown_peer, "DeviceId": fixture.sited_device }),
            404,
            "ResourceNotFoundException",
        ),
        (
            "an unknown device",
            fixture.global_network.clone(),
            json!({ "ConnectPeerId": peer, "DeviceId": unknown_device }),
            404,
            "ResourceNotFoundException",
        ),
        (
            "an unknown link",
            fixture.global_network.clone(),
            json!({
                "ConnectPeerId": peer,
                "DeviceId": fixture.sited_device,
                "LinkId": unknown_link,
            }),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a link the device is not associated with",
            fixture.global_network.clone(),
            json!({
                "ConnectPeerId": peer,
                "DeviceId": fixture.sited_device,
                "LinkId": fixture.unassociated_link,
            }),
            400,
            "ValidationException",
        ),
        (
            "a link of a device that sits at no site",
            fixture.global_network.clone(),
            json!({
                "ConnectPeerId": peer,
                "DeviceId": fixture.siteless_device,
                "LinkId": fixture.link,
            }),
            400,
            "ValidationException",
        ),
    ];

    for (case, network, body, status_wanted, code) in rows {
        let (status, answered) = associate(&emulator, &network, body).await;
        assert_eq!(
            status, status_wanted,
            "case {case:?}: expected {status_wanted}, got {status} with {answered}"
        );
        assert_eq!(
            error_code(&answered),
            code,
            "case {case:?}: expected {code}, got {answered}"
        );
        assert!(
            answered["ConnectPeerAssociation"].is_null(),
            "case {case:?}: a refused request must report no association, got {answered}"
        );
    }

    // Nothing any refusal carried was stored: the global network still holds no
    // association at all.
    let page = get_associations_ok(&emulator, "after the refusals", &fixture.global_network, "").await;
    assert_eq!(
        reported("after the refusals", &page),
        Vec::<String>::new(),
        "case after the refusals: expected an empty page, got {page}"
    );

    // An unknown global network is not found by the READ either, whatever the query
    // string carries.
    let (status, body) = get_associations(
        &emulator,
        unknown_network,
        "connectPeerIds=connect-peer-f0000000000000001&maxResults=1",
    )
    .await;
    assert_eq!(
        status, 404,
        "case reading an unknown global network: expected 404, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "case reading an unknown global network: expected ResourceNotFoundException, got {body}"
    );
}

#[tokio::test]
async fn the_peer_is_the_associations_identity_so_a_repeat_reports_it_and_another_device_conflicts() {
    let emulator = Emulator::new();
    let fixture = fixture(&emulator, "identity", 0).await;
    let case = &CASES[0];
    let peer = fixture.peers[0].clone();

    let first = associate(
        &emulator,
        &fixture.global_network,
        case.request(&fixture, &peer),
    )
    .await;
    assert_eq!(
        first.0, 200,
        "case the first association: expected 200, got {} with {}",
        first.0, first.1
    );

    // The same request again reports the association already stored and writes nothing:
    // the settle span is not restarted, so the state keeps advancing from the FIRST call.
    tokio::time::sleep(PAST_SETTLE).await;
    let (status, repeated) = associate(
        &emulator,
        &fixture.global_network,
        case.request(&fixture, &peer),
    )
    .await;
    assert_eq!(
        status, 200,
        "case a repeated association: expected 200, got {status} with {repeated}"
    );
    assert_association(
        case,
        &fixture,
        &peer,
        &repeated["ConnectPeerAssociation"],
        "AVAILABLE",
    );

    // Naming another device for the same peer conflicts with the association held: one
    // peering reaches its appliance over one device.
    let (status, body) = associate(
        &emulator,
        &fixture.global_network,
        json!({
            "ConnectPeerId": peer,
            "DeviceId": fixture.siteless_device,
        }),
    )
    .await;
    assert_eq!(
        status, 409,
        "case another device for the same peer: expected 409, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "ConflictException",
        "case another device for the same peer: expected ConflictException, got {body}"
    );
    assert_eq!(
        body["ResourceId"].as_str(),
        Some(peer.as_str()),
        "case another device for the same peer: the conflict must name the peer, got {body}"
    );

    // Neither the repeat nor the conflict changed what a later caller sees: one
    // association, still over the device and link the first call named.
    let page = get_associations_ok(&emulator, "after the repeat", &fixture.global_network, "").await;
    assert_eq!(
        reported("after the repeat", &page),
        vec![peer.clone()],
        "case after the repeat: expected exactly one association, got {page}"
    );
    let reported_once = association_of("after the repeat", &page, &peer)
        .unwrap_or_else(|| panic!("case after the repeat: peer {peer} is not reported in {page}"));
    assert_association(case, &fixture, &peer, &reported_once, "AVAILABLE");

    // Reading again reports the identical association, so the read wrote nothing.
    let again = get_associations_ok(&emulator, "a later read", &fixture.global_network, "").await;
    assert_eq!(
        again["ConnectPeerAssociations"], page["ConnectPeerAssociations"],
        "case a later read: expected the identical page, got {again} after {page}"
    );
}

#[tokio::test]
async fn associating_a_peer_registers_its_bgp_session_with_the_global_networks_telemetry() {
    let emulator = Emulator::new();
    let fixture = fixture(&emulator, "telemetry", 0).await;

    // Before any association the global network knows of no session at all.
    let (status, body) = emulator
        .get(&format!(
            "/global-networks/{}/network-telemetry",
            fixture.global_network
        ))
        .await;
    assert_eq!(
        status, 200,
        "case before any association: expected 200, got {status} with {body}"
    );
    assert_eq!(
        body["NetworkTelemetry"],
        json!([]),
        "case before any association: expected no telemetry, got {body}"
    );

    associate_all(&emulator, &fixture).await;

    let (status, body) = emulator
        .get(&format!(
            "/global-networks/{}/network-telemetry?resourceType=connect-peer",
            fixture.global_network
        ))
        .await;
    assert_eq!(
        status, 200,
        "case after the associations: expected 200, got {status} with {body}"
    );
    let entries = body["NetworkTelemetry"]
        .as_array()
        .unwrap_or_else(|| panic!("case after the associations: no NetworkTelemetry in {body}"));
    assert_eq!(
        entries.len(),
        CASES.len(),
        "case after the associations: expected one entry per associated peer, got {body}"
    );
    for (index, case) in CASES.iter().enumerate() {
        let peer = &fixture.peers[index];
        let entry = entries
            .iter()
            .find(|entry| entry["ResourceId"].as_str() == Some(peer.as_str()))
            .unwrap_or_else(|| {
                panic!("case {:?}: peer {peer} has no telemetry entry in {body}", case.name)
            });
        assert_eq!(
            entry["CoreNetworkId"].as_str(),
            Some(fixture.core_network.as_str()),
            "case {:?}: expected the peer's core network, got {entry}",
            case.name
        );
        assert_eq!(
            entry["AwsRegion"].as_str(),
            Some(EDGES[case.edge]),
            "case {:?}: expected the edge location of the peer's attachment, got {entry}",
            case.name
        );
        assert_eq!(
            entry["Health"]["Type"].as_str(),
            Some("BGP"),
            "case {:?}: a Connect peer peers over BGP, got {entry}",
            case.name
        );
    }
}
