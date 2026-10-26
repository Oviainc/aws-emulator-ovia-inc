// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `CreateClientVpnRoute` and the `DescribeClientVpnRoutes` that reads
//! its writes back, driven over the wire the service actually speaks: `ec2Query` — an
//! `application/x-www-form-urlencoded` POST whose body carries `Action=<Operation>`, answered
//! with XML (README.md / OPERATIONS.md "Wire protocol"). ec2Query is selected by the `ec2`
//! signing name in the request's SigV4 credential scope, so every call here carries one.
//!
//! The rule under test is that `CreateClientVpnRoute` ADDS ONE ROUTE TO THE ROUTE TABLE OF
//! THE ENDPOINT THE REQUEST NAMES, from the destination range it carries and the target it
//! names a path through, answers that route `creating` (the clock owes it `active` a
//! transition span later), and holds the route under the endpoint + destination + target that
//! name it — so a second request for that same path is a duplicate rather than a second
//! route, while the same destination through another target, or on another endpoint, is a
//! route of its own. Every table row below is a distinct input along what that generalises
//! over: which destination, how large a block, which target (a target network of the endpoint
//! or the local network), whether a description is carried, which endpoint, and — for the
//! refusals — which part of the request cannot be served.

use std::time::Duration;

use ec2::ops::types::{ClientVpnEndpoint, TransitGatewayConfigurationDescribeEndpointStructure};
use ec2::state::{
    ClientVpnTargetNetworkSpec as TargetNetworkSpec, ComAmazonawsEc2Amazonec2State as Ec2State,
    MAX_ECHOED_CLIENT_VPN_ENDPOINT_ID_CHARS, TRANSITION_DELAY,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, elements, error_code, error_message, filters};

/// The Region every call here is signed for. The endpoint and route stores are per account
/// rather than per Region, so the Region only selects the ec2Query envelope.
const REGION: &str = "us-east-1";

/// The ec2Query action that adds a route.
const CREATE: &str = "CreateClientVpnRoute";

/// The ec2Query action that reports them.
const DESCRIBE: &str = "DescribeClientVpnRoutes";

/// A subnet id that is a target network of the seeded endpoint.
const TARGET_SUBNET_A: &str = "subnet-00000000000000001";

/// A second target network of the same endpoint, so a destination can be routed through
/// either of two paths.
const TARGET_SUBNET_B: &str = "subnet-00000000000000002";

/// A well-formed subnet id that is NOT a target network of the endpoint.
const UNASSOCIATED_SUBNET: &str = "subnet-00000000000000009";

/// The VPC the target networks sit in.
const TARGET_VPC: &str = "vpc-00000000000000001";

/// Long enough past [`TRANSITION_DELAY`] that the `creating` -> `active` move the clock owes
/// a route is certainly due.
fn past_the_transition() -> Duration {
    TRANSITION_DELAY + Duration::from_millis(250)
}

/// One `CreateClientVpnRoute` call carrying `params`, as its HTTP status and XML body.
async fn create(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    super::ec2_query::call(router, REGION, CREATE, params).await
}

/// One `DescribeClientVpnRoutes` call carrying `params`.
async fn describe(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    super::ec2_query::call(router, REGION, DESCRIBE, params).await
}

/// The form fields of one add: the endpoint, the destination, and optionally the target and
/// the description.
fn add_params(
    endpoint_id: &str,
    destination: &str,
    target: Option<&str>,
    description: Option<&str>,
) -> Vec<(String, String)> {
    let mut params = vec![
        (
            "ClientVpnEndpointId".to_string(),
            endpoint_id.to_string(),
        ),
        (
            "DestinationCidrBlock".to_string(),
            destination.to_string(),
        ),
    ];
    if let Some(target) = target {
        params.push(("TargetVpcSubnetId".to_string(), target.to_string()));
    }
    if let Some(description) = description {
        params.push(("Description".to_string(), description.to_string()));
    }
    params
}

/// The one `ClientVpnEndpointId` parameter every describe below carries.
fn endpoint_param(endpoint_id: &str) -> Vec<(String, String)> {
    vec![(
        "ClientVpnEndpointId".to_string(),
        endpoint_id.to_string(),
    )]
}

/// `endpoint_param` plus `extra`.
fn describe_params(endpoint_id: &str, extra: &[(String, String)]) -> Vec<(String, String)> {
    let mut params = endpoint_param(endpoint_id);
    params.extend(extra.iter().cloned());
    params
}

/// Write one Client VPN endpoint, the way `CreateClientVpnEndpoint` would, and answer the
/// `ClientVpnEndpointId` it was minted with.
fn seed_endpoint(engine: &engine::InMemoryEngine, description: &str) -> String {
    engine
        .state::<Ec2State>()
        .lock()
        .insert_client_vpn_endpoint(ClientVpnEndpoint {
            description: Some(description.to_string()),
            client_cidr_block: Some("10.100.0.0/22".to_string()),
            vpc_id: Some(TARGET_VPC.to_string()),
            ..Default::default()
        })
        .expect("the store must take the Client VPN endpoint")
}

