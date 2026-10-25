// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `SearchLocalGatewayRoutes`, driven over the wire the service
//! actually speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose body
//! carries `Action=SearchLocalGatewayRoutes`, answered with XML (README.md /
//! OPERATIONS.md "Wire protocol"). ec2Query is selected by the `ec2` signing name in the
//! request's SigV4 credential scope, so every call here carries one.
//!
//! The rule under test is that the operation REPORTS THE ROUTES OF THE NAMED LOCAL GATEWAY
//! ROUTE TABLE out of the store — narrowed by the `Filters` the request names (an attribute
//! filter by value, a `route-search.*` filter by the CIDR relation it names), cut into
//! `MaxResults`-sized pages, and refused with `InvalidLocalGatewayRouteTableID.NotFound`
//! when the account holds no table for the id.
//!
//! Every table row is a distinct input along what that generalises over — which filter,
//! which relation, how many values, the address family, the prefix length, which table, and
//! which page size — so the one case a recording happens to contain (an id of no table) is
//! one row among many rather than the specification.

use std::collections::BTreeMap;

use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, LocalGatewayRouteDestination,
    LocalGatewayRouteSpec, LocalGatewayRouteTableSpec, LOCAL_GATEWAY_ROUTE_STATE_ACTIVE,
    LOCAL_GATEWAY_ROUTE_STATE_BLACKHOLE, LOCAL_GATEWAY_ROUTE_STATE_PENDING,
    LOCAL_GATEWAY_ROUTE_TYPE_PROPAGATED, LOCAL_GATEWAY_ROUTE_TYPE_STATIC, MAX_FILTERS,
    MAX_FILTER_VALUES, MAX_FILTER_VALUE_BYTES,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, error_code, filters, unescaped};
use super::wire::document;

/// The Region every call here is signed for, and the Region the seeded route tables' ARNs
/// name (the table's ARN namespace is the Region its writer was signed for).
const REGION: &str = "us-east-1";

/// The ec2Query action under test.
const ACTION: &str = "SearchLocalGatewayRoutes";

/// The local gateway the seeded route tables belong to.
const LOCAL_GATEWAY_ID: &str = "lgw-00000000000000001";

/// The state a local gateway route table AWS has finished setting up is in — an
/// unconstrained string in the model (`LocalGatewayRouteTable$State`).
const ROUTE_TABLE_STATE: &str = "available";

/// "The mode of the local gateway route table" (`LocalGatewayRouteTableMode`), one of the
/// two members of the model's own catalog
/// (`smithy_query(query="shape",shape="LocalGatewayRouteTableMode")` — `direct-vpc-routing`
/// and `coip`); the seeded tables take one each so neither mode is the only one covered.
const MODE_DIRECT_VPC_ROUTING: &str = "direct-vpc-routing";

/// The other member of that catalog.
const MODE_COIP: &str = "coip";

/// Members of the model's own `LocalGatewayRouteState` catalog
/// (`smithy_query(query="shape",shape="LocalGatewayRouteState")`: `pending`, `active`,
/// `blackhole`, `deleting`, `deleted`), which the seeded routes span. `active` comes from
/// the crate's own export, so the catalog member has one definition.
const ROUTE_STATE_ACTIVE: &str = LOCAL_GATEWAY_ROUTE_STATE_ACTIVE;
const ROUTE_STATE_BLACKHOLE: &str = LOCAL_GATEWAY_ROUTE_STATE_BLACKHOLE;
const ROUTE_STATE_PENDING: &str = LOCAL_GATEWAY_ROUTE_STATE_PENDING;

