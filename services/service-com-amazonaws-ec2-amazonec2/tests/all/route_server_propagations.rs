// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the route server PROPAGATION slice: `EnableRouteServerPropagation`,
//! which defines which route table a route server may install its learned routes into,
//! `DisableRouteServerPropagation`, which takes that back, and
//! `GetRouteServerPropagations`, which reports what one route server propagates into.
//! Driven over the wire the service actually speaks — `ec2Query`, through the crate's
//! shared [`super::ec2_query`] harness.
//!
//! The rules under test:
//!
//! 1. `GetRouteServerPropagations` REPORTS THE STORE for the route server the request names:
//!    exactly the (route server, route table) pairs an enable recorded for THAT route
//!    server, in `RouteTableId` order, narrowed to one table when the request names one, and
//!    the empty set for a route server that propagates into none;
//! 2. a propagation is answered in the non-terminal `pending` state and the CLOCK, not a
//!    caller, moves it to `available`; a disable puts it in `deleting` and the clock then
//!    stops it being reported at all;
//! 3. route server propagation installs routes through the association the route server
//!    holds with the VPC a route table routes for, so a pair whose route server is not
//!    associated with that VPC is REFUSED (`InvalidRouteServerId.NotAssociated`) and nothing
//!    is written;
//! 4. an id that resolves to nothing — a missing parameter, a malformed or unknown route
//!    server id, a route table the account does not hold — is REFUSED rather than answered
//!    with an empty set, on the read and on both writes, and before any `DryRun` is
//!    honoured;
//! 5. an enable needs a pair that does not propagate yet and a disable needs one that does,
//!    so the other way round each is refused with `IncorrectState` and NOTHING is written —
//!    which is how the first call's write stays observable to a later one;
//! 6. reading propagations WRITES NOTHING: two reads of one route server report the same
//!    propagations, and a read never creates one.
//!
//! Each table row is a distinct input along what those rules generalise over — which route
//! server, which route table, which VPC that table is in, how many propagations one route
//! server holds, whether the request narrows by route table, the form of an id that names
//! nothing, which parameter is missing, and which of the two writes is asked — so no one row
//! is the specification.

use std::time::Duration;

use service_com_amazonaws_ec2_amazonec2::state::TRANSITION_DELAY;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, error_code, error_message};
use super::wire::{Param, child, child_text, document, param};

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// The state a propagation the service has just recorded is in.
const PENDING: &str = "pending";

/// The state the clock then owes it.
const AVAILABLE: &str = "available";

/// The state a propagation a disable has started taking down is in.
const DELETING: &str = "deleting";

/// One row of the report table: the case, the route server read, the route table the request
/// narrows to (none for "every table"), and the `(route table, state)` pairs it must report.
type ReportRow<'a> = (&'a str, &'a str, Option<&'a str>, Vec<(String, String)>);

/// One row of a refusal table: the case, the operation, the form fields it carries, and the
/// error code and message it must be refused with.
type RefusalRow<'a> = (&'a str, &'a str, Vec<Param>, &'a str, String);

fn emulator() -> MultiServiceRouter {
    super::ec2_query::emulator().0
}

/// Wait until every clock-owed transition started before now has certainly fallen due.
/// Derived from the service's own span, not a literal, so the two cannot drift.
async fn past_the_transition() {
    tokio::time::sleep(TRANSITION_DELAY + Duration::from_millis(200)).await;
}

/// One `action` call carrying `params`, as its HTTP status and XML body.
async fn action(router: &MultiServiceRouter, name: &str, params: &[Param]) -> (u16, String) {
    call(router, REGION, name, params).await
}

/// One call that must succeed, and the body it answered.
async fn ok(case: &str, router: &MultiServiceRouter, name: &str, params: &[Param]) -> String {
    let (status, body) = action(router, name, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {name}, got {status} with body {body}"
    );
    body
}