/// Write one Client VPN endpoint that is attached to a Transit Gateway rather than to a VPC,
/// and answer its id beside the attachment its routes leave through.
fn seed_transit_gateway_endpoint(engine: &engine::InMemoryEngine) -> (String, String) {
    let attachment = "tgw-attach-00000000000000001".to_string();
    let endpoint = engine
        .state::<Ec2State>()
        .lock()
        .insert_client_vpn_endpoint(ClientVpnEndpoint {
            description: Some("a Transit Gateway-based endpoint".to_string()),
            client_cidr_block: Some("10.200.0.0/22".to_string()),
            transit_gateway_configuration: Some(
                TransitGatewayConfigurationDescribeEndpointStructure {
                    transit_gateway_id: Some("tgw-00000000000000001".to_string()),
                    transit_gateway_attachment_id: Some(attachment.clone()),
                    ..Default::default()
                },
            ),
            ..Default::default()
        })
        .expect("the store must take the Client VPN endpoint");
    (endpoint, attachment)
}

/// Associate `subnets` with `endpoint_id`, the way `AssociateClientVpnTargetNetwork` would,
/// so a route may be added through them.
fn seed_target_networks(engine: &engine::InMemoryEngine, endpoint_id: &str, subnets: &[&str]) {
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    for subnet in subnets {
        guard
            .insert_client_vpn_target_network(TargetNetworkSpec {
                client_vpn_endpoint_id: endpoint_id.to_string(),
                target_network_id: (*subnet).to_string(),
                vpc_id: TARGET_VPC.to_string(),
                security_groups: Vec::new(),
            })
            .unwrap_or_else(|| panic!("the store must take the association for {subnet}"));
    }
}

/// How many routes the store holds, whichever endpoint they belong to — what the "nothing was
/// written" assertions read.
fn stored_routes(engine: &engine::InMemoryEngine) -> usize {
    engine.state::<Ec2State>().lock().client_vpn_routes.len()
}

/// One row of the add table: a destination, the target it is routed through and the
/// description the request carries.
struct AddRow {
    case: &'static str,
    destination: &'static str,
    target: &'static str,
    description: Option<&'static str>,
}

/// The adds the report below is written against. They differ along every axis the rule
/// generalises over: the size of the destination block (a /0 default route, a /16, a /8 and a
/// single /32 host), which target the traffic leaves through, whether a description is
/// carried, and — the last two rows — one destination routed through two different targets,
/// which is two routes rather than a duplicate.
fn add_rows() -> Vec<AddRow> {
    vec![
        AddRow {
            case: "internet access through the first target network",
            destination: "0.0.0.0/0",
            target: TARGET_SUBNET_A,
            description: Some("internet access"),
        },
        AddRow {
            case: "a /8 block, no description",
            destination: "172.0.0.0/8",
            target: TARGET_SUBNET_A,
            description: None,
        },
        AddRow {
            case: "a single host",
            destination: "192.168.5.7/32",
            target: TARGET_SUBNET_B,
            description: Some("one host"),
        },
        AddRow {
            case: "the local network",
            destination: "10.100.0.0/22",
            target: "local",
            description: None,
        },
        AddRow {
            case: "a peered VPC through the first target network",
            destination: "10.20.0.0/16",
            target: TARGET_SUBNET_A,
            description: None,
        },
        AddRow {
            case: "the same peered VPC through the second target network",
            destination: "10.20.0.0/16",
            target: TARGET_SUBNET_B,
            description: None,
        },
    ]
}

/// The rows in the order the report pages over them: destination, then target.
fn rows_in_report_order() -> Vec<AddRow> {
    let mut rows = add_rows();
    rows.sort_by(|left, right| {
        (left.destination, left.target).cmp(&(right.destination, right.target))
    });
    rows
}

/// Add every row of `rows` to `endpoint_id`, asserting each answers the `creating` the clock
/// owes an `active`.
async fn add_every_row(router: &MultiServiceRouter, endpoint_id: &str, rows: &[AddRow]) {
    for row in rows {
        let (status, body) = create(
            router,
            &add_params(
                endpoint_id,
                row.destination,
                Some(row.target),
                row.description,
            ),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}",
            case = row.case
        );
        assert_eq!(
            element(&body, "code").as_deref(),
            Some("creating"),
            "case {case:?}: a route is answered creating, got {body}",
            case = row.case
        );
    }
}

// ---------------------------------------------------------------------------
// What an add writes, and what reads it back
// ---------------------------------------------------------------------------