/// What one seeded route's destination is: a CIDR block, or a prefix list — the two things
/// `LocalGatewayRoute` names a destination with.
#[derive(Debug, Clone, Copy)]
enum Destination {
    Cidr(&'static str),
    PrefixList(&'static str),
}

/// What one seeded route forwards to: exactly one of the four target members the modelled
/// `LocalGatewayRoute` carries.
#[derive(Debug, Clone, Copy)]
enum Target {
    VirtualInterfaceGroup(&'static str),
    NetworkInterface(&'static str),
    CoipPool(&'static str),
    Subnet(&'static str),
}

impl Target {
    /// The ec2Query element this target is reported in, and the value it carries.
    fn reported(self) -> (&'static str, &'static str) {
        match self {
            Self::VirtualInterfaceGroup(id) => ("localGatewayVirtualInterfaceGroupId", id),
            Self::NetworkInterface(id) => ("networkInterfaceId", id),
            Self::CoipPool(id) => ("coipPoolId", id),
            Self::Subnet(id) => ("subnetId", id),
        }
    }
}

/// One seeded route.
#[derive(Debug, Clone, Copy)]
struct RouteRow {
    destination: Destination,
    route_type: &'static str,
    state: &'static str,
    target: Target,
}

impl RouteRow {
    /// What names this route inside its table — its destination CIDR block, or its
    /// destination prefix list id.
    fn destination(&self) -> &'static str {
        match self.destination {
            Destination::Cidr(block) => block,
            Destination::PrefixList(prefix_list_id) => prefix_list_id,
        }
    }

    /// The route as the store's writer takes it.
    fn spec(&self, route_table_id: &str) -> LocalGatewayRouteSpec {
        let destination = match self.destination {
            Destination::Cidr(block) => {
                LocalGatewayRouteDestination::CidrBlock(block.to_string())
            }
            Destination::PrefixList(id) => {
                LocalGatewayRouteDestination::PrefixListId(id.to_string())
            }
        };
        let mut spec = LocalGatewayRouteSpec {
            local_gateway_route_table_id: route_table_id.to_string(),
            destination,
            local_gateway_virtual_interface_group_id: None,
            network_interface_id: None,
            subnet_id: None,
            coip_pool_id: None,
            route_type: self.route_type.to_string(),
            state: self.state.to_string(),
        };
        match self.target {
            Target::VirtualInterfaceGroup(id) => {
                spec.local_gateway_virtual_interface_group_id = Some(id.to_string());
            }
            Target::NetworkInterface(id) => spec.network_interface_id = Some(id.to_string()),
            Target::CoipPool(id) => spec.coip_pool_id = Some(id.to_string()),
            Target::Subnet(id) => spec.subnet_id = Some(id.to_string()),
        }
        spec
    }
}

/// The routes of the table every filter and paging table below is written against.
///
/// They differ along everything the rule generalises over: the address family, the prefix
/// length (two of them nested inside a third), the destination kind (a prefix list beside
/// the CIDR blocks), the route type, the state and the kind of target. Listed in the order
/// the store holds them — destination order, which is the order a search reports.
fn seeded_routes() -> Vec<RouteRow> {
    vec![
        RouteRow {
            destination: Destination::Cidr("10.0.0.0/16"),
            route_type: LOCAL_GATEWAY_ROUTE_TYPE_STATIC,
            state: ROUTE_STATE_ACTIVE,
            target: Target::VirtualInterfaceGroup("lgw-vif-grp-00000000000000001"),
        },
        RouteRow {
            destination: Destination::Cidr("10.0.1.0/29"),
            route_type: LOCAL_GATEWAY_ROUTE_TYPE_STATIC,
            state: ROUTE_STATE_ACTIVE,
            target: Target::VirtualInterfaceGroup("lgw-vif-grp-00000000000000002"),
        },
        RouteRow {
            destination: Destination::Cidr("10.0.1.0/31"),
            route_type: LOCAL_GATEWAY_ROUTE_TYPE_STATIC,
            state: ROUTE_STATE_BLACKHOLE,
            target: Target::VirtualInterfaceGroup("lgw-vif-grp-00000000000000002"),
        },
        RouteRow {
            destination: Destination::Cidr("10.0.2.0/24"),
            route_type: LOCAL_GATEWAY_ROUTE_TYPE_PROPAGATED,
            state: ROUTE_STATE_ACTIVE,
            target: Target::NetworkInterface("eni-00000000000000001"),
        },
        RouteRow {
            destination: Destination::Cidr("172.16.0.0/12"),
            route_type: LOCAL_GATEWAY_ROUTE_TYPE_STATIC,
            state: ROUTE_STATE_PENDING,
            target: Target::CoipPool("ipv4pool-coip-00000000000000001"),
        },
        RouteRow {
            destination: Destination::Cidr("2600:1f00:1000::/48"),
            route_type: LOCAL_GATEWAY_ROUTE_TYPE_STATIC,
            state: ROUTE_STATE_ACTIVE,
            target: Target::Subnet("subnet-00000000000000001"),
        },
        RouteRow {
            destination: Destination::PrefixList("pl-00000000000000001"),
            route_type: LOCAL_GATEWAY_ROUTE_TYPE_STATIC,
            state: ROUTE_STATE_ACTIVE,
            target: Target::VirtualInterfaceGroup("lgw-vif-grp-00000000000000003"),
        },
    ]
}

/// The two routes `SearchLocalGatewayRoutesRequest$Filters`' own `supernet-of-match`
/// example names: "if you have 10.0.1.0/29 and 10.0.1.0/31 routes in your route table and
/// you specify supernet-of-match as 10.0.1.0/30, then the result returns 10.0.1.0/29".
///
/// Seeded into a SECOND table, which is also what shows that a search reports one table's
/// routes and not another's: these two destinations are the first table's as well.
fn doc_example_routes() -> Vec<RouteRow> {
    vec![
        RouteRow {
            destination: Destination::Cidr("10.0.1.0/29"),
            route_type: LOCAL_GATEWAY_ROUTE_TYPE_STATIC,
            state: ROUTE_STATE_ACTIVE,
            target: Target::VirtualInterfaceGroup("lgw-vif-grp-00000000000000004"),
        },
        RouteRow {
            destination: Destination::Cidr("10.0.1.0/31"),
            route_type: LOCAL_GATEWAY_ROUTE_TYPE_STATIC,
            state: ROUTE_STATE_ACTIVE,
            target: Target::VirtualInterfaceGroup("lgw-vif-grp-00000000000000005"),
        },
    ]
}

/// The three route tables the suite seeds: the one [`seeded_routes`] goes into, the one
/// holding the documented example, and one with NO routes at all.
#[derive(Debug, Clone)]
struct Seeded {
    main: String,
    doc_example: String,
    empty: String,
}

/// Write the route tables and their routes into the engine's store, the way the operations
/// that create a local gateway route table and its routes would.
fn seed(engine: &engine::InMemoryEngine) -> Seeded {
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    let mut table = |mode: &str| -> String {
        guard
            .insert_local_gateway_route_table(LocalGatewayRouteTableSpec {
                local_gateway_id: LOCAL_GATEWAY_ID.to_string(),
                outpost_arn: format!(
                    "arn:aws:outposts:{REGION}:{}:outpost/op-00000000000000001",
                    engine::emulator_account_id()
                ),
                region: REGION.to_string(),
                state: ROUTE_TABLE_STATE.to_string(),
                mode: mode.to_string(),
                tags: Vec::new(),
            })
            .expect("the store must take a seeded local gateway route table")
    };
    let seeded = Seeded {
        main: table(MODE_DIRECT_VPC_ROUTING),
        doc_example: table(MODE_COIP),
        empty: table(MODE_DIRECT_VPC_ROUTING),
    };
    for (route_table_id, rows) in [
        (&seeded.main, seeded_routes()),
        (&seeded.doc_example, doc_example_routes()),
    ] {
        for row in rows {
            let destination = row.destination();
            let stored = guard
                .insert_local_gateway_route(row.spec(route_table_id))
                .unwrap_or_else(|| {
                    panic!("case {destination:?}: the store must take the seeded route")
                });
            assert_eq!(
                stored
                    .destination_cidr_block
                    .as_deref()
                    .or(stored.destination_prefix_list_id.as_deref()),
                Some(destination),
                "case {destination:?}: the stored route must carry the destination it was \
                 written with, got {stored:?}"
            );
        }
    }
    seeded
}

/// One `SearchLocalGatewayRoutes` call for `route_table_id` carrying `params`, as its HTTP
/// status and XML body.
async fn search(
    router: &MultiServiceRouter,
    route_table_id: &str,
    params: &[(String, String)],
) -> (u16, String) {
    let mut fields = vec![(
        "LocalGatewayRouteTableId".to_string(),
        route_table_id.to_string(),
    )];
    fields.extend(params.iter().cloned());
    super::ec2_query::call(router, REGION, ACTION, &fields).await
}

/// Every route a successful response reports, as its ec2Query members, in the order it
/// reported them.
fn reported_routes(case: &str, body: &str) -> Vec<BTreeMap<String, String>> {
    let parsed = document(case, body);
    parsed
        .descendants()
        .find(|node| node.is_element() && node.tag_name().name() == "routeSet")
        .map(|set| {
            set.children()
                .filter(|item| item.is_element() && item.tag_name().name() == "item")
                .map(|item| {
                    item.children()
                        .filter(|member| member.is_element())
                        .map(|member| {
                            (
                                member.tag_name().name().to_string(),
                                member.text().unwrap_or_default().to_string(),
                            )
                        })
                        .collect()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The destination of each route a successful response reports — its CIDR block, or its
/// prefix list id for a route whose destination is a prefix list.
fn reported_destinations(case: &str, body: &str) -> Vec<String> {
    reported_routes(case, body)
        .iter()
        .map(|route| {
            route
                .get("destinationCidrBlock")
                .or_else(|| route.get("destinationPrefixListId"))
                .cloned()
                .unwrap_or_default()
        })
        .collect()
}

/// The destinations a search requiring success reports.
async fn searched_destinations(
    router: &MultiServiceRouter,
    case: &str,
    route_table_id: &str,
    params: &[(String, String)],
) -> Vec<String> {
    let (status, body) = search(router, route_table_id, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    reported_destinations(case, &body)
}

/// The destinations of the seeded routes at `indices`, in store order.
fn destinations_at(indices: &[usize]) -> Vec<String> {
    let rows = seeded_routes();
    indices
        .iter()
        .map(|index| rows[*index].destination().to_string())
        .collect()
}

/// One `MaxResults` form field.
fn max_results_param(max_results: i64) -> Vec<(String, String)> {
    vec![("MaxResults".to_string(), max_results.to_string())]
}

/// The ARN of a local gateway route table, as `LocalGatewayRoute$LocalGatewayRouteTableArn`
/// carries it: `arn:aws:ec2:<region>:<account>:local-gateway-route-table/<id>`.
fn route_table_arn(route_table_id: &str) -> String {
    format!(
        "arn:aws:ec2:{REGION}:{}:local-gateway-route-table/{route_table_id}",
        engine::emulator_account_id()
    )
}

// ---------------------------------------------------------------------------
// The table being searched has to be one the account holds
// ---------------------------------------------------------------------------

/// An account with no Outpost holds no local gateway route table — a local gateway and its
/// route tables come with an Outpost rather than from any EC2 call — so every WELL-FORMED id
/// it is asked about is refused with `InvalidLocalGatewayRouteTableID.NotFound` rather than
/// answered with an empty set, while something that is not an id of this resource at all is
/// refused as `.Malformed` before any lookup (the order the crate's one reader of this id
/// documents, shared with the create, modify and delete of a route).
#[tokio::test]
async fn an_account_with_no_route_table_refuses_every_id_it_can_read() {
    let (router, _engine) = super::ec2_query::emulator();

    // Rows: the two well-formed id forms EC2 issues (17 and 8 hex digits) are NOT FOUND;
    // an id of another resource, the same digits under no prefix, and text that is not an
    // id at all are MALFORMED.
    let cases: [(&str, &str, &str); 5] = [
        (
            "lgw-rtb-00000000000000001",
            "InvalidLocalGatewayRouteTableID.NotFound",
            "The LocalGateway Route Table ID 'lgw-rtb-00000000000000001' does not exist",
        ),
        (
            "lgw-rtb-0abc1234",
            "InvalidLocalGatewayRouteTableID.NotFound",
            "The LocalGateway Route Table ID 'lgw-rtb-0abc1234' does not exist",
        ),
        (
            "lgw-00000000000000001",
            "InvalidLocalGatewayRouteTableID.Malformed",
            "The local-gateway-route-table ID 'lgw-00000000000000001' is malformed",
        ),
        (
            "lgwrtb-00000000000000001",
            "InvalidLocalGatewayRouteTableID.Malformed",
            "The local-gateway-route-table ID 'lgwrtb-00000000000000001' is malformed",
        ),
        (
            "not-a-route-table-id",
            "InvalidLocalGatewayRouteTableID.Malformed",
            "The local-gateway-route-table ID 'not-a-route-table-id' is malformed",
        ),
    ];
    for (route_table_id, code, expected_message) in cases {
        let (status, body) = search(&router, route_table_id, &[]).await;
        assert_eq!(
            status, 400,
            "case {route_table_id:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            code,
            "case {route_table_id:?}: expected {code}, got {body}"
        );
        // The refusal names the id the request sent, so a caller holding several ids can
        // tell which one was refused.
        let message = element(&body, "Message").map(|text| unescaped(&text));
        assert_eq!(
            message.as_deref(),
            Some(expected_message),
            "case {route_table_id:?}: the refusal must name the id, got {body}"
        );
        assert!(
            reported_destinations(route_table_id, &body).is_empty(),
            "case {route_table_id:?}: a refused request reports no route, got {body}"
        );
    }
}

/// With route tables in the store, an id of none of them is still refused — whatever else
/// the request carries — while a table that simply holds no route answers HTTP 200 with an
/// empty set: "no such table" and "no routes" are different answers.
#[tokio::test]
async fn an_id_of_no_table_is_refused_while_a_table_with_no_routes_reports_an_empty_set() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    let unknown = "lgw-rtb-0ffffffffffffffff";
    let cases: Vec<(&str, Vec<(String, String)>)> = vec![
        ("no other member", Vec::new()),
        ("with a filter", filters(&[("state", &["active"][..])])),
        ("with MaxResults", max_results_param(5)),
    ];
    for (case, params) in cases {
        let (status, body) = search(&router, unknown, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidLocalGatewayRouteTableID.NotFound",
            "case {case:?}: expected the not-found code, got {body}"
        );
    }

    let (status, body) = search(&router, &seeded.empty, &[]).await;
    assert_eq!(
        status, 200,
        "a table with no route: expected HTTP 200, got {status} with body {body}"
    );
    let reported = reported_destinations("a table with no route", &body);
    assert!(
        reported.is_empty(),
        "a table with no route: expected no route, got {reported:?} from {body}"
    );
    assert!(
        element(&body, "nextToken").is_none(),
        "a table with no route: a complete report carries no NextToken, got {body}"
    );
}

/// A `route-search.*` filter relates ADDRESS RANGES, not destination text: a route whose
/// destination was written with host bits set — the store keeps it exactly as written, the
/// form `CreateLocalGatewayRoute` and `DeleteLocalGatewayRoute` match a route by — is still
/// selected by a relation naming the block it denotes, and a destination no CIDR reader can
/// read relates to nothing while still being reported by default.
///
/// Rows: both address families, each written at a prefix length that clears different bits,
/// plus a destination that is not an address range at all.
#[tokio::test]
async fn a_route_search_filter_relates_address_ranges_not_destination_text() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    // (destination as written, the block it denotes)
    let written: [(&str, &str); 3] = [
        ("10.4.5.6/24", "10.4.5.0/24"),
        ("192.168.200.1/20", "192.168.192.0/20"),
        ("2600:1f00:2000:dead::/48", "2600:1f00:2000::/48"),
    ];
    // A destination the CIDR readers cannot read, so no relation can place it.
    let unreadable = "not-a-cidr-block";

    {
        let store = engine.state::<Ec2State>();
        let mut guard = store.lock();
        for as_written in written
            .iter()
            .map(|(as_written, _)| *as_written)
            .chain([unreadable])
        {
            let spec = LocalGatewayRouteSpec {
                local_gateway_route_table_id: seeded.empty.clone(),
                destination: LocalGatewayRouteDestination::CidrBlock(as_written.to_string()),
                local_gateway_virtual_interface_group_id: Some(
                    "lgw-vif-grp-00000000000000009".to_string(),
                ),
                network_interface_id: None,
                subnet_id: None,
                coip_pool_id: None,
                route_type: LOCAL_GATEWAY_ROUTE_TYPE_STATIC.to_string(),
                state: ROUTE_STATE_ACTIVE.to_string(),
            };
            let stored = guard
                .insert_local_gateway_route(spec)
                .unwrap_or_else(|| panic!("case {as_written:?}: the store must take the route"));
            assert_eq!(
                stored.destination_cidr_block.as_deref(),
                Some(as_written),
                "case {as_written:?}: the destination is stored as written, got {stored:?}"
            );
        }
    }

    // Reported as written, in the store's destination order.
    let mut expected: Vec<String> = written
        .iter()
        .map(|(as_written, _)| (*as_written).to_string())
        .chain([unreadable.to_string()])
        .collect();
    expected.sort();
    let reported =
        searched_destinations(&router, "destinations as written", &seeded.empty, &[]).await;
    assert_eq!(
        reported, expected,
        "destinations as written: expected every stored destination in order, got {reported:?}"
    );

    // Each relation names the BLOCK, and selects the route whose destination denotes it —
    // whichever text that destination was written as.
    for (as_written, block) in written {
        for (relation, value) in [
            ("route-search.exact-match", block),
            ("route-search.exact-match", as_written),
            ("route-search.subnet-of-match", block),
        ] {
            let found = searched_destinations(
                &router,
                as_written,
                &seeded.empty,
                &filters(&[(relation, &[value][..])]),
            )
            .await;
            assert_eq!(
                found,
                vec![as_written.to_string()],
                "case {as_written:?} under {relation} {value:?}: expected the stored route, \
                 got {found:?}"
            );
        }
    }

    // Every IPv4 destination is inside `0.0.0.0/0`; the destination no reader can read is
    // inside nothing, so it is selected by no relation at all.
    let inside_everything = searched_destinations(
        &router,
        "every IPv4 destination",
        &seeded.empty,
        &filters(&[("route-search.subnet-of-match", &["0.0.0.0/0"][..])]),
    )
    .await;
    let mut ipv4: Vec<String> = written
        .iter()
        .map(|(as_written, _)| (*as_written).to_string())
        .filter(|destination| !destination.contains(':'))
        .collect();
    ipv4.sort();
    assert_eq!(
        inside_everything, ipv4,
        "every IPv4 destination: a destination no CIDR reader can read relates to nothing, \
         got {inside_everything:?}"
    );
}

// ---------------------------------------------------------------------------
// The store is what is reported
// ---------------------------------------------------------------------------

/// Every route of the NAMED table is reported, in destination order, with every member of
/// the modelled shape computed from the stored route — and the routes of another table are
/// not, even where the two tables hold the same destinations.
#[tokio::test]
async fn every_route_of_the_named_table_is_reported_with_every_member() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    let (status, body) = search(&router, &seeded.main, &[]).await;
    assert_eq!(
        status, 200,
        "all routes: expected HTTP 200, got {status} with body {body}"
    );
    let reported = reported_destinations("all routes", &body);
    let expected = destinations_at(&(0..seeded_routes().len()).collect::<Vec<_>>());
    assert_eq!(
        reported, expected,
        "all routes: expected every route of the table in destination order, got {reported:?}"
    );

    // Every member of each reported route comes from the stored route: its destination, its
    // type and state, the resource it forwards to, the table it belongs to (id and ARN) and
    // the owning account.
    let routes = reported_routes("all routes", &body);
    for (row, route) in seeded_routes().iter().zip(routes.iter()) {
        let case = row.destination();
        let (target_member, target_value) = row.target.reported();
        let destination_member = match row.destination {
            Destination::Cidr(_) => "destinationCidrBlock",
            Destination::PrefixList(_) => "destinationPrefixListId",
        };
        for (member, value) in [
            (destination_member, case.to_string()),
            ("type", row.route_type.to_string()),
            ("state", row.state.to_string()),
            (target_member, target_value.to_string()),
            ("localGatewayRouteTableId", seeded.main.clone()),
            ("localGatewayRouteTableArn", route_table_arn(&seeded.main)),
            ("ownerId", engine::emulator_account_id().to_string()),
        ] {
            assert_eq!(
                route.get(member).map(String::as_str),
                Some(value.as_str()),
                "case {case:?}: expected <{member}>{value}</{member}>, got {route:?}"
            );
        }
    }

    // The other table's routes are its own, even though it holds the same two destinations.
    let other = searched_destinations(&router, "the other table", &seeded.doc_example, &[]).await;
    assert_eq!(
        other,
        vec!["10.0.1.0/29".to_string(), "10.0.1.0/31".to_string()],
        "the other table: expected only its own routes, got {other:?}"
    );
    let other_body = search(&router, &seeded.doc_example, &[]).await.1;
    for route in reported_routes("the other table", &other_body) {
        assert_eq!(
            route.get("localGatewayRouteTableId").map(String::as_str),
            Some(seeded.doc_example.as_str()),
            "the other table: every reported route belongs to it, got {route:?}"
        );
    }
}

/// Searching is a READ: two calls report the same thing and nothing is written — and a route
/// stored after a call shows up in the next one, so the answer is computed from the store
/// rather than fixed.
#[tokio::test]
async fn searching_writes_nothing_and_a_later_write_is_visible() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);
    let stored = |field: fn(&Ec2State) -> usize| field(&engine.state::<Ec2State>().lock());
    let routes_before = stored(|state| {
        state
            .local_gateway_route_tables
            .values()
            .map(|record| record.routes.len())
            .sum()
    });
    let tables_before = stored(|state| state.local_gateway_route_tables.len());

    let first = searched_destinations(&router, "first read", &seeded.main, &[]).await;
    let second = searched_destinations(&router, "second read", &seeded.main, &[]).await;
    assert_eq!(
        second, first,
        "a read must not change what a later caller sees: {first:?} then {second:?}"
    );
    assert_eq!(
        (
            stored(|state| {
                state
                    .local_gateway_route_tables
                    .values()
                    .map(|record| record.routes.len())
                    .sum()
            }),
            stored(|state| state.local_gateway_route_tables.len())
        ),
        (routes_before, tables_before),
        "searching routes must not write to the store"
    );

    let late = RouteRow {
        destination: Destination::Cidr("10.1.0.0/16"),
        route_type: LOCAL_GATEWAY_ROUTE_TYPE_STATIC,
        state: ROUTE_STATE_ACTIVE,
        target: Target::VirtualInterfaceGroup("lgw-vif-grp-00000000000000009"),
    };
    engine
        .state::<Ec2State>()
        .lock()
        .insert_local_gateway_route(late.spec(&seeded.main))
        .expect("after a later write: the store must take the route");
    let after = searched_destinations(&router, "after a later write", &seeded.main, &[]).await;
    let mut expected = first.clone();
    // Destination order: `10.1.0.0/16` sorts after `10.0.2.0/24` and before `172.16.0.0/12`.
    expected.insert(4, late.destination().to_string());
    assert_eq!(
        after, expected,
        "after a later write: expected the new route to be reported too, got {after:?}"
    );
}

// ---------------------------------------------------------------------------
// The attribute filters
// ---------------------------------------------------------------------------

/// Each documented filter that compares an ATTRIBUTE of the route selects on its own
/// attribute, values within one filter are OR'd, and the comparison is case-sensitive with
/// EC2's `*`/`?` wildcards.
#[tokio::test]
async fn each_attribute_filter_selects_the_routes_whose_attribute_matches_a_value() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    let cases: Vec<(&str, Vec<&str>, Vec<usize>)> = vec![
        ("state", vec![ROUTE_STATE_ACTIVE], vec![0, 1, 3, 5, 6]),
        ("state", vec![ROUTE_STATE_BLACKHOLE], vec![2]),
        ("state", vec![ROUTE_STATE_PENDING], vec![4]),
        // Two values for one filter: joined with OR.
        (
            "state",
            vec![ROUTE_STATE_BLACKHOLE, ROUTE_STATE_PENDING],
            vec![2, 4],
        ),
        // A state no seeded route is in, and a state of the wrong case.
        ("state", vec!["deleted"], vec![]),
        ("state", vec!["Active"], vec![]),
        (
            "type",
            vec![LOCAL_GATEWAY_ROUTE_TYPE_STATIC],
            vec![0, 1, 2, 4, 5, 6],
        ),
        ("type", vec![LOCAL_GATEWAY_ROUTE_TYPE_PROPAGATED], vec![3]),
        // The prefix list is the destination of exactly one seeded route; a route whose
        // destination is a CIDR block carries no prefix list, so it matches neither the id
        // nor a wildcard over it.
        ("prefix-list-id", vec!["pl-00000000000000001"], vec![6]),
        ("prefix-list-id", vec!["pl-*"], vec![6]),
        ("prefix-list-id", vec!["pl-00000000000000002"], vec![]),
        // EC2's wildcards: `*` for any run, `?` for one character.
        ("state", vec!["activ?"], vec![0, 1, 3, 5, 6]),
        ("type", vec!["*ated"], vec![3]),
        // A documented filter carrying no value at all: there is no value for the attribute
        // to match, so it selects nothing rather than everything.
        ("state", vec![], vec![]),
        ("type", vec![], vec![]),
    ];

    for (name, values, expected_indices) in cases {
        let case = format!("{name}={values:?}");
        let reported = searched_destinations(
            &router,
            &case,
            &seeded.main,
            &filters(&[(name, &values[..])]),
        )
        .await;
        let expected = destinations_at(&expected_indices);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The route-search filters
// ---------------------------------------------------------------------------

/// Each `route-search.*` filter selects by the CIDR RELATION it names between the route's
/// destination and the block the filter carries — not by text — across both address
/// families, and a route whose destination is a prefix list carries no block to relate, so
/// it is never selected by one.
#[tokio::test]
async fn each_route_search_filter_selects_by_the_cidr_relation_it_names() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    let cases: Vec<(&str, Vec<&str>, Vec<usize>)> = vec![
        // exact-match: the destination IS the block, whatever family, and the block is read
        // as a CIDR block rather than as text — so a value with its host bits set names the
        // same block as its canonical form.
        ("route-search.exact-match", vec!["10.0.1.0/29"], vec![1]),
        ("route-search.exact-match", vec!["10.0.1.4/29"], vec![1]),
        ("route-search.exact-match", vec!["10.0.1.0/31"], vec![2]),
        (
            "route-search.exact-match",
            vec!["2600:1f00:1000::/48"],
            vec![5],
        ),
        (
            "route-search.exact-match",
            vec!["10.0.0.0/16", "172.16.0.0/12"],
            vec![0, 4],
        ),
        // A block that is a prefix of a stored one but not equal to it.
        ("route-search.exact-match", vec!["10.0.1.0/30"], vec![]),
        // subnet-of-match: the destination is INSIDE the block (a block is inside itself).
        ("route-search.subnet-of-match", vec!["10.0.1.0/30"], vec![2]),
        (
            "route-search.subnet-of-match",
            vec!["10.0.1.0/29"],
            vec![1, 2],
        ),
        (
            "route-search.subnet-of-match",
            vec!["10.0.0.0/8"],
            vec![0, 1, 2, 3],
        ),
        (
            "route-search.subnet-of-match",
            vec!["2600:1f00::/32"],
            vec![5],
        ),
        ("route-search.subnet-of-match", vec!["192.168.0.0/16"], vec![]),
        // An IPv4 block relates to no IPv6 destination and the other way round.
        ("route-search.subnet-of-match", vec!["::/0"], vec![5]),
        // supernet-of-match: the destination ENCOMPASSES the block.
        (
            "route-search.supernet-of-match",
            vec!["10.0.1.0/30"],
            vec![0, 1],
        ),
        (
            "route-search.supernet-of-match",
            vec!["10.0.1.0/31"],
            vec![0, 1, 2],
        ),
        (
            "route-search.supernet-of-match",
            vec!["2600:1f00:1000:1::/64"],
            vec![5],
        ),
        ("route-search.supernet-of-match", vec!["10.9.0.0/16"], vec![]),
        // longest-prefix-match: of the destinations that contain the block, the most
        // specific one — which depends on what ELSE the table holds, so the rows differ in
        // how deeply the block is nested.
        (
            "route-search.longest-prefix-match",
            vec!["10.0.1.0/31"],
            vec![2],
        ),
        (
            "route-search.longest-prefix-match",
            vec!["10.0.1.4/32"],
            vec![1],
        ),
        (
            "route-search.longest-prefix-match",
            vec!["10.0.9.0/24"],
            vec![0],
        ),
        (
            "route-search.longest-prefix-match",
            vec!["172.16.5.0/24"],
            vec![4],
        ),
        (
            "route-search.longest-prefix-match",
            vec!["2600:1f00:1000:1::/64"],
            vec![5],
        ),
        // No destination contains it at all.
        (
            "route-search.longest-prefix-match",
            vec!["10.9.0.0/16"],
            vec![],
        ),
        // Two values: the longest prefix is found per value and the results are OR'd.
        (
            "route-search.longest-prefix-match",
            vec!["10.0.1.0/31", "172.16.5.0/24"],
            vec![2, 4],
        ),
        // A `route-search.*` filter carrying no value selects nothing.
        ("route-search.exact-match", vec![], vec![]),
        ("route-search.subnet-of-match", vec![], vec![]),
        ("route-search.supernet-of-match", vec![], vec![]),
        ("route-search.longest-prefix-match", vec![], vec![]),
    ];

    for (name, values, expected_indices) in cases {
        let case = format!("{name}={values:?}");
        let reported = searched_destinations(
            &router,
            &case,
            &seeded.main,
            &filters(&[(name, &values[..])]),
        )
        .await;
        let expected = destinations_at(&expected_indices);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }
}

/// The example `SearchLocalGatewayRoutesRequest$Filters` documents, on the table that holds
/// exactly the two routes it names: "if you have 10.0.1.0/29 and 10.0.1.0/31 routes in your
/// route table and you specify supernet-of-match as 10.0.1.0/30, then the result returns
/// 10.0.1.0/29".
#[tokio::test]
async fn the_documented_supernet_of_match_example_holds() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    let reported = searched_destinations(
        &router,
        "the documented example",
        &seeded.doc_example,
        &filters(&[("route-search.supernet-of-match", &["10.0.1.0/30"][..])]),
    )
    .await;
    assert_eq!(
        reported,
        vec!["10.0.1.0/29".to_string()],
        "the documented example: expected only 10.0.1.0/29, got {reported:?}"
    );
}

/// One row of the AND table: the filters to send, and the indices of the routes that must
/// come back.
type FilterCombination<'a> = (Vec<(&'a str, &'a [&'a str])>, Vec<usize>);

/// Several filters are joined with an AND — including an attribute filter with a
/// `route-search.*` one: only the routes matching every filter are reported.
#[tokio::test]
async fn several_filters_are_joined_with_an_and() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    let cases: Vec<FilterCombination<'_>> = vec![
        (
            vec![
                ("type", &[LOCAL_GATEWAY_ROUTE_TYPE_STATIC][..]),
                ("route-search.subnet-of-match", &["10.0.0.0/8"][..]),
            ],
            vec![0, 1, 2],
        ),
        (
            vec![
                ("state", &[ROUTE_STATE_ACTIVE][..]),
                ("route-search.supernet-of-match", &["10.0.1.0/30"][..]),
            ],
            vec![0, 1],
        ),
        (
            vec![
                ("state", &[ROUTE_STATE_BLACKHOLE][..]),
                ("route-search.exact-match", &["10.0.1.0/29"][..]),
            ],
            vec![],
        ),
        (
            vec![
                ("route-search.subnet-of-match", &["10.0.0.0/8"][..]),
                ("route-search.supernet-of-match", &["10.0.1.0/31"][..]),
            ],
            vec![0, 1, 2],
        ),
        (
            vec![
                ("state", &[ROUTE_STATE_ACTIVE][..]),
                ("type", &[LOCAL_GATEWAY_ROUTE_TYPE_PROPAGATED][..]),
            ],
            vec![3],
        ),
    ];

