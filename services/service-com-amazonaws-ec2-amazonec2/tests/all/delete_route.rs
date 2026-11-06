// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DeleteRoute`, driven over the wire the service actually speaks:
//! `ec2Query` — an `application/x-www-form-urlencoded` POST whose body carries
//! `Action=DeleteRoute`, answered with XML (README.md / OPERATIONS.md "Wire protocol").
//!
//! The rule under test: **a route is addressed inside its table by the ONE destination it
//! matches on, so a delete naming that destination takes exactly that route out of exactly
//! that table and leaves every other route of every table where it was** — "deletes the
//! specified route from the specified route table", the destination being what identifies a
//! route (Amazon EC2 mints no identifier for one). Its refusal half follows from the same
//! reading: a destination the named table holds no route for is `InvalidRoute.NotFound`, a
//! table the account does not hold is `InvalidRouteTableID.NotFound`, the local route every
//! table comes with cannot be removed, and a request naming no destination or two names no
//! route at all.
//!
//! The rows differ along what the rule generalises over: which destination member carries it
//! (IPv4, IPv6, a prefix list), the destination's value and size, how many host bits the
//! request set — the canonical form a create stored it under — and which target the route
//! pointed at.
//!
//! The delete side of the route slice. [`super::vpc_internet_path`] drives the creates this
//! suite takes back out, and [`super::create_route_table`] the table they live in.

use ec2::state::{DRY_RUN_OPERATION, INVALID_ROUTE_TABLE_ID_NOT_FOUND, LOCAL_ROUTE_GATEWAY_ID};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, elements, error_code, error_message};
use super::wire::{param, Param};

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// Amazon EC2's code for a route the named table does not hold ("InvalidRoute.NotFound — the
/// specified route does not exist", EC2 API "Error codes").
const INVALID_ROUTE_NOT_FOUND: &str = "InvalidRoute.NotFound";

/// The block the VPC every row's table belongs to is created with, and a subnet-sized block
/// inside it — the destination the local route of that VPC matches on.
const VPC_CIDR: &str = "10.80.0.0/16";

/// The emulator the tests drive, as [`super::ec2_query::emulator`] builds it.
fn emulator() -> MultiServiceRouter {
    super::ec2_query::emulator().0
}

/// One `action` call carrying `params`, as its HTTP status and XML body.
async fn call(router: &MultiServiceRouter, action: &str, params: &[Param]) -> (u16, String) {
    super::ec2_query::call(router, REGION, action, params).await
}

/// One `action` call that has to succeed, and the body it answered.
async fn ok(case: &str, router: &MultiServiceRouter, action: &str, params: &[Param]) -> String {
    let (status, body) = call(router, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {action}, got {status} with body {body}"
    );
    body
}

/// One element of a response body, or a panic naming the case.
fn member(case: &str, body: &str, name: &str) -> String {
    element(body, name).unwrap_or_else(|| panic!("case {case:?}: no <{name}> in {body}"))
}

/// The `DescribeRouteTables` body for the one table `route_table_id` names.
async fn describe(case: &str, router: &MultiServiceRouter, route_table_id: &str) -> String {
    ok(
        case,
        router,
        "DescribeRouteTables",
        &[param("RouteTableId.1", route_table_id)],
    )
    .await
}

/// One row: the destination member a route matches through, the value the request writes, the
/// canonical form the create stored it under, and the element `DescribeRouteTables` reports it
/// in.
struct Row {
    case: &'static str,
    member: &'static str,
    requested: &'static str,
    stored: &'static str,
    reported: &'static str,
}

/// Source: `CreateRoute`'s and `DeleteRoute`'s own destination members
/// (`smithy_query(query="operation",operation="DeleteRoute")`: `DestinationCidrBlock`,
/// `DestinationIpv6CidrBlock`, `DestinationPrefixListId`) and the canonical form the create
/// documents for a block ("we modify the specified CIDR block to its canonical form").
const ROWS: &[Row] = &[
    Row {
        case: "every IPv4 address",
        member: "DestinationCidrBlock",
        requested: "0.0.0.0/0",
        stored: "0.0.0.0/0",
        reported: "destinationCidrBlock",
    },
    Row {
        case: "a /24 written as the block it is",
        member: "DestinationCidrBlock",
        requested: "192.0.2.0/24",
        stored: "192.0.2.0/24",
        reported: "destinationCidrBlock",
    },
    Row {
        case: "a block with host bits set, which the create canonicalised",
        member: "DestinationCidrBlock",
        requested: "198.51.100.77/26",
        stored: "198.51.100.64/26",
        reported: "destinationCidrBlock",
    },
    Row {
        case: "a single address",
        member: "DestinationCidrBlock",
        requested: "203.0.113.9/32",
        stored: "203.0.113.9/32",
        reported: "destinationCidrBlock",
    },
    Row {
        case: "every IPv6 address",
        member: "DestinationIpv6CidrBlock",
        requested: "::/0",
        stored: "::/0",
        reported: "destinationIpv6CidrBlock",
    },
    Row {
        case: "an IPv6 block with host bits set",
        member: "DestinationIpv6CidrBlock",
        requested: "2001:db8:abcd::5/48",
        stored: "2001:db8:abcd::/48",
        reported: "destinationIpv6CidrBlock",
    },
];