/// Every row is stored under the endpoint, the destination and the target that name it, and
/// `DescribeClientVpnRoutes` reports each one with the members the model declares — the route
/// type, the `add-route` origin that says a caller added it, the description the request
/// carried — in destination-then-target order. The route the clock owes `active` reaches it
/// once the span has passed, and a second endpoint's report carries none of them.
#[tokio::test]
async fn every_added_route_is_reported_for_its_own_endpoint_and_reaches_active() {
    let (router, engine) = super::ec2_query::emulator();
    let endpoint = seed_endpoint(&engine, "the endpoint under test");
    let other_endpoint = seed_endpoint(&engine, "another endpoint of the same account");
    seed_target_networks(&engine, &endpoint, &[TARGET_SUBNET_A, TARGET_SUBNET_B]);
    seed_target_networks(&engine, &other_endpoint, &[TARGET_SUBNET_A]);

    let rows = add_rows();
    add_every_row(&router, &endpoint, &rows).await;

    // One route per row: the two rows that share a destination differ in their target, so
    // neither is a duplicate of the other.
    assert_eq!(
        stored_routes(&engine),
        rows.len(),
        "expected one stored route per added row, got {held}",
        held = stored_routes(&engine)
    );

    let (status, body) = describe(&router, &endpoint_param(&endpoint)).await;
    assert_eq!(
        status, 200,
        "the report: expected HTTP 200, got {status} with body {body}"
    );
    let expected: Vec<String> = rows_in_report_order()
        .iter()
        .map(|row| row.destination.to_string())
        .collect();
    assert_eq!(
        elements(&body, "destinationCidr"),
        expected,
        "the report: expected every destination in destination-then-target order, got {body}"
    );
    let expected_targets: Vec<String> = rows_in_report_order()
        .iter()
        .map(|row| row.target.to_string())
        .collect();
    assert_eq!(
        elements(&body, "targetSubnet"),
        expected_targets,
        "the report: expected each route's own target, got {body}"
    );
    for origin in elements(&body, "origin") {
        assert_eq!(
            origin, "add-route",
            "the report: a route a caller added carries the add-route origin, got {body}"
        );
    }
    for reported in elements(&body, "clientVpnEndpointId") {
        assert_eq!(
            reported, endpoint,
            "the report: every route belongs to the endpoint asked for, got {body}"
        );
    }

    // The description round-trips for the row that carried one, and no description element is
    // reported for the row that carried none.
    for row in rows.iter().filter(|row| row.target == TARGET_SUBNET_B) {
        let (status, body) = describe(
            &router,
            &describe_params(&endpoint, &filters(&[("destination-cidr", &[row.destination])])),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}",
            case = row.case
        );
        assert_eq!(
            element(&body, "description").as_deref(),
            row.description,
            "case {case:?}: expected the description the add carried, got {body}",
            case = row.case
        );
    }

    // The other endpoint holds nothing: a route belongs to the endpoint it was added to.
    let (status, body) = describe(&router, &endpoint_param(&other_endpoint)).await;
    assert_eq!(
        status, 200,
        "the other endpoint: expected HTTP 200, got {status} with body {body}"
    );
    assert!(
        elements(&body, "destinationCidr").is_empty(),
        "the other endpoint: expected no route, got {body}"
    );

    // The move the clock owes each route, applied once its instant has passed.
    tokio::time::sleep(past_the_transition()).await;
    let (status, body) = describe(&router, &endpoint_param(&endpoint)).await;
    assert_eq!(
        status, 200,
        "after the transition: expected HTTP 200, got {status} with body {body}"
    );
    let codes = elements(&body, "code");
    assert_eq!(
        codes.len(),
        rows.len(),
        "after the transition: expected one status per route, got {body}"
    );
    for code in codes {
        assert_eq!(
            code, "active",
            "after the transition: every route is active, got {body}"
        );
    }
}

