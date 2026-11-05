// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DescribeTransitGatewayRouteTables`, driven over the wire the
//! service actually speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose
//! body carries `Action=DescribeTransitGatewayRouteTables`, answered with XML (README.md /
//! OPERATIONS.md "Wire protocol").
//!
//! The rule under test is the one every EC2 describe follows: it REPORTS THE RESOURCES THE
//! STORE HOLDS, each with every member it carries, narrowed by the id list and by every
//! filter the operation publishes (joined with AND), in resource-id order, paged by
//! `MaxResults` with a token the next page resumes after — and it refuses what it cannot
//! resolve rather than answering an absent row: an unknown id with that resource's own
//! `NotFound` code, a value that is not an id of the kind with its `Malformed` code, a filter
//! name the operation publishes none of, and a token the service did not mint.
//!
//! Every table row below is a distinct input along what that rule generalises over: which
//! transit gateway a route table belongs to, which of the four modelled states it is in,
//! which default role it holds, when it was created, whether it carries tags (several, one,
//! an empty set, and no tag member at all), how many tables one gateway holds, which selector
//! the request uses, how many ids it names, and how large a page it asks for (both published
//! bounds of `TransitGatewayMaxResults` and one past each). The seeded tables are written
//! through the store's own writer (`insert_transit_gateway_route_table`) and then put into
//! the state and default roles each row names, because the only writers that mint a route
//! table in another state are still scaffold stubs.
//!
//! The round trip is asserted separately, over the creates that ARE implemented:
//! `CreateTransitGateway`'s default route table and `CreateTransitGatewayRouteTable`'s
//! additional one both read back through this describe exactly as they were stored, including
//! the `pending` -> `available` move the clock owes each of them.

use std::time::{Duration, SystemTime};

use ec2::ops::types::{TRANSIT_GATEWAY_ROUTE_TABLE_STATE_VALUES, Tag};
use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, TRANSITION_DELAY,
    TRANSIT_GATEWAY_ROUTE_TABLE_ID_PREFIX, TRANSIT_GATEWAY_ROUTE_TABLE_STATE_AVAILABLE,
    TRANSIT_GATEWAY_ROUTE_TABLE_STATE_PENDING,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{as_timestamp, element, elements, error_code, filters, indexed, unescaped};
use super::wire::{Param, child, child_text, document, param};

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// The HTTP status EC2 answers `DryRunOperation` with.
const DRY_RUN_STATUS: u16 = 412;

/// The ec2Query `Action` under test.
const ACTION: &str = "DescribeTransitGatewayRouteTables";

/// Long enough past [`TRANSITION_DELAY`] that a move the clock owes is certainly due.
fn past_the_transition() -> Duration {
    TRANSITION_DELAY + Duration::from_millis(250)
}

/// One seeded route table: the facts that distinguish it along what the rule generalises
/// over.
struct Fixture {
    /// The transit gateway the route table belongs to — the `transit-gateway-id` filter's
    /// value.
    transit_gateway_id: &'static str,
    /// The table's state, a member of `TransitGatewayRouteTableState`.
    state: &'static str,
    /// Whether it is its gateway's default ASSOCIATION route table.
    default_association: bool,
    /// Whether it is its gateway's default PROPAGATION route table.
    default_propagation: bool,
    /// The instant the table was created, as seconds since the epoch.
    created_at: u64,
    /// The tags it carries, as (key, value) pairs — `Some(&[])` for a table carrying an
    /// empty tag set and `None` for one the store holds no tag member for at all.
    tags: Option<&'static [(&'static str, &'static str)]>,
}

/// A member of `TransitGatewayRouteTableState`, from the modelled catalog
/// (`smithy_query(query="shape",shape="TransitGatewayRouteTableState")`, whose values are
/// `available | deleted | deleting | pending`) rather than written out here.
fn table_state(name: &str) -> &'static str {
    TRANSIT_GATEWAY_ROUTE_TABLE_STATE_VALUES
        .iter()
        .find(|value| **value == name)
        .unwrap_or_else(|| panic!("{name:?} is no TransitGatewayRouteTableState member"))
}