    for (entries, expected_indices) in cases {
        let case = format!("{entries:?}");
        let reported =
            searched_destinations(&router, &case, &seeded.main, &filters(&entries)).await;
        let expected = destinations_at(&expected_indices);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }
}

/// A filter name this operation does not document is refused with EC2's
/// `InvalidParameterValue`, rather than silently reported as an empty set.
#[tokio::test]
async fn an_undefined_filter_name_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    // Rows: a near-miss of a documented name, the underscore spelling, a documented name
    // with the wrong case, the prefix of the route-search family, a name of another
    // operation's filter, the tag filters this operation's `Filters` doc does not name, and
    // an empty name.
    let cases = [
        "route-search",
        "route-search.exact",
        "route-search.subnet-match",
        "prefix_list_id",
        "Prefix-List-Id",
        "local-gateway-id",
        "tag-key",
        "tag:Name",
        "",
    ];
    for name in cases {
        let (status, body) = search(&router, &seeded.main, &filters(&[(name, &[][..])])).await;
        assert_eq!(
            status, 400,
            "case {name:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValue",
            "case {name:?}: expected InvalidParameterValue, got {body}"
        );
        assert!(
            reported_destinations(name, &body).is_empty(),
            "case {name:?}: a refused request reports no route, got {body}"
        );
    }
}

