// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DescribeTransitGatewayRouteTableAnnouncements`, driven over the
//! wire the service actually speaks: `ec2Query` — an
//! `application/x-www-form-urlencoded` POST whose body carries
//! `Action=DescribeTransitGatewayRouteTableAnnouncements`, answered with XML (README.md /
//! OPERATIONS.md "Wire protocol").
//!
//! The rule under test is the one every EC2 describe follows: it REPORTS THE RESOURCES THE
//! STORE HOLDS, each with every member it carries, narrowed by the id list and by every
//! filter (joined with AND), in resource-id order, paged by `MaxResults` with a token naming
//! the last id of the page it was issued with, which the next page resumes after — and it
//! refuses what it cannot resolve rather than
//! answering an absent row: an unknown id with that resource's own `NotFound` code, a value
//! that is not an id of the kind with its `Malformed` code, a filter name the operation
//! answers none of, and a token the service did not mint.
//!
//! Every table row below is a distinct input along what that generalises over: which transit
//! gateway and route table an announcement belongs to, which direction it was made in, which
//! state it is in, whether it names a core network or a peer core network at all, when it was
//! created, whether it carries tags, how many announcements one gateway holds, which selector
//! the request uses, how many ids it names, and how large a page it asks for (both published
//! bounds of `TransitGatewayMaxResults` and one past each). The announcements are seeded
//! through the store's own writer (`insert_transit_gateway_route_table_announcement`), so
//! this suite covers the describe alone; what
//! `CreateTransitGatewayRouteTableAnnouncement` writes has its own suite.

use ec2::ops::types::{
    TRANSIT_GATEWAY_ROUTE_TABLE_ANNOUNCEMENT_DIRECTION_VALUES,
    TRANSIT_GATEWAY_ROUTE_TABLE_ANNOUNCEMENT_STATE_VALUES, Tag,
    TransitGatewayRouteTableAnnouncement,
};
use ec2::state::ComAmazonawsEc2Amazonec2State as Ec2State;
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{as_timestamp, element, elements, error_code, filters, indexed, unescaped};
use super::wire::{Param, child, child_text, document, param};

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// The HTTP status EC2 answers `DryRunOperation` with.
const DRY_RUN_STATUS: u16 = 412;

/// The ec2Query `Action` under test.
const ACTION: &str = "DescribeTransitGatewayRouteTableAnnouncements";

/// One seeded announcement: the facts that distinguish it along what the rule generalises
/// over.
struct Fixture {
    /// The transit gateway the announcement was made on — the `transit-gateway-id` filter's
    /// value.
    transit_gateway_id: &'static str,
    /// The route table being advertised.
    route_table_id: &'static str,
    /// The peering attachment the advertisement travels over.
    peering_attachment_id: &'static str,
    /// The transit gateway on the other side of that attachment.
    peer_transit_gateway_id: &'static str,
    /// The core network the announcement belongs to, or `None` for an announcement between
    /// two transit gateways with no core network in it.
    core_network_id: Option<&'static str>,
    /// The peer's core network, or `None` for the same reason.
    peer_core_network_id: Option<&'static str>,
    /// The direction, a member of `TransitGatewayRouteTableAnnouncementDirection`.
    direction: &'static str,
    /// The announcement's state, a member of
    /// `TransitGatewayRouteTableAnnouncementState`.
    state: &'static str,
    /// The instant it was created, as seconds since the epoch.
    created_at: i64,
    /// The tags it carries, as (key, value) pairs — empty for an untagged announcement.
    tags: &'static [(&'static str, &'static str)],
}

/// A member of `TransitGatewayRouteTableAnnouncementState`, from the modelled catalog
/// (`smithy_query(query="shape",shape="TransitGatewayRouteTableAnnouncementState")`, whose
/// values are `available | pending | failing | failed | deleting | deleted`) rather than
/// written out here.
fn announcement_state(name: &str) -> &'static str {
    TRANSIT_GATEWAY_ROUTE_TABLE_ANNOUNCEMENT_STATE_VALUES
        .iter()
        .find(|value| **value == name)
        .unwrap_or_else(|| {
            panic!("{name:?} is no TransitGatewayRouteTableAnnouncementState member")
        })
}

/// A member of `TransitGatewayRouteTableAnnouncementDirection`, from the modelled catalog
/// (`smithy_query(query="shape",shape="TransitGatewayRouteTableAnnouncementDirection")`,
/// whose values are `outgoing | incoming`) rather than written out here.
fn announcement_direction(name: &str) -> &'static str {
    TRANSIT_GATEWAY_ROUTE_TABLE_ANNOUNCEMENT_DIRECTION_VALUES
        .iter()
        .find(|value| **value == name)
        .unwrap_or_else(|| {
            panic!("{name:?} is no TransitGatewayRouteTableAnnouncementDirection member")
        })
}