/// The seven route tables every test here is written against: three transit gateways holding
/// two, two and three tables, all four states the modelled catalog publishes (`available`
/// three times and `pending` twice, so a state filter selects more than one row), each
/// default role held by some table and not by others, distinct creation instants, and tables
/// carrying several tags, one tag, an empty tag set and no tag member at all.
fn fixtures() -> Vec<Fixture> {
    vec![
        Fixture {
            transit_gateway_id: "tgw-0000000000000000a",
            state: table_state("available"),
            default_association: true,
            default_propagation: true,
            created_at: 1_760_000_000,
            tags: Some(&[("Name", "north"), ("Team", "network")]),
        },
        Fixture {
            transit_gateway_id: "tgw-0000000000000000a",
            state: table_state("pending"),
            default_association: false,
            default_propagation: false,
            created_at: 1_760_000_100,
            tags: Some(&[]),
        },
        Fixture {
            transit_gateway_id: "tgw-0000000000000000b",
            state: table_state("available"),
            default_association: false,
            default_propagation: false,
            created_at: 1_760_000_200,
            tags: None,
        },
        Fixture {
            transit_gateway_id: "tgw-0000000000000000b",
            state: table_state("deleting"),
            default_association: false,
            default_propagation: true,
            created_at: 1_760_000_300,
            tags: Some(&[("Team", "network")]),
        },
        Fixture {
            transit_gateway_id: "tgw-0000000000000000c",
            state: table_state("deleted"),
            default_association: true,
            default_propagation: false,
            created_at: 1_760_000_400,
            tags: None,
        },
        Fixture {
            transit_gateway_id: "tgw-0000000000000000c",
            state: table_state("available"),
            default_association: false,
            default_propagation: false,
            created_at: 1_760_000_500,
            tags: Some(&[("Name", "north")]),
        },
        Fixture {
            transit_gateway_id: "tgw-0000000000000000c",
            state: table_state("pending"),
            default_association: false,
            default_propagation: false,
            created_at: 1_760_000_600,
            tags: Some(&[("Name", "east")]),
        },
    ]
}

/// The modelled tags one fixture is stored with.
fn stored_tags(fixture: &Fixture) -> Vec<Tag> {
    fixture
        .tags
        .unwrap_or_default()
        .iter()
        .map(|(key, value)| Tag {
            key: Some((*key).to_string()),
            value: Some((*value).to_string()),
        })
        .collect()
}

/// Seed [`fixtures`] into the store through its own writer and answer the ids it minted, in
/// fixture order.
///
/// The writer mints every table `pending` with neither default role, so each row's state and
/// roles are then put on the stored record — and the `available` move the writer owed each of
/// them is dropped, so a seeded state is the one the row names however long the test runs.
fn seed(engine: &engine::InMemoryEngine) -> Vec<String> {
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    let rows = fixtures();
    let ids: Vec<String> = rows
        .iter()
        .map(|fixture| {
            let created_at = SystemTime::UNIX_EPOCH + Duration::from_secs(fixture.created_at);
            guard
                .insert_transit_gateway_route_table(
                    fixture.transit_gateway_id,
                    created_at,
                    stored_tags(fixture),
                )
                .and_then(|route_table| route_table.transit_gateway_route_table_id)
                .expect("the store holds the seeded route table")
        })
        .collect();
    for (fixture, route_table_id) in rows.iter().zip(&ids) {
        let route_table = guard
            .transit_gateway_route_tables
            .get_mut(route_table_id)
            .expect("the seeded route table is stored");
        route_table.state = Some(fixture.state.to_string());
        route_table.default_association_route_table = Some(fixture.default_association);
        route_table.default_propagation_route_table = Some(fixture.default_propagation);
        route_table.tags = fixture.tags.map(|_| stored_tags(fixture));
    }
    guard.transit_gateway_route_table_available_at.clear();
    ids
}

/// The emulator with the tables above seeded: the router to call and the ids they were
/// given, in fixture order.
fn seeded() -> (
    MultiServiceRouter,
    std::sync::Arc<engine::InMemoryEngine>,
    Vec<String>,
) {
    let (router, engine) = super::ec2_query::emulator();
    let ids = seed(&engine);
    (router, engine, ids)
}

/// One `DescribeTransitGatewayRouteTables` call carrying `params`.
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

/// The ids of every route table a response reports, in document order.
fn reported_ids(body: &str) -> Vec<String> {
    elements(body, "transitGatewayRouteTableId")
}

/// `TransitGatewayRouteTableIds.N` form fields for `ids`.
fn id_params(ids: &[&str]) -> Vec<Param> {
    indexed("TransitGatewayRouteTableIds", ids)
}

