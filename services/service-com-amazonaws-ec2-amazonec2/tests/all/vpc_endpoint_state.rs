// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the STATE a VPC endpoint reports, driven over `ec2Query` (README.md
//! / OPERATIONS.md "Wire protocol").
//!
//! The rule under test: the state of a VPC endpoint is reported in the vocabulary the model
//! publishes for the filter that selects on that very state — "`vpc-endpoint-state` - The
//! state of the endpoint (`pendingAcceptance` | `pending` | `available` | `deleting` |
//! `deleted` | `rejected` | `failed`)" (`DescribeVpcEndpointsRequest$Filters`) — so an
//! endpoint a create answers is `pending` and the one the clock has carried past its span is
//! `available`, under exactly those spellings, and the `vpc-endpoint-state` filter selects an
//! endpoint by the value it reports. A client that waits for an endpoint to be in service
//! compares against that value, so the spelling is the contract.
//!
//! Each table row is a distinct input along what that rule generalises over: which endpoint
//! type (each one this service creates), which endpoint service the endpoint is to —
//! including the dedicated regional PrivateLink service a syslog ingestion endpoint is
//! created to — and how many subnets and security groups the endpoint was given. The state
//! an endpoint reports is a fact about the endpoint, not about any of those, so every row
//! reports the same two values.

use std::time::Duration;

use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, elements, filters};
use super::wire::{param, Param};

/// The Region every call here is signed for, and the Region of the endpoint services below.
const REGION: &str = "us-east-1";

/// Long enough past [`ec2::state::TRANSITION_DELAY`] that the move the clock owed a created
/// endpoint is certainly due, with a margin for a slow machine.
const SETTLE_WAIT: Duration = Duration::from_millis(900);

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
        &[param("VpcEndpointId.1", endpoint_id)],
    )
    .await;
    member(case, &body, "state")
}

/// The endpoint ids `DescribeVpcEndpoints` reports for the `vpc-endpoint-state` filter
/// `value`.
async fn ids_in_state(case: &str, router: &MultiServiceRouter, value: &str) -> Vec<String> {
    let body = ok(
        case,
        router,
        "DescribeVpcEndpoints",
        &filters(&[("vpc-endpoint-state", &[value])]),
    )
    .await;
    elements(&body, "vpcEndpointId")
}

/// One row of the table: an endpoint of one type, to one endpoint service.
struct EndpointCase {
    case: &'static str,
    /// The `VpcEndpointType`, or `None` to leave it to the documented default (`Gateway`).
    endpoint_type: Option<&'static str>,
    /// The endpoint service the endpoint is created to.
    service_name: &'static str,
    /// How many of the network's subnets the create names.
    subnets: usize,
    /// Whether the create names the VPC's own security group.
    with_group: bool,
}

/// The rows: every endpoint type this service creates, and the service names a client uses —
/// a Region's own service, the dedicated regional PrivateLink service a CloudWatch Logs
/// syslog ingestion endpoint is created to, and a partner service name.
fn endpoint_cases() -> Vec<EndpointCase> {
    vec![
        EndpointCase {
            case: "an interface endpoint to the Region's syslog ingestion service",
            endpoint_type: Some("Interface"),
            service_name: "com.amazonaws.us-east-1.syslog-logs",
            subnets: 1,
            with_group: true,
        },
        EndpointCase {
            case: "an interface endpoint in two subnets, with no group of its own",
            endpoint_type: Some("Interface"),
            service_name: "com.amazonaws.us-east-1.logs",
            subnets: 2,
            with_group: false,
        },
        EndpointCase {
            case: "a gateway endpoint, the documented default type",
            endpoint_type: None,
            service_name: "com.amazonaws.us-east-1.s3",
            subnets: 0,
            with_group: false,
        },
        EndpointCase {
            case: "a Gateway Load Balancer endpoint to a partner service",
            endpoint_type: Some("GatewayLoadBalancer"),
            service_name: "com.amazonaws.vpce.us-east-1.vpce-svc-0123456789abcdef0",
            subnets: 1,
            with_group: false,
        },
    ]
}