/// A route of a Transit Gateway-based endpoint needs no target subnet — "For Transit
/// Gateway-based endpoints, this parameter is not required" — and leaves through that
/// endpoint's own attachment instead, which is what the report states.
#[tokio::test]
async fn a_transit_gateway_endpoint_routes_through_its_attachment_without_a_target_subnet() {
    let (router, engine) = super::ec2_query::emulator();
    let (endpoint, attachment) = seed_transit_gateway_endpoint(&engine);

    let (status, body) = create(
        &router,
        &add_params(&endpoint, "10.30.0.0/16", None, Some("through the gateway")),
    )
    .await;
    assert_eq!(
        status, 200,
        "transit gateway add: expected HTTP 200, got {status} with body {body}"
    );

    let (status, body) = describe(&router, &endpoint_param(&endpoint)).await;
    assert_eq!(
        status, 200,
        "transit gateway report: expected HTTP 200, got {status} with body {body}"
    );
    assert_eq!(
        element(&body, "destinationCidr").as_deref(),
        Some("10.30.0.0/16"),
        "transit gateway report: expected the destination the add carried, got {body}"
    );
    assert_eq!(
        element(&body, "transitGatewayAttachmentId").as_deref(),
        Some(attachment.as_str()),
        "transit gateway report: expected the endpoint's own attachment, got {body}"
    );
    assert!(
        element(&body, "targetSubnet").is_none(),
        "transit gateway report: a route through an attachment names no subnet, got {body}"
    );

    // A route of the SAME endpoint that DOES name a target network leaves through that
    // subnet, so it carries one path and not two.
    seed_target_networks(&engine, &endpoint, &[TARGET_SUBNET_A]);
    let (status, body) = create(
        &router,
        &add_params(&endpoint, "10.31.0.0/16", Some(TARGET_SUBNET_A), None),
    )
    .await;
    assert_eq!(
        status, 200,
        "transit gateway add through a subnet: expected HTTP 200, got {status} with body {body}"
    );
    let (status, body) = describe(
        &router,
        &describe_params(&endpoint, &filters(&[("destination-cidr", &["10.31.0.0/16"])])),
    )
    .await;
    assert_eq!(
        status, 200,
        "the subnet route of a transit gateway endpoint: expected HTTP 200, got {status} with \
         body {body}"
    );
    assert_eq!(
        element(&body, "targetSubnet").as_deref(),
        Some(TARGET_SUBNET_A),
        "the subnet route of a transit gateway endpoint: expected the subnet the add named, \
         got {body}"
    );
    assert!(
        element(&body, "transitGatewayAttachmentId").is_none(),
        "a route through a subnet carries ONE path, not the attachment too, got {body}"
    );

    // An endpoint whose Transit Gateway configuration names no attachment offers no path
    // either, so a request that names no target subnet is refused exactly as it is on a
    // VPC-based endpoint rather than storing a route that routes nowhere.
    let pathless = engine
        .state::<Ec2State>()
        .lock()
        .insert_client_vpn_endpoint(ClientVpnEndpoint {
            description: Some("a Transit Gateway endpoint with no attachment".to_string()),
            transit_gateway_configuration: Some(
                TransitGatewayConfigurationDescribeEndpointStructure {
                    transit_gateway_id: Some("tgw-00000000000000002".to_string()),
                    ..Default::default()
                },
            ),
            ..Default::default()
        })
        .expect("the store must take the Client VPN endpoint");
    let (status, body) = create(&router, &add_params(&pathless, "10.32.0.0/16", None, None)).await;
    assert_eq!(
        status, 400,
        "a configuration with no attachment: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "MissingParameter",
        "a configuration with no attachment: expected the missing-parameter code, got {body}"
    );
}

// ---------------------------------------------------------------------------
// What an add refuses
// ---------------------------------------------------------------------------

/// One refusal row: the request's three members and the EC2 code it is answered with.
struct RefusalRow {
    case: &'static str,
    endpoint: Option<&'static str>,
    destination: &'static str,
    target: Option<&'static str>,
    code: &'static str,
}

/// The requests this operation cannot serve, one per reason, each with the code a client
/// branches on. `endpoint: None` means "the endpoint this suite seeded", so a row exercises
/// only the member it is about.
fn refusal_rows() -> Vec<RefusalRow> {
    vec![
        // An endpoint id this account holds no endpoint for, in each of the forms a caller
        // reaches for: the Client VPN surface answers the one refusal for all of them.
        RefusalRow {
            case: "an endpoint id of the right form that names nothing",
            endpoint: Some("cvpn-endpoint-0123456789abcdef0"),
            destination: "10.20.0.0/16",
            target: Some(TARGET_SUBNET_A),
            code: "InvalidClientVpnEndpointId.NotFound",
        },
        RefusalRow {
            case: "the all-zero placeholder endpoint id",
            endpoint: Some("cvpn-endpoint-00000000000000000"),
            destination: "10.20.0.0/16",
            target: Some(TARGET_SUBNET_A),
            code: "InvalidClientVpnEndpointId.NotFound",
        },
        RefusalRow {
            case: "an endpoint id that is another resource's",
            endpoint: Some(TARGET_VPC),
            destination: "10.20.0.0/16",
            target: Some(TARGET_SUBNET_A),
            code: "InvalidClientVpnEndpointId.NotFound",
        },
        RefusalRow {
            case: "an empty endpoint id",
            endpoint: Some(""),
            destination: "10.20.0.0/16",
            target: Some(TARGET_SUBNET_A),
            code: "InvalidClientVpnEndpointId.NotFound",
        },
        // A destination that is not an IPv4 CIDR block naming its own range.
        RefusalRow {
            case: "a destination that is an address and no block",
            endpoint: None,
            destination: "10.20.0.0",
            target: Some(TARGET_SUBNET_A),
            code: "InvalidParameterValue",
        },
        RefusalRow {
            case: "a destination that is not an address at all",
            endpoint: None,
            destination: "not-a-cidr-block",
            target: Some(TARGET_SUBNET_A),
            code: "InvalidParameterValue",
        },
        RefusalRow {
            case: "a prefix length past the width of an IPv4 address",
            endpoint: None,
            destination: "10.20.0.0/33",
            target: Some(TARGET_SUBNET_A),
            code: "InvalidParameterValue",
        },
        RefusalRow {
            case: "a block that names an address inside a range",
            endpoint: None,
            destination: "10.20.0.5/16",
            target: Some(TARGET_SUBNET_A),
            code: "InvalidParameterValue",
        },
        RefusalRow {
            case: "an IPv6 destination",
            endpoint: None,
            destination: "2001:db8::/32",
            target: Some(TARGET_SUBNET_A),
            code: "InvalidParameterValue",
        },
        RefusalRow {
            case: "an empty destination",
            endpoint: None,
            destination: "",
            target: Some(TARGET_SUBNET_A),
            code: "InvalidParameterValue",
        },
        // A target that is not a subnet id, and one that is but is not a target network of
        // this endpoint: two different answers.
        RefusalRow {
            case: "a target that is not a subnet id",
            endpoint: None,
            destination: "10.20.0.0/16",
            target: Some("not-a-subnet"),
            code: "InvalidSubnetID.Malformed",
        },
        RefusalRow {
            case: "a target that is another resource's id",
            endpoint: None,
            destination: "10.20.0.0/16",
            target: Some(TARGET_VPC),
            code: "InvalidSubnetID.Malformed",
        },
        RefusalRow {
            case: "a well-formed subnet that is not a target network of the endpoint",
            endpoint: None,
            destination: "10.20.0.0/16",
            target: Some(UNASSOCIATED_SUBNET),
            code: "InvalidClientVpnActiveAssociationNotFound",
        },
        // A VPC-based endpoint needs the target this request did not name.
        RefusalRow {
            case: "no target at all on a VPC-based endpoint",
            endpoint: None,
            destination: "10.20.0.0/16",
            target: None,
            code: "MissingParameter",
        },
    ]
}