/// The seven announcements every test here is written against: three transit gateways
/// holding two, two and three announcements, both directions the modelled catalog publishes,
/// five of its six states (`available` three times and `pending` twice, so a state filter
/// selects more than one row), announcements with and without a core network on either side,
/// distinct creation instants, and both tagged and untagged rows.
fn fixtures() -> Vec<Fixture> {
    vec![
        Fixture {
            transit_gateway_id: "tgw-0000000000000000a",
            route_table_id: "tgw-rtb-0000000000000000a",
            peering_attachment_id: "tgw-attach-0000000000000000a",
            peer_transit_gateway_id: "tgw-00000000000000aa1",
            core_network_id: Some("core-network-0000000000000000a"),
            peer_core_network_id: Some("core-network-00000000000000aa1"),
            direction: announcement_direction("outgoing"),
            state: announcement_state("available"),
            created_at: 1_760_000_000,
            tags: &[("Name", "north"), ("Team", "network")],
        },
        Fixture {
            transit_gateway_id: "tgw-0000000000000000a",
            route_table_id: "tgw-rtb-0000000000000000b",
            peering_attachment_id: "tgw-attach-0000000000000000b",
            peer_transit_gateway_id: "tgw-00000000000000aa2",
            core_network_id: None,
            peer_core_network_id: None,
            direction: announcement_direction("incoming"),
            state: announcement_state("pending"),
            created_at: 1_760_000_100,
            tags: &[("Name", "south")],
        },
        Fixture {
            transit_gateway_id: "tgw-0000000000000000b",
            route_table_id: "tgw-rtb-0000000000000000c",
            peering_attachment_id: "tgw-attach-0000000000000000c",
            peer_transit_gateway_id: "tgw-00000000000000bb1",
            core_network_id: Some("core-network-0000000000000000b"),
            peer_core_network_id: None,
            direction: announcement_direction("outgoing"),
            state: announcement_state("available"),
            created_at: 1_760_000_200,
            tags: &[],
        },
        Fixture {
            transit_gateway_id: "tgw-0000000000000000b",
            route_table_id: "tgw-rtb-0000000000000000d",
            peering_attachment_id: "tgw-attach-0000000000000000d",
            peer_transit_gateway_id: "tgw-00000000000000bb2",
            core_network_id: None,
            peer_core_network_id: Some("core-network-00000000000000bb2"),
            direction: announcement_direction("incoming"),
            state: announcement_state("deleting"),
            created_at: 1_760_000_300,
            tags: &[("Team", "network")],
        },
        Fixture {
            transit_gateway_id: "tgw-0000000000000000c",
            route_table_id: "tgw-rtb-0000000000000000e",
            peering_attachment_id: "tgw-attach-0000000000000000e",
            peer_transit_gateway_id: "tgw-00000000000000cc1",
            core_network_id: None,
            peer_core_network_id: None,
            direction: announcement_direction("outgoing"),
            state: announcement_state("failed"),
            created_at: 1_760_000_400,
            tags: &[],
        },
        Fixture {
            transit_gateway_id: "tgw-0000000000000000c",
            route_table_id: "tgw-rtb-0000000000000000f",
            peering_attachment_id: "tgw-attach-0000000000000000f",
            peer_transit_gateway_id: "tgw-00000000000000cc2",
            core_network_id: Some("core-network-0000000000000000c"),
            peer_core_network_id: Some("core-network-00000000000000cc2"),
            direction: announcement_direction("incoming"),
            state: announcement_state("available"),
            created_at: 1_760_000_500,
            tags: &[("Name", "north")],
        },
        Fixture {
            transit_gateway_id: "tgw-0000000000000000c",
            route_table_id: "tgw-rtb-00000000000000aaa",
            peering_attachment_id: "tgw-attach-00000000000000aaa",
            peer_transit_gateway_id: "tgw-00000000000000cc3",
            core_network_id: None,
            peer_core_network_id: None,
            direction: announcement_direction("outgoing"),
            state: announcement_state("pending"),
            created_at: 1_760_000_600,
            tags: &[("Name", "east")],
        },
    ]
}

/// The modelled announcement one fixture is stored as.
fn stored_announcement(fixture: &Fixture) -> TransitGatewayRouteTableAnnouncement {
    TransitGatewayRouteTableAnnouncement {
        // The store mints the id; what a writer puts here is replaced by it.
        transit_gateway_route_table_announcement_id: None,
        transit_gateway_id: Some(fixture.transit_gateway_id.to_string()),
        core_network_id: fixture.core_network_id.map(str::to_string),
        peer_transit_gateway_id: Some(fixture.peer_transit_gateway_id.to_string()),
        peer_core_network_id: fixture.peer_core_network_id.map(str::to_string),
        peering_attachment_id: Some(fixture.peering_attachment_id.to_string()),
        announcement_direction: Some(fixture.direction.to_string()),
        transit_gateway_route_table_id: Some(fixture.route_table_id.to_string()),
        state: Some(fixture.state.to_string()),
        creation_time: Some(::aws_smithy_types::DateTime::from_secs(fixture.created_at)),
        tags: (!fixture.tags.is_empty()).then(|| {
            fixture
                .tags
                .iter()
                .map(|(key, value)| Tag {
                    key: Some((*key).to_string()),
                    value: Some((*value).to_string()),
                })
                .collect()
        }),
    }
}

/// Seed [`fixtures`] into the store through its own writer and answer the ids it minted, in
/// fixture order.
fn seed(engine: &engine::InMemoryEngine) -> Vec<String> {
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    fixtures()
        .iter()
        .map(|fixture| {
            guard
                .insert_transit_gateway_route_table_announcement(stored_announcement(fixture))
                .expect("the store holds the seeded announcement")
        })
        .collect()
}

/// The emulator with the announcements above seeded: the router to call and the ids they
/// were given, in fixture order.
fn seeded() -> (MultiServiceRouter, std::sync::Arc<engine::InMemoryEngine>, Vec<String>) {
    let (router, engine) = super::ec2_query::emulator();
    let ids = seed(&engine);
    (router, engine, ids)
}

/// One `DescribeTransitGatewayRouteTableAnnouncements` call carrying `params`.
async fn describe(router: &MultiServiceRouter, params: &[Param]) -> (u16, String) {
    super::ec2_query::call(router, REGION, ACTION, params).await
}