/// A created endpoint is `pending`, the clock carries it to `available`, and both values are
/// the ones the filter's published list spells — for every row, and read back through the
/// describe and through the filter.
#[tokio::test]
async fn an_endpoint_reports_its_state_in_the_published_vocabulary() {
    let router = super::ec2_query::emulator().0;

    // The network every endpoint below is created in: one VPC, one subnet per Availability
    // Zone, and the VPC's own security group.
    let vpc_id = {
        let body = ok(
            "the network",
            &router,
            "CreateVpc",
            &[param("CidrBlock", "10.80.0.0/16")],
        )
        .await;
        member("the network", &body, "vpcId")
    };
    let mut subnets: Vec<String> = Vec::new();
    for (index, zone) in ["us-east-1a", "us-east-1b"].iter().enumerate() {
        let body = ok(
            "the network",
            &router,
            "CreateSubnet",
            &[
                param("VpcId", &vpc_id),
                param("CidrBlock", &format!("10.80.{}.0/24", index + 1)),
                param("AvailabilityZone", zone),
            ],
        )
        .await;
        subnets.push(member("the network", &body, "subnetId"));
    }
    let group_id = {
        let body = ok(
            "the network",
            &router,
            "CreateSecurityGroup",
            &[
                param("VpcId", &vpc_id),
                param("GroupName", "syslog-endpoint"),
                param("GroupDescription", "the syslog ingestion endpoint"),
            ],
        )
        .await;
        member("the network", &body, "groupId")
    };

    let mut created: Vec<(&str, String)> = Vec::new();
    for row in endpoint_cases() {
        let case = row.case;
        let mut params = vec![
            param("VpcId", &vpc_id),
            param("ServiceName", row.service_name),
        ];
        if let Some(endpoint_type) = row.endpoint_type {
            params.push(param("VpcEndpointType", endpoint_type));
        }
        for (ordinal, subnet_id) in subnets.iter().take(row.subnets).enumerate() {
            params.push(param(&format!("SubnetId.{}", ordinal + 1), subnet_id));
        }
        if row.with_group {
            params.push(param("SecurityGroupId.1", &group_id));
        }

        let body = ok(case, &router, "CreateVpcEndpoint", &params).await;
        let endpoint_id = member(case, &body, "vpcEndpointId");
        // The create ANSWERS the stored record, so the state it reports is the stored one.
        assert_eq!(
            member(case, &body, "state"),
            ec2::state::VPC_ENDPOINT_STATE_PENDING,
            "case {case:?}: a created endpoint is in the non-terminal pending state, got {body}"
        );
        assert_eq!(
            described_state(case, &router, &endpoint_id).await,
            ec2::state::VPC_ENDPOINT_STATE_PENDING,
            "case {case:?}: the describe reports the state the create answered"
        );
        created.push((case, endpoint_id));
    }

    // Every value reported here is one the published list spells — the catalog is
    // `VPC_ENDPOINT_STATE_VALUES`, read off `DescribeVpcEndpointsRequest$Filters`.
    for state in [
        ec2::state::VPC_ENDPOINT_STATE_PENDING,
        ec2::state::VPC_ENDPOINT_STATE_AVAILABLE,
    ] {
        assert!(
            ec2::state::VPC_ENDPOINT_STATE_VALUES.contains(&state),
            "case the vocabulary: {state:?} is not one of {:?}",
            ec2::state::VPC_ENDPOINT_STATE_VALUES
        );
    }

    // The filter selects an endpoint by the value it reports, and nothing is in service yet.
    let pending = ids_in_state("the pending filter", &router, "pending").await;
    assert_eq!(
        pending.len(),
        created.len(),
        "case the pending filter: every created endpoint is pending, got {pending:?}"
    );
    assert!(
        ids_in_state("the available filter", &router, "available")
            .await
            .is_empty(),
        "case the available filter: no endpoint is in service before its span has passed"
    );

    // The move the clock owed each of them falls due on its own, with nothing called in
    // between to make it happen.
    tokio::time::sleep(SETTLE_WAIT).await;
    for (case, endpoint_id) in &created {
        assert_eq!(
            described_state(case, &router, endpoint_id).await,
            ec2::state::VPC_ENDPOINT_STATE_AVAILABLE,
            "case {case:?}: the clock owed {endpoint_id} its available state"
        );
    }
    let available = ids_in_state("the available filter", &router, "available").await;
    assert_eq!(
        available.len(),
        created.len(),
        "case the available filter: every endpoint is in service now, got {available:?}"
    );
    assert!(
        ids_in_state("the pending filter", &router, "pending")
            .await
            .is_empty(),
        "case the pending filter: no endpoint is pending once the clock has moved them"
    );

    // "Filter values are case-sensitive" (`Filter$Values`): the capitalized member name of
    // the shared `State` enum is not the value this attribute reports, so it selects nothing.
    assert!(
        ids_in_state("a capitalized filter value", &router, "Available")
            .await
            .is_empty(),
        "case a capitalized filter value: the reported state is the published lower-case one"
    );
}