/// Every unservable request is refused with its own EC2 code, and writes nothing — the store
/// holds exactly the one route the suite added before the table ran.
#[tokio::test]
async fn an_unservable_add_is_refused_with_its_own_code_and_writes_nothing() {
    let (router, engine) = super::ec2_query::emulator();
    let endpoint = seed_endpoint(&engine, "the endpoint under test");
    seed_target_networks(&engine, &endpoint, &[TARGET_SUBNET_A, TARGET_SUBNET_B]);

    // One route that every row below must leave exactly as it is.
    let (status, body) = create(
        &router,
        &add_params(&endpoint, "10.99.0.0/16", Some(TARGET_SUBNET_A), None),
    )
    .await;
    assert_eq!(
        status, 200,
        "the route the table must not disturb: expected HTTP 200, got {status} with body {body}"
    );

    for row in refusal_rows() {
        let endpoint_id = row.endpoint.unwrap_or(endpoint.as_str());
        let (status, body) = create(
            &router,
            &add_params(endpoint_id, row.destination, row.target, None),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}",
            case = row.case
        );
        assert_eq!(
            error_code(&body),
            row.code,
            "case {case:?}: expected the code {expected:?}, got {body}",
            case = row.case,
            expected = row.code
        );
        assert_eq!(
            stored_routes(&engine),
            1,
            "case {case:?}: a refused add writes nothing, got {held} stored routes",
            case = row.case,
            held = stored_routes(&engine)
        );
    }

    // The refusal of an endpoint it cannot resolve names the id the request sent, so a caller
    // can see which one was wrong.
    let unknown = "cvpn-endpoint-0123456789abcdef0";
    let (_, body) = create(
        &router,
        &add_params(unknown, "10.20.0.0/16", Some(TARGET_SUBNET_A), None),
    )
    .await;
    assert_eq!(
        error_message(&body),
        format!("Endpoint {unknown} does not exist"),
        "the unknown-endpoint refusal names the id the request sent, got {body}"
    );
}

/// The same destination through the same target on the same endpoint is ONE route: the second
/// request is refused as a duplicate and writes nothing, while that destination through
/// another target, and on another endpoint, is a route of its own.
#[tokio::test]
async fn a_route_the_endpoint_already_holds_is_refused_as_a_duplicate() {
    let (router, engine) = super::ec2_query::emulator();
    let endpoint = seed_endpoint(&engine, "the endpoint under test");
    let other_endpoint = seed_endpoint(&engine, "another endpoint of the same account");
    seed_target_networks(&engine, &endpoint, &[TARGET_SUBNET_A, TARGET_SUBNET_B]);
    seed_target_networks(&engine, &other_endpoint, &[TARGET_SUBNET_A]);

    let destination = "10.40.0.0/16";
    let (status, _) = create(
        &router,
        &add_params(&endpoint, destination, Some(TARGET_SUBNET_A), None),
    )
    .await;
    assert_eq!(status, 200, "the first add must succeed, got {status}");

    let (status, body) = create(
        &router,
        &add_params(&endpoint, destination, Some(TARGET_SUBNET_A), None),
    )
    .await;
    assert_eq!(
        status, 400,
        "the repeat: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidClientVpnDuplicateRoute",
        "the repeat: expected the duplicate-route code, got {body}"
    );
    assert_eq!(
        stored_routes(&engine),
        1,
        "the repeat writes nothing, got {held} stored routes",
        held = stored_routes(&engine)
    );

    // The same destination through the OTHER target is a second path, not a duplicate.
    let (status, body) = create(
        &router,
        &add_params(&endpoint, destination, Some(TARGET_SUBNET_B), None),
    )
    .await;
    assert_eq!(
        status, 200,
        "the second target: expected HTTP 200, got {status} with body {body}"
    );

    // And the same destination and target on ANOTHER endpoint is a route of that endpoint.
    let (status, body) = create(
        &router,
        &add_params(&other_endpoint, destination, Some(TARGET_SUBNET_A), None),
    )
    .await;
    assert_eq!(
        status, 200,
        "the other endpoint: expected HTTP 200, got {status} with body {body}"
    );
    assert_eq!(
        stored_routes(&engine),
        3,
        "expected three distinct routes, got {held}",
        held = stored_routes(&engine)
    );
}