/// The same call, requiring HTTP 200, and the body it answered.
async fn describe_ok(case: &str, router: &MultiServiceRouter, params: &[Param]) -> String {
    let (status, body) = describe(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {ACTION}, got {status} with body {body}"
    );
    body
}

/// The ids of every announcement a response reports, in document order.
fn reported_ids(body: &str) -> Vec<String> {
    elements(body, "transitGatewayRouteTableAnnouncementId")
}

/// `TransitGatewayRouteTableAnnouncementIds.N` form fields for `ids`.
fn id_params(ids: &[&str]) -> Vec<Param> {
    indexed("TransitGatewayRouteTableAnnouncementIds", ids)
}

/// Read something off the one announcement element a response carries — the element that has
/// a `transitGatewayRouteTableAnnouncementId` child.
fn with_announcement<T>(
    case: &str,
    body: &str,
    read: impl FnOnce(roxmltree::Node<'_, '_>) -> T,
) -> T {
    let parsed = document(case, body);
    let announcement = parsed
        .descendants()
        .find(|node| {
            node.is_element()
                && child(*node, "transitGatewayRouteTableAnnouncementId").is_some()
        })
        .unwrap_or_else(|| panic!("case {case:?}: no announcement in {body}"));
    read(announcement)
}

/// A member of the one announcement a response carries, or `None` when it carries no such
/// member.
fn member(case: &str, body: &str, name: &str) -> Option<String> {
    with_announcement(case, body, |announcement| child_text(announcement, name))
}

/// Whether the one announcement a response carries has a `name` child ELEMENT at all.
///
/// The absence check for a member nothing recorded: [`member`] reads an element's TEXT,
/// which a present-but-empty container (`<tagSet/>`) does not carry, so only element absence
/// shows that nothing was reported for it.
fn has_member(case: &str, body: &str, name: &str) -> bool {
    with_announcement(case, body, |announcement| {
        child(announcement, name).is_some()
    })
}

/// The (key, value) pairs of the one announcement a response carries.
fn reported_tags(case: &str, body: &str) -> Vec<(String, String)> {
    with_announcement(case, body, |announcement| {
        super::wire::set_items(announcement, "tagSet")
            .into_iter()
            .map(|item| {
                (
                    child_text(item, "key").unwrap_or_default(),
                    child_text(item, "value").unwrap_or_default(),
                )
            })
            .collect()
    })
}

/// Every fact a stored announcement carries reads back: the id the store minted, the transit
/// gateway and route table it was made on, the peering attachment and peer gateway it travels
/// to, the core networks on either side, the direction, the state, the instant it was created
/// and the tags on it. One row per seeded announcement, so no single announcement is the
/// specification — and the rows that name no core network and no tags assert that a member
/// the store holds no value for is reported absent rather than invented.
#[tokio::test]
async fn every_fact_a_stored_announcement_carries_reads_back() {
    let (router, _engine, ids) = seeded();

    for (fixture, announcement_id) in fixtures().iter().zip(&ids) {
        let case = format!(
            "announcement {announcement_id} of {}",
            fixture.transit_gateway_id
        );
        let body = describe_ok(&case, &router, &id_params(&[announcement_id.as_str()])).await;

        assert_eq!(
            reported_ids(&body),
            vec![announcement_id.clone()],
            "case {case:?}: expected exactly the announcement named, got {body}"
        );
        for (name, expected) in [
            ("transitGatewayId", fixture.transit_gateway_id.to_string()),
            (
                "transitGatewayRouteTableId",
                fixture.route_table_id.to_string(),
            ),
            (
                "peeringAttachmentId",
                fixture.peering_attachment_id.to_string(),
            ),
            (
                "peerTransitGatewayId",
                fixture.peer_transit_gateway_id.to_string(),
            ),
            ("announcementDirection", fixture.direction.to_string()),
            ("state", fixture.state.to_string()),
        ] {
            let actual = member(&case, &body, name);
            assert_eq!(
                actual.as_deref(),
                Some(expected.as_str()),
                "case {case:?}: expected <{name}> {expected:?}, got {actual:?} in {body}"
            );
        }
        for (name, expected) in [
            ("coreNetworkId", fixture.core_network_id),
            ("peerCoreNetworkId", fixture.peer_core_network_id),
        ] {
            let actual = member(&case, &body, name);
            assert_eq!(
                actual.as_deref(),
                expected,
                "case {case:?}: expected <{name}> {expected:?}, got {actual:?} in {body}"
            );
        }
        let reported_creation = member(&case, &body, "creationTime");
        let seconds = reported_creation.as_deref().and_then(as_timestamp);
        let expected_seconds =
            ::aws_smithy_types::DateTime::from_secs(fixture.created_at).as_secs_f64();
        assert_eq!(
            seconds,
            Some(expected_seconds),
            "case {case:?}: expected the instant the announcement was created, got \
             {reported_creation:?} in {body}"
        );

        let expected_tags: Vec<(String, String)> = fixture
            .tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        if expected_tags.is_empty() {
            assert!(
                !has_member(&case, &body, "tagSet"),
                "case {case:?}: an untagged announcement carries no <tagSet>, got {body}"
            );
        } else {
            let actual = reported_tags(&case, &body);
            assert_eq!(
                actual, expected_tags,
                "case {case:?}: expected the tags stored, got {actual:?} in {body}"
            );
        }
    }
}

/// A request that names no selector reports every announcement the store holds, in
/// `TransitGatewayRouteTableAnnouncementId` order.
#[tokio::test]
async fn an_unselective_describe_reports_every_stored_announcement_in_id_order() {
    let (router, _engine, ids) = seeded();
    let case = "no selector";

    let body = describe_ok(case, &router, &[]).await;
    let mut expected = ids.clone();
    expected.sort();
    let actual = reported_ids(&body);
    assert_eq!(
        actual, expected,
        "case {case:?}: expected {expected:?} in id order, got {actual:?} in {body}"
    );
    assert!(
        element(&body, "nextToken").is_none(),
        "case {case:?}: a complete answer carries no nextToken, got {body}"
    );
}

