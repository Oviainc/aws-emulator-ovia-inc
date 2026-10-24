// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DeleteLocalGatewayRoute`, driven over the wire the service
//! actually speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose body
//! carries `Action=DeleteLocalGatewayRoute`, answered with XML (README.md /
//! OPERATIONS.md "Wire protocol"). ec2Query is selected by the `ec2` signing name in the
//! request's SigV4 credential scope, so every call here carries one.
//!
//! The rule under test: the delete takes OUT OF THE STORE the one route whose destination
//! equals the destination the request names, inside the local gateway route table the
//! request names, and answers that route as it was stored with its state moved to the
//! terminal `deleted`; a route table id outside the form EC2 issues is refused before any
//! lookup, a well-formed id the account holds no table for and a destination the table
//! holds no route for are refused with their own codes, and nothing else in the store
//! moves. Every table below is a distinct input along what that generalises over — which
//! destination form (IPv4 block, IPv4 host route, default route, IPv6 block, 8- and
//! 17-digit prefix list id), which target, which of several tables, and which malformed
//! shape — and the route table the recorded corpus named is one row among them rather
//! than the specification.

use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, LocalGatewayRouteDestination as Destination,
    LocalGatewayRouteSpec as RouteSpec, LocalGatewayRouteTableSpec as RouteTableSpec,
    LOCAL_GATEWAY_ROUTE_STATE_ACTIVE as STATE_ACTIVE,
    LOCAL_GATEWAY_ROUTE_STATE_BLACKHOLE as STATE_BLACKHOLE,
    LOCAL_GATEWAY_ROUTE_STATE_DELETED as STATE_DELETED,
    LOCAL_GATEWAY_ROUTE_STATE_PENDING as STATE_PENDING,
    LOCAL_GATEWAY_ROUTE_TYPE_PROPAGATED as TYPE_PROPAGATED,
    LOCAL_GATEWAY_ROUTE_TYPE_STATIC as TYPE_STATIC,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

use super::ec2_query::{element, error_code, unescaped};

/// The Region every call here is signed for. The route table store is per account rather
/// than per Region, so the Region only selects the ec2Query envelope and the ARN namespace
/// a seeded table's ARN names.
const REGION: &str = "us-east-1";

/// The ec2Query action under test.
const ACTION: &str = "DeleteLocalGatewayRoute";

/// The local gateway the seeded route tables belong to — one Outpost's gateway, in the id
/// form `lgw-` ids take.
const LOCAL_GATEWAY_ID: &str = "lgw-00000000000000001";

/// The virtual interface group some seeded routes send traffic to.
const VIF_GROUP_ID: &str = "lgw-vif-grp-00000000000000001";

/// The network interface the other seeded routes send traffic to.
const NETWORK_INTERFACE_ID: &str = "eni-00000000000000001";

/// A CIDR destination, as a seeded route and a request both name it.
fn cidr(block: &str) -> Destination {
    Destination::CidrBlock(block.to_string())
}

/// A prefix list destination ("Use a prefix list in place of DestinationCidrBlock").
fn prefix_list(id: &str) -> Destination {
    Destination::PrefixListId(id.to_string())
}

/// One seeded route: what names it in its table, what it sends traffic to, and the type
/// and state it carries.
struct RouteRow {
    destination: Destination,
    /// The target, as exactly one of the two members a local gateway route carries.
    virtual_interface_group_id: Option<&'static str>,
    network_interface_id: Option<&'static str>,
    /// "The route type" (`LocalGatewayRoute$Type`), one of
    /// `LOCAL_GATEWAY_ROUTE_TYPE_VALUES` (`static`, `propagated`).
    route_type: &'static str,
    /// "The state of the route" (`LocalGatewayRoute$State`), one of
    /// `LOCAL_GATEWAY_ROUTE_STATE_VALUES`.
    state: &'static str,
}