/// `DryRun` reports the permission outcome and reads nothing: it answers HTTP 412
/// `DryRunOperation` for a request that names an endpoint this account does not hold just as
/// it does for one that would have succeeded, and it writes nothing either way.
#[tokio::test]
async fn a_dry_run_reports_the_permission_outcome_and_writes_nothing() {
    let (router, engine) = super::ec2_query::emulator();
    let endpoint = seed_endpoint(&engine, "the endpoint under test");
    seed_target_networks(&engine, &endpoint, &[TARGET_SUBNET_A]);

    let cases: [(&str, &str, &str); 3] = [
        ("a request that would have succeeded", endpoint.as_str(), "10.50.0.0/16"),
        (
            "a request naming an endpoint this account does not hold",
            "cvpn-endpoint-0123456789abcdef0",
            "10.50.0.0/16",
        ),
        (
            "a request whose destination is not a CIDR block",
            endpoint.as_str(),
            "not-a-cidr-block",
        ),
    ];
    for (case, endpoint_id, destination) in cases {
        let mut params = add_params(endpoint_id, destination, Some(TARGET_SUBNET_A), None);
        params.push(("DryRun".to_string(), "true".to_string()));
        let (status, body) = create(&router, &params).await;
        assert_eq!(
            status, 412,
            "case {case:?}: expected HTTP 412, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "DryRunOperation",
            "case {case:?}: expected the dry-run code, got {body}"
        );
        assert_eq!(
            stored_routes(&engine),
            0,
            "case {case:?}: a dry run writes nothing, got {held} stored routes",
            held = stored_routes(&engine)
        );
    }
}

/// A `ClientToken` makes the add idempotent: the same request replayed with the same token
/// reports the route that token added rather than refusing it as a duplicate, and adds
/// nothing; the same token with a different request is refused as a mismatch.
#[tokio::test]
async fn a_client_token_makes_one_add_idempotent() {
    let (router, engine) = super::ec2_query::emulator();
    let endpoint = seed_endpoint(&engine, "the endpoint under test");
    seed_target_networks(&engine, &endpoint, &[TARGET_SUBNET_A, TARGET_SUBNET_B]);

    let token = ("ClientToken".to_string(), "a-token-of-this-request".to_string());
    let mut params = add_params(&endpoint, "10.60.0.0/16", Some(TARGET_SUBNET_A), Some("once"));
    params.push(token.clone());

    let (status, body) = create(&router, &params).await;
    assert_eq!(
        status, 200,
        "the first add: expected HTTP 200, got {status} with body {body}"
    );

    let (status, body) = create(&router, &params).await;
    assert_eq!(
        status, 200,
        "the replay: expected HTTP 200, got {status} with body {body}"
    );
    assert_eq!(
        stored_routes(&engine),
        1,
        "the replay adds no second route, got {held}",
        held = stored_routes(&engine)
    );

    let mut different = add_params(&endpoint, "10.61.0.0/16", Some(TARGET_SUBNET_B), None);
    different.push(token);
    let (status, body) = create(&router, &different).await;
    assert_eq!(
        status, 400,
        "the reused token: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "IdempotentParameterMismatch",
        "the reused token: expected the idempotency-mismatch code, got {body}"
    );
    assert_eq!(
        stored_routes(&engine),
        1,
        "the reused token writes nothing, got {held}",
        held = stored_routes(&engine)
    );
}

// ---------------------------------------------------------------------------
// What the report narrows by, and how it pages
// ---------------------------------------------------------------------------