/// An empty store is an empty answer, not a refusal: a describe of a resource kind nothing
/// has created reports no announcements at all.
#[tokio::test]
async fn an_empty_store_reports_no_announcements() {
    let (router, _engine) = super::ec2_query::emulator();
    let case = "empty store";

    let body = describe_ok(case, &router, &[]).await;
    let actual = reported_ids(&body);
    assert!(
        actual.is_empty(),
        "case {case:?}: expected no announcements, got {actual:?} in {body}"
    );
}

/// Each filter selects exactly the announcements carrying its value, and several filters are
/// joined with AND. The rows span every filter name the operation answers, a value nothing
/// carries, a wildcard value, two values in one filter, and two-filter combinations.
#[tokio::test]
async fn each_filter_selects_the_announcements_that_carry_its_value() {
    let (router, _engine, ids) = seeded();
    let rows = fixtures();

    // (case, the filters as sent, the fixture positions expected back)
    let cases: Vec<(&str, Vec<Param>, Vec<usize>)> = vec![
        (
            "state available",
            filters(&[("state", &[announcement_state("available")])]),
            vec![0, 2, 5],
        ),
        (
            "state pending",
            filters(&[("state", &[announcement_state("pending")])]),
            vec![1, 6],
        ),
        (
            "state deleting",
            filters(&[("state", &[announcement_state("deleting")])]),
            vec![3],
        ),
        (
            "state failed",
            filters(&[("state", &[announcement_state("failed")])]),
            vec![4],
        ),
        (
            "a state no announcement is in",
            filters(&[("state", &[announcement_state("failing")])]),
            Vec::new(),
        ),
        (
            "two states in one filter",
            filters(&[(
                "state",
                &[
                    announcement_state("deleting"),
                    announcement_state("failed"),
                ],
            )]),
            vec![3, 4],
        ),
        (
            "announcement-direction outgoing",
            filters(&[(
                "announcement-direction",
                &[announcement_direction("outgoing")],
            )]),
            vec![0, 2, 4, 6],
        ),
        (
            "announcement-direction incoming",
            filters(&[(
                "announcement-direction",
                &[announcement_direction("incoming")],
            )]),
            vec![1, 3, 5],
        ),
        (
            "transit-gateway-id of two announcements",
            filters(&[("transit-gateway-id", &[rows[0].transit_gateway_id])]),
            vec![0, 1],
        ),
        (
            "transit-gateway-id of three announcements",
            filters(&[("transit-gateway-id", &[rows[4].transit_gateway_id])]),
            vec![4, 5, 6],
        ),
        (
            // "Filter values support wildcards" is EC2's filter contract: `*` stands for
            // zero or more characters, so a prefix pattern selects every transit gateway.
            "transit-gateway-id as a wildcard",
            filters(&[("transit-gateway-id", &["tgw-*"])]),
            (0..rows.len()).collect(),
        ),
        (
            "transit-gateway-id nothing carries",
            filters(&[("transit-gateway-id", &["tgw-00000000000000fff"])]),
            Vec::new(),
        ),
        (
            "transit-gateway-route-table-id of one announcement",
            filters(&[("transit-gateway-route-table-id", &[rows[3].route_table_id])]),
            vec![3],
        ),
        (
            "peering-attachment-id of one announcement",
            filters(&[(
                "peering-attachment-id",
                &[rows[5].peering_attachment_id],
            )]),
            vec![5],
        ),
        (
            "peer-transit-gateway-id of one announcement",
            filters(&[(
                "peer-transit-gateway-id",
                &[rows[2].peer_transit_gateway_id],
            )]),
            vec![2],
        ),
        (
            "core-network-id of one announcement",
            filters(&[(
                "core-network-id",
                &[rows[0].core_network_id.expect("row 0 names a core network")],
            )]),
            vec![0],
        ),
        (
            // An announcement the store holds no core network for compares against NO
            // values, so a wildcard over that member selects only the rows that carry one.
            "core-network-id as a wildcard",
            filters(&[("core-network-id", &["core-network-*"])]),
            vec![0, 2, 5],
        ),
        (
            "peer-core-network-id of one announcement",
            filters(&[(
                "peer-core-network-id",
                &[rows[3]
                    .peer_core_network_id
                    .expect("row 3 names a peer core network")],
            )]),
            vec![3],
        ),
        (
            "peer-core-network-id as a wildcard",
            filters(&[("peer-core-network-id", &["core-network-*"])]),
            vec![0, 3, 5],
        ),
        (
            "state and transit-gateway-id joined with AND",
            filters(&[
                ("state", &[announcement_state("available")]),
                ("transit-gateway-id", &[rows[4].transit_gateway_id]),
            ]),
            vec![5],
        ),
        (
            "state and transit-gateway-id that no one announcement carries together",
            filters(&[
                ("state", &[announcement_state("deleting")]),
                ("transit-gateway-id", &[rows[0].transit_gateway_id]),
            ]),
            Vec::new(),
        ),
        (
            "announcement-direction and state joined with AND",
            filters(&[
                (
                    "announcement-direction",
                    &[announcement_direction("incoming")],
                ),
                ("state", &[announcement_state("available")]),
            ]),
            vec![5],
        ),
        (
            "tag:Name of two announcements",
            filters(&[("tag:Name", &["north"])]),
            vec![0, 5],
        ),
        (
            "tag:Name of one announcement",
            filters(&[("tag:Name", &["east"])]),
            vec![6],
        ),
        (
            "tag:Name no announcement carries",
            filters(&[("tag:Name", &["west"])]),
            Vec::new(),
        ),
        (
            "tag:Team",
            filters(&[("tag:Team", &["network"])]),
            vec![0, 3],
        ),
        (
            "tag-key Name",
            filters(&[("tag-key", &["Name"])]),
            vec![0, 1, 5, 6],
        ),
        (
            "tag-key Team",
            filters(&[("tag-key", &["Team"])]),
            vec![0, 3],
        ),
        (
            "tag-key no announcement carries",
            filters(&[("tag-key", &["Owner"])]),
            Vec::new(),
        ),
        (
            "tag:Name and state joined with AND",
            filters(&[
                ("tag:Name", &["north"]),
                ("state", &[announcement_state("available")]),
            ]),
            vec![0, 5],
        ),
    ];

    for (case, params, expected_positions) in cases {
        let body = describe_ok(case, &router, &params).await;
        let mut expected: Vec<String> = expected_positions
            .iter()
            .map(|position| ids[*position].clone())
            .collect();
        expected.sort();
        let actual = reported_ids(&body);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?}, got {actual:?} in {body}"
        );
    }

    // The id filter is the id selector's twin, so it is asserted over every seeded
    // announcement rather than over one row — and so is `creation-time`, whose value is the
    // rendering the caller was shown rather than one written out here.
    for announcement_id in &ids {
        let case = format!("transit-gateway-route-table-announcement-id {announcement_id}");
        let body = describe_ok(
            &case,
            &router,
            &filters(&[(
                "transit-gateway-route-table-announcement-id",
                &[announcement_id.as_str()],
            )]),
        )
        .await;
        let actual = reported_ids(&body);
        assert_eq!(
            actual,
            vec![announcement_id.clone()],
            "case {case:?}: expected exactly that announcement, got {actual:?} in {body}"
        );

        let created = member(&case, &body, "creationTime")
            .unwrap_or_else(|| panic!("case {case:?}: no creationTime in {body}"));
        let case = format!("creation-time {created}");
        let body = describe_ok(
            &case,
            &router,
            &filters(&[("creation-time", &[created.as_str()])]),
        )
        .await;
        let actual = reported_ids(&body);
        assert_eq!(
            actual,
            vec![announcement_id.clone()],
            "case {case:?}: expected exactly the announcement created then, got {actual:?} \
             in {body}"
        );
    }

    // A creation instant no announcement carries selects nothing, rather than everything.
    let case = "creation-time nothing carries";
    let body = describe_ok(
        case,
        &router,
        &filters(&[("creation-time", &["1999-01-01T00:00:00Z"])]),
    )
    .await;
    assert!(
        reported_ids(&body).is_empty(),
        "case {case:?}: expected no announcements, got {body}"
    );
}

