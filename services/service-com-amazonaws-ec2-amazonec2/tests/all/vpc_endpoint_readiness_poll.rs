// SPDX-License-Identifier: CC0-1.0

//! The readiness POLL a client makes in front of a PrivateLink-dependent workflow, driven
//! over `ec2Query` (README.md / OPERATIONS.md "Wire protocol").
//!
//! The rule under test: a VPC endpoint's state is reported in the vocabulary the model
//! publishes for the filter that selects on that very state — "`vpc-endpoint-state` - The
//! state of the endpoint (`pendingAcceptance` | `pending` | `available` | `deleting` |
//! `deleted` | `rejected` | `failed`)" (`DescribeVpcEndpointsRequest$Filters`), each value
//! the `State` enum's member name with its first letter lowercased — so a client that polls
//! `DescribeVpcEndpoints` *while the state equals `pending`* leaves that loop, and leaves it
//! at exactly `available`, with nothing but the clock moving the endpoint.
//!
//! What this suite covers that its siblings do not: the poll itself — the loop form a waiter
//! uses, which a fixed sleep does not exercise — over rows that differ along what the rule
//! generalises over: which endpoint service of Amazon Web Services, how many subnets,
//! whether a security group was named, and whether private DNS was asked for. The state is a
//! fact of the endpoint's age, not of any of those. The vocabulary every state is spelled in
//! is the sibling suite `vpc_endpoint_states.rs`.

use std::time::{Duration, Instant};

use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, elements, filters, indexed};
use super::wire::{Param, param};

/// The Region every call here is signed for, and the Region of the endpoint services below.
const REGION: &str = "us-east-1";

/// How long the poll below is given before it reports the endpoint never left `pending` —
/// generously past [`ec2::state::TRANSITION_DELAY`], so a slow machine fails nothing, while
/// a state value the loop's guard never matches still ends the test instead of hanging.
const POLL_DEADLINE: Duration = Duration::from_secs(10);

/// How long the poll waits between reads, as a client's own waiter does.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

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

// ---------------------------------------------------------------------------
// The poll a client makes
// ---------------------------------------------------------------------------

/// One row of the poll table: an interface endpoint to one endpoint service of Amazon Web
/// Services, created the way a client that is about to use it creates one.
struct EndpointCase {
    case: &'static str,
    /// The endpoint service the endpoint is created to.
    service_name: &'static str,
    /// How many of the network's subnets the create names.
    subnets: usize,
    /// Whether the create names the VPC's own security group.
    with_group: bool,
    /// What the create asks for in `PrivateDnsEnabled`, or `None` to leave it out.
    private_dns: Option<bool>,
}