/// Each documented filter selects on the attribute it names, an undocumented one is refused
/// rather than read as "no results", and describing writes nothing.
#[tokio::test]
async fn the_report_narrows_by_each_documented_filter_and_refuses_any_other() {
    let (router, engine) = super::ec2_query::emulator();
    let endpoint = seed_endpoint(&engine, "the endpoint under test");
    seed_target_networks(&engine, &endpoint, &[TARGET_SUBNET_A, TARGET_SUBNET_B]);
    let rows = add_rows();
    add_every_row(&router, &endpoint, &rows).await;

    // (case, filter name, filter values, how many routes must be reported)
    let cases: [(&str, &str, &[&str], usize); 5] = [
        ("one destination through two targets", "destination-cidr", &["10.20.0.0/16"], 2),
        ("a destination through one target", "destination-cidr", &["0.0.0.0/0"], 1),
        ("two destinations at once", "destination-cidr", &["0.0.0.0/0", "172.0.0.0/8"], 2),
        ("one target network", "target-subnet", &[TARGET_SUBNET_B], 2),
        ("every manually added route", "origin", &["add-route"], 6),
    ];
    for (case, name, values, expected) in cases {
        let (status, body) = describe(
            &router,
            &describe_params(&endpoint, &filters(&[(name, values)])),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        let reported = elements(&body, "destinationCidr");
        assert_eq!(
            reported.len(),
            expected,
            "case {case:?}: expected {expected} route(s), got {reported:?} from {body}"
        );
    }

    // A filter the operation does not document, and one whose value selects nothing.
    let (status, body) = describe(
        &router,
        &describe_params(&endpoint, &filters(&[("route-type", &["Nat"])])),
    )
    .await;
    assert_eq!(
        status, 400,
        "an undocumented filter: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidFilter",
        "an undocumented filter: expected the filter code, got {body}"
    );

    let (status, body) = describe(
        &router,
        &describe_params(&endpoint, &filters(&[("origin", &["associate"])])),
    )
    .await;
    assert_eq!(
        status, 200,
        "a filter that selects nothing: expected HTTP 200, got {status} with body {body}"
    );
    assert!(
        elements(&body, "destinationCidr").is_empty(),
        "a filter that selects nothing reports no route, got {body}"
    );

    assert_eq!(
        stored_routes(&engine),
        rows.len(),
        "describing writes nothing, got {held} stored routes",
        held = stored_routes(&engine)
    );
}

/// The report pages in destination-then-target order, the token resumes where the page
/// stopped, and a token minted for one endpoint's report is refused for another's.
#[tokio::test]
async fn the_report_pages_in_route_order_and_scopes_its_token_to_the_endpoint() {
    let (router, engine) = super::ec2_query::emulator();
    let endpoint = seed_endpoint(&engine, "the endpoint under test");
    let other_endpoint = seed_endpoint(&engine, "another endpoint of the same account");
    seed_target_networks(&engine, &endpoint, &[TARGET_SUBNET_A, TARGET_SUBNET_B]);
    seed_target_networks(&engine, &other_endpoint, &[TARGET_SUBNET_A]);
    let rows = add_rows();
    add_every_row(&router, &endpoint, &rows).await;

    // The model publishes `range 5..1000` for the page size, so 5 is the smallest page a
    // request may ask for; six routes are one more than that.
    let page_size = vec![("MaxResults".to_string(), "5".to_string())];
    let (status, body) = describe(&router, &describe_params(&endpoint, &page_size)).await;
    assert_eq!(
        status, 200,
        "the first page: expected HTTP 200, got {status} with body {body}"
    );
    let first = elements(&body, "destinationCidr");
    assert_eq!(
        first.len(),
        5,
        "the first page: expected five routes, got {first:?} from {body}"
    );
    let token = element(&body, "nextToken")
        .unwrap_or_else(|| panic!("the first page must carry a token, got {body}"));

    let mut resumed = page_size.clone();
    resumed.push(("NextToken".to_string(), token.clone()));
    let (status, body) = describe(&router, &describe_params(&endpoint, &resumed)).await;
    assert_eq!(
        status, 200,
        "the second page: expected HTTP 200, got {status} with body {body}"
    );
    let second = elements(&body, "destinationCidr");
    assert_eq!(
        second.len(),
        1,
        "the second page: expected the one remaining route, got {second:?} from {body}"
    );
    assert!(
        element(&body, "nextToken").is_none(),
        "the last page carries no token, got {body}"
    );
    let walked: Vec<String> = first.into_iter().chain(second).collect();
    let expected: Vec<String> = rows_in_report_order()
        .iter()
        .map(|row| row.destination.to_string())
        .collect();
    assert_eq!(
        walked, expected,
        "the walk reports every route once, in destination-then-target order"
    );

    // A token names a position inside ONE endpoint's report.
    let mut foreign = page_size;
    foreign.push(("NextToken".to_string(), token));
    let (status, body) = describe(&router, &describe_params(&other_endpoint, &foreign)).await;
    assert_eq!(
        status, 400,
        "another endpoint's token: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidPaginationToken",
        "another endpoint's token: expected the pagination-token code, got {body}"
    );

    // And the report refuses an endpoint it cannot resolve rather than reporting an empty set.
    let (status, body) = describe(&router, &endpoint_param("cvpn-endpoint-0123456789abcdef0")).await;
    assert_eq!(
        status, 400,
        "an unknown endpoint: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidClientVpnEndpointId.NotFound",
        "an unknown endpoint: expected the not-found code, got {body}"
    );
}

// ---------------------------------------------------------------------------
// The endpoint id, whatever form the value takes
// ---------------------------------------------------------------------------

/// A `ClientVpnEndpointId` this account holds no endpoint for is refused HTTP 400
/// `InvalidClientVpnEndpointId.NotFound`, worded `Endpoint <id> does not exist`, WHATEVER FORM
/// the value takes: the member carries no pattern
/// (`smithy_query(query="shape",shape="ClientVpnEndpointId")` is a plain `String`) and the
/// Client VPN surface publishes no malformed counterpart to tell "not an id of this kind" from
/// "no such endpoint" with, so one lookup answers every value it cannot resolve. An endpoint
/// the account DOES hold and that carries no route reports an empty set instead — the answer
/// that must never stand in for an id that resolves to nothing — and a dry run reports the
/// permission outcome without resolving the id at all.
#[tokio::test]
async fn an_endpoint_id_that_cannot_be_resolved_is_refused_as_not_found() {
    let (router, engine) = super::ec2_query::emulator();
    let endpoint = seed_endpoint(&engine, "the endpoint under test");
    seed_target_networks(&engine, &endpoint, &[TARGET_SUBNET_A]);
    let rows = add_rows();
    add_every_row(&router, &endpoint, &rows[..1]).await;

    // Rows along what the rule generalises over — the FORM of the value: both id lengths EC2
    // issues (8 and 17 hex digits), the all-zero placeholder AWS's own documentation uses, a
    // bare prefix, a prefix with an empty suffix, a suffix one digit short and one digit long,
    // a non-hex suffix, an uppercase suffix, another resource's id, a target network
    // association's id, a value carrying no resource prefix at all, and an empty value. None
    // of them names an endpoint of this account, so each is refused the same way.
    let cases: [&str; 13] = [
        "cvpn-endpoint-0123abcd",
        "cvpn-endpoint-0123456789abcdef0",
        "cvpn-endpoint-00000000000000000",
        "cvpn-endpoint",
        "cvpn-endpoint-",
        "cvpn-endpoint-0123456789abcdef",
        "cvpn-endpoint-0123456789abcdef01",
        "cvpn-endpoint-0123456789abcdefg",
        "cvpn-endpoint-0123456789ABCDEF0",
        TARGET_VPC,
        "cvpn-assoc-00000000000000001",
        "not-an-id-at-all",
        "",
    ];
    for endpoint_id in cases {
        let (status, body) = describe(&router, &endpoint_param(endpoint_id)).await;
        assert_eq!(
            status, 400,
            "case {endpoint_id:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidClientVpnEndpointId.NotFound",
            "case {endpoint_id:?}: expected the not-found code, got {body}"
        );
        assert_eq!(
            error_message(&body),
            format!("Endpoint {endpoint_id} does not exist"),
            "case {endpoint_id:?}: expected the refusal to name the id the request sent, got \
             {body}"
        );

        // The same unresolvable id under `DryRun`: the permission outcome, reported without
        // resolving anything.
        let mut dry = endpoint_param(endpoint_id);
        dry.push(("DryRun".to_string(), "true".to_string()));
        let (status, body) = describe(&router, &dry).await;
        assert_eq!(
            status, 412,
            "case {endpoint_id:?} as a dry run: expected HTTP 412, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "DryRunOperation",
            "case {endpoint_id:?} as a dry run: expected the dry-run code, got {body}"
        );
    }

    // An id longer than the message will repeat back: the refusal names its first
    // `MAX_ECHOED_CLIENT_VPN_ENDPOINT_ID_CHARS` characters and no more, so an arbitrarily long
    // value cannot be reflected in full.
    let overlong = format!("cvpn-endpoint-{}", "0".repeat(4_096));
    let (status, body) = describe(&router, &endpoint_param(&overlong)).await;
    let named: String = overlong
        .chars()
        .take(MAX_ECHOED_CLIENT_VPN_ENDPOINT_ID_CHARS)
        .collect();
    assert_eq!(
        status, 400,
        "an over-long endpoint id: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_message(&body),
        format!("Endpoint {named} does not exist"),
        "an over-long endpoint id: expected the echo bounded to {MAX_ECHOED_CLIENT_VPN_ENDPOINT_ID_CHARS} characters, got {body}"
    );

    // An endpoint that exists and holds no route of its own: an empty set, not a refusal.
    let empty_endpoint = seed_endpoint(&engine, "an endpoint carrying no route");
    let (status, body) = describe(&router, &endpoint_param(&empty_endpoint)).await;
    assert_eq!(
        status, 200,
        "an endpoint with no route: expected HTTP 200, got {status} with body {body}"
    );
    assert!(
        elements(&body, "destinationCidr").is_empty(),
        "an endpoint with no route: expected an empty set, got {body}"
    );

    // And the endpoint under test still reports the route it holds, so the refusals above are
    // about the id rather than about the operation.
    let (status, body) = describe(&router, &endpoint_param(&endpoint)).await;
    assert_eq!(
        status, 200,
        "the stored endpoint: expected HTTP 200, got {status} with body {body}"
    );
    assert_eq!(
        elements(&body, "destinationCidr"),
        vec![rows[0].destination.to_string()],
        "the stored endpoint: expected its one route, got {body}"
    );
    assert_eq!(
        stored_routes(&engine),
        1,
        "no refusal and no dry run writes a route, got {held} stored routes",
        held = stored_routes(&engine)
    );
}