/// One row of the id-list table: the case, the ids the request names, the filters it carries
/// beside them, and the fixture positions the answer must carry.
type IdListCase<'a> = (&'static str, Vec<&'a str>, Vec<Param>, Vec<usize>);

/// The id list selects exactly the announcements it names, whatever its length and however
/// often an id is repeated, and it narrows what a filter would have selected.
#[tokio::test]
async fn an_id_list_selects_exactly_the_announcements_it_names() {
    let (router, _engine, ids) = seeded();

    let cases: Vec<IdListCase<'_>> = vec![
        ("one id", vec![ids[3].as_str()], Vec::new(), vec![3]),
        (
            "three ids, named out of order",
            vec![ids[5].as_str(), ids[0].as_str(), ids[2].as_str()],
            Vec::new(),
            vec![0, 2, 5],
        ),
        (
            "the same id twice",
            vec![ids[1].as_str(), ids[1].as_str()],
            Vec::new(),
            vec![1],
        ),
        (
            "every id",
            ids.iter().map(String::as_str).collect(),
            Vec::new(),
            (0..ids.len()).collect(),
        ),
        (
            "two ids narrowed by a state filter",
            vec![ids[0].as_str(), ids[1].as_str()],
            filters(&[("state", &[announcement_state("pending")])]),
            vec![1],
        ),
        (
            "two ids narrowed by a direction filter",
            vec![ids[0].as_str(), ids[1].as_str()],
            filters(&[(
                "announcement-direction",
                &[announcement_direction("outgoing")],
            )]),
            vec![0],
        ),
        (
            "two ids narrowed by a tag filter",
            vec![ids[0].as_str(), ids[5].as_str()],
            filters(&[("tag:Team", &["network"])]),
            vec![0],
        ),
    ];

    for (case, named, extra, expected_positions) in cases {
        let mut params = id_params(&named);
        params.extend(extra);
        let body = describe_ok(case, &router, &params).await;
        let mut expected: Vec<String> = expected_positions
            .iter()
            .map(|position| ids[*position].clone())
            .collect();
        expected.sort();
        let actual = reported_ids(&body);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?}, got {actual:?} in {body}"
        );
    }
}