/// The routes seeded into the FIRST route table. They differ along every form a
/// destination takes and both targets a route carries, and one is `propagated` rather than
/// `static` so the refusal that belongs to the route TYPE has a row too.
fn seeded_routes() -> Vec<RouteRow> {
    vec![
        RouteRow {
            destination: cidr("10.0.1.0/24"),
            virtual_interface_group_id: Some(VIF_GROUP_ID),
            network_interface_id: None,
            route_type: TYPE_STATIC,
            state: STATE_ACTIVE,
        },
        RouteRow {
            // A host route: the narrowest IPv4 block there is.
            destination: cidr("10.0.2.5/32"),
            virtual_interface_group_id: None,
            network_interface_id: Some(NETWORK_INTERFACE_ID),
            route_type: TYPE_STATIC,
            state: STATE_ACTIVE,
        },
        RouteRow {
            // The default route: the widest IPv4 block there is.
            destination: cidr("0.0.0.0/0"),
            virtual_interface_group_id: Some(VIF_GROUP_ID),
            network_interface_id: None,
            route_type: TYPE_STATIC,
            state: STATE_BLACKHOLE,
        },
        RouteRow {
            destination: cidr("2600:1f00:1000::/56"),
            virtual_interface_group_id: Some(VIF_GROUP_ID),
            network_interface_id: None,
            route_type: TYPE_STATIC,
            state: STATE_ACTIVE,
        },
        RouteRow {
            // The short (8-digit) prefix list id form.
            destination: prefix_list("pl-0123abcd"),
            virtual_interface_group_id: None,
            network_interface_id: Some(NETWORK_INTERFACE_ID),
            route_type: TYPE_STATIC,
            state: STATE_ACTIVE,
        },
        RouteRow {
            // The long (17-digit) prefix list id form.
            destination: prefix_list("pl-0123456789abcdef0"),
            virtual_interface_group_id: Some(VIF_GROUP_ID),
            network_interface_id: None,
            route_type: TYPE_STATIC,
            state: STATE_PENDING,
        },
        RouteRow {
            // A route the local gateway learned rather than a caller created.
            destination: cidr("192.168.0.0/16"),
            virtual_interface_group_id: Some(VIF_GROUP_ID),
            network_interface_id: None,
            route_type: TYPE_PROPAGATED,
            state: STATE_ACTIVE,
        },
    ]
}

/// The destination the SECOND seeded route table holds a route for. It is deliberately the
/// same destination as the first table's first row: a route is named by its destination
/// INSIDE one table, so deleting it from one table must leave the other table's route
/// alone.
const SHARED_DESTINATION: &str = "10.0.1.0/24";

/// The emulator with two local gateway route tables seeded, the first holding
/// [`seeded_routes`] and the second one route for [`SHARED_DESTINATION`]. Answers the
/// router and the two table ids, in that order.
fn emulator_with_routes() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>, String, String) {
    let (router, engine) = super::ec2_query::emulator();
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    let mut table_ids = Vec::new();
    for mode in ["coip", "direct-vpc-routing"] {
        table_ids.push(
            guard
                .insert_local_gateway_route_table(RouteTableSpec {
                    local_gateway_id: LOCAL_GATEWAY_ID.to_string(),
                    outpost_arn: format!(
                        "arn:aws:outposts:{REGION}:111122223333:outpost/op-0123456789abcdef0"
                    ),
                    region: REGION.to_string(),
                    mode: mode.to_string(),
                    state: "available".to_string(),
                    tags: Vec::new(),
                })
                .unwrap_or_else(|| panic!("case {mode:?}: the store must take the route table")),
        );
    }
    for row in seeded_routes() {
        let case = row.destination.key().to_string();
        assert!(
            guard
                .insert_local_gateway_route(RouteSpec {
                    local_gateway_route_table_id: table_ids[0].clone(),
                    destination: row.destination.clone(),
                    local_gateway_virtual_interface_group_id: row
                        .virtual_interface_group_id
                        .map(str::to_string),
                    network_interface_id: row.network_interface_id.map(str::to_string),
                    subnet_id: None,
                    coip_pool_id: None,
                    route_type: row.route_type.to_string(),
                    state: row.state.to_string(),
                })
                .is_some(),
            "case {case:?}: the store must take the seeded route"
        );
    }
    assert!(
        guard
            .insert_local_gateway_route(RouteSpec {
                local_gateway_route_table_id: table_ids[1].clone(),
                destination: cidr(SHARED_DESTINATION),
                local_gateway_virtual_interface_group_id: Some(VIF_GROUP_ID.to_string()),
                network_interface_id: None,
                subnet_id: None,
                coip_pool_id: None,
                route_type: TYPE_STATIC.to_string(),
                state: STATE_ACTIVE.to_string(),
            })
            .is_some(),
        "the second table must take its seeded route"
    );
    drop(guard);
    let (first, second) = (table_ids[0].clone(), table_ids[1].clone());
    (router, engine, first, second)
}

/// The destination form fields one request carries, as ec2Query flattens them.
fn destination_params(destination: &Destination) -> Vec<(String, String)> {
    match destination {
        Destination::CidrBlock(block) => {
            vec![("DestinationCidrBlock".to_string(), block.clone())]
        }
        Destination::PrefixListId(id) => {
            vec![("DestinationPrefixListId".to_string(), id.clone())]
        }
    }
}