/// The VPC, its internet gateway and the attachment that makes that gateway a target a route
/// of this VPC may point at — the estate every row's route is created in.
async fn estate(router: &MultiServiceRouter) -> (String, String) {
    let case = "the estate the routes are in";
    let vpc = ok(case, router, "CreateVpc", &[param("CidrBlock", VPC_CIDR)]).await;
    let vpc_id = member(case, &vpc, "vpcId");
    let gateway = ok(case, router, "CreateInternetGateway", &[]).await;
    let internet_gateway_id = member(case, &gateway, "internetGatewayId");
    ok(
        case,
        router,
        "AttachInternetGateway",
        &[
            param("InternetGatewayId", &internet_gateway_id),
            param("VpcId", &vpc_id),
        ],
    )
    .await;
    (vpc_id, internet_gateway_id)
}

/// A route table of `vpc_id`, as its id.
async fn route_table(case: &str, router: &MultiServiceRouter, vpc_id: &str) -> String {
    let body = ok(case, router, "CreateRouteTable", &[param("VpcId", vpc_id)]).await;
    member(case, &body, "routeTableId")
}

/// A delete takes the route its destination names out of its table, leaves the local route
/// and the other tables' routes alone, and refuses the same destination a second time.
#[tokio::test]
async fn delete_route_takes_out_the_route_its_destination_names() {
    let router = emulator();
    let (vpc_id, internet_gateway_id) = estate(&router).await;

    for row in ROWS {
        let case = row.case;
        // TWO tables carrying the same route: what the delete must leave behind is the other
        // table's, since a route is addressed by its table as well as its destination.
        let deleted_from = route_table(case, &router, &vpc_id).await;
        let untouched = route_table(case, &router, &vpc_id).await;
        for table in [&deleted_from, &untouched] {
            ok(
                case,
                &router,
                "CreateRoute",
                &[
                    param("RouteTableId", table),
                    param(row.member, row.requested),
                    param("GatewayId", &internet_gateway_id),
                ],
            )
            .await;
        }

        // A dry run of a delete that WOULD have succeeded takes nothing out.
        let (status, dry) = call(
            &router,
            "DeleteRoute",
            &[
                param("RouteTableId", &deleted_from),
                param(row.member, row.requested),
                param("DryRun", "true"),
            ],
        )
        .await;
        assert_eq!(
            status, 412,
            "case {case:?}: expected a dry run to be refused, got {status} with body {dry}"
        );
        assert_eq!(
            error_code(&dry),
            DRY_RUN_OPERATION,
            "case {case:?}: expected {DRY_RUN_OPERATION}, got {dry}"
        );
        let read = describe(case, &router, &deleted_from).await;
        assert!(
            elements(&read, row.reported).contains(&row.stored.to_string()),
            "case {case:?}: expected the dry run to leave the route for {stored} in the table, \
             got {read}",
            stored = row.stored
        );

        let (status, body) = call(
            &router,
            "DeleteRoute",
            &[
                param("RouteTableId", &deleted_from),
                param(row.member, row.requested),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DeleteRoute, got {status} with body {body}"
        );

        // The route is out of the table it was deleted from, and the local route every table
        // comes with is still there.
        let read = describe(case, &router, &deleted_from).await;
        assert!(
            !elements(&read, row.reported).contains(&row.stored.to_string()),
            "case {case:?}: expected the route for {stored} to be out of the table, got {read}",
            stored = row.stored
        );
        assert!(
            elements(&read, "gatewayId").contains(&LOCAL_ROUTE_GATEWAY_ID.to_string()),
            "case {case:?}: expected the table to keep the local route of its VPC, got {read}"
        );
        assert!(
            !elements(&read, "gatewayId").contains(&internet_gateway_id),
            "case {case:?}: expected the deleted route's target to be gone from the table, got \
             {read}"
        );
        // The other table's route is exactly where it was: a delete addresses ONE table.
        let other = describe(case, &router, &untouched).await;
        assert!(
            elements(&other, row.reported).contains(&row.stored.to_string()),
            "case {case:?}: expected the other table to keep its route for {stored}, got {other}",
            stored = row.stored
        );

        // A destination the table no longer holds a route for is refused, naming the
        // destination and the table.
        let (status, again) = call(
            &router,
            "DeleteRoute",
            &[
                param("RouteTableId", &deleted_from),
                param(row.member, row.requested),
            ],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the second delete to be refused, got {status} with body \
             {again}"
        );
        assert_eq!(
            error_code(&again),
            INVALID_ROUTE_NOT_FOUND,
            "case {case:?}: expected {INVALID_ROUTE_NOT_FOUND}, got {again}"
        );
        let message = error_message(&again);
        assert!(
            message.contains(row.stored) && message.contains(&deleted_from),
            "case {case:?}: expected the refusal to name {stored} and {deleted_from}, got \
             {message:?}",
            stored = row.stored
        );
    }
}

/// A delete of a route through a PREFIX LIST destination: the third member a route may match
/// on, taken out the same way.
#[tokio::test]
async fn delete_route_takes_out_a_prefix_list_route() {
    let router = emulator();
    let case = "a prefix-list destination";
    let (vpc_id, internet_gateway_id) = estate(&router).await;
    let table = route_table(case, &router, &vpc_id).await;

    // The prefix list the route matches on, created in this account: a destination list the
    // account does not hold is one no route may point at.
    let list = ok(
        case,
        &router,
        "CreateManagedPrefixList",
        &[
            param("PrefixListName", "a-prefix-list"),
            param("MaxEntries", "5"),
            param("AddressFamily", "IPv4"),
            param("Entry.1.Cidr", "192.0.2.0/24"),
        ],
    )
    .await;
    let prefix_list_id = member(case, &list, "prefixListId");

    ok(
        case,
        &router,
        "CreateRoute",
        &[
            param("RouteTableId", &table),
            param("DestinationPrefixListId", &prefix_list_id),
            param("GatewayId", &internet_gateway_id),
        ],
    )
    .await;
    ok(
        case,
        &router,
        "DeleteRoute",
        &[
            param("RouteTableId", &table),
            param("DestinationPrefixListId", &prefix_list_id),
        ],
    )
    .await;
    let read = describe(case, &router, &table).await;
    assert!(
        !elements(&read, "destinationPrefixListId").contains(&prefix_list_id),
        "case {case:?}: expected the prefix-list route to be out of the table, got {read}"
    );
}

/// The rule's refusal half: a table the account does not hold, the local route every table
/// comes with, a request naming no destination at all and one naming two are each refused
/// with the code for what is wrong, and nothing is taken out of any table.
#[tokio::test]
async fn delete_route_refuses_what_names_no_route_of_a_table() {
    let router = emulator();
    let case = "the estate the refusals are read against";
    let (vpc_id, internet_gateway_id) = estate(&router).await;
    let table = route_table(case, &router, &vpc_id).await;
    ok(
        case,
        &router,
        "CreateRoute",
        &[
            param("RouteTableId", &table),
            param("DestinationCidrBlock", "0.0.0.0/0"),
            param("GatewayId", &internet_gateway_id),
        ],
    )
    .await;

    // (case, the parameters, the code the refusal carries)
    let cases: Vec<(&str, Vec<Param>, &str)> = vec![
        (
            "a route table the account does not hold",
            vec![
                param("RouteTableId", "rtb-0123456789abcdef0"),
                param("DestinationCidrBlock", "0.0.0.0/0"),
            ],
            INVALID_ROUTE_TABLE_ID_NOT_FOUND,
        ),
        (
            "the local route of the table's own VPC",
            vec![
                param("RouteTableId", &table),
                param("DestinationCidrBlock", VPC_CIDR),
            ],
            "InvalidParameterValue",
        ),
        (
            "no destination at all",
            vec![param("RouteTableId", &table)],
            "MissingParameter",
        ),
        (
            "two destinations, which name no one route",
            vec![
                param("RouteTableId", &table),
                param("DestinationCidrBlock", "0.0.0.0/0"),
                param("DestinationIpv6CidrBlock", "::/0"),
            ],
            "InvalidParameterCombination",
        ),
        (
            "a destination that is not a CIDR block",
            vec![
                param("RouteTableId", &table),
                param("DestinationCidrBlock", "not-a-block"),
            ],
            "InvalidParameterValue",
        ),
        (
            "a destination the table holds no route for",
            vec![
                param("RouteTableId", &table),
                param("DestinationCidrBlock", "192.0.2.0/24"),
            ],
            INVALID_ROUTE_NOT_FOUND,
        ),
    ];

    for (case, params, expected) in cases {
        let (status, body) = call(&router, "DeleteRoute", &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the delete to be refused, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected,
            "case {case:?}: expected {expected}, got {body}"
        );
    }

    // Nothing the refusals named was taken out: the table still carries its own two routes.
    let read = describe(case, &router, &table).await;
    for destination in [VPC_CIDR, "0.0.0.0/0"] {
        assert!(
            elements(&read, "destinationCidrBlock").contains(&destination.to_string()),
            "case {case:?}: expected the table to keep its route for {destination}, got {read}"
        );
    }
}