/// A well-formed id naming no announcement is refused with the announcement's own `NotFound`
/// code, and a value that is not an announcement id at all with its `Malformed` code — so a
/// client can tell the two apart, and neither is reported as an absent row. Both are asserted
/// beside an id that DOES name an announcement, so the refusal does not depend on the request
/// naming nothing real.
#[tokio::test]
async fn an_unknown_id_is_refused_not_found_and_a_value_that_is_no_id_malformed() {
    let (router, _engine, ids) = seeded();
    let prefix = ec2::state::TRANSIT_GATEWAY_ROUTE_TABLE_ANNOUNCEMENT_ID_PREFIX;

    let cases: Vec<(&str, Vec<String>, &str)> = vec![
        (
            "one unknown id",
            vec![format!("{prefix}-00000000000000fff")],
            ec2::state::INVALID_TRANSIT_GATEWAY_ROUTE_TABLE_ANNOUNCEMENT_ID_NOT_FOUND,
        ),
        (
            "a known id beside an unknown one",
            vec![ids[0].clone(), format!("{prefix}-00000000000000eee")],
            ec2::state::INVALID_TRANSIT_GATEWAY_ROUTE_TABLE_ANNOUNCEMENT_ID_NOT_FOUND,
        ),
        (
            "a suffix that is not hexadecimal",
            vec![format!("{prefix}-not-hex")],
            ec2::state::INVALID_TRANSIT_GATEWAY_ROUTE_TABLE_ANNOUNCEMENT_ID_MALFORMED,
        ),
        (
            // The route table's own id prefix is a PREFIX of the announcement's, so an id of
            // that neighbouring resource is not an announcement id either.
            "an id of another resource of the same family",
            vec!["tgw-rtb-0000000000000000a".to_string()],
            ec2::state::INVALID_TRANSIT_GATEWAY_ROUTE_TABLE_ANNOUNCEMENT_ID_MALFORMED,
        ),
        (
            "a known id beside a value that is no id",
            vec![ids[0].clone(), "not-an-id".to_string()],
            ec2::state::INVALID_TRANSIT_GATEWAY_ROUTE_TABLE_ANNOUNCEMENT_ID_MALFORMED,
        ),
    ];

    for (case, named, expected) in cases {
        let borrowed: Vec<&str> = named.iter().map(String::as_str).collect();
        let (status, body) = describe(&router, &id_params(&borrowed)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected,
            "case {case:?}: expected {expected:?}, got {body}"
        );
    }

    // The refusal names the first id the CALLER listed that could not be resolved, not
    // whichever of them sorts first: the two rows name the same pair of unknown ids in the
    // two possible orders, so an implementation that walked a sorted set would fail one.
    let lower = format!("{prefix}-00000000000000aaa");
    let higher = format!("{prefix}-00000000000000bbb");
    for (case, first, second) in [
        ("the lower id listed first", &lower, &higher),
        ("the higher id listed first", &higher, &lower),
    ] {
        let (status, body) =
            describe(&router, &id_params(&[first.as_str(), second.as_str()])).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        let message = unescaped(&element(&body, "Message").unwrap_or_default());
        assert!(
            message.contains(first.as_str()),
            "case {case:?}: the refusal must name {first:?}, got {message:?}"
        );
    }
}

/// A filter name this operation answers none of is refused, naming the filter — never read
/// as "no results". The rows span a misspelling, a filter of ANOTHER describe, this
/// operation's own names in the wrong case ("Filter names are case-sensitive"), a `tag:`
/// prefix with no key, and an empty name.
#[tokio::test]
async fn a_filter_this_operation_does_not_answer_is_refused() {
    let (router, _engine, _ids) = seeded();

    for name in [
        "transit-gateway-route-table-announcement-state",
        "announcement-id",
        "owner-id",
        "vpc-id",
        "State",
        "Announcement-Direction",
        "tag:",
        "",
    ] {
        let case = format!("filter {name:?}");
        let (status, body) = describe(&router, &filters(&[(name, &["anything"])])).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            ec2::state::INVALID_PARAMETER_VALUE,
            "case {case:?}: expected InvalidParameterValue, got {body}"
        );
        let message = unescaped(&element(&body, "Message").unwrap_or_default());
        assert!(
            message.contains(name),
            "case {case:?}: the refusal must name the filter, got {message:?}"
        );
    }
}

