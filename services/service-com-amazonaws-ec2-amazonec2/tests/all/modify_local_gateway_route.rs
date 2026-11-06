// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ModifyLocalGatewayRoute` and the `SearchLocalGatewayRoutes` that
//! reads a modified route back, driven over the wire the service actually speaks: `ec2Query`
//! — an `application/x-www-form-urlencoded` POST whose body carries
//! `Action=ModifyLocalGatewayRoute`, answered with XML (README.md / OPERATIONS.md "Wire
//! protocol"). ec2Query is selected by the `ec2` signing name in the request's SigV4
//! credential scope, so every call here carries one.
//!
//! The rule under test: the modify POINTS THE ONE STORED ROUTE the request's destination
//! names inside the named local gateway route table at the one target the request names, and
//! answers that route as the store now holds it — same destination, same type, same table
//! id, ARN and owning account, exactly one target member set, and the transient `pending`
//! state the clock owes `active` one transition later. A route table id outside the form EC2
//! issues, a request that does not name exactly one well-formed destination and exactly one
//! well-formed target, an id of a resource the account does not hold, a destination the
//! table holds no route for, and a route the local gateway learned rather than a caller
//! created are each refused for the reason they are wrong, and a refused call and a dry run
//! leave every stored route exactly as it was.
//!
//! Every table below is a distinct input along what that rule generalises over — which
//! destination form (IPv4 block, IPv4 host route, default route, IPv6 block, 8- and
//! 17-digit prefix list id), which target the route had and which it is moved to, which of
//! several route tables, and which way of being malformed — so the route table id the
//! recorded corpus named is one row among them rather than the specification.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, LocalGatewayRouteDestination as Destination,
    LocalGatewayRouteSpec as RouteSpec, LocalGatewayRouteTableSpec as RouteTableSpec,
    LocalGatewayVirtualInterfaceGroupSpec as VifGroupSpec, NetworkInterfaceSpec,
    LOCAL_GATEWAY_ROUTE_STATE_ACTIVE as STATE_ACTIVE,
    LOCAL_GATEWAY_ROUTE_STATE_PENDING as STATE_PENDING,
    LOCAL_GATEWAY_ROUTE_TYPE_PROPAGATED as TYPE_PROPAGATED,
    LOCAL_GATEWAY_ROUTE_TYPE_STATIC as TYPE_STATIC, NETWORK_INTERFACE_TYPE_INTERFACE,
    TRANSITION_DELAY,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, error_code, error_message};
use super::wire::{document, param, Param};

/// The Region every call here is signed for. The local gateway stores are per account rather
/// than per Region, so the Region selects the ec2Query envelope and the ARN namespace a
/// seeded table's ARN names.
const REGION: &str = "us-east-1";

/// The ec2Query action under test.
const ACTION: &str = "ModifyLocalGatewayRoute";

/// The action a modified route is read back through.
const SEARCH: &str = "SearchLocalGatewayRoutes";

/// The local gateway the seeded route tables and virtual interface groups belong to.
const LOCAL_GATEWAY_ID: &str = "lgw-00000000000000001";

/// Long enough past [`TRANSITION_DELAY`] that the move the clock owes a just-modified route
/// has fallen due, without depending on where inside the span the test started.
fn past_the_transition() -> Duration {
    TRANSITION_DELAY + Duration::from_millis(250)
}

/// A CIDR destination, as a seeded route and a request both name it.
fn cidr(block: &str) -> Destination {
    Destination::CidrBlock(block.to_string())
}

/// A prefix list destination ("Use a prefix list in place of DestinationCidrBlock").
fn prefix_list(id: &str) -> Destination {
    Destination::PrefixListId(id.to_string())
}

/// What a local gateway route sends traffic to: "You must specify one of the following
/// targets: LocalGatewayVirtualInterfaceGroupId NetworkInterfaceId"
/// (`CreateLocalGatewayRoute`), as an index into the account's seeded resources of that kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    /// The seeded virtual interface group at this index of [`Fixtures::groups`].
    VirtualInterfaceGroup(usize),
    /// The seeded network interface at this index of [`Fixtures::network_interfaces`].
    NetworkInterface(usize),
}