/// The XML element a reported route's destination arrives in (the member's `xmlName`).
fn destination_element(destination: &Destination) -> &'static str {
    match destination {
        Destination::CidrBlock(_) => "destinationCidrBlock",
        Destination::PrefixListId(_) => "destinationPrefixListId",
    }
}

/// One `DeleteLocalGatewayRoute` call naming `route_table_id` and `destination`, as its
/// HTTP status and XML body.
async fn delete(
    router: &MultiServiceRouter,
    route_table_id: &str,
    destination: &Destination,
) -> (u16, String) {
    let mut params = vec![(
        "LocalGatewayRouteTableId".to_string(),
        route_table_id.to_string(),
    )];
    params.extend(destination_params(destination));
    super::ec2_query::call(router, REGION, ACTION, &params).await
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
// The delete itself
// ---------------------------------------------------------------------------

/// Each `static` route is deleted by naming its own destination: the call answers that
/// route as it was stored with `state` moved to `deleted`, the route leaves the table, and
/// a repeat of the same call reports it gone. Every other route — including the route the
/// second table holds for the SAME destination — stays where it was.
#[tokio::test]
async fn deleting_a_route_answers_the_stored_route_deleted_and_takes_it_out_of_its_table() {
    let (router, engine, table_id, other_table_id) = emulator_with_routes();

    let mut expected_remaining: Vec<String> = stored_destinations(&engine, &table_id);
    for row in seeded_routes()
        .into_iter()
        .filter(|row| row.route_type == TYPE_STATIC)
    {
        let case = format!("{} in {table_id}", row.destination.key());
        let (status, body) = delete(&router, &table_id, &row.destination).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );

        // The route as the store held it, not as the request named it: the destination,
        // the target, the type, the table it was in and that table's ARN and account.
        assert_member(
            &case,
            &body,
            destination_element(&row.destination),
            row.destination.key(),
        );
        assert_member(&case, &body, "type", row.route_type);
        assert_member(&case, &body, "localGatewayRouteTableId", &table_id);
        assert_member(
            &case,
            &body,
            "localGatewayRouteTableArn",
            &ec2::state::local_gateway_route_table_arn(REGION, &table_id),
        );
        assert_member(&case, &body, "ownerId", engine::emulator_account_id());
        if let Some(group_id) = row.virtual_interface_group_id {
            assert_member(&case, &body, "localGatewayVirtualInterfaceGroupId", group_id);
        }
        if let Some(interface_id) = row.network_interface_id {
            assert_member(&case, &body, "networkInterfaceId", interface_id);
        }
        // The state the route is reported in is the terminal `deleted`, whatever state it
        // was stored in (`active`, `blackhole`, `pending` all appear among the rows).
        let reported_state = element(&body, "state").unwrap_or_default();
        assert_eq!(
            reported_state, STATE_DELETED,
            "case {case:?}: expected the route to be reported deleted, got \
             {reported_state:?} in {body}"
        );

        // It is gone from the store, so a repeat of the same call cannot find it.
        expected_remaining.retain(|destination| destination != row.destination.key());
        let remaining = stored_destinations(&engine, &table_id);
        assert_eq!(
            remaining, expected_remaining,
            "case {case:?}: expected the table to hold {expected_remaining:?}, got {remaining:?}"
        );
        let (repeat_status, repeat_body) = delete(&router, &table_id, &row.destination).await;
        let repeat_code = error_code(&repeat_body);
        assert_eq!(
            (repeat_status, repeat_code.as_str()),
            (400, "InvalidRoute.NotFound"),
            "case {case:?}: expected a repeat delete to report the route gone, got \
             {repeat_status} {repeat_body}"
        );
    }

    // The other table's route for the same destination was never touched.
    let other = stored_destinations(&engine, &other_table_id);
    assert_eq!(
        other,
        vec![SHARED_DESTINATION.to_string()],
        "after deleting from {table_id}: expected the other table to still hold \
         {SHARED_DESTINATION:?}, got {other:?}"
    );
    let (status, body) = delete(&router, &other_table_id, &cidr(SHARED_DESTINATION)).await;
    assert_eq!(
        status, 200,
        "the other table's route must still be deletable, got {status} with body {body}"
    );
}

// ---------------------------------------------------------------------------
// The route table id
// ---------------------------------------------------------------------------