/// The rows: the data plane of AWS IoT, which is reached through its own endpoint service;
/// a service reached with private DNS left out; and one asked for with private DNS enabled
/// in two subnets. None of those is what decides the state.
fn endpoint_cases() -> Vec<EndpointCase> {
    vec![
        EndpointCase {
            case: "the IoT data plane, with private DNS off",
            service_name: "com.amazonaws.us-east-1.iot.data",
            subnets: 1,
            with_group: true,
            private_dns: Some(false),
        },
        EndpointCase {
            case: "Amazon Kinesis Data Streams, with no private DNS member at all",
            service_name: "com.amazonaws.us-east-1.kinesis-streams",
            subnets: 1,
            with_group: false,
            private_dns: None,
        },
        EndpointCase {
            case: "AWS STS in two subnets, with private DNS on",
            service_name: "com.amazonaws.us-east-1.sts",
            subnets: 2,
            with_group: true,
            private_dns: Some(true),
        },
    ]
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

/// Poll `DescribeVpcEndpoints` while the endpoint reports the non-terminal state, exactly as
/// a client's readiness waiter does, and answer the state it finally reported.
///
/// The guard is the published `pending` value: a state the vocabulary does not spell leaves
/// this loop at once, which is what the assertion on the result then catches.
async fn poll_until_not_pending(
    case: &str,
    router: &MultiServiceRouter,
    endpoint_id: &str,
) -> String {
    let deadline = Instant::now() + POLL_DEADLINE;
    let mut state = described_state(case, router, endpoint_id).await;
    while state == ec2::state::VPC_ENDPOINT_STATE_PENDING && Instant::now() < deadline {
        tokio::time::sleep(POLL_INTERVAL).await;
        state = described_state(case, router, endpoint_id).await;
    }
    state
}

/// A client that creates an interface endpoint and waits for it: the create answers the
/// non-terminal state, the poll on that same value terminates, and the endpoint it then
/// holds is `available` — reported by the describe and selected by the filter, with nothing
/// called in between to make the move happen.
#[tokio::test]
async fn a_readiness_poll_on_a_created_endpoint_terminates_at_available() {
    let router = super::ec2_query::emulator().0;
    let pending = ec2::state::VPC_ENDPOINT_STATE_PENDING;
    let available = ec2::state::VPC_ENDPOINT_STATE_AVAILABLE;

    // The network every endpoint below is created in, built through the API.
    let vpc_id = {
        let body = ok(
            "the network",
            &router,
            "CreateVpc",
            &[param("CidrBlock", "10.45.0.0/16")],
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
                param("CidrBlock", &format!("10.45.{}.0/24", index + 1)),
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
                param("GroupName", "endpoint-readiness"),
                param("GroupDescription", "the group the readiness suite created"),
            ],
        )
        .await;
        member("the network", &body, "groupId")
    };
    // "To use private DNS, the VPC must have DNS hostnames and DNS resolution enabled"
    // (`CreateVpcEndpointRequest$PrivateDnsEnabled`), so the row that asks for it turns both
    // on first, the way a client does.
    for attribute in ["EnableDnsSupport", "EnableDnsHostnames"] {
        ok(
            "the network",
            &router,
            "ModifyVpcAttribute",
            &[
                param("VpcId", &vpc_id),
                param(&format!("{attribute}.Value"), "true"),
            ],
        )
        .await;
    }

    let mut created: Vec<(&str, String)> = Vec::new();
    for row in endpoint_cases() {
        let case = row.case;
        let mut params = vec![
            param("VpcId", &vpc_id),
            param("ServiceName", row.service_name),
            param("VpcEndpointType", "Interface"),
            // "The IP address type for the endpoint" (`IpAddressType`), as a client that
            // pins it to IPv4 sends it.
            param("IpAddressType", "ipv4"),
        ];
        let chosen: Vec<&str> = subnets
            .iter()
            .take(row.subnets)
            .map(String::as_str)
            .collect();
        params.extend(indexed("SubnetId", &chosen));
        if row.with_group {
            params.extend(indexed("SecurityGroupId", &[group_id.as_str()]));
        }
        if let Some(private_dns) = row.private_dns {
            params.push(param("PrivateDnsEnabled", &private_dns.to_string()));
        }

        let body = ok(case, &router, "CreateVpcEndpoint", &params).await;
        let endpoint_id = member(case, &body, "vpcEndpointId");
        let answered = member(case, &body, "state");
        assert_eq!(
            answered, pending,
            "case {case:?}: expected the create to answer {pending:?}, got {answered:?}"
        );
        // The endpoint the poll will read back is the one the request described.
        assert_eq!(
            member(case, &body, "serviceName"),
            row.service_name,
            "case {case:?}: the create must answer the service the request named, got {body}"
        );
        created.push((case, endpoint_id));
    }

    // The poll a waiter makes, on each endpoint, reads the store until the clock has carried
    // it out of the non-terminal state — and what it holds then is the terminal one.
    for (case, endpoint_id) in &created {
        let polled = poll_until_not_pending(case, &router, endpoint_id).await;
        assert_eq!(
            polled, available,
            "case {case:?}: expected the poll on {endpoint_id:?} to end at {available:?}, got \
             {polled:?}"
        );
    }

    // The same materialized value is what the filter selects on, so one endpoint is never
    // reported two ways.
    let body = ok(
        "the available filter",
        &router,
        "DescribeVpcEndpoints",
        &filters(&[("vpc-endpoint-state", &[available])]),
    )
    .await;
    let selected = elements(&body, "vpcEndpointId");
    for (case, endpoint_id) in &created {
        assert!(
            selected.contains(endpoint_id),
            "case {case:?}: expected {available:?} to select {endpoint_id:?}, got {selected:?}"
        );
    }
}