impl Target {
    /// The member of a reported route this target arrives in (the model's `xmlName`), and
    /// the member it cannot also arrive in.
    fn element(self) -> &'static str {
        match self {
            Self::VirtualInterfaceGroup(_) => "localGatewayVirtualInterfaceGroupId",
            Self::NetworkInterface(_) => "networkInterfaceId",
        }
    }

    /// The other member of the pair, which a route carrying this target must not report.
    fn other_element(self) -> &'static str {
        match self {
            Self::VirtualInterfaceGroup(_) => "networkInterfaceId",
            Self::NetworkInterface(_) => "localGatewayVirtualInterfaceGroupId",
        }
    }
}

/// One route under test: the destination that names it in its table, the target it is seeded
/// with, and the target the modify moves it to.
#[derive(Debug, Clone)]
struct Row {
    destination: Destination,
    seeded: Target,
    modified_to: Target,
}

/// The routes this suite modifies. The destination forms are every one the two members
/// accept — an IPv4 block, the narrowest IPv4 block (a host route), the widest (the default
/// route), an IPv6 block, and the 8- and 17-digit prefix list id forms — crossed with every
/// move a target can make: group to another group, group to interface, interface to group,
/// interface to another interface, and each kind of target to itself.
fn rows() -> Vec<Row> {
    vec![
        Row {
            destination: cidr("10.0.1.0/24"),
            seeded: Target::VirtualInterfaceGroup(0),
            modified_to: Target::NetworkInterface(0),
        },
        Row {
            // A host route: the narrowest IPv4 block there is.
            destination: cidr("10.0.2.5/32"),
            seeded: Target::NetworkInterface(0),
            modified_to: Target::VirtualInterfaceGroup(1),
        },
        Row {
            // The default route: the widest IPv4 block there is.
            destination: cidr("0.0.0.0/0"),
            seeded: Target::VirtualInterfaceGroup(0),
            modified_to: Target::VirtualInterfaceGroup(1),
        },
        Row {
            destination: cidr("2600:1f00:1000::/56"),
            seeded: Target::NetworkInterface(0),
            modified_to: Target::NetworkInterface(1),
        },
        Row {
            // The short (8-digit) prefix list id form, moved to the target it already has:
            // the request names a target, so the route takes it whether or not it differs.
            destination: prefix_list("pl-0123abcd"),
            seeded: Target::VirtualInterfaceGroup(1),
            modified_to: Target::VirtualInterfaceGroup(1),
        },
        Row {
            // The long (17-digit) prefix list id form.
            destination: prefix_list("pl-0123456789abcdef0"),
            seeded: Target::NetworkInterface(1),
            modified_to: Target::NetworkInterface(1),
        },
    ]
}

/// The destination of the route the local gateway PROPAGATED into the first table: one a
/// caller did not create, so its target is not a caller's to move.
fn propagated_destination() -> Destination {
    cidr("192.168.0.0/16")
}

/// The destination the SECOND seeded route table holds a route for. Deliberately the same
/// destination as the first table's first row: a route is named by its destination INSIDE
/// one table, so modifying it in one table must leave the other table's route alone.
const SHARED_DESTINATION: &str = "10.0.1.0/24";

/// The resources a modify names: two local gateway route tables, two virtual interface
/// groups and two network interfaces of the account.
#[derive(Debug, Clone)]
struct Fixtures {
    route_tables: Vec<String>,
    groups: Vec<String>,
    network_interfaces: Vec<String>,
}

impl Fixtures {
    /// The id the target names.
    fn target_id(&self, target: Target) -> String {
        match target {
            Target::VirtualInterfaceGroup(index) => self.groups[index].clone(),
            Target::NetworkInterface(index) => self.network_interfaces[index].clone(),
        }
    }