/// The ceilings this handler puts on the SHAPE of a request, which the model bounds nowhere
/// (`FilterList`, `Filter$Values` and
/// `TransitGatewayRouteTableAnnouncementIdStringList` carry no `@length`), so the handler is
/// what enforces them. Every row is asserted AT the published ceiling — where the request is
/// answered, not refused for its size — and one past it, where it is refused
/// `InvalidParameterValue`: a ceiling that drifted either way fails here.
///
/// At the id ceiling the ids still name no announcement, so that row is answered with the
/// announcement's own `NotFound` code: proof the SIZE check passed and the request went on to
/// be resolved.
#[tokio::test]
async fn a_request_past_a_published_shape_ceiling_is_refused() {
    let (router, _engine, _ids) = seeded();
    let prefix = ec2::state::TRANSIT_GATEWAY_ROUTE_TABLE_ANNOUNCEMENT_ID_PREFIX;
    // A state no announcement is in, so an in-bounds filter row selects nothing and the
    // assertion is about the ceiling rather than about what matched.
    let unmatched = "no-such-state";
    let one_unmatched: [&str; 1] = [unmatched];
    let at_byte_ceiling = "v".repeat(ec2::state::MAX_FILTER_VALUE_BYTES);
    let past_byte_ceiling = "v".repeat(ec2::state::MAX_FILTER_VALUE_BYTES + 1);

    // (case, the request's fields, the error code expected — `None` where HTTP 200 is)
    let mut cases: Vec<(String, Vec<Param>, Option<&str>)> = Vec::new();
    for (case, count, expected) in [
        ("filters at the ceiling", ec2::state::MAX_FILTERS, None),
        (
            "one filter past the ceiling",
            ec2::state::MAX_FILTERS + 1,
            Some(ec2::state::INVALID_PARAMETER_VALUE),
        ),
    ] {
        let entries: Vec<(&str, &[&str])> = (0..count)
            .map(|_| ("state", &one_unmatched[..]))
            .collect();
        cases.push((case.to_string(), filters(&entries), expected));
    }
    for (case, count, expected) in [
        (
            "filter values at the ceiling",
            ec2::state::MAX_FILTER_VALUES,
            None,
        ),
        (
            "one filter value past the ceiling",
            ec2::state::MAX_FILTER_VALUES + 1,
            Some(ec2::state::INVALID_PARAMETER_VALUE),
        ),
    ] {
        let values: Vec<&str> = (0..count).map(|_| unmatched).collect();
        cases.push((case.to_string(), filters(&[("state", &values)]), expected));
    }
    for (case, value, expected) in [
        ("a filter value at the byte ceiling", &at_byte_ceiling, None),
        (
            "a filter value one byte past the ceiling",
            &past_byte_ceiling,
            Some(ec2::state::INVALID_PARAMETER_VALUE),
        ),
    ] {
        cases.push((
            case.to_string(),
            filters(&[("state", &[value.as_str()])]),
            expected,
        ));
    }
    for (case, count, expected) in [
        (
            "ids at the ceiling",
            ec2::state::MAX_RESOURCE_IDS,
            Some(ec2::state::INVALID_TRANSIT_GATEWAY_ROUTE_TABLE_ANNOUNCEMENT_ID_NOT_FOUND),
        ),
        (
            "one id past the ceiling",
            ec2::state::MAX_RESOURCE_IDS + 1,
            Some(ec2::state::INVALID_PARAMETER_VALUE),
        ),
    ] {
        let named: Vec<String> = (0..count)
            .map(|index| format!("{prefix}-{:017x}", index + 1))
            .collect();
        let borrowed: Vec<&str> = named.iter().map(String::as_str).collect();
        cases.push((case.to_string(), id_params(&borrowed), expected));
    }

    for (case, params, expected) in cases {
        let (status, body) = describe(&router, &params).await;
        match expected {
            None => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: a request at a ceiling is answered, got {status} with body \
                     {body}"
                );
                let reported = reported_ids(&body);
                assert!(
                    reported.is_empty(),
                    "case {case:?}: no announcement carries {unmatched:?}, got {reported:?} in \
                     {body}"
                );
            }
            Some(code) => {
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected HTTP 400, got {status} with body {body}"
                );
                assert_eq!(
                    error_code(&body),
                    code,
                    "case {case:?}: expected {code}, got {body}"
                );
            }
        }
    }
}

/// The answer pages in id order: a page carries at most `MaxResults` announcements and a
/// token naming the id the next page starts at, the token walks the whole set exactly once,
/// and the last page carries none. Asserted at the published floor of
/// `TransitGatewayMaxResults`, at a size between the bounds, and at a size that holds
/// everything.
#[tokio::test]
async fn the_answer_pages_in_id_order_with_a_token_that_advances() {
    let (router, _engine, ids) = seeded();
    let mut all = ids.clone();
    all.sort();

    for page_size in [5_usize, 7, 1000] {
        let case = format!("MaxResults {page_size}");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        // One iteration per page, plus one to catch a token that never clears.
        for _ in 0..=all.len() {
            let mut params = vec![param("MaxResults", &page_size.to_string())];
            if let Some(cursor) = token.as_deref() {
                params.push(param("NextToken", cursor));
            }
            let body = describe_ok(&case, &router, &params).await;
            let page = reported_ids(&body);
            assert!(
                page.len() <= page_size,
                "case {case:?}: a page carries at most {page_size}, got {page:?}"
            );
            walked.extend(page.clone());
            token = element(&body, "nextToken");
            match token.as_deref() {
                None => break,
                Some(cursor) => assert!(
                    !cursor.is_empty() && page.len() == page_size,
                    "case {case:?}: a token is minted only for a full page, got {body}"
                ),
            }
        }
        assert!(
            token.is_none(),
            "case {case:?}: the walk must end with no token, got {token:?}"
        );
        assert_eq!(
            walked, all,
            "case {case:?}: the pages must cover every announcement once, in id order, got \
             {walked:?}"
        );
    }
}

/// A `MaxResults` outside the range the model publishes (`TransitGatewayMaxResults`,
/// `range 5..1000`) is refused at the boundary, one past each published bound, while each
/// bound itself is served. A page size below the smallest the operation serves carries EC2's
/// own `InvalidMaxResults`; one above the largest keeps the boundary validator's
/// `ValidationError`. Either way the message names the member it rejected — asserted, so a
/// 400 for some other reason could not pass for it.
#[tokio::test]
async fn a_page_size_outside_the_published_range_is_refused() {
    let (router, _engine, _ids) = seeded();

    for (case, page_size, expected_status, expected_code) in [
        (
            "one below the published minimum",
            4,
            400,
            INVALID_MAX_RESULTS,
        ),
        ("the published minimum", 5, 200, ""),
        ("the published maximum", 1000, 200, ""),
        (
            "one above the published maximum",
            1001,
            400,
            BOUNDARY_VALIDATION_ERROR,
        ),
    ] {
        let (status, body) =
            describe(&router, &[param("MaxResults", &page_size.to_string())]).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        if expected_status == 400 {
            assert_eq!(
                error_code(&body),
                expected_code,
                "case {case:?}: expected {expected_code}, got {body}"
            );
            let message = unescaped(&element(&body, "Message").unwrap_or_default());
            assert!(
                message.contains("maxResults"),
                "case {case:?}: the refusal must name the member it rejected, got {message:?}"
            );
        }
    }
}