/// A route server with `amazon_side_asn`, and the id it was minted with.
async fn create_route_server(case: &str, router: &MultiServiceRouter, amazon_side_asn: &str) -> String {
    let body = ok(
        case,
        router,
        "CreateRouteServer",
        &[param("AmazonSideAsn", amazon_side_asn)],
    )
    .await;
    element(&body, "routeServerId")
        .unwrap_or_else(|| panic!("case {case:?}: no routeServerId in {body}"))
}

/// A VPC with `cidr_block`, and the id it was minted with.
async fn create_vpc(case: &str, router: &MultiServiceRouter, cidr_block: &str) -> String {
    let body = ok(case, router, "CreateVpc", &[param("CidrBlock", cidr_block)]).await;
    element(&body, "vpcId").unwrap_or_else(|| panic!("case {case:?}: no vpcId in {body}"))
}

/// A route table of the VPC `vpc_id`, and the id it was minted with.
async fn create_route_table(case: &str, router: &MultiServiceRouter, vpc_id: &str) -> String {
    let body = ok(case, router, "CreateRouteTable", &[param("VpcId", vpc_id)]).await;
    element(&body, "routeTableId")
        .unwrap_or_else(|| panic!("case {case:?}: no routeTableId in {body}"))
}

/// `AssociateRouteServer` for one (route server, VPC) pair, which must succeed: it is what
/// the propagation rule under test is defined against.
async fn associate(case: &str, router: &MultiServiceRouter, route_server_id: &str, vpc_id: &str) {
    ok(
        case,
        router,
        "AssociateRouteServer",
        &[
            param("RouteServerId", route_server_id),
            param("VpcId", vpc_id),
        ],
    )
    .await;
}

/// Every propagation a response carries, as `(routeServerId, routeTableId, state)` triples
/// in document order.
///
/// A propagation is the element that carries BOTH ids, which is the one `<item>` shape of
/// `GetRouteServerPropagations`'s set and of the single `<routeServerPropagation>` the enable
/// and the disable answer with alike — so all three responses are read the one way, and the
/// propagation's own `<state>` is never confused with a member of something else.
fn propagations(case: &str, body: &str) -> Vec<(String, String, String)> {
    let parsed = document(case, body);
    parsed
        .descendants()
        .filter(|node| {
            node.is_element()
                && child(*node, "routeServerId").is_some()
                && child(*node, "routeTableId").is_some()
        })
        .map(|node| {
            (
                child_text(node, "routeServerId").unwrap_or_default(),
                child_text(node, "routeTableId").unwrap_or_default(),
                child_text(node, "state").unwrap_or_default(),
            )
        })
        .collect()
}

/// The form fields one `GetRouteServerPropagations` call carries: the route server it asks
/// about, and the route table it narrows to when the case names one.
fn get_params(route_server_id: &str, route_table_id: Option<&str>) -> Vec<Param> {
    let mut params = vec![param("RouteServerId", route_server_id)];
    if let Some(route_table_id) = route_table_id {
        params.push(param("RouteTableId", route_table_id));
    }
    params
}

/// The `(routeTableId, state)` pairs `GetRouteServerPropagations` reports for
/// `route_server_id`, each triple's route server having first been asserted to be the one
/// asked about.
async fn reported_pairs(
    case: &str,
    router: &MultiServiceRouter,
    route_server_id: &str,
    route_table_id: Option<&str>,
) -> Vec<(String, String)> {
    let body = ok(
        case,
        router,
        "GetRouteServerPropagations",
        &get_params(route_server_id, route_table_id),
    )
    .await;
    propagations(case, &body)
        .into_iter()
        .map(|(reported_server, reported_table, state)| {
            assert_eq!(
                reported_server, route_server_id,
                "case {case:?}: a propagation of {route_server_id} must report that route \
                 server, got {reported_server:?}"
            );
            (reported_table, state)
        })
        .collect()
}

