// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `CreateLocalGatewayRoute` and the `SearchLocalGatewayRoutes` that
//! reads one back, driven over the wire the service actually speaks: `ec2Query` — an
//! `application/x-www-form-urlencoded` POST whose body carries
//! `Action=CreateLocalGatewayRoute`, answered with XML (README.md / OPERATIONS.md "Wire
//! protocol"). ec2Query is selected by the `ec2` signing name in the request's SigV4
//! credential scope, so every call here carries one.
//!
//! The rule under test: the create WRITES ONE STATIC ROUTE INTO THE STORE, keyed inside the
//! named local gateway route table by the destination it matches, pointed at the one target
//! the request names, and answers that stored route — `static`, `pending`, carrying the
//! table's own id, ARN and owning account — so a search of that table reports exactly the
//! same route and the clock moves it to `active` one transition later. A destination the
//! table already holds a route for has nothing to create, a request whose own form does not
//! name exactly one destination and exactly one target is refused for the reason it is
//! wrong, an id of a resource the account does not hold is refused with that resource's own
//! code, and a dry run writes nothing.
//!
//! Every table below is a distinct input along what that rule generalises over — which
//! destination form (IPv4 block, IPv4 host route, default route, IPv6 block, 8- and
//! 17-digit prefix list id), which of the two targets, which of several route tables, and
//! which way of being malformed — so the route table id the recorded corpus named is one
//! row among them rather than the specification.

use std::collections::BTreeMap;
use std::time::Duration;

use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, LocalGatewayRouteTableSpec,
    LocalGatewayVirtualInterfaceGroupSpec, NetworkInterfaceSpec,
    LOCAL_GATEWAY_ROUTE_STATE_ACTIVE as STATE_ACTIVE,
    LOCAL_GATEWAY_ROUTE_STATE_PENDING as STATE_PENDING,
    LOCAL_GATEWAY_ROUTE_TYPE_STATIC as TYPE_STATIC, NETWORK_INTERFACE_TYPE_INTERFACE,
    TRANSITION_DELAY,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, error_code, error_message};
use super::wire::{document, param, Param};

/// The Region every call here is signed for. The local gateway stores are per account
/// rather than per Region, so the Region selects the ec2Query envelope and the ARN
/// namespace a seeded table's ARN names.
const REGION: &str = "us-east-1";

/// The ec2Query action under test.
const CREATE: &str = "CreateLocalGatewayRoute";

/// The action a created route is read back through.
const SEARCH: &str = "SearchLocalGatewayRoutes";

/// The local gateway the seeded route tables and virtual interface groups belong to.
const LOCAL_GATEWAY_ID: &str = "lgw-00000000000000001";

/// Long enough past [`TRANSITION_DELAY`] that the move the clock owes a new route has
/// fallen due, without depending on where inside the span the test started.
fn past_the_transition() -> Duration {
    TRANSITION_DELAY + Duration::from_millis(250)
}

/// What a route matches, and therefore what names it inside its table: the two members
/// `CreateLocalGatewayRoute` accepts, one of which a request carries.
#[derive(Debug, Clone, Copy)]
enum Destination {
    /// "The CIDR range used for destination matches" (`$DestinationCidrBlock`).
    Cidr(&'static str),
    /// "The ID of the prefix list. Use a prefix list in place of DestinationCidrBlock"
    /// (`$DestinationPrefixListId`).
    PrefixList(&'static str),
}

impl Destination {
    /// The value, whichever member carries it.
    fn value(self) -> &'static str {
        match self {
            Self::Cidr(block) => block,
            Self::PrefixList(id) => id,
        }
    }

    /// The form field one request carries for this destination.
    fn param(self) -> Param {
        match self {
            Self::Cidr(block) => param("DestinationCidrBlock", block),
            Self::PrefixList(id) => param("DestinationPrefixListId", id),
        }
    }

    /// The member of the reported route this destination arrives in (the model's `xmlName`).
    fn element(self) -> &'static str {
        match self {
            Self::Cidr(_) => "destinationCidrBlock",
            Self::PrefixList(_) => "destinationPrefixListId",
        }
    }
}

