// SPDX-License-Identifier: CC0-1.0

//! The state a GATEWAY VPC endpoint is created in, driven over `ec2Query` (README.md /
//! OPERATIONS.md "Wire protocol").
//!
//! The rule under test: a VPC endpoint is answered in a non-terminal state only while
//! something it stood up is still being provisioned. A gateway endpoint stands up nothing —
//! it creates no endpoint network interface in any subnet and takes no address — so it is
//! `available` from the moment it exists, whether or not any route table was associated with
//! it: availability is not reachability, and the tables that reach it are what
//! `ModifyVpcEndpoint` changes for the whole life of the endpoint. An endpoint that DOES
//! stand up interfaces (`Interface`, `GatewayLoadBalancer`) is answered `pending` and is
//! carried to `available` by the clock, which is the sibling suite `vpc_endpoint_state.rs`.
//!
//! The rows differ along what the rule generalises over: which gateway endpoint service of
//! the Region, how many of the VPC's route tables the create associates (none, one, several),
//! and whether the create named the type or left it to the documented default. None of those
//! is what decides the state, and no row owes a later move: the state is still `available`
//! after the span a pending endpoint's move would have fallen due in.

use std::time::Duration;

use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, elements, emulator, indexed};
use super::wire::{param, Param};

/// The Region every call here is signed for, and the Region of the endpoint services below.
const REGION: &str = "us-east-1";

/// Long enough past [`ec2::state::TRANSITION_DELAY`] that a move the clock owed would
/// certainly have fallen due, with a margin for a slow machine — so a row that is still
/// `available` after it was never waiting on one.
const SETTLE_WAIT: Duration = Duration::from_millis(900);

/// One `action` call that has to succeed, and the body it answered.
async fn ok(case: &str, router: &MultiServiceRouter, action: &str, params: &[Param]) -> String {
    let (status, body) = super::ec2_query::call(router, REGION, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {action}, got {status} with body {body}"
    );
    body
}

/// One element of a response body, or a panic naming the case and showing the body.
fn member(case: &str, body: &str, name: &str) -> String {
    element(body, name).unwrap_or_else(|| panic!("case {case:?}: no <{name}> in {body}"))
}

/// The state `DescribeVpcEndpoints` reports for the one endpoint `endpoint_id`.
async fn described_state(case: &str, router: &MultiServiceRouter, endpoint_id: &str) -> String {
    let body = ok(
        case,
        router,
        "DescribeVpcEndpoints",
        &indexed("VpcEndpointId", &[endpoint_id]),
    )
    .await;
    member(case, &body, "state")
}

/// One row: a gateway endpoint to one endpoint service, associated with some number of the
/// VPC's route tables.
struct GatewayCase {
    case: &'static str,
    /// The endpoint service the endpoint is created to — a service the Region publishes a
    /// prefix list for, which is what a gateway endpoint is created to.
    service_name: &'static str,
    /// Whether the create names `VpcEndpointType` or leaves it to the documented default.
    names_type: bool,
    /// How many of the VPC's route tables the create associates.
    route_tables: usize,
}

/// The rows: both services the Region serves gateway endpoints for, the defaulted spelling of
/// the type, and none, one and several route table associations.
fn gateway_cases() -> Vec<GatewayCase> {
    vec![
        GatewayCase {
            case: "Amazon S3, with no route table at all",
            service_name: "com.amazonaws.us-east-1.s3",
            names_type: true,
            route_tables: 0,
        },
        GatewayCase {
            case: "Amazon S3, associated with one route table",
            service_name: "com.amazonaws.us-east-1.s3",
            names_type: true,
            route_tables: 1,
        },
        GatewayCase {
            case: "Amazon DynamoDB, associated with three route tables",
            service_name: "com.amazonaws.us-east-1.dynamodb",
            names_type: true,
            route_tables: 3,
        },
        GatewayCase {
            case: "Amazon DynamoDB, with the type left to the documented default",
            service_name: "com.amazonaws.us-east-1.dynamodb",
            names_type: false,
            route_tables: 0,
        },
    ]
}

/// A gateway endpoint is `available` as soon as it exists — answered by the create, reported
/// by the describe, selected by the `vpc-endpoint-state` filter — and stays `available` past
/// the span a pending endpoint's move falls due in.
#[tokio::test]
async fn a_gateway_endpoint_is_in_service_as_soon_as_it_exists() {
    let available = ec2::state::VPC_ENDPOINT_STATE_AVAILABLE;

    for row in gateway_cases() {
        let case = row.case;
        // One VPC per row, so the tables a row associates are its own.
        let (router, _engine) = emulator();
        let vpc_id = {
            let body = ok(
                case,
                &router,
                "CreateVpc",
                &[param("CidrBlock", "10.70.0.0/16")],
            )
            .await;
            member(case, &body, "vpcId")
        };
        let mut tables: Vec<String> = Vec::new();
        for _ in 0..row.route_tables {
            let body = ok(
                case,
                &router,
                "CreateRouteTable",
                &[param("VpcId", &vpc_id)],
            )
            .await;
            tables.push(member(case, &body, "routeTableId"));
        }

        let mut params = vec![
            param("VpcId", &vpc_id),
            param("ServiceName", row.service_name),
        ];
        if row.names_type {
            params.push(param("VpcEndpointType", "Gateway"));
        }
        let named: Vec<&str> = tables.iter().map(String::as_str).collect();
        params.extend(indexed("RouteTableId", &named));

        let body = ok(case, &router, "CreateVpcEndpoint", &params).await;
        let endpoint_id = member(case, &body, "vpcEndpointId");
        let answered = member(case, &body, "state");
        assert_eq!(
            answered, available,
            "case {case:?}: expected the create to answer {available:?}, got {answered:?} in \
             {body}"
        );
        // The association set the endpoint reports is the tables the request named, so a
        // state of `available` is not a state reported for an endpoint that dropped them.
        let reported_tables = element(&body, "routeTableIdSet")
            .map(|set| elements(&set, "item"))
            .unwrap_or_default();
        assert_eq!(
            reported_tables, tables,
            "case {case:?}: expected the endpoint to report the tables the create named, got \
             {body}"
        );

        let described = described_state(case, &router, &endpoint_id).await;
        assert_eq!(
            described, available,
            "case {case:?}: expected the describe to report {available:?}, got {described:?}"
        );

        // Nothing is owed on it: the span a pending endpoint's move falls due in passes and
        // the endpoint is still in service, rather than moving to some other state.
        tokio::time::sleep(SETTLE_WAIT).await;
        let settled = described_state(case, &router, &endpoint_id).await;
        assert_eq!(
            settled, available,
            "case {case:?}: expected it to still report {available:?} once the span of a \
             pending endpoint's move had passed, got {settled:?}"
        );
    }
}