/// Read something off the one route table element a response carries — the element that has
/// a `transitGatewayRouteTableId` child.
fn with_table<T>(case: &str, body: &str, read: impl FnOnce(roxmltree::Node<'_, '_>) -> T) -> T {
    let parsed = document(case, body);
    let table = parsed
        .descendants()
        .find(|node| node.is_element() && child(*node, "transitGatewayRouteTableId").is_some())
        .unwrap_or_else(|| panic!("case {case:?}: no route table in {body}"));
    read(table)
}

/// A member of the one route table a response carries, or `None` when it carries no such
/// member.
fn member(case: &str, body: &str, name: &str) -> Option<String> {
    with_table(case, body, |table| child_text(table, name))
}

/// Whether the one route table a response carries has a `name` child ELEMENT at all.
///
/// The absence check for a member nothing recorded: [`member`] reads an element's TEXT,
/// which a present-but-empty container (`<tagSet/>`) does not carry, so only element absence
/// shows that nothing was reported for it.
fn has_member(case: &str, body: &str, name: &str) -> bool {
    with_table(case, body, |table| child(table, name).is_some())
}

/// The (key, value) pairs of the one route table a response carries.
fn reported_tags(case: &str, body: &str) -> Vec<(String, String)> {
    with_table(case, body, |table| {
        super::wire::set_items(table, "tagSet")
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

/// Every fact a stored route table carries reads back: the id the store minted, the transit
/// gateway it belongs to, the state it is in, both default-role flags, the instant it was
/// created and the tags on it. One row per seeded table, so no single table is the
/// specification — and the row that holds no tag member asserts that a member the store
/// holds no value for is reported absent rather than invented.
#[tokio::test]
async fn every_fact_a_stored_route_table_carries_reads_back() {
    let (router, _engine, ids) = seeded();

    for (fixture, route_table_id) in fixtures().iter().zip(&ids) {
        let case = format!(
            "route table {route_table_id} of {}",
            fixture.transit_gateway_id
        );
        let body = describe_ok(&case, &router, &id_params(&[route_table_id.as_str()])).await;

        assert_eq!(
            reported_ids(&body),
            vec![route_table_id.clone()],
            "case {case:?}: expected exactly the route table named, got {body}"
        );
        for (name, expected) in [
            ("transitGatewayId", fixture.transit_gateway_id.to_string()),
            ("state", fixture.state.to_string()),
            (
                "defaultAssociationRouteTable",
                fixture.default_association.to_string(),
            ),
            (
                "defaultPropagationRouteTable",
                fixture.default_propagation.to_string(),
            ),
        ] {
            let actual = member(&case, &body, name);
            assert_eq!(
                actual.as_deref(),
                Some(expected.as_str()),
                "case {case:?}: expected <{name}> {expected:?}, got {actual:?} in {body}"
            );
        }
        let reported_creation = member(&case, &body, "creationTime");
        let seconds = reported_creation.as_deref().and_then(as_timestamp);
        let expected_seconds =
            ::aws_smithy_types::DateTime::from_secs(fixture.created_at as i64).as_secs_f64();
        assert_eq!(
            seconds,
            Some(expected_seconds),
            "case {case:?}: expected the instant the table was created, got \
             {reported_creation:?} in {body}"
        );

        match fixture.tags {
            None => assert!(
                !has_member(&case, &body, "tagSet"),
                "case {case:?}: a table the store holds no tag member for carries no <tagSet>, \
                 got {body}"
            ),
            Some(tags) => {
                let expected: Vec<(String, String)> = tags
                    .iter()
                    .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
                    .collect();
                let actual = reported_tags(&case, &body);
                assert_eq!(
                    actual, expected,
                    "case {case:?}: expected the tags stored, got {actual:?} in {body}"
                );
            }
        }
    }
}

/// A request that names no selector reports every route table the store holds, in
/// `TransitGatewayRouteTableId` order.
#[tokio::test]
async fn an_unselective_describe_reports_every_stored_table_in_id_order() {
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
/// has created reports no route tables at all.
#[tokio::test]
async fn an_empty_store_reports_no_route_tables() {
    let (router, _engine) = super::ec2_query::emulator();
    let case = "empty store";

    let body = describe_ok(case, &router, &[]).await;
    let actual = reported_ids(&body);
    assert!(
        actual.is_empty(),
        "case {case:?}: expected no route tables, got {actual:?} in {body}"
    );
}

/// Each filter selects exactly the tables carrying its value, and several filters are joined
/// with AND. The rows span every filter name the operation publishes, both values of each
/// boolean filter, a value nothing carries, a wildcard value, two values in one filter, and
/// two-filter combinations.
#[tokio::test]
async fn each_filter_selects_the_tables_that_carry_its_value() {
    let (router, _engine, ids) = seeded();
    let rows = fixtures();

    // (case, the filters as sent, the fixture positions expected back)
    let cases: Vec<(&str, Vec<Param>, Vec<usize>)> = vec![
        (
            "state available",
            filters(&[("state", &[table_state("available")])]),
            vec![0, 2, 5],
        ),
        (
            "state pending",
            filters(&[("state", &[table_state("pending")])]),
            vec![1, 6],
        ),
        (
            "state deleting",
            filters(&[("state", &[table_state("deleting")])]),
            vec![3],
        ),
        (
            "state deleted",
            filters(&[("state", &[table_state("deleted")])]),
            vec![4],
        ),
        (
            "two states in one filter",
            filters(&[("state", &[table_state("deleting"), table_state("deleted")])]),
            vec![3, 4],
        ),
        (
            "transit-gateway-id of two tables",
            filters(&[("transit-gateway-id", &[rows[0].transit_gateway_id])]),
            vec![0, 1],
        ),
        (
            "transit-gateway-id of three tables",
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
            "default-association-route-table true",
            filters(&[("default-association-route-table", &["true"])]),
            vec![0, 4],
        ),
        (
            "default-association-route-table false",
            filters(&[("default-association-route-table", &["false"])]),
            vec![1, 2, 3, 5, 6],
        ),
        (
            "default-propagation-route-table true",
            filters(&[("default-propagation-route-table", &["true"])]),
            vec![0, 3],
        ),
        (
            "default-propagation-route-table false",
            filters(&[("default-propagation-route-table", &["false"])]),
            vec![1, 2, 4, 5, 6],
        ),
        (
            "both default-role filters true, joined with AND",
            filters(&[
                ("default-association-route-table", &["true"]),
                ("default-propagation-route-table", &["true"]),
            ]),
            vec![0],
        ),
        (
            "a default-role filter and a state joined with AND",
            filters(&[
                ("default-association-route-table", &["true"]),
                ("state", &[table_state("deleted")]),
            ]),
            vec![4],
        ),
        (
            "a boolean filter value that is neither true nor false",
            filters(&[("default-association-route-table", &["yes"])]),
            Vec::new(),
        ),
        (
            "state and transit-gateway-id joined with AND",
            filters(&[
                ("state", &[table_state("available")]),
                ("transit-gateway-id", &[rows[4].transit_gateway_id]),
            ]),
            vec![5],
        ),
        (
            "state and transit-gateway-id that no one table carries together",
            filters(&[
                ("state", &[table_state("deleting")]),
                ("transit-gateway-id", &[rows[0].transit_gateway_id]),
            ]),
            Vec::new(),
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

    // The id filter is the id selector's twin, so it is asserted over every seeded table
    // rather than over one row.
    for route_table_id in &ids {
        let case = format!("transit-gateway-route-table-id {route_table_id}");
        let body = describe_ok(
            &case,
            &router,
            &filters(&[(
                "transit-gateway-route-table-id",
                &[route_table_id.as_str()],
            )]),
        )
        .await;
        let actual = reported_ids(&body);
        assert_eq!(
            actual,
            vec![route_table_id.clone()],
            "case {case:?}: expected exactly that table, got {actual:?} in {body}"
        );
    }

    // An id of the right form that no table carries selects nothing, rather than everything.
    let case = "transit-gateway-route-table-id nothing carries";
    let body = describe_ok(
        case,
        &router,
        &filters(&[(
            "transit-gateway-route-table-id",
            &[format!("{TRANSIT_GATEWAY_ROUTE_TABLE_ID_PREFIX}-00000000000000fff").as_str()],
        )]),
    )
    .await;
    assert!(
        reported_ids(&body).is_empty(),
        "case {case:?}: expected no route tables, got {body}"
    );
}

/// One row of the id-list table: the case, the ids the request names, the filters it carries
/// beside them, and the fixture positions the answer must carry.
type IdListCase<'a> = (&'static str, Vec<&'a str>, Vec<Param>, Vec<usize>);

/// The id list selects exactly the tables it names, whatever its length and however often an
/// id is repeated, and it narrows what a filter would have selected.
#[tokio::test]
async fn an_id_list_selects_exactly_the_tables_it_names() {
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
            filters(&[("state", &[table_state("pending")])]),
            vec![1],
        ),
        (
            "two ids narrowed by a default-role filter",
            vec![ids[0].as_str(), ids[5].as_str()],
            filters(&[("default-association-route-table", &["true"])]),
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

/// A well-formed id naming no route table is refused with that resource's own `NotFound`
/// code, and a value that is not a transit gateway route table id at all with its `Malformed`
/// code — so a client can tell the two apart, and neither is reported as an absent row. Both
/// are asserted beside an id that DOES name a table, so the refusal does not depend on the
/// request naming nothing real.
#[tokio::test]
async fn an_unknown_id_is_refused_not_found_and_a_value_that_is_no_id_malformed() {
    let (router, _engine, ids) = seeded();
    let prefix = TRANSIT_GATEWAY_ROUTE_TABLE_ID_PREFIX;

    let cases: Vec<(&str, Vec<String>, &str)> = vec![
        (
            "one unknown id",
            vec![format!("{prefix}-00000000000000fff")],
            ec2::state::INVALID_ROUTE_TABLE_ID_NOT_FOUND,
        ),
        (
            "a known id beside an unknown one",
            vec![ids[0].clone(), format!("{prefix}-00000000000000eee")],
            ec2::state::INVALID_ROUTE_TABLE_ID_NOT_FOUND,
        ),
        (
            "a suffix that is not hexadecimal",
            vec![format!("{prefix}-not-hex")],
            ec2::state::INVALID_ROUTE_TABLE_ID_MALFORMED,
        ),
        (
            "an id of another resource of the same family",
            vec!["tgw-attach-0000000000000000a".to_string()],
            ec2::state::INVALID_ROUTE_TABLE_ID_MALFORMED,
        ),
        (
            "the transit gateway's own id where a route table id belongs",
            vec![fixtures()[0].transit_gateway_id.to_string()],
            ec2::state::INVALID_ROUTE_TABLE_ID_MALFORMED,
        ),
        (
            "a known id beside a value that is no id",
            vec![ids[0].clone(), "not-an-id".to_string()],
            ec2::state::INVALID_ROUTE_TABLE_ID_MALFORMED,
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

/// A filter name this operation publishes none of is refused, naming the filter — never read
/// as "no results". The rows span a misspelling, a filter of ANOTHER describe, the tag family
/// this operation's own filter list does not publish, this operation's own names in the wrong
/// case ("Filter names are case-sensitive"), and an empty name.
#[tokio::test]
async fn a_filter_this_operation_does_not_publish_is_refused() {
    let (router, _engine, _ids) = seeded();

    for name in [
        "transit-gateway-route-table-state",
        "owner-id",
        "creation-time",
        "tag-key",
        "tag:Name",
        "tag:",
        "State",
        "Transit-Gateway-Id",
        "default_association_route_table",
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
/// (`FilterList`, `Filter$Values` and `TransitGatewayRouteTableIdStringList` carry no
/// `@length`), so the handler is what enforces them. Every row is asserted AT the published
/// ceiling — where the request is answered, not refused for its size — and one past it, where
/// it is refused `InvalidParameterValue`: a ceiling that drifted either way fails here.
///
/// At the id ceiling the ids still name no table, so that row is answered with the route
/// table's own `NotFound` code: proof the SIZE check passed and the request went on to be
/// resolved.
#[tokio::test]
async fn a_request_past_a_published_shape_ceiling_is_refused() {
    let (router, _engine, _ids) = seeded();
    let prefix = TRANSIT_GATEWAY_ROUTE_TABLE_ID_PREFIX;
    // A state no table carries, so an in-bounds filter row selects nothing and the assertion
    // is about the ceiling rather than about what matched.
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
        let entries: Vec<(&str, &[&str])> =
            (0..count).map(|_| ("state", &one_unmatched[..])).collect();
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
            Some(ec2::state::INVALID_ROUTE_TABLE_ID_NOT_FOUND),
        ),
        (
            "one id past the ceiling",
            ec2::state::MAX_RESOURCE_IDS + 1,
            Some(ec2::state::INVALID_PARAMETER_VALUE),
        ),
    ] {
        let ids: Vec<String> = (0..count)
            .map(|index| format!("{prefix}-{:017x}", index + 1))
            .collect();
        let borrowed: Vec<&str> = ids.iter().map(String::as_str).collect();
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
                    "case {case:?}: no table carries {unmatched:?}, got {reported:?} in {body}"
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

/// The answer pages in id order: a page carries at most `MaxResults` tables and a token the
/// next page resumes after, the token walks the whole set exactly once, and the last page
/// carries none. Asserted at the published floor of `TransitGatewayMaxResults`, at a size
/// between the bounds, and at a size that holds everything.
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
            "case {case:?}: the pages must cover every table once, in id order, got {walked:?}"
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
        let (status, body) = describe(&router, &[param("MaxResults", &page_size.to_string())]).await;
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
        ("an id that names a table", id_params(&[ids[0].as_str()])),
        (
            "a filter that selects tables",
            filters(&[("state", &[table_state("available")])]),
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

/// Describing changes nothing a later caller sees: the same request answers the same tables
/// twice over, and the store holds exactly the tables it held before — a read neither mints
/// nor moves a resource.
#[tokio::test]
async fn describing_does_not_change_what_a_later_caller_sees() {
    let (router, engine, ids) = seeded();
    let case = "the same request twice";

    let stored_before = engine
        .state::<Ec2State>()
        .lock()
        .transit_gateway_route_tables
        .len();
    let first = describe_ok(case, &router, &[]).await;
    let second = describe_ok(case, &router, &[]).await;
    assert_eq!(
        reported_ids(&first),
        reported_ids(&second),
        "case {case:?}: two reads must report the same tables, got {first} then {second}"
    );
    for name in [
        "state",
        "transitGatewayId",
        "creationTime",
        "defaultAssociationRouteTable",
        "defaultPropagationRouteTable",
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
        .transit_gateway_route_tables
        .len();
    assert_eq!(
        stored_after, stored_before,
        "case {case:?}: the store held {stored_before} tables, now holds {stored_after}"
    );
    assert_eq!(
        stored_after,
        ids.len(),
        "case {case:?}: the store must still hold every seeded table"
    );
}

/// The round trip over the writers that ARE implemented: the default route table
/// `CreateTransitGateway` owes a gateway and the additional one
/// `CreateTransitGatewayRouteTable` adds both read back through this describe exactly as they
/// were stored — the default one in both default roles, the additional one in neither, each
/// in the `pending` state its create answered and each `available` once the move the clock
/// owed it falls due. A gateway created with both default-route-table options disabled holds
/// no route table at all until one is asked for, so the gateway filter reports exactly what
/// was created for it.
#[tokio::test]
async fn the_tables_the_implemented_creates_store_read_back() {
    let (router, _engine) = super::ec2_query::emulator();

    // A gateway created with the documented defaults, which owes it one default route table.
    let (status, body) = super::ec2_query::call(&router, REGION, "CreateTransitGateway", &[]).await;
    assert_eq!(
        status, 200,
        "CreateTransitGateway must succeed, got {status} with body {body}"
    );
    let with_defaults =
        element(&body, "transitGatewayId").expect("a created gateway reports its id");
    let default_route_table_id = element(&body, "associationDefaultRouteTableId")
        .expect("a gateway created with the documented defaults reports its default table");

    // A second gateway created with both default-route-table options disabled, which owes it
    // no route table at all.
    let (status, body) = super::ec2_query::call(
        &router,
        REGION,
        "CreateTransitGateway",
        &[
            param("Options.DefaultRouteTableAssociation", "disable"),
            param("Options.DefaultRouteTablePropagation", "disable"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateTransitGateway must succeed, got {status} with body {body}"
    );
    let without_defaults =
        element(&body, "transitGatewayId").expect("a created gateway reports its id");

    let case = "a gateway holding no default route table";
    let body = describe_ok(
        case,
        &router,
        &filters(&[("transit-gateway-id", &[without_defaults.as_str()])]),
    )
    .await;
    assert!(
        reported_ids(&body).is_empty(),
        "case {case:?}: nothing has created a route table for it yet, got {body}"
    );

    // The default route table of the first gateway, as this describe reports it.
    let case = "the default route table a create owed a gateway";
    let body = describe_ok(case, &router, &id_params(&[default_route_table_id.as_str()])).await;
    assert_eq!(
        reported_ids(&body),
        vec![default_route_table_id.clone()],
        "case {case:?}: expected exactly the default table, got {body}"
    );
    for (name, expected) in [
        ("transitGatewayId", with_defaults.as_str()),
        ("state", TRANSIT_GATEWAY_ROUTE_TABLE_STATE_PENDING),
        ("defaultAssociationRouteTable", "true"),
        ("defaultPropagationRouteTable", "true"),
    ] {
        let actual = member(case, &body, name);
        assert_eq!(
            actual.as_deref(),
            Some(expected),
            "case {case:?}: expected <{name}> {expected:?}, got {actual:?} in {body}"
        );
    }

    // An additional route table of the same gateway, created with one tag.
    let (status, body) = super::ec2_query::call(
        &router,
        REGION,
        "CreateTransitGatewayRouteTable",
        &[
            param("TransitGatewayId", &with_defaults),
            param(
                "TagSpecifications.1.ResourceType",
                ec2::state::RESOURCE_TYPE_TRANSIT_GATEWAY_ROUTE_TABLE,
            ),
            param("TagSpecifications.1.Tag.1.Key", "Name"),
            param("TagSpecifications.1.Tag.1.Value", "spoke"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateTransitGatewayRouteTable must succeed, got {status} with body {body}"
    );
    let additional_route_table_id =
        element(&body, "transitGatewayRouteTableId").expect("a created table reports its id");

    let case = "an additional route table of the same gateway";
    let body = describe_ok(
        case,
        &router,
        &id_params(&[additional_route_table_id.as_str()]),
    )
    .await;
    for (name, expected) in [
        ("transitGatewayId", with_defaults.as_str()),
        ("state", TRANSIT_GATEWAY_ROUTE_TABLE_STATE_PENDING),
        ("defaultAssociationRouteTable", "false"),
        ("defaultPropagationRouteTable", "false"),
    ] {
        let actual = member(case, &body, name);
        assert_eq!(
            actual.as_deref(),
            Some(expected),
            "case {case:?}: expected <{name}> {expected:?}, got {actual:?} in {body}"
        );
    }
    assert_eq!(
        reported_tags(case, &body),
        vec![("Name".to_string(), "spoke".to_string())],
        "case {case:?}: expected the tag the create asked for, got {body}"
    );

    // Both tables of that gateway, and only those, are reported for its id — in id order.
    let case = "every route table of one gateway";
    let body = describe_ok(
        case,
        &router,
        &filters(&[("transit-gateway-id", &[with_defaults.as_str()])]),
    )
    .await;
    let mut expected = vec![
        default_route_table_id.clone(),
        additional_route_table_id.clone(),
    ];
    expected.sort();
    let actual = reported_ids(&body);
    assert_eq!(
        actual, expected,
        "case {case:?}: expected {expected:?}, got {actual:?} in {body}"
    );

    // The move the clock owed each table falls due on its own, without anybody asking for it,
    // and this describe reports the state the store then holds.
    tokio::time::sleep(past_the_transition()).await;
    let case = "after the move the clock owed each table";
    let body = describe_ok(
        case,
        &router,
        &filters(&[("transit-gateway-id", &[with_defaults.as_str()])]),
    )
    .await;
    let states = elements(&body, "state");
    assert_eq!(
        states,
        vec![
            TRANSIT_GATEWAY_ROUTE_TABLE_STATE_AVAILABLE.to_string(),
            TRANSIT_GATEWAY_ROUTE_TABLE_STATE_AVAILABLE.to_string(),
        ],
        "case {case:?}: both tables were owed their available state, got {body}"
    );
    // And the state filter now selects them, which it did not while they were pending.
    let body = describe_ok(
        case,
        &router,
        &filters(&[
            ("transit-gateway-id", &[with_defaults.as_str()]),
            (
                "state",
                &[table_state(TRANSIT_GATEWAY_ROUTE_TABLE_STATE_AVAILABLE)],
            ),
        ]),
    )
    .await;
    let actual = reported_ids(&body);
    assert_eq!(
        actual, expected,
        "case {case:?}: expected {expected:?} to be available, got {actual:?} in {body}"
    );
}