/// What the route sends traffic to: "You must specify one of the following targets:
/// LocalGatewayVirtualInterfaceGroupId NetworkInterfaceId" (`CreateLocalGatewayRoute`).
#[derive(Debug, Clone, Copy)]
enum Target {
    /// The seeded virtual interface group at this index of [`Fixtures::groups`].
    VirtualInterfaceGroup(usize),
    /// The seeded network interface at this index of [`Fixtures::network_interfaces`].
    NetworkInterface(usize),
}

/// One create under test: the destination the route matches and the target it sends traffic
/// to. The rows differ along every form a destination takes and both targets a route
/// carries.
#[derive(Debug, Clone, Copy)]
struct Row {
    destination: Destination,
    target: Target,
}

/// The creates this suite drives. The destination forms are every one the two members
/// accept — an IPv4 block, the narrowest IPv4 block (a host route), the widest (the default
/// route), an IPv6 block, and the 8- and 17-digit prefix list id forms — crossed with both
/// targets.
fn rows() -> Vec<Row> {
    vec![
        Row {
            destination: Destination::Cidr("10.0.1.0/24"),
            target: Target::VirtualInterfaceGroup(0),
        },
        Row {
            destination: Destination::Cidr("10.0.2.5/32"),
            target: Target::NetworkInterface(0),
        },
        Row {
            destination: Destination::Cidr("0.0.0.0/0"),
            target: Target::VirtualInterfaceGroup(1),
        },
        Row {
            destination: Destination::Cidr("2600:1f00:1000::/56"),
            target: Target::NetworkInterface(1),
        },
        Row {
            destination: Destination::PrefixList("pl-0123abcd"),
            target: Target::VirtualInterfaceGroup(1),
        },
        Row {
            destination: Destination::PrefixList("pl-0123456789abcdef0"),
            target: Target::NetworkInterface(0),
        },
    ]
}

/// The resources a create names: two local gateway route tables (so a route written into
/// one is never read out of the other), two virtual interface groups and two network
/// interfaces of the account.
#[derive(Debug, Clone)]
struct Fixtures {
    route_tables: Vec<String>,
    groups: Vec<String>,
    network_interfaces: Vec<String>,
}

impl Fixtures {
    /// The id the row's target names.
    fn target_id(&self, target: Target) -> String {
        match target {
            Target::VirtualInterfaceGroup(index) => self.groups[index].clone(),
            Target::NetworkInterface(index) => self.network_interfaces[index].clone(),
        }
    }

    /// The form field one request carries for the row's target.
    fn target_param(&self, target: Target) -> Param {
        let id = self.target_id(target);
        match target {
            Target::VirtualInterfaceGroup(_) => {
                param("LocalGatewayVirtualInterfaceGroupId", &id)
            }
            Target::NetworkInterface(_) => param("NetworkInterfaceId", &id),
        }
    }

    /// The member of the reported route the row's target arrives in.
    fn target_element(target: Target) -> &'static str {
        match target {
            Target::VirtualInterfaceGroup(_) => "localGatewayVirtualInterfaceGroupId",
            Target::NetworkInterface(_) => "networkInterfaceId",
        }
    }
}