/// The wire error code the boundary validator refuses a request with when a modelled
/// constraint rejects it before the handler runs — as `MaxResults` ABOVE
/// `TransitGatewayMaxResults`' published maximum is.
const BOUNDARY_VALIDATION_ERROR: &str = "ValidationError";

/// EC2's own code for a page size below the smallest the operation serves, which this
/// service answers in place of the boundary validator's generic one (the service's
/// `INVALID_MAX_RESULTS`).
const INVALID_MAX_RESULTS: &str = "InvalidMaxResults";

/// A `NextToken` this service did not mint is refused rather than read as an id, whatever
/// else the request names.
#[tokio::test]
async fn a_token_this_service_did_not_mint_is_refused() {
    let (router, _engine, ids) = seeded();

    for (case, token) in [
        ("plain text", "not-a-token"),
        ("an id of the kind being paged", ids[0].as_str()),
        ("odd-length hex", "abc"),
    ] {
        let (status, body) = describe(&router, &[param("NextToken", token)]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            ec2::state::INVALID_PAGINATION_TOKEN,
            "case {case:?}: expected InvalidPaginationToken, got {body}"
        );
    }
}

/// An authorized dry run reports the permission outcome and describes nothing, whichever
/// selector the request carries — "checks whether you have the required permissions for the
/// action, WITHOUT ACTUALLY MAKING THE REQUEST".
#[tokio::test]
async fn a_dry_run_reports_the_permission_outcome_and_describes_nothing() {
    let (router, _engine, ids) = seeded();

    let cases: Vec<(&str, Vec<Param>)> = vec![
        ("no selector", Vec::new()),
        (
            "an id that names an announcement",
            id_params(&[ids[0].as_str()]),
        ),
        (
            "a filter that selects announcements",
            filters(&[("state", &[announcement_state("available")])]),
        ),
    ];

    for (case, selector) in cases {
        let mut params = selector;
        params.push(param("DryRun", "true"));
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, DRY_RUN_STATUS,
            "case {case:?}: expected HTTP {DRY_RUN_STATUS}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            ec2::state::DRY_RUN_OPERATION,
            "case {case:?}: expected DryRunOperation, got {body}"
        );
        assert!(
            reported_ids(&body).is_empty(),
            "case {case:?}: a dry run describes nothing, got {body}"
        );
    }
}

/// Describing changes nothing a later caller sees: the same request answers the same
/// announcements twice over, and the store holds exactly the announcements it held before —
/// a read neither mints nor moves a resource.
#[tokio::test]
async fn describing_does_not_change_what_a_later_caller_sees() {
    let (router, engine, ids) = seeded();
    let case = "the same request twice";

    let stored_before = engine
        .state::<Ec2State>()
        .lock()
        .transit_gateway_route_table_announcements
        .len();
    let first = describe_ok(case, &router, &[]).await;
    let second = describe_ok(case, &router, &[]).await;
    assert_eq!(
        reported_ids(&first),
        reported_ids(&second),
        "case {case:?}: two reads must report the same announcements, got {first} then {second}"
    );
    for name in [
        "state",
        "announcementDirection",
        "transitGatewayId",
        "transitGatewayRouteTableId",
        "peeringAttachmentId",
        "coreNetworkId",
        "creationTime",
        "key",
        "value",
    ] {
        assert_eq!(
            elements(&first, name),
            elements(&second, name),
            "case {case:?}: <{name}> must not change between reads, got {first} then {second}"
        );
    }
    let stored_after = engine
        .state::<Ec2State>()
        .lock()
        .transit_gateway_route_table_announcements
        .len();
    assert_eq!(
        stored_after, stored_before,
        "case {case:?}: the store held {stored_before} announcements, now holds {stored_after}"
    );
    assert_eq!(
        stored_after,
        ids.len(),
        "case {case:?}: the store must still hold every seeded announcement"
    );
}

/// The delete the announcement family already serves and this describe read it back: an
/// announcement a `DeleteTransitGatewayRouteTableAnnouncement` has started on reads back in
/// the TRANSIENT `deleting` state, and in `deleted` once the clock-owed move falls due — so
/// the describe reports the state the store actually holds rather than the one its writer
/// first stored.
#[tokio::test]
async fn a_deleted_announcement_reads_back_in_the_state_the_delete_left_it() {
    let (router, _engine, ids) = seeded();
    let announcement_id = ids[0].as_str();
    let case = "an announcement a delete has started on";

    let (status, body) = super::ec2_query::call(
        &router,
        REGION,
        "DeleteTransitGatewayRouteTableAnnouncement",
        &[param("TransitGatewayRouteTableAnnouncementId", announcement_id)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the delete must succeed, got {status} with body {body}"
    );

    let body = describe_ok(case, &router, &id_params(&[announcement_id])).await;
    let reported = member(case, &body, "state");
    assert_eq!(
        reported.as_deref(),
        Some(ec2::state::ANNOUNCEMENT_STATE_DELETING),
        "case {case:?}: expected the transient state the delete left, got {reported:?} in {body}"
    );

    // Past the span the clock owes the move, the same request reports the terminal state —
    // without this read having been what moved it.
    tokio::time::sleep(ec2::state::TRANSITION_DELAY + std::time::Duration::from_millis(250)).await;
    let case = "once the delete's move has fallen due";
    let body = describe_ok(case, &router, &id_params(&[announcement_id])).await;
    let reported = member(case, &body, "state");
    assert_eq!(
        reported.as_deref(),
        Some(ec2::state::ANNOUNCEMENT_STATE_DELETED),
        "case {case:?}: expected the terminal state the clock owed, got {reported:?} in {body}"
    );
}