/// The form fields `EnableRouteServerPropagation` and `DisableRouteServerPropagation` share,
/// plus whatever extra parameters the case needs.
fn pair_params(route_server_id: &str, route_table_id: &str, extra: &[Param]) -> Vec<Param> {
    let mut params = vec![
        param("RouteServerId", route_server_id),
        param("RouteTableId", route_table_id),
    ];
    params.extend_from_slice(extra);
    params
}

/// `EnableRouteServerPropagation` for one pair, with whatever extra parameters the case
/// needs.
async fn enable(
    router: &MultiServiceRouter,
    route_server_id: &str,
    route_table_id: &str,
    extra: &[Param],
) -> (u16, String) {
    action(
        router,
        "EnableRouteServerPropagation",
        &pair_params(route_server_id, route_table_id, extra),
    )
    .await
}

/// `DisableRouteServerPropagation` for one pair, with whatever extra parameters the case
/// needs.
async fn disable(
    router: &MultiServiceRouter,
    route_server_id: &str,
    route_table_id: &str,
    extra: &[Param],
) -> (u16, String) {
    action(
        router,
        "DisableRouteServerPropagation",
        &pair_params(route_server_id, route_table_id, extra),
    )
    .await
}

// ---------------------------------------------------------------------------
// Rules 1, 2 and 6: the read reports the store, and only observes the clock
// ---------------------------------------------------------------------------