/// The emulator with those fixtures in its store, written through the store's own writer
/// seams — the ones `CreateLocalGatewayRouteTable`, `CreateLocalGatewayVirtualInterfaceGroup`
/// and `CreateNetworkInterface` write through — so no fixture is a value this suite invented
/// beside the store.
fn emulator() -> (MultiServiceRouter, std::sync::Arc<engine::InMemoryEngine>, Fixtures) {
    let (router, engine) = super::ec2_query::emulator();
    let handle = engine.state::<Ec2State>();
    let mut store = handle.lock();

    // Two route tables, one in each `LocalGatewayRouteTableMode` member.
    let route_tables: Vec<String> = ["coip", "direct-vpc-routing"]
        .into_iter()
        .map(|mode| {
            store
                .insert_local_gateway_route_table(LocalGatewayRouteTableSpec {
                    local_gateway_id: LOCAL_GATEWAY_ID.to_string(),
                    outpost_arn: format!(
                        "arn:aws:outposts:{REGION}:{}:outpost/op-00000000000000001",
                        engine::emulator_account_id()
                    ),
                    region: REGION.to_string(),
                    mode: mode.to_string(),
                    state: "available".to_string(),
                    becomes: None,
                    tags: Vec::new(),
                })
                .unwrap_or_else(|| panic!("case {mode:?}: the store must take the route table"))
        })
        .collect();

    // Two virtual interface groups of the same local gateway, differing in how many virtual
    // interfaces they hold.
    let groups: Vec<String> = [
        vec!["lgw-vif-00000000000000001"],
        vec!["lgw-vif-00000000000000002", "lgw-vif-00000000000000003"],
    ]
    .into_iter()
    .map(|interfaces| {
        let case = format!("{interfaces:?}");
        store
            .insert_local_gateway_virtual_interface_group(LocalGatewayVirtualInterfaceGroupSpec {
                local_gateway_id: LOCAL_GATEWAY_ID.to_string(),
                local_gateway_virtual_interface_ids: interfaces
                    .iter()
                    .map(|id| (*id).to_string())
                    .collect(),
                local_bgp_asn: Some(64512),
                local_bgp_asn_extended: None,
                region: REGION.to_string(),
                configuration_state: "available".to_string(),
                tags: Vec::new(),
            })
            .unwrap_or_else(|| panic!("case {case:?}: the store must take the group"))
    })
    .collect();

    // Two network interfaces, in the account's default VPC's first default subnet — the
    // place a fresh store already offers, rather than an invented one.
    let vpc_id = store
        .default_vpc_id(REGION)
        .expect("a fresh account has a default VPC");
    let (subnet_id, subnet) = store
        .subnets
        .iter()
        .find(|(_, subnet)| subnet.vpc_id.as_deref() == Some(&vpc_id))
        .map(|(id, subnet)| (id.clone(), subnet.clone()))
        .expect("the default VPC comes with default subnets");
    let interface_spec = NetworkInterfaceSpec {
        subnet_id,
        vpc_id,
        availability_zone: subnet.availability_zone.clone().unwrap_or_default(),
        availability_zone_id: subnet.availability_zone_id.clone().unwrap_or_default(),
        interface_type: NETWORK_INTERFACE_TYPE_INTERFACE.to_string(),
        ..NetworkInterfaceSpec::default()
    };
    let network_interfaces: Vec<String> = (0..2)
        .map(|index| {
            store
                .insert_network_interface(interface_spec.clone())
                .unwrap_or_else(|| {
                    panic!("case {index}: the store must take the network interface")
                })
        })
        .collect();

    drop(store);
    (
        router,
        engine,
        Fixtures {
            route_tables,
            groups,
            network_interfaces,
        },
    )
}

/// One `CreateLocalGatewayRoute` call carrying `params`, as its HTTP status and XML body.
async fn create(router: &MultiServiceRouter, params: &[Param]) -> (u16, String) {
    super::ec2_query::call(router, REGION, CREATE, params).await
}

/// The parameters one row's create carries for `route_table_id`.
fn row_params(fixtures: &Fixtures, route_table_id: &str, row: Row) -> Vec<Param> {
    vec![
        param("LocalGatewayRouteTableId", route_table_id),
        row.destination.param(),
        fixtures.target_param(row.target),
    ]
}