/// A `route-search.*` value that is not an address range in CIDR notation is refused: the
/// filter relates CIDR blocks, so a value that is not one is not something to compare as
/// text and report nothing for.
#[tokio::test]
async fn a_route_search_value_that_is_not_a_cidr_block_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    // Rows: a bare address, a prefix length past the family's maximum, a non-numeric prefix
    // length, an id, text, and an empty value — each sent to every one of the four
    // route-search filters.
    let values = [
        "10.0.1.0",
        "10.0.1.0/33",
        "2600:1f00::/129",
        "10.0.1.0/x",
        "10.0.1.0/",
        "pl-00000000000000001",
        "not-a-cidr-block",
        "",
    ];
    let names = [
        "route-search.exact-match",
        "route-search.longest-prefix-match",
        "route-search.subnet-of-match",
        "route-search.supernet-of-match",
    ];
    for name in names {
        for value in values {
            let case = format!("{name}={value:?}");
            let (status, body) =
                search(&router, &seeded.main, &filters(&[(name, &[value][..])])).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} with body {body}"
            );
            assert_eq!(
                error_code(&body),
                "InvalidParameterValue",
                "case {case:?}: expected InvalidParameterValue, got {body}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Pagination
// ---------------------------------------------------------------------------

/// Whatever the page size, walking the pages covers every route of the table exactly once,
/// in destination order, and the last page carries no token.
#[tokio::test]
async fn paging_covers_every_route_exactly_once_for_each_page_size() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);
    let all = destinations_at(&(0..seeded_routes().len()).collect::<Vec<_>>());

    // Rows: the smallest page there is, sizes that divide the table exactly and that do not,
    // the whole table in one page, and one page size larger than the table.
    for max_results in [1, 2, 3, 7, 1000] {
        let case = format!("MaxResults={max_results}");
        let mut params = max_results_param(max_results);
        let mut walked: Vec<String> = Vec::new();
        let mut pages = 0;
        loop {
            let (status, body) = search(&router, &seeded.main, &params).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with body {body}"
            );
            let page = reported_destinations(&case, &body);
            assert!(
                page.len() <= usize::try_from(max_results).unwrap_or_default(),
                "case {case:?}: a page may not exceed MaxResults, got {page:?}"
            );
            walked.extend(page);
            pages += 1;
            assert!(
                pages <= all.len() + 1,
                "case {case:?}: paging did not terminate, walked {walked:?}"
            );
            match element(&body, "nextToken") {
                Some(token) => {
                    params = max_results_param(max_results);
                    params.push(("NextToken".to_string(), token));
                }
                None => break,
            }
        }
        assert_eq!(
            walked, all,
            "case {case:?}: expected every route exactly once in destination order, got \
             {walked:?}"
        );
    }
}