    /// The form field one request carries for the target.
    fn target_param(&self, target: Target) -> Param {
        let id = self.target_id(target);
        match target {
            Target::VirtualInterfaceGroup(_) => param("LocalGatewayVirtualInterfaceGroupId", &id),
            Target::NetworkInterface(_) => param("NetworkInterfaceId", &id),
        }
    }

    /// The pair of stored-route members the target occupies, as the store's own spec takes
    /// them.
    fn target_members(&self, target: Target) -> (Option<String>, Option<String>) {
        let id = self.target_id(target);
        match target {
            Target::VirtualInterfaceGroup(_) => (Some(id), None),
            Target::NetworkInterface(_) => (None, Some(id)),
        }
    }
}

/// The emulator with those fixtures in its store and [`rows`] seeded as `active` static
/// routes of the first table, written through the store's own writer seams — the ones
/// `CreateLocalGatewayRouteTable`, `CreateLocalGatewayVirtualInterfaceGroup`,
/// `CreateNetworkInterface` and `CreateLocalGatewayRoute` write through — so no fixture is a
/// value this suite invented beside the store.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>, Fixtures) {
    let (router, engine) = super::ec2_query::emulator();
    let handle = engine.state::<Ec2State>();
    let mut store = handle.lock();

    // Two route tables, one in each `LocalGatewayRouteTableMode` member.
    let route_tables: Vec<String> = ["coip", "direct-vpc-routing"]
        .into_iter()
        .map(|mode| {
            store
                .insert_local_gateway_route_table(RouteTableSpec {
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
            .insert_local_gateway_virtual_interface_group(VifGroupSpec {
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

    let fixtures = Fixtures {
        route_tables,
        groups,
        network_interfaces,
    };

    // Every row's route, plus one route the local gateway PROPAGATED, in the first table —
    // each in the state its row names and staying in it: nothing here is a route a call of
    // this emulator just introduced.
    for row in rows() {
        let case = row.destination.key().to_string();
        let (group_id, interface_id) = fixtures.target_members(row.seeded);
        assert!(
            store
                .insert_local_gateway_route(RouteSpec {
                    local_gateway_route_table_id: fixtures.route_tables[0].clone(),
                    destination: row.destination.clone(),
                    local_gateway_virtual_interface_group_id: group_id,
                    network_interface_id: interface_id,
                    subnet_id: None,
                    coip_pool_id: None,
                    route_type: TYPE_STATIC.to_string(),
                    state: STATE_ACTIVE.to_string(),
                    becomes: None,
                })
                .is_some(),
            "case {case:?}: the store must take the seeded route"
        );
    }
    assert!(
        store
            .insert_local_gateway_route(RouteSpec {
                local_gateway_route_table_id: fixtures.route_tables[0].clone(),
                destination: propagated_destination(),
                local_gateway_virtual_interface_group_id: Some(fixtures.groups[0].clone()),
                network_interface_id: None,
                subnet_id: None,
                coip_pool_id: None,
                route_type: TYPE_PROPAGATED.to_string(),
                state: STATE_ACTIVE.to_string(),
                becomes: None,
            })
            .is_some(),
        "the store must take the seeded propagated route"
    );
    assert!(
        store
            .insert_local_gateway_route(RouteSpec {
                local_gateway_route_table_id: fixtures.route_tables[1].clone(),
                destination: cidr(SHARED_DESTINATION),
                local_gateway_virtual_interface_group_id: Some(fixtures.groups[0].clone()),
                network_interface_id: None,
                subnet_id: None,
                coip_pool_id: None,
                route_type: TYPE_STATIC.to_string(),
                state: STATE_ACTIVE.to_string(),
                becomes: None,
            })
            .is_some(),
        "the second table must take its seeded route"
    );

    drop(store);
    (router, engine, fixtures)
}

/// The destination form fields one request carries, as ec2Query flattens them.
fn destination_params(destination: &Destination) -> Vec<Param> {
    match destination {
        Destination::CidrBlock(block) => vec![param("DestinationCidrBlock", block)],
        Destination::PrefixListId(id) => vec![param("DestinationPrefixListId", id)],
    }
}

/// The XML element a reported route's destination arrives in (the member's `xmlName`).
fn destination_element(destination: &Destination) -> &'static str {
    match destination {
        Destination::CidrBlock(_) => "destinationCidrBlock",
        Destination::PrefixListId(_) => "destinationPrefixListId",
    }
}

/// One `ModifyLocalGatewayRoute` call carrying `params`, as its HTTP status and XML body.
async fn modify(router: &MultiServiceRouter, params: &[Param]) -> (u16, String) {
    super::ec2_query::call(router, REGION, ACTION, params).await
}

/// The parameters one row's modify carries for `route_table_id`.
fn row_params(fixtures: &Fixtures, route_table_id: &str, row: &Row) -> Vec<Param> {
    let mut params = vec![param("LocalGatewayRouteTableId", route_table_id)];
    params.extend(destination_params(&row.destination));
    params.push(fixtures.target_param(row.modified_to));
    params
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

/// The target each route of `route_table_id` sends traffic to, by the destination that names
/// it, read straight out of the store: the pair of target members, exactly one of them set.
fn stored_targets(
    engine: &engine::InMemoryEngine,
    route_table_id: &str,
) -> BTreeMap<String, (Option<String>, Option<String>)> {
    engine
        .state::<Ec2State>()
        .lock()
        .local_gateway_route_tables
        .get(route_table_id)
        .map(|record| {
            record
                .routes
                .iter()
                .map(|(destination, route)| {
                    (
                        destination.clone(),
                        (
                            route.view.local_gateway_virtual_interface_group_id.clone(),
                            route.view.network_interface_id.clone(),
                        ),
                    )
                })
                .collect()
        })
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
// The modify itself
// ---------------------------------------------------------------------------

/// Each row's route is re-pointed by naming its own destination: the call answers the stored
/// route — the same destination, type, table id, ARN and owning account, the new target in
/// its own member and nothing in the other one, `pending` — the store holds that target, and
/// a search of the table reports exactly the same members. No route is added or removed, and
/// the second table's route for the SAME destination is never touched.
#[tokio::test]
async fn modifying_a_route_repoints_the_stored_route_and_reports_it() {
    let (router, engine, fixtures) = emulator();
    let route_table_id = fixtures.route_tables[0].clone();
    let other_table_id = fixtures.route_tables[1].clone();
    let table_arn = ec2::state::local_gateway_route_table_arn(REGION, &route_table_id);
    let destinations_before: Vec<String> = stored_targets(&engine, &route_table_id)
        .keys()
        .cloned()
        .collect();

    for row in rows() {
        let case = format!(
            "{} {:?} -> {:?}",
            row.destination.key(),
            row.seeded,
            row.modified_to
        );
        let target_id = fixtures.target_id(row.modified_to);
        let (status, body) = modify(&router, &row_params(&fixtures, &route_table_id, &row)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );

        // The route as the store now holds it: the destination that still names it, the new
        // target, the type it keeps, and the table facts read off the stored table rather
        // than off the request.
        assert_member(
            &case,
            &body,
            destination_element(&row.destination),
            row.destination.key(),
        );
        assert_member(&case, &body, row.modified_to.element(), &target_id);
        assert_member(&case, &body, "type", TYPE_STATIC);
        assert_member(&case, &body, "localGatewayRouteTableId", &route_table_id);
        assert_member(&case, &body, "localGatewayRouteTableArn", &table_arn);
        assert_member(&case, &body, "ownerId", engine::emulator_account_id());
        // A route has ONE target, so the member the new target does not occupy is absent —
        // whatever the route used to carry there.
        let vacated = element(&body, row.modified_to.other_element());
        assert_eq!(
            vacated, None,
            "case {case:?}: a route reports one target, got {vacated:?} in \
             <{}> too",
            row.modified_to.other_element()
        );
        let reported_state = element(&body, "state").unwrap_or_default();
        assert_eq!(
            reported_state, STATE_PENDING,
            "case {case:?}: a route this call just re-pointed is not carrying that traffic \
             yet, expected {STATE_PENDING:?}, got {reported_state:?} in {body}"
        );

        // The store holds the new target under the same destination, and holds exactly the
        // routes it held before: a modify moves a route's target, it does not add or remove
        // one.
        let stored = stored_targets(&engine, &route_table_id);
        let destinations: Vec<String> = stored.keys().cloned().collect();
        assert_eq!(
            destinations, destinations_before,
            "case {case:?}: expected the table to still hold {destinations_before:?}, got \
             {destinations:?}"
        );
        let expected_members = fixtures.target_members(row.modified_to);
        let actual_members = stored.get(row.destination.key()).cloned();
        assert_eq!(
            actual_members,
            Some(expected_members.clone()),
            "case {case:?}: expected the stored route to point at {expected_members:?}, got \
             {actual_members:?}"
        );

        // And a search of that table reports the same route, member for member, rather than
        // a second reading of the request.
        let reported = search(&router, &case, &route_table_id).await;
        let route = reported
            .get(row.destination.key())
            .unwrap_or_else(|| panic!("case {case:?}: {SEARCH} reported {reported:?}"));
        for (member, value) in [
            (
                destination_element(&row.destination),
                row.destination.key().to_string(),
            ),
            (row.modified_to.element(), target_id.clone()),
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
        assert_eq!(
            route.get(row.modified_to.other_element()),
            None,
            "case {case:?}: the searched route must report one target, got {route:?}"
        );
    }

    // The other table's route for the same destination was never touched: a route is named
    // by its destination INSIDE one table.
    let other = stored_targets(&engine, &other_table_id);
    assert_eq!(
        other.get(SHARED_DESTINATION).cloned(),
        Some((Some(fixtures.groups[0].clone()), None)),
        "expected the other table's route to keep its seeded target, got {other:?}"
    );
}

/// A re-pointed route is answered in the non-terminal `pending` state, and the clock owes it
/// `active` one `TRANSITION_DELAY` later whether or not anybody calls again.
#[tokio::test]
async fn a_repointed_route_goes_pending_and_the_clock_owes_it_active() {
    let (router, _engine, fixtures) = emulator();
    let route_table_id = fixtures.route_tables[0].clone();

    for row in rows() {
        let case = format!("the clock: {}", row.destination.key());
        let (status, body) = modify(&router, &row_params(&fixtures, &route_table_id, &row)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        assert_eq!(
            element(&body, "state").as_deref(),
            Some(STATE_PENDING),
            "case {case:?}: a just-modified route is in the transient state, got {body}"
        );
    }

    tokio::time::sleep(past_the_transition()).await;

    let reported = search(&router, "after the transition", &route_table_id).await;
    for row in rows() {
        let destination = row.destination.key().to_string();
        let state = reported
            .get(&destination)
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

// ---------------------------------------------------------------------------
// The route table id
// ---------------------------------------------------------------------------

/// Something that is not a local gateway route table id at all is refused with
/// `InvalidLocalGatewayRouteTableID.Malformed` BEFORE anything is looked up, and a
/// well-formed id the account holds no table for with that resource's own `.NotFound` — the
/// two questions a caller has to be able to tell apart. Nothing moves either way.
#[tokio::test]
async fn a_route_table_id_outside_the_issued_form_is_malformed_and_an_unknown_one_not_found() {
    let (router, engine, fixtures) = emulator();
    let known = fixtures.route_tables[0].clone();
    let before = stored_targets(&engine, &known);
    let row = rows().remove(0);

    // Each row is a different way of being outside `lgw-rtb-<8 or 17 lowercase hex digits>`.
    for malformed in [
        "lgwrtb-00000000000000000",
        "lgw-rtb-",
        "lgw-rtb-0123456",
        "lgw-rtb-0123456789abcdefg",
        "lgw-rtb-0123456789ABCDEF0",
        "rtb-0123456789abcdef0",
        "lgw-00000000000000001",
        "not-a-route-table-id",
    ] {
        let (status, body) = modify(&router, &row_params(&fixtures, malformed, &row)).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidLocalGatewayRouteTableID.Malformed"),
            "case {malformed:?}: expected a malformed-id refusal, got {status} {body}"
        );
        let message = error_message(&body);
        assert_eq!(
            message,
            format!("The local-gateway-route-table ID '{malformed}' is malformed"),
            "case {malformed:?}: expected the sentence every reader of this id bar the \
             create of a route words the class with, got {message:?}"
        );
    }

    // Both issued id forms are well formed, so an id the account holds no table for gets the
    // route table's own `.NotFound` instead. The first row is the id the recorded corpus
    // named.
    for unknown in [
        "lgw-rtb-00000000000000000",
        "lgw-rtb-0123456789abcdef0",
        "lgw-rtb-0123abcd",
    ] {
        let (status, body) = modify(&router, &row_params(&fixtures, unknown, &row)).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidLocalGatewayRouteTableID.NotFound"),
            "case {unknown:?}: expected an unknown-route-table refusal, got {status} {body}"
        );
        let message = error_message(&body);
        assert_eq!(
            message,
            format!("The LocalGateway Route Table ID '{unknown}' does not exist"),
            "case {unknown:?}: expected the sentence AWS words this class with, got \
             {message:?}"
        );
    }

    // A request that carries the route table id as an EMPTY value names no table whose route
    // to modify: the id is modelled `clientOptional`, so an empty one reaches the handler and
    // is refused with EC2's `MissingParameter`. One that leaves the parameter out ALTOGETHER
    // is a null the boundary validator refuses before the handler, with the framework's own
    // `MalformedInput`.
    for (case, params, expected_code) in [
        (
            "an empty LocalGatewayRouteTableId",
            row_params(&fixtures, "", &row),
            "MissingParameter",
        ),
        (
            "no LocalGatewayRouteTableId",
            {
                let mut params = destination_params(&row.destination);
                params.push(fixtures.target_param(row.modified_to));
                params
            },
            "MalformedInput",
        ),
    ] {
        let (status, body) = modify(&router, &params).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, expected_code),
            "case {case:?}: expected a {expected_code} refusal, got {status} {body}"
        );
    }

    let after = stored_targets(&engine, &known);
    assert_eq!(
        after, before,
        "a refused modify must change nothing: expected {before:?}, got {after:?}"
    );
}

// ---------------------------------------------------------------------------
// The destination
// ---------------------------------------------------------------------------

/// A request that names no destination, both destinations, or a destination value of the
/// wrong form is refused for the reason it is wrong, before anything is looked up.
#[tokio::test]
async fn a_request_that_does_not_name_exactly_one_well_formed_destination_is_refused() {
    let (router, engine, fixtures) = emulator();
    let route_table_id = fixtures.route_tables[0].clone();
    let before = stored_targets(&engine, &route_table_id);
    let target = fixtures.target_param(Target::NetworkInterface(0));

    // (the destination fields, the code EC2 answers).
    let cases: Vec<(Vec<Param>, &str)> = vec![
        (Vec::new(), "MissingParameter"),
        (
            vec![
                param("DestinationCidrBlock", SHARED_DESTINATION),
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
        let (status, body) = modify(&router, &params).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, expected_code),
            "case {case:?}: expected a {expected_code} refusal, got {status} {body}"
        );
    }

    let after = stored_targets(&engine, &route_table_id);
    assert_eq!(
        after, before,
        "a refused modify must change nothing: expected {before:?}, got {after:?}"
    );
}

/// "The value that you provide must match the CIDR of an existing route in the table"
/// (`$DestinationCidrBlock`): a destination the named table holds no route for is refused
/// with `InvalidRoute.NotFound` — including a block that is the same NETWORK as a stored
/// route's but not the same block, and a destination another table does hold a route for.
#[tokio::test]
async fn a_destination_the_table_holds_no_route_for_is_refused() {
    let (router, engine, fixtures) = emulator();
    let route_table_id = fixtures.route_tables[0].clone();
    let other_table_id = fixtures.route_tables[1].clone();
    let before = stored_targets(&engine, &route_table_id);

    for destination in [
        cidr("172.16.0.0/16"),
        // A block inside a stored route's, and a stored route's network at another prefix
        // length: neither matches it exactly.
        cidr("10.0.1.128/25"),
        cidr("10.0.1.0/23"),
        cidr("2600:1f00:2000::/56"),
        prefix_list("pl-0123abce"),
        prefix_list("pl-00000000000000000"),
    ] {
        let case = destination.key().to_string();
        let mut params = vec![param("LocalGatewayRouteTableId", &route_table_id)];
        params.extend(destination_params(&destination));
        params.push(fixtures.target_param(Target::NetworkInterface(0)));
        let (status, body) = modify(&router, &params).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidRoute.NotFound"),
            "case {case:?}: expected an unknown-route refusal, got {status} {body}"
        );
        let message = error_message(&body);
        assert_eq!(
            message,
            format!(
                "The route '{}' does not exist in the local gateway route table \
                 '{route_table_id}'",
                destination.key()
            ),
            "case {case:?}: expected the refusal to name the route and its table, got \
             {message:?}"
        );
    }

    // The second table holds a route the first does not: the destinations of one table are
    // not the destinations of another.
    let mut params = vec![param("LocalGatewayRouteTableId", &other_table_id)];
    params.extend(destination_params(&prefix_list("pl-0123abcd")));
    params.push(fixtures.target_param(Target::NetworkInterface(0)));
    let (status, body) = modify(&router, &params).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "InvalidRoute.NotFound"),
        "a destination only the FIRST table routes must not be found in the second, got \
         {status} {body}"
    );

    let after = stored_targets(&engine, &route_table_id);
    assert_eq!(
        after, before,
        "a refused modify must change nothing: expected {before:?}, got {after:?}"
    );
}

// ---------------------------------------------------------------------------
// The target
// ---------------------------------------------------------------------------

/// The target is what this call changes, and a route has one: a request that names neither
/// member is refused for the value it had to carry, one that names both cannot be carried
/// out, a value that is not an id of its resource at all is that resource's `.Malformed`, and
/// a well-formed id the account holds no such resource for is its `.NotFound`.
#[tokio::test]
async fn a_request_that_does_not_name_exactly_one_known_target_is_refused() {
    let (router, engine, fixtures) = emulator();
    let route_table_id = fixtures.route_tables[0].clone();
    let before = stored_targets(&engine, &route_table_id);
    let known_group = fixtures.groups[0].clone();
    let known_interface = fixtures.network_interfaces[0].clone();

    // (the target fields, the code EC2 answers).
    let cases: Vec<(Vec<Param>, &str)> = vec![
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
            vec![param(
                "LocalGatewayVirtualInterfaceGroupId",
                "lgw-vif-grp-01234567",
            )],
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
        let mut params = vec![param("LocalGatewayRouteTableId", &route_table_id)];
        params.extend(destination_params(&cidr(SHARED_DESTINATION)));
        params.extend(target_fields);
        let (status, body) = modify(&router, &params).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, expected_code),
            "case {case:?}: expected a {expected_code} refusal, got {status} {body}"
        );
    }

    let after = stored_targets(&engine, &route_table_id);
    assert_eq!(
        after, before,
        "a refused modify must change nothing: expected {before:?}, got {after:?}"
    );
}

// ---------------------------------------------------------------------------
// The route type, and DryRun
// ---------------------------------------------------------------------------

/// A `propagated` route is one the local gateway learned, so where it sends traffic is not a
/// caller's to move: the call is refused and the route keeps the target it was stored with.
#[tokio::test]
async fn a_propagated_route_cannot_be_modified() {
    let (router, engine, fixtures) = emulator();
    let route_table_id = fixtures.route_tables[0].clone();
    let destination = propagated_destination();
    let before = stored_targets(&engine, &route_table_id);

    let mut params = vec![param("LocalGatewayRouteTableId", &route_table_id)];
    params.extend(destination_params(&destination));
    params.push(fixtures.target_param(Target::NetworkInterface(0)));
    let (status, body) = modify(&router, &params).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "OperationNotPermitted"),
        "case {:?}: expected a propagated route to be refused, got {status} {body}",
        destination.key()
    );
    let message = error_message(&body);
    assert_eq!(
        message,
        format!(
            "The route '{}' in the local gateway route table '{route_table_id}' is a \
             {TYPE_PROPAGATED} route, which cannot be modified",
            destination.key()
        ),
        "case {:?}: expected the refusal to name the route, its table and its type, got \
         {message:?}",
        destination.key()
    );

    let after = stored_targets(&engine, &route_table_id);
    assert_eq!(
        after, before,
        "case {:?}: a refused modify must change nothing: expected {before:?}, got {after:?}",
        destination.key()
    );
}