/// Every route a `SearchLocalGatewayRoutes` response reports, by the destination that names
/// it, as its ec2Query members.
fn searched_routes(case: &str, body: &str) -> BTreeMap<String, BTreeMap<String, String>> {
    let parsed = document(case, body);
    parsed
        .descendants()
        .find(|node| node.is_element() && node.tag_name().name() == "routeSet")
        .map(|set| {
            set.children()
                .filter(|item| item.is_element() && item.tag_name().name() == "item")
                .map(|item| {
                    let members: BTreeMap<String, String> = item
                        .children()
                        .filter(|member| member.is_element())
                        .map(|member| {
                            (
                                member.tag_name().name().to_string(),
                                member.text().unwrap_or_default().to_string(),
                            )
                        })
                        .collect();
                    let destination = members
                        .get("destinationCidrBlock")
                        .or_else(|| members.get("destinationPrefixListId"))
                        .cloned()
                        .unwrap_or_default();
                    (destination, members)
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The routes the named table reports, requiring the search itself to succeed.
async fn search(
    router: &MultiServiceRouter,
    case: &str,
    route_table_id: &str,
) -> BTreeMap<String, BTreeMap<String, String>> {
    let (status, body) = super::ec2_query::call(
        router,
        REGION,
        SEARCH,
        &[param("LocalGatewayRouteTableId", route_table_id)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {SEARCH}, got {status} with body {body}"
    );
    searched_routes(case, &body)
}

/// The destinations the store holds routes for in `route_table_id`, in store order.
fn stored_destinations(engine: &engine::InMemoryEngine, route_table_id: &str) -> Vec<String> {
    engine
        .state::<Ec2State>()
        .lock()
        .local_gateway_route_tables
        .get(route_table_id)
        .map(|record| record.routes.keys().cloned().collect())
        .unwrap_or_default()
}

/// Assert `body` carries `<element>value</element>`.
fn assert_member(case: &str, body: &str, element_name: &str, value: &str) {
    assert!(
        body.contains(&format!("<{element_name}>{value}</{element_name}>")),
        "case {case:?}: expected <{element_name}>{value}</{element_name}>, got {body}"
    );
}

// ---------------------------------------------------------------------------
// The create itself
// ---------------------------------------------------------------------------

/// Each row is created in the first route table: the call answers the stored route —
/// `static`, `pending`, carrying the destination, the target, and the table's id, ARN and
/// owning account — the route lands in the store under its destination, and a search of
/// that table reports exactly the same members. The second table stays empty throughout,
/// because a route is written into the table the request named.
#[tokio::test]
async fn creating_a_route_stores_it_under_its_destination_and_reports_the_stored_route() {
    let (router, engine, fixtures) = emulator();
    let route_table_id = fixtures.route_tables[0].clone();
    let other_table_id = fixtures.route_tables[1].clone();
    let table_arn = ec2::state::local_gateway_route_table_arn(REGION, &route_table_id);

    let mut expected: Vec<String> = Vec::new();
    for row in rows() {
        let case = format!("{} -> {:?}", row.destination.value(), row.target);
        let (status, body) = create(&router, &row_params(&fixtures, &route_table_id, row)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );

        // The route as the store now holds it: its destination and target as the request
        // named them, the type a static route carries, and the table facts read off the
        // stored table rather than off the request.
        assert_member(
            &case,
            &body,
            row.destination.element(),
            row.destination.value(),
        );
        assert_member(
            &case,
            &body,
            Fixtures::target_element(row.target),
            &fixtures.target_id(row.target),
        );
        assert_member(&case, &body, "type", TYPE_STATIC);
        assert_member(&case, &body, "localGatewayRouteTableId", &route_table_id);
        assert_member(&case, &body, "localGatewayRouteTableArn", &table_arn);
        assert_member(&case, &body, "ownerId", engine::emulator_account_id());
        let reported_state = element(&body, "state").unwrap_or_default();
        assert_eq!(
            reported_state, STATE_PENDING,
            "case {case:?}: a route this call just introduced is not carrying traffic yet, \
             expected {STATE_PENDING:?}, got {reported_state:?} in {body}"
        );

        // It is in the store, keyed by the destination that names it in its table.
        expected.push(row.destination.value().to_string());
        expected.sort();
        let stored = stored_destinations(&engine, &route_table_id);
        assert_eq!(
            stored, expected,
            "case {case:?}: expected the table to hold {expected:?}, got {stored:?}"
        );

        // And a search of that table reports the same route, member for member, rather than
        // a second reading of the request.
        let reported = search(&router, &case, &route_table_id).await;
        let route = reported
            .get(row.destination.value())
            .unwrap_or_else(|| panic!("case {case:?}: {SEARCH} reported {reported:?}"));
        for (member, value) in [
            (row.destination.element(), row.destination.value().to_string()),
            (
                Fixtures::target_element(row.target),
                fixtures.target_id(row.target),
            ),
            ("type", TYPE_STATIC.to_string()),
            ("localGatewayRouteTableId", route_table_id.clone()),
            ("localGatewayRouteTableArn", table_arn.clone()),
            ("ownerId", engine::emulator_account_id().to_string()),
        ] {
            assert_eq!(
                route.get(member).map(String::as_str),
                Some(value.as_str()),
                "case {case:?}: expected {member} = {value:?} in the searched route, got \
                 {route:?}"
            );
        }
    }

    let other = stored_destinations(&engine, &other_table_id);
    assert!(
        other.is_empty(),
        "the table no create named must hold no route, got {other:?}"
    );
}

/// A new route is answered in the non-terminal `pending` state, and the clock owes it
/// `active` one `TRANSITION_DELAY` later whether or not anybody calls again.
#[tokio::test]
async fn a_new_route_starts_pending_and_the_clock_owes_it_active() {
    let (router, _engine, fixtures) = emulator();
    let route_table_id = fixtures.route_tables[0].clone();

    for row in rows() {
        let case = format!("the clock: {}", row.destination.value());
        let (status, body) = create(&router, &row_params(&fixtures, &route_table_id, row)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        assert_eq!(
            element(&body, "state").as_deref(),
            Some(STATE_PENDING),
            "case {case:?}: a new route starts in the transient state, got {body}"
        );
    }

    tokio::time::sleep(past_the_transition()).await;

    let reported = search(&router, "after the transition", &route_table_id).await;
    for row in rows() {
        let destination = row.destination.value();
        let state = reported
            .get(destination)
            .and_then(|route| route.get("state"))
            .map(String::as_str);
        assert_eq!(
            state,
            Some(STATE_ACTIVE),
            "case {destination:?}: the clock owed the route {STATE_ACTIVE:?}, got {state:?} \
             in {reported:?}"
        );
    }
}

/// A destination the named table already holds a route for has nothing to create: the
/// second call is refused with `RouteAlreadyExists` and the stored route is left exactly as
/// it was — while the SAME destination is still free in another table, a route being named
/// by its destination INSIDE one table.
#[tokio::test]
async fn a_destination_the_table_already_routes_is_refused_and_the_stored_route_is_untouched() {
    let (router, engine, fixtures) = emulator();
    let route_table_id = fixtures.route_tables[0].clone();
    let other_table_id = fixtures.route_tables[1].clone();

    for row in rows() {
        let case = row.destination.value().to_string();
        let (status, body) = create(&router, &row_params(&fixtures, &route_table_id, row)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the first create must succeed, got {status} with body {body}"
        );

        // The repeat names a DIFFERENT target for the same destination, so a create that
        // overwrote the stored route would be visible rather than idempotent.
        let other_target = match row.target {
            Target::VirtualInterfaceGroup(_) => Target::NetworkInterface(0),
            Target::NetworkInterface(_) => Target::VirtualInterfaceGroup(0),
        };
        let repeat = vec![
            param("LocalGatewayRouteTableId", &route_table_id),
            row.destination.param(),
            fixtures.target_param(other_target),
        ];
        let (repeat_status, repeat_body) = create(&router, &repeat).await;
        assert_eq!(
            (repeat_status, error_code(&repeat_body).as_str()),
            (400, "RouteAlreadyExists"),
            "case {case:?}: expected the second create to be refused, got {repeat_status} \
             {repeat_body}"
        );
        let message = error_message(&repeat_body);
        assert!(
            message.contains(row.destination.value()),
            "case {case:?}: expected the refusal to name the destination, got {message:?}"
        );

        // The route in the store is the one the FIRST create wrote.
        let reported = search(&router, &case, &route_table_id).await;
        let route = reported
            .get(row.destination.value())
            .unwrap_or_else(|| panic!("case {case:?}: {SEARCH} reported {reported:?}"));
        assert_eq!(
            route
                .get(Fixtures::target_element(row.target))
                .map(String::as_str),
            Some(fixtures.target_id(row.target).as_str()),
            "case {case:?}: the refused create must leave the stored route alone, got \
             {route:?}"
        );

        // The same destination in the OTHER table is a different route, so it is created.
        let (other_status, other_body) =
            create(&router, &row_params(&fixtures, &other_table_id, row)).await;
        assert_eq!(
            other_status, 200,
            "case {case:?}: the same destination in another table must be created, got \
             {other_status} with body {other_body}"
        );
    }

    assert_eq!(
        stored_destinations(&engine, &route_table_id),
        stored_destinations(&engine, &other_table_id),
        "both tables must end up holding the same destinations, one route each"
    );
}

// ---------------------------------------------------------------------------
// The route table id
// ---------------------------------------------------------------------------

/// Something that is not a local gateway route table id at all is refused with
/// `InvalidLocalGatewayRouteTableID.Malformed` BEFORE anything is looked up, and a
/// well-formed id the account holds no table for with that resource's own `.NotFound` — the
/// two questions a caller has to be able to tell apart. Nothing is written either way.
#[tokio::test]
async fn a_route_table_id_outside_the_issued_form_is_malformed_and_an_unknown_one_not_found() {
    let (router, engine, fixtures) = emulator();
    let known = fixtures.route_tables[0].clone();
    let row = rows()[0];

    // Each row is a different way of being outside, or inside, `lgw-rtb-<8 or 17 lowercase
    // hex digits>`; the first NotFound row is the id the recorded corpus named.
    let cases = [
        (
            "lgw-rtb-0123456789abcdef0",
            "InvalidLocalGatewayRouteTableID.NotFound",
        ),
        ("lgw-rtb-01234567", "InvalidLocalGatewayRouteTableID.NotFound"),
        (
            "lgw-vif-grp-0123456789abcdef0",
            "InvalidLocalGatewayRouteTableID.Malformed",
        ),
        (
            "lgw-vpc-assoc-0123456789abcdef0",
            "InvalidLocalGatewayRouteTableID.Malformed",
        ),
        ("rtb-0123456789abcdef0", "InvalidLocalGatewayRouteTableID.Malformed"),
        ("lgw-rtb-", "InvalidLocalGatewayRouteTableID.Malformed"),
        ("lgw-rtb-0123456", "InvalidLocalGatewayRouteTableID.Malformed"),
        (
            "lgw-rtb-0123456789ABCDEF0",
            "InvalidLocalGatewayRouteTableID.Malformed",
        ),
        (
            "lgw-rtb-zzzzzzzzzzzzzzzzz",
            "InvalidLocalGatewayRouteTableID.Malformed",
        ),
        ("not-a-route-table-id", "InvalidLocalGatewayRouteTableID.Malformed"),
    ];
    for (route_table_id, expected_code) in cases {
        let (status, body) = create(&router, &row_params(&fixtures, route_table_id, row)).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, expected_code),
            "case {route_table_id:?}: expected a {expected_code} refusal, got {status} {body}"
        );
        let message = error_message(&body);
        assert!(
            message.contains(route_table_id),
            "case {route_table_id:?}: expected the refusal to name the id, got {message:?}"
        );
    }

    // A request that carries the route table id as an EMPTY value names no table to create
    // a route in: the id is modelled `clientOptional`, so an empty one reaches the handler
    // and is refused with EC2's `MissingParameter`. One that leaves the parameter out
    // ALTOGETHER is a null the boundary validator refuses before the handler, with the
    // framework's own `MalformedInput`, as every other operation of this service answers it.
    for (case, params, expected_code) in [
        (
            "an empty LocalGatewayRouteTableId",
            row_params(&fixtures, "", row),
            "MissingParameter",
        ),
        (
            "no LocalGatewayRouteTableId",
            vec![row.destination.param(), fixtures.target_param(row.target)],
            "MalformedInput",
        ),
    ] {
        let (status, body) = create(&router, &params).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, expected_code),
            "case {case:?}: expected a {expected_code} refusal, got {status} {body}"
        );
    }

    let stored = stored_destinations(&engine, &known);
    assert!(
        stored.is_empty(),
        "a refused create must write nothing, got {stored:?}"
    );
}

// ---------------------------------------------------------------------------
// The destination and the target
// ---------------------------------------------------------------------------

/// A request that names no destination, both destinations, or a destination value of the
/// wrong form is refused for the reason it is wrong, before anything is looked up.
#[tokio::test]
async fn a_request_that_does_not_name_exactly_one_well_formed_destination_is_refused() {
    let (router, engine, fixtures) = emulator();
    let route_table_id = fixtures.route_tables[0].clone();
    let target = fixtures.target_param(Target::VirtualInterfaceGroup(0));

    // (the destination fields, the code EC2 answers).
    let cases: Vec<(Vec<Param>, &str)> = vec![
        (Vec::new(), "MissingParameter"),
        (
            vec![
                param("DestinationCidrBlock", "10.0.1.0/24"),
                param("DestinationPrefixListId", "pl-0123abcd"),
            ],
            "InvalidParameterCombination",
        ),
        (
            vec![param("DestinationCidrBlock", "10.0.1.0")],
            "InvalidParameterValue",
        ),
        (
            vec![param("DestinationCidrBlock", "10.0.1.0/33")],
            "InvalidParameterValue",
        ),
        (
            vec![param("DestinationCidrBlock", "2600:1f00::/129")],
            "InvalidParameterValue",
        ),
        (
            vec![param("DestinationCidrBlock", "ten-dot-oh")],
            "InvalidParameterValue",
        ),
        (
            vec![param("DestinationPrefixListId", "pl-0123abc")],
            "InvalidParameterValue",
        ),
        (
            vec![param("DestinationPrefixListId", "10.0.1.0/24")],
            "InvalidParameterValue",
        ),
    ];
    for (destination_fields, expected_code) in cases {
        let case = format!("{destination_fields:?}");
        let mut params = vec![param("LocalGatewayRouteTableId", &route_table_id)];
        params.extend(destination_fields);
        params.push(target.clone());
        let (status, body) = create(&router, &params).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, expected_code),
            "case {case:?}: expected a {expected_code} refusal, got {status} {body}"
        );
    }

    let stored = stored_destinations(&engine, &route_table_id);
    assert!(
        stored.is_empty(),
        "a refused create must write nothing, got {stored:?}"
    );
}

/// "You must specify one of the following targets: LocalGatewayVirtualInterfaceGroupId
/// NetworkInterfaceId": a request that names neither is missing a parameter, one that names
/// both cannot be carried out, a value that is not an id of its resource at all is that
/// resource's `.Malformed`, and a well-formed id the account holds no such resource for is
/// its `.NotFound`.
#[tokio::test]
async fn a_request_that_does_not_name_exactly_one_known_target_is_refused() {
    let (router, engine, fixtures) = emulator();
    let route_table_id = fixtures.route_tables[0].clone();
    let destination = Destination::Cidr("10.0.1.0/24");
    let known_group = fixtures.groups[0].clone();
    let known_interface = fixtures.network_interfaces[0].clone();

    // (the target fields, the code EC2 answers).
    let cases: Vec<(Vec<Param>, &str)> = vec![
        // "You must specify ONE of the following targets" (`CreateLocalGatewayRoute`): a
        // request that names neither is refused for the VALUE it had to carry — there are two
        // members it could have used and neither is required on its own — which is the code
        // this operation is recorded answering such a request with.
        (Vec::new(), "InvalidParameterValue"),
        (
            vec![
                param("LocalGatewayVirtualInterfaceGroupId", &known_group),
                param("NetworkInterfaceId", &known_interface),
            ],
            "InvalidParameterCombination",
        ),
        // A well-formed virtual interface group id, in both issued id forms, that the
        // account holds no group for.
        (
            vec![param(
                "LocalGatewayVirtualInterfaceGroupId",
                "lgw-vif-grp-0123456789abcdef0",
            )],
            "InvalidLocalGatewayVirtualInterfaceGroupID.NotFound",
        ),
        (
            vec![param("LocalGatewayVirtualInterfaceGroupId", "lgw-vif-grp-01234567")],
            "InvalidLocalGatewayVirtualInterfaceGroupID.NotFound",
        ),
        // Values that are not a virtual interface group id at all: another local gateway
        // resource kind, a bare prefix, a non-hexadecimal suffix.
        (
            vec![param("LocalGatewayVirtualInterfaceGroupId", &route_table_id)],
            "InvalidLocalGatewayVirtualInterfaceGroupID.Malformed",
        ),
        (
            vec![param("LocalGatewayVirtualInterfaceGroupId", "lgw-vif-grp-")],
            "InvalidLocalGatewayVirtualInterfaceGroupID.Malformed",
        ),
        (
            vec![param(
                "LocalGatewayVirtualInterfaceGroupId",
                "lgw-vif-grp-zzzzzzzzzzzzzzzzz",
            )],
            "InvalidLocalGatewayVirtualInterfaceGroupID.Malformed",
        ),
        // The same two questions about a network interface id.
        (
            vec![param("NetworkInterfaceId", "eni-0123456789abcdef0")],
            "InvalidNetworkInterfaceID.NotFound",
        ),
        (
            vec![param("NetworkInterfaceId", "eni-01234567")],
            "InvalidNetworkInterfaceID.NotFound",
        ),
        (
            vec![param("NetworkInterfaceId", "eni-")],
            "InvalidNetworkInterfaceId.Malformed",
        ),
        (
            vec![param("NetworkInterfaceId", "eni-0123456789abcdefg")],
            "InvalidNetworkInterfaceId.Malformed",
        ),
        (
            vec![param("NetworkInterfaceId", &known_group)],
            "InvalidNetworkInterfaceId.Malformed",
        ),
    ];
    for (target_fields, expected_code) in cases {
        let case = format!("{target_fields:?}");
        let mut params = vec![
            param("LocalGatewayRouteTableId", &route_table_id),
            destination.param(),
        ];
        params.extend(target_fields);
        let (status, body) = create(&router, &params).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, expected_code),
            "case {case:?}: expected a {expected_code} refusal, got {status} {body}"
        );
    }

    let stored = stored_destinations(&engine, &route_table_id);
    assert!(
        stored.is_empty(),
        "a refused create must write nothing, got {stored:?}"
    );
}