/// A page is a cut of the FILTERED set, not of the whole table: `MaxResults` and a filter
/// compose.
#[tokio::test]
async fn a_page_is_cut_from_the_filtered_set() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    let page_params = || {
        let mut params = filters(&[("state", &[ROUTE_STATE_ACTIVE][..])]);
        params.extend(max_results_param(2));
        params
    };
    let (status, body) = search(&router, &seeded.main, &page_params()).await;
    assert_eq!(
        status, 200,
        "first page: expected HTTP 200, got {status} with body {body}"
    );
    let first = reported_destinations("first page", &body);
    assert_eq!(
        first,
        destinations_at(&[0, 1]),
        "first page: expected the first two ACTIVE routes, got {first:?}"
    );
    let token = element(&body, "nextToken")
        .unwrap_or_else(|| panic!("first page: a token must follow a cut page, got {body}"));

    let mut params = page_params();
    params.push(("NextToken".to_string(), token));
    let second = searched_destinations(&router, "second page", &seeded.main, &params).await;
    assert_eq!(
        second,
        destinations_at(&[3, 5]),
        "second page: expected the next two ACTIVE routes, got {second:?}"
    );
}

/// A `NextToken` this service did not issue is refused rather than read as a destination to
/// start at.
#[tokio::test]
async fn a_next_token_this_service_did_not_issue_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    for token in ["10.0.1.0/29", "not-a-token", "zzzz", "0"] {
        let params = vec![("NextToken".to_string(), token.to_string())];
        let (status, body) = search(&router, &seeded.main, &params).await;
        assert_eq!(
            status, 400,
            "case {token:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidPaginationToken",
            "case {token:?}: expected InvalidPaginationToken, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// DryRun and the request-controlled factors of the scan
// ---------------------------------------------------------------------------

/// A `DryRun` request reports that it would have succeeded — HTTP 412 `DryRunOperation` —
/// and reports no route, whether or not the table even exists.
#[tokio::test]
async fn a_dry_run_reports_that_the_request_would_have_succeeded() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    for route_table_id in [seeded.main.as_str(), "lgw-rtb-0ffffffffffffffff"] {
        let params = vec![("DryRun".to_string(), "true".to_string())];
        let (status, body) = search(&router, route_table_id, &params).await;
        assert_eq!(
            status, 412,
            "case {route_table_id:?}: expected HTTP 412, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "DryRunOperation",
            "case {route_table_id:?}: expected DryRunOperation, got {body}"
        );
        assert!(
            reported_destinations(route_table_id, &body).is_empty(),
            "case {route_table_id:?}: a dry run reports no route, got {body}"
        );
    }
}

/// One row of the bounds table: what the case is called, the form fields to send, and the
/// HTTP status the request must answer with.
type BoundsCase = (String, Vec<(String, String)>, u16);

/// The request-controlled factors of the scan are bounded: each of the crate's caps is
/// accepted at its published value and refused one past it, so no request can ask for
/// unbounded filtering work.
#[tokio::test]
async fn the_filter_factors_are_accepted_at_their_caps_and_refused_past_them() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    let many_filters = |count: usize| -> Vec<(String, String)> {
        let entries: Vec<(&str, &[&str])> = (0..count)
            .map(|_| ("state", &[ROUTE_STATE_ACTIVE][..]))
            .collect();
        filters(&entries)
    };
    let many_values = |count: usize| -> Vec<(String, String)> {
        let values: Vec<String> = (0..count).map(|index| format!("state-{index}")).collect();
        let borrowed: Vec<&str> = values.iter().map(String::as_str).collect();
        filters(&[("state", &borrowed)])
    };
    let one_value_of = |bytes: usize| -> Vec<(String, String)> {
        let value = "a".repeat(bytes);
        filters(&[("state", &[value.as_str()][..])])
    };

    let cases: Vec<BoundsCase> = vec![
        (format!("{MAX_FILTERS} filters"), many_filters(MAX_FILTERS), 200),
        (
            format!("{} filters", MAX_FILTERS + 1),
            many_filters(MAX_FILTERS + 1),
            400,
        ),
        (
            format!("{MAX_FILTER_VALUES} values"),
            many_values(MAX_FILTER_VALUES),
            200,
        ),
        (
            format!("{} values", MAX_FILTER_VALUES + 1),
            many_values(MAX_FILTER_VALUES + 1),
            400,
        ),
        (
            format!("a value of {MAX_FILTER_VALUE_BYTES} bytes"),
            one_value_of(MAX_FILTER_VALUE_BYTES),
            200,
        ),
        (
            format!("a value of {} bytes", MAX_FILTER_VALUE_BYTES + 1),
            one_value_of(MAX_FILTER_VALUE_BYTES + 1),
            400,
        ),
    ];

    for (case, params, expected_status) in cases {
        let (status, body) = search(&router, &seeded.main, &params).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        if expected_status == 400 {
            assert_eq!(
                error_code(&body),
                "InvalidParameterValue",
                "case {case:?}: expected InvalidParameterValue, got {body}"
            );
            assert!(
                reported_destinations(&case, &body).is_empty(),
                "case {case:?}: a refused request reports no route, got {body}"
            );
        }
    }
}