/// Something that is not a local gateway route table id at all is refused with
/// `InvalidLocalGatewayRouteTableID.Malformed` BEFORE any lookup, while a well-formed id
/// the account holds no table for is refused with that resource's `.NotFound` — the two
/// questions a caller has to be able to tell apart.
#[tokio::test]
async fn a_route_table_id_outside_the_issued_form_is_malformed_and_an_unknown_one_not_found() {
    let (router, engine, table_id, _other) = emulator_with_routes();
    let before = stored_destinations(&engine, &table_id);

    // Each row is a different way of being outside `lgw-rtb-<8 or 17 lowercase hex
    // digits>`; the first is the id the recorded corpus named.
    for malformed in [
        "lgwrtb-00000000000000000",
        "lgw-rtb-",
        "lgw-rtb-0123456",
        "lgw-rtb-0123456789abcdefg",
        "lgw-rtb-0123456789ABCDEF0",
        "rtb-0123456789abcdef0",
        "lgw-00000000000000001",
    ] {
        let (status, body) = delete(&router, malformed, &cidr(SHARED_DESTINATION)).await;
        let code = error_code(&body);
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidLocalGatewayRouteTableID.Malformed"),
            "case {malformed:?}: expected a malformed-id refusal, got {status} {body}"
        );
        let message = unescaped(&element(&body, "Message").unwrap_or_default());
        assert!(
            message.contains(malformed),
            "case {malformed:?}: expected the refusal to name the id, got {message:?}"
        );
    }

    // Both issued id forms are well formed, so an id the account holds no table for gets
    // the route table's own `.NotFound` instead.
    for unknown in ["lgw-rtb-0123456789abcdef0", "lgw-rtb-0123abcd"] {
        let (status, body) = delete(&router, unknown, &cidr(SHARED_DESTINATION)).await;
        let code = error_code(&body);
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidLocalGatewayRouteTableID.NotFound"),
            "case {unknown:?}: expected an unknown-route-table refusal, got {status} {body}"
        );
    }

    let after = stored_destinations(&engine, &table_id);
    assert_eq!(
        after, before,
        "a refused request must change nothing: expected {before:?}, got {after:?}"
    );
}

// ---------------------------------------------------------------------------
// The destination
// ---------------------------------------------------------------------------

/// A destination the named table holds no route for is refused with `InvalidRoute.NotFound`
/// — including a CIDR block that is the same NETWORK as a stored route's but not the same
/// block, because "This must match the CIDR for the route exactly"
/// (`$DestinationCidrBlock`).
#[tokio::test]
async fn a_destination_the_table_holds_no_route_for_is_refused() {
    let (router, engine, table_id, _other) = emulator_with_routes();
    let before = stored_destinations(&engine, &table_id);

    for destination in [
        cidr("172.16.0.0/16"),
        // A block inside a stored route's, and the stored route's network at another
        // prefix length: neither matches it exactly.
        cidr("10.0.1.128/25"),
        cidr("10.0.1.0/23"),
        cidr("2600:1f00:2000::/56"),
        prefix_list("pl-0123abce"),
    ] {
        let case = destination.key().to_string();
        let (status, body) = delete(&router, &table_id, &destination).await;
        let code = error_code(&body);
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidRoute.NotFound"),
            "case {case:?}: expected an unknown-route refusal, got {status} {body}"
        );
    }

    let after = stored_destinations(&engine, &table_id);
    assert_eq!(
        after, before,
        "a refused request must change nothing: expected {before:?}, got {after:?}"
    );
}