/// `DryRun` answers whether the caller MAY make the request and moves nothing — and it is
/// answered only once the request's own form has been read, so a dry run that names a
/// malformed id, no destination or no target still gets that refusal instead.
#[tokio::test]
async fn a_dry_run_reports_the_permission_check_and_changes_nothing() {
    let (router, engine, fixtures) = emulator();
    let route_table_id = fixtures.route_tables[0].clone();
    let before = stored_targets(&engine, &route_table_id);
    let row = rows().remove(0);

    // (the case, the parameters beside DryRun, the status and code expected).
    let cases: Vec<(&str, Vec<Param>, u16, &str)> = vec![
        (
            "a modify that would have succeeded",
            row_params(&fixtures, &route_table_id, &row),
            412,
            "DryRunOperation",
        ),
        (
            // A route table the account does not hold is still a permitted request, so the
            // dry run reports the permission check rather than the missing table.
            "an unknown route table",
            row_params(&fixtures, "lgw-rtb-00000000000000000", &row),
            412,
            "DryRunOperation",
        ),
        (
            // As is a destination the table holds no route for.
            "an unknown destination",
            {
                let mut params = vec![param("LocalGatewayRouteTableId", &route_table_id)];
                params.extend(destination_params(&cidr("172.16.0.0/16")));
                params.push(fixtures.target_param(row.modified_to));
                params
            },
            412,
            "DryRunOperation",
        ),
        (
            "a malformed route table id",
            row_params(&fixtures, "lgw-rtb-", &row),
            400,
            "InvalidLocalGatewayRouteTableID.Malformed",
        ),
        (
            "no destination",
            vec![
                param("LocalGatewayRouteTableId", &route_table_id),
                fixtures.target_param(row.modified_to),
            ],
            400,
            "MissingParameter",
        ),
        (
            "no target",
            {
                let mut params = vec![param("LocalGatewayRouteTableId", &route_table_id)];
                params.extend(destination_params(&row.destination));
                params
            },
            400,
            "InvalidParameterValue",
        ),
    ];
    for (case, mut params, expected_status, expected_code) in cases {
        params.push(param("DryRun", "true"));
        let (status, body) = modify(&router, &params).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (expected_status, expected_code),
            "case {case:?}: expected a {expected_code} refusal, got {status} {body}"
        );
    }

    let after = stored_targets(&engine, &route_table_id);
    assert_eq!(
        after, before,
        "a dry run must change nothing: expected {before:?}, got {after:?}"
    );
}