// ---------------------------------------------------------------------------
// DryRun
// ---------------------------------------------------------------------------

/// `DryRun` answers whether the caller MAY make the request and writes nothing — and it is
/// answered only once the request's own form has been read, so a dry run that names a
/// malformed id or no destination still gets that refusal instead.
#[tokio::test]
async fn a_dry_run_reports_the_permission_check_and_writes_nothing() {
    let (router, engine, fixtures) = emulator();
    let route_table_id = fixtures.route_tables[0].clone();

    // (the case, the parameters beside DryRun, the status and code expected).
    let cases: Vec<(&str, Vec<Param>, u16, &str)> = vec![
        (
            "a create that would have succeeded",
            row_params(&fixtures, &route_table_id, rows()[0]),
            412,
            "DryRunOperation",
        ),
        (
            // A route table the account does not hold is still a permitted request, so the
            // dry run reports the permission check rather than the missing table.
            "an unknown route table",
            row_params(&fixtures, "lgw-rtb-0123456789abcdef0", rows()[0]),
            412,
            "DryRunOperation",
        ),
        (
            "a malformed route table id",
            row_params(&fixtures, "lgw-rtb-", rows()[0]),
            400,
            "InvalidLocalGatewayRouteTableID.Malformed",
        ),
        (
            "no destination",
            vec![
                param("LocalGatewayRouteTableId", &route_table_id),
                fixtures.target_param(rows()[0].target),
            ],
            400,
            "MissingParameter",
        ),
        (
            "no target",
            vec![
                param("LocalGatewayRouteTableId", &route_table_id),
                rows()[0].destination.param(),
            ],
            400,
            "InvalidParameterValue",
        ),
    ];
    for (case, mut params, expected_status, expected_code) in cases {
        params.push(param("DryRun", "true"));
        let (status, body) = create(&router, &params).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (expected_status, expected_code),
            "case {case:?}: expected a {expected_code} refusal, got {status} {body}"
        );
    }

    let stored = stored_destinations(&engine, &route_table_id);
    assert!(
        stored.is_empty(),
        "a dry run must write nothing, got {stored:?}"
    );
}