/// A request that names no destination, both destinations, or a value of the wrong form is
/// refused for the reason it is wrong, before anything is looked up.
#[tokio::test]
async fn a_request_that_does_not_name_exactly_one_well_formed_destination_is_refused() {
    let (router, engine, table_id, _other) = emulator_with_routes();
    let before = stored_destinations(&engine, &table_id);

    // (the destination fields, the code EC2 answers).
    let cases: Vec<(Vec<(String, String)>, &str)> = vec![
        (Vec::new(), "MissingParameter"),
        (
            vec![
                (
                    "DestinationCidrBlock".to_string(),
                    SHARED_DESTINATION.to_string(),
                ),
                (
                    "DestinationPrefixListId".to_string(),
                    "pl-0123abcd".to_string(),
                ),
            ],
            "InvalidParameterCombination",
        ),
        (
            vec![("DestinationCidrBlock".to_string(), "10.0.1.0".to_string())],
            "InvalidParameterValue",
        ),
        (
            vec![("DestinationCidrBlock".to_string(), "10.0.1.0/33".to_string())],
            "InvalidParameterValue",
        ),
        (
            vec![(
                "DestinationCidrBlock".to_string(),
                "2600:1f00::/129".to_string(),
            )],
            "InvalidParameterValue",
        ),
        (
            vec![("DestinationCidrBlock".to_string(), "ten-dot-oh".to_string())],
            "InvalidParameterValue",
        ),
        (
            vec![(
                "DestinationPrefixListId".to_string(),
                "pl-0123abc".to_string(),
            )],
            "InvalidParameterValue",
        ),
        (
            vec![(
                "DestinationPrefixListId".to_string(),
                "10.0.1.0/24".to_string(),
            )],
            "InvalidParameterValue",
        ),
    ];
    for (destination_fields, expected_code) in cases {
        let case = format!("{destination_fields:?}");
        let mut params = vec![(
            "LocalGatewayRouteTableId".to_string(),
            table_id.clone(),
        )];
        params.extend(destination_fields);
        let (status, body) = super::ec2_query::call(&router, REGION, ACTION, &params).await;
        let code = error_code(&body);
        assert_eq!(
            (status, code.as_str()),
            (400, expected_code),
            "case {case:?}: expected a {expected_code} refusal, got {status} {body}"
        );
    }

    let after = stored_destinations(&engine, &table_id);
    assert_eq!(
        after, before,
        "a refused request must change nothing: expected {before:?}, got {after:?}"
    );
}

// ---------------------------------------------------------------------------
// The route type, and DryRun
// ---------------------------------------------------------------------------

/// A `propagated` route is one the local gateway learned, so there is no static route to
/// remove: the call is refused and the route stays in the table.
#[tokio::test]
async fn a_propagated_route_cannot_be_deleted() {
    let (router, engine, table_id, _other) = emulator_with_routes();
    let propagated = seeded_routes()
        .into_iter()
        .find(|row| row.route_type == TYPE_PROPAGATED)
        .map(|row| row.destination)
        .expect("one seeded route is propagated");

    let (status, body) = delete(&router, &table_id, &propagated).await;
    let code = error_code(&body);
    assert_eq!(
        (status, code.as_str()),
        (400, "OperationNotPermitted"),
        "case {:?}: expected a propagated route to be refused, got {status} {body}",
        propagated.key()
    );
    let remaining = stored_destinations(&engine, &table_id);
    assert!(
        remaining.contains(&propagated.key().to_string()),
        "case {:?}: expected the propagated route to stay in the table, got {remaining:?}",
        propagated.key()
    );
}

/// `DryRun` answers whether the caller MAY make the request and deletes nothing — and it
/// is answered only once the request's own form has been read, so a dry run that names a
/// malformed route table id still gets the malformed-id refusal.
#[tokio::test]
async fn a_dry_run_reports_the_permission_check_and_deletes_nothing() {
    let (router, engine, table_id, _other) = emulator_with_routes();
    let before = stored_destinations(&engine, &table_id);

    // (the route table id, the destination, the code and status expected).
    let cases: Vec<(String, Destination, &str, u16)> = vec![
        (table_id.clone(), cidr(SHARED_DESTINATION), "DryRunOperation", 412),
        (table_id.clone(), prefix_list("pl-0123abcd"), "DryRunOperation", 412),
        // A destination the table holds no route for is still a permitted request, so the
        // dry run reports the permission check rather than the missing route.
        (table_id.clone(), cidr("172.16.0.0/16"), "DryRunOperation", 412),
        (
            "lgwrtb-00000000000000000".to_string(),
            cidr(SHARED_DESTINATION),
            "InvalidLocalGatewayRouteTableID.Malformed",
            400,
        ),
        (
            table_id.clone(),
            cidr("10.0.1.0"),
            "InvalidParameterValue",
            400,
        ),
    ];
    for (route_table_id, destination, expected_code, expected_status) in cases {
        let case = format!("{route_table_id} {}", destination.key());
        let mut params = vec![
            (
                "LocalGatewayRouteTableId".to_string(),
                route_table_id.clone(),
            ),
            ("DryRun".to_string(), "true".to_string()),
        ];
        params.extend(destination_params(&destination));
        let (status, body) = super::ec2_query::call(&router, REGION, ACTION, &params).await;
        let code = error_code(&body);
        assert_eq!(
            (status, code.as_str()),
            (expected_status, expected_code),
            "case {case:?}: expected a {expected_code} refusal, got {status} {body}"
        );
    }

    let after = stored_destinations(&engine, &table_id);
    assert_eq!(
        after, before,
        "a dry run must change nothing: expected {before:?}, got {after:?}"
    );
}