/// The rule: `GetRouteServerPropagations` reports exactly the route tables the named route
/// server propagates into, in `RouteTableId` order, each in the state the clock has reached —
/// a propagation being `pending` until the service's own transition span has passed and
/// `available` after it — narrowed to one table when the request names one, and the read
/// itself writes nothing.
///
/// The rows differ in which route server holds the propagation, which VPC the route table is
/// in, how many propagations one route server holds (none, one, two), and whether the request
/// narrows by route table.
#[tokio::test]
async fn get_route_server_propagations_reports_the_propagations_of_the_route_server_named() {
    let router = emulator();

    // Two route servers that propagate and a third that propagates into nothing, so "the
    // propagations of THIS route server" is a property of the store rather than of the one
    // route server a recording used.
    let first_server = create_route_server("the first route server", &router, "64512").await;
    let second_server = create_route_server("the second route server", &router, "4200000000").await;
    let idle_server = create_route_server("a route server nobody propagates with", &router, "1").await;

    // Two VPCs the propagating route servers serve, so the network side of a pair varies;
    // the first holds several route tables, so one route server can propagate into more than
    // one table of one VPC. The third VPC is what the idle route server is associated with:
    // one VPC holds at most one route server association, and a route server that propagates
    // into nothing still has to be told from one that is not associated at all.
    let first_vpc = create_vpc("the first VPC", &router, "10.11.0.0/16").await;
    let second_vpc = create_vpc("the second VPC", &router, "10.12.0.0/16").await;
    let idle_vpc = create_vpc("the idle route server's VPC", &router, "10.13.0.0/16").await;
    associate("the first route server with the first VPC", &router, &first_server, &first_vpc).await;
    associate("the second route server with the second VPC", &router, &second_server, &second_vpc).await;
    associate("the idle route server with its own VPC", &router, &idle_server, &idle_vpc).await;

    let first_table = create_route_table("a table of the first VPC", &router, &first_vpc).await;
    let second_table = create_route_table("a second table of the first VPC", &router, &first_vpc).await;
    let third_table = create_route_table("a table of the second VPC", &router, &second_vpc).await;
    let quiet_table = create_route_table("a table nobody propagates into", &router, &first_vpc).await;

    // One row per pair the table enables.
    let pairs: [(&str, &str, &str); 3] = [
        ("the first route server into the first table", &first_server, &first_table),
        ("the first route server into a second table", &first_server, &second_table),
        ("the second route server into the third table", &second_server, &third_table),
    ];

    for (case, route_server_id, route_table_id) in pairs {
        let (status, body) = enable(&router, route_server_id, route_table_id, &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from EnableRouteServerPropagation, got \
             {status} with body {body}"
        );
        // The propagation AWS hands back is not yet in force, and the pair it reports is the
        // pair the request named.
        assert_eq!(
            propagations(case, &body),
            vec![(
                route_server_id.to_string(),
                route_table_id.to_string(),
                PENDING.to_string()
            )],
            "case {case:?}: expected the requested pair in {PENDING}, got body {body}"
        );
    }

    // Still inside the transition span the enable started: the read OBSERVES the transient
    // state rather than a state nobody waited for.
    let transient = reported_pairs("the first route server, read at once", &router, &first_server, None).await;
    assert!(
        !transient.is_empty() && transient.iter().all(|(_, state)| state == PENDING),
        "case {case:?}: every propagation must still be {PENDING:?}, got {transient:?}",
        case = "the first route server, read at once"
    );

    past_the_transition().await;

    // Rule 1: each route server reports exactly its own pairs, in RouteTableId order; a
    // request naming a route table reports that table alone; and a route server, or a table,
    // with no propagation reports the empty set.
    let mut first_expected = vec![
        (first_table.clone(), AVAILABLE.to_string()),
        (second_table.clone(), AVAILABLE.to_string()),
    ];
    first_expected.sort();
    let expectations: [ReportRow<'_>; 6] = [
        (
            "the route server propagating into two tables",
            &first_server,
            None,
            first_expected,
        ),
        (
            "that route server narrowed to one of them",
            &first_server,
            Some(&second_table),
            vec![(second_table.clone(), AVAILABLE.to_string())],
        ),
        (
            "that route server narrowed to a table it does not propagate into",
            &first_server,
            Some(&quiet_table),
            Vec::new(),
        ),
        (
            "the route server propagating into one table",
            &second_server,
            None,
            vec![(third_table.clone(), AVAILABLE.to_string())],
        ),
        (
            "that route server narrowed to the table of another VPC",
            &second_server,
            Some(&first_table),
            Vec::new(),
        ),
        (
            "the route server propagating into none",
            &idle_server,
            None,
            Vec::new(),
        ),
    ];

    for (case, route_server_id, route_table_id, expected) in expectations {
        let reported = reported_pairs(case, &router, route_server_id, route_table_id).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?} for {route_server_id}, got {reported:?}"
        );
        // Rule 6: the read changed nothing a later caller sees — not for the route server
        // read, and not for a route server that has no propagation to create by being asked
        // about.
        let again = reported_pairs(case, &router, route_server_id, route_table_id).await;
        assert_eq!(
            again, expected,
            "case {case:?}: a second read of {route_server_id} must report {expected:?}, \
             got {again:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2 and 5: the disable, and what each write needs of the pair
// ---------------------------------------------------------------------------

/// The rule: a disable puts the pair in `deleting` and the clock then stops it being
/// reported at all, after which the pair can be enabled afresh; an enable of a pair that
/// already propagates and a disable of one that does not are both `IncorrectState` and write
/// nothing.
///
/// The rows differ in which call is asked of which pair and in the state that pair is in
/// (propagating, being taken down, gone, never enabled).
#[tokio::test]
async fn a_disable_takes_the_propagation_back_and_each_write_needs_its_own_state() {
    let router = emulator();
    let route_server_id = create_route_server("the route server", &router, "65001").await;
    let vpc_id = create_vpc("the VPC", &router, "10.21.0.0/16").await;
    associate("the route server with the VPC", &router, &route_server_id, &vpc_id).await;
    let propagated = create_route_table("the propagated table", &router, &vpc_id).await;
    let never_propagated = create_route_table("a table never enabled", &router, &vpc_id).await;

    let (status, body) = enable(&router, &route_server_id, &propagated, &[]).await;
    assert_eq!(
        status, 200,
        "the first enable must succeed, got {status} with body {body}"
    );

    // Rule 5: a second enable of a pair that already propagates is refused, and the state it
    // names is the state the read reports.
    let (repeat_status, repeat_body) = enable(&router, &route_server_id, &propagated, &[]).await;
    assert_eq!(
        repeat_status, 400,
        "a repeated enable must be refused, got {repeat_status} with body {repeat_body}"
    );
    assert_eq!(
        error_code(&repeat_body),
        "IncorrectState",
        "a repeated enable must answer IncorrectState, got body {repeat_body}"
    );
    assert_eq!(
        error_message(&repeat_body),
        format!(
            "The route-server ID '{route_server_id}' already propagates routes to \
             routeTable ID '{propagated}' in state '{PENDING}'"
        ),
        "a repeated enable must name the pair and the state it is in, got body {repeat_body}"
    );

    // Rule 5: a disable of a pair that never propagated is refused too, and writes nothing.
    let (unknown_status, unknown_body) = disable(&router, &route_server_id, &never_propagated, &[]).await;
    assert_eq!(
        unknown_status, 400,
        "disabling a pair that does not propagate must be refused, got {unknown_status} \
         with body {unknown_body}"
    );
    assert_eq!(
        error_code(&unknown_body),
        "IncorrectState",
        "disabling a pair that does not propagate must answer IncorrectState, got body \
         {unknown_body}"
    );
    assert_eq!(
        error_message(&unknown_body),
        format!(
            "The route-server ID '{route_server_id}' does not propagate routes to \
             routeTable ID '{never_propagated}'"
        ),
        "the refusal must name the pair the request asked about, got body {unknown_body}"
    );

    past_the_transition().await;
    let in_force = reported_pairs("after the enable settled", &router, &route_server_id, None).await;
    assert_eq!(
        in_force,
        vec![(propagated.clone(), AVAILABLE.to_string())],
        "case {case:?}: the enabled pair must be {AVAILABLE:?}, got {in_force:?}",
        case = "after the enable settled"
    );

    // A dry run of the disable answers DryRunOperation and takes nothing down.
    let (dry_status, dry_body) = disable(
        &router,
        &route_server_id,
        &propagated,
        &[param("DryRun", "true")],
    )
    .await;
    assert_eq!(
        dry_status, 412,
        "a dry run of the disable must answer HTTP 412, got {dry_status} with body {dry_body}"
    );
    assert_eq!(
        error_code(&dry_body),
        "DryRunOperation",
        "a dry run of the disable must answer DryRunOperation, got body {dry_body}"
    );
    let after_dry_run = reported_pairs("after the dry run of the disable", &router, &route_server_id, None).await;
    assert_eq!(
        after_dry_run, in_force,
        "case {case:?}: a dry run must take nothing down, got {after_dry_run:?}",
        case = "after the dry run of the disable"
    );

    // Rule 2: the disable answers the pair being taken down, and a read straight after it
    // sees that transient state rather than a pair that silently vanished.
    let (disable_status, disable_body) = disable(&router, &route_server_id, &propagated, &[]).await;
    assert_eq!(
        disable_status, 200,
        "the disable must succeed, got {disable_status} with body {disable_body}"
    );
    assert_eq!(
        propagations("the disable", &disable_body),
        vec![(
            route_server_id.clone(),
            propagated.clone(),
            DELETING.to_string()
        )],
        "the disable must answer the pair in {DELETING}, got body {disable_body}"
    );
    let taking_down = reported_pairs("read while the disable is being carried out", &router, &route_server_id, None).await;
    assert_eq!(
        taking_down,
        vec![(propagated.clone(), DELETING.to_string())],
        "case {case:?}: expected the pair {DELETING:?}, got {taking_down:?}",
        case = "read while the disable is being carried out"
    );

    // ...and once the clock has carried the disable out the pair does not propagate at all,
    // so it is reported no longer.
    past_the_transition().await;
    let gone = reported_pairs("after the disable settled", &router, &route_server_id, None).await;
    assert!(
        gone.is_empty(),
        "case {case:?}: a disabled pair must be reported no longer, got {gone:?}",
        case = "after the disable settled"
    );

    // The pair can then be enabled afresh, and the propagation that enable records is the
    // one the store reports.
    let (again_status, again_body) = enable(&router, &route_server_id, &propagated, &[]).await;
    assert_eq!(
        again_status, 200,
        "enabling a pair whose disable was carried out must succeed, got {again_status} \
         with body {again_body}"
    );
    let re_enabled = reported_pairs("after the re-enable", &router, &route_server_id, None).await;
    assert_eq!(
        re_enabled,
        vec![(propagated.clone(), PENDING.to_string())],
        "case {case:?}: expected the re-enabled pair in {PENDING:?}, got {re_enabled:?}",
        case = "after the re-enable"
    );
}

// ---------------------------------------------------------------------------
// Rule 3: propagation needs the association that carries it
// ---------------------------------------------------------------------------

/// The rule: route server propagation installs routes through the association a route server
/// holds with the VPC a route table routes for, so a pair whose route server is not
/// associated with that VPC is refused `InvalidRouteServerId.NotAssociated` and nothing is
/// written — and an association with ANOTHER VPC does not carry it either.
///
/// The rows differ in why the association is missing: no association at all, an association
/// with a different VPC, and a table of a VPC a second route server holds the association for.
#[tokio::test]
async fn propagation_needs_the_route_server_to_be_associated_with_the_tables_vpc() {
    let router = emulator();
    let unassociated = create_route_server("a route server with no association", &router, "65020").await;
    let elsewhere = create_route_server("a route server associated elsewhere", &router, "65021").await;
    let associated = create_route_server("the associated route server", &router, "65022").await;

    let served_vpc = create_vpc("the served VPC", &router, "10.31.0.0/16").await;
    let other_vpc = create_vpc("another VPC", &router, "10.32.0.0/16").await;
    associate("the route server associated elsewhere", &router, &elsewhere, &other_vpc).await;
    associate("the associated route server", &router, &associated, &served_vpc).await;

    let served_table = create_route_table("a table of the served VPC", &router, &served_vpc).await;

    // (case, the route server, the message's VPC)
    let rows: [(&str, &str, &str); 2] = [
        ("a route server with no association at all", &unassociated, &served_vpc),
        ("a route server associated with another VPC", &elsewhere, &served_vpc),
    ];

    for (case, route_server_id, vpc_id) in rows {
        for name in ["EnableRouteServerPropagation", "DisableRouteServerPropagation"] {
            let (status, body) = action(
                &router,
                name,
                &pair_params(route_server_id, &served_table, &[]),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400 from {name}, got {status} with body {body}"
            );
            assert_eq!(
                error_code(&body),
                "InvalidRouteServerId.NotAssociated",
                "case {case:?}: expected InvalidRouteServerId.NotAssociated from {name}, got \
                 body {body}"
            );
            assert_eq!(
                error_message(&body),
                format!(
                    "The route-server ID '{route_server_id}' is not associated with the vpc \
                     ID '{vpc_id}' of routeTable ID '{served_table}'"
                ),
                "case {case:?}: expected the refusal to name the pair and the VPC, got body \
                 {body}"
            );
        }
        // Nothing was written for a pair the service refused.
        let reported = reported_pairs(case, &router, route_server_id, None).await;
        assert!(
            reported.is_empty(),
            "case {case:?}: a refused request must write no propagation, got {reported:?}"
        );
    }

    // The route server that IS associated with that VPC propagates into its table, so the
    // refusal above is about the association and not about the table.
    let (status, body) = enable(&router, &associated, &served_table, &[]).await;
    assert_eq!(
        status, 200,
        "the associated route server must be able to propagate, got {status} with body {body}"
    );
}

// ---------------------------------------------------------------------------
// Rule 4: an id that resolves to nothing is refused, not answered
// ---------------------------------------------------------------------------

/// The rule: a `RouteServerId` the request left out is `MissingParameter`, one that is not a
/// route server id at all is `InvalidRouteServerId.Malformed`, a well-formed one the account
/// holds no route server for is `InvalidRouteServerId.NotFound`, and a `RouteTableId` the
/// account holds no table for is `InvalidRouteTableID.NotFound` — on the read and on both
/// writes, and before any `DryRun` is honoured.
///
/// The rows differ in the FORM of the id (absent, no suffix, a suffix of neither published
/// length, a non-hexadecimal digit, the wrong case, another resource's id, a missing
/// separator, a well-formed id at each of the two published EC2 id lengths), in which
/// parameter is missing, and in which operation is asked.
#[tokio::test]
async fn an_unresolvable_id_is_refused_rather_than_answered() {
    let router = emulator();
    let route_server_id = create_route_server("the route server", &router, "65030").await;
    let vpc_id = create_vpc("the VPC", &router, "10.41.0.0/16").await;
    associate("the route server with the VPC", &router, &route_server_id, &vpc_id).await;
    let route_table_id = create_route_table("the route table", &router, &vpc_id).await;

    // (case, the id, the code, the message) — the route server id forms every operation of
    // this slice resolves the same way.
    let server_ids: [(&str, &str, &str, String); 9] = [
        (
            "a RouteServerId the request carried empty",
            "",
            "MissingParameter",
            "The request must contain the parameter routeServerId".to_string(),
        ),
        (
            "the prefix with no digits",
            "rs-",
            "InvalidRouteServerId.Malformed",
            "Invalid id: \"rs-\"".to_string(),
        ),
        (
            "a suffix of neither published length",
            "rs-0a1b2c",
            "InvalidRouteServerId.Malformed",
            "Invalid id: \"rs-0a1b2c\"".to_string(),
        ),
        (
            "a non-hexadecimal digit in the suffix",
            "rs-0a1b2c3d4e5f6071z",
            "InvalidRouteServerId.Malformed",
            "Invalid id: \"rs-0a1b2c3d4e5f6071z\"".to_string(),
        ),
        (
            "an upper-case suffix",
            "rs-0A1B2C3D",
            "InvalidRouteServerId.Malformed",
            "Invalid id: \"rs-0A1B2C3D\"".to_string(),
        ),
        (
            "another resource's id",
            "rtb-0a1b2c3d4e5f60718",
            "InvalidRouteServerId.Malformed",
            "Invalid id: \"rtb-0a1b2c3d4e5f60718\"".to_string(),
        ),
        (
            "the prefix without its separator",
            "rs0a1b2c3d",
            "InvalidRouteServerId.Malformed",
            "Invalid id: \"rs0a1b2c3d\"".to_string(),
        ),
        (
            "a well-formed short id no route server has",
            "rs-9f8e7d6c",
            "InvalidRouteServerId.NotFound",
            "The route-server ID 'rs-9f8e7d6c' does not exist".to_string(),
        ),
        (
            "a well-formed long id no route server has",
            "rs-0a1b2c3d4e5f60718",
            "InvalidRouteServerId.NotFound",
            "The route-server ID 'rs-0a1b2c3d4e5f60718' does not exist".to_string(),
        ),
    ];

    let mut rows: Vec<RefusalRow<'_>> = Vec::new();
    for (case, named_server, code, message) in server_ids {
        rows.push((
            case,
            "GetRouteServerPropagations",
            get_params(named_server, None),
            code,
            message.clone(),
        ));
        rows.push((
            case,
            "EnableRouteServerPropagation",
            pair_params(named_server, &route_table_id, &[]),
            code,
            message.clone(),
        ));
        rows.push((
            case,
            "DisableRouteServerPropagation",
            pair_params(named_server, &route_table_id, &[]),
            code,
            message,
        ));
    }

    // The route table side: the writes require one, the read narrows by one, and neither
    // accepts a table the account does not hold.
    let unknown_table_message =
        "The routeTable ID 'rtb-0a1b2c3d4e5f6071f' does not exist".to_string();
    rows.extend([
        (
            "an empty RouteTableId on the enable",
            "EnableRouteServerPropagation",
            pair_params(&route_server_id, "", &[]),
            "MissingParameter",
            "The request must contain the parameter routeTableId".to_string(),
        ),
        (
            "an empty RouteTableId on the disable",
            "DisableRouteServerPropagation",
            pair_params(&route_server_id, "", &[]),
            "MissingParameter",
            "The request must contain the parameter routeTableId".to_string(),
        ),
        (
            "a route table the account does not hold, on the read",
            "GetRouteServerPropagations",
            get_params(&route_server_id, Some("rtb-0a1b2c3d4e5f6071f")),
            "InvalidRouteTableID.NotFound",
            unknown_table_message.clone(),
        ),
        (
            "a route table the account does not hold, on the enable",
            "EnableRouteServerPropagation",
            pair_params(&route_server_id, "rtb-0a1b2c3d4e5f6071f", &[]),
            "InvalidRouteTableID.NotFound",
            unknown_table_message.clone(),
        ),
        (
            "a route table the account does not hold, on the disable",
            "DisableRouteServerPropagation",
            pair_params(&route_server_id, "rtb-0a1b2c3d4e5f6071f", &[]),
            "InvalidRouteTableID.NotFound",
            unknown_table_message,
        ),
    ]);

    for (case, name, params, code, message) in rows {
        let (status, body) = action(&router, name, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 from {name}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            code,
            "case {case:?}: expected {code} from {name}, got body {body}"
        );
        assert_eq!(
            error_message(&body),
            message,
            "case {case:?}: expected the message {message:?} from {name}, got body {body}"
        );

        // A dry run of the same request is refused the same way: the flag asks whether the
        // request WOULD have worked, and this one would not.
        let mut dry_params = params.clone();
        dry_params.push(param("DryRun", "true"));
        let (dry_status, dry_body) = action(&router, name, &dry_params).await;
        assert_eq!(
            error_code(&dry_body),
            code,
            "case {case:?}: a dry run of {name} must be refused with {code} too, got \
             {dry_status} with body {dry_body}"
        );
    }
}

/// A `DryRun` of a request that WOULD have succeeded answers `DryRunOperation` and writes
/// nothing — asserted on the read and on the enable, the two calls of this slice a request
/// that has nothing yet can make.
#[tokio::test]
async fn a_dry_run_of_a_request_that_would_succeed_writes_nothing() {
    let router = emulator();
    let route_server_id = create_route_server("the route server", &router, "65040").await;
    let vpc_id = create_vpc("the VPC", &router, "10.51.0.0/16").await;
    associate("the route server with the VPC", &router, &route_server_id, &vpc_id).await;
    let route_table_id = create_route_table("the route table", &router, &vpc_id).await;

    for (case, name, params) in [
        (
            "a dry run of the read",
            "GetRouteServerPropagations",
            {
                let mut params = get_params(&route_server_id, Some(&route_table_id));
                params.push(param("DryRun", "true"));
                params
            },
        ),
        (
            "a dry run of the enable",
            "EnableRouteServerPropagation",
            pair_params(&route_server_id, &route_table_id, &[param("DryRun", "true")]),
        ),
    ] {
        let (status, body) = action(&router, name, &params).await;
        assert_eq!(
            status, 412,
            "case {case:?}: expected HTTP 412 from {name}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "DryRunOperation",
            "case {case:?}: expected DryRunOperation from {name}, got body {body}"
        );
    }

    // Neither dry run recorded a propagation.
    let after = reported_pairs("after the dry runs", &router, &route_server_id, None).await;
    assert!(
        after.is_empty(),
        "case {case:?}: a dry run must record no propagation, got {after:?}",
        case = "after the dry runs"
    );
}
