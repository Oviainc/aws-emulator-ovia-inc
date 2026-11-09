// SPDX-License-Identifier: CC0-1.0

//! The MESSAGE Classic Elastic Load Balancing states when a request names a subnet or a
//! security group the account holds no such resource for.
//!
//! RULE: both refusals state the EC2 fault the request ran into RELAYED WHOLE — the
//! sentence EC2 worded about the ids the request named, followed by the context of the call
//! that raised it: which service answered, the status and the error code it answered with,
//! and that call's own request id. Neither half alone is the message: the sentence by itself
//! is EC2's refusal passed off as this service's own, and the context by itself names
//! nothing the caller sent.
//!
//! The rows below drive the ONE wording both operations share
//! (`state::relayed_network_refusal_message`) over the faults an EC2 read can come back
//! with — each kind of resource, one id and several, a fault EC2 worded a sentence for and
//! one it answered only a code for, the status it came with and an answer that carried
//! none — because the rule is about relaying the fault, not about one recorded request. The
//! end-to-end refusals the two operations answer with it, against a real EC2 store, are
//! pinned in `core/router/tests/all/elb_classic_network_resources_resolve_in_ec2.rs`: a
//! subnet and a security group are EC2's resources, and this crate mounts no EC2 service.
//!
//! The last test is the other side of that: with no EC2 to ask, nothing is refused on
//! silence, so neither operation answers a relayed fault for a read that never happened.

use super::wire::{call, emulator, is_request_id, param};
use service_com_amazonaws_elasticloadbalancing_elasticloadbalancing_v7::state;

/// The region every request below is signed for.
const REGION: &str = "us-east-1";

/// One row: the case, the EC2 fault, and the sentence and code the relayed message must
/// state it with.
struct Relayed {
    case: &'static str,
    refusal: state::NetworkRefusal,
    sentence: &'static str,
    code: &'static str,
    status: u16,
}

/// Every EC2 fault is relayed the same way: its own sentence, then the context of the call
/// that raised it, with that call's request id in it.
#[test]
fn an_ec2_fault_is_relayed_whole() {
    let rows = vec![
        Relayed {
            case: "a subnet id EC2 holds no subnet for",
            refusal: state::NetworkRefusal::answered(
                "InvalidSubnetID.NotFound",
                Some("The subnet ID 'subnet-0f1e2d3c4b5a69780' does not exist".to_string()),
                Some(400),
            ),
            sentence: "The subnet ID 'subnet-0f1e2d3c4b5a69780' does not exist",
            code: "InvalidSubnetID.NotFound",
            status: 400,
        },
        Relayed {
            case: "several subnet ids in one read",
            refusal: state::NetworkRefusal::answered(
                "InvalidSubnetID.NotFound",
                Some(
                    "The subnet IDs 'subnet-1a2b3c4d', 'subnet-5e6f7a8b' do not exist"
                        .to_string(),
                ),
                Some(400),
            ),
            sentence: "The subnet IDs 'subnet-1a2b3c4d', 'subnet-5e6f7a8b' do not exist",
            code: "InvalidSubnetID.NotFound",
            status: 400,
        },
        Relayed {
            case: "a value that is no subnet id at all",
            refusal: state::NetworkRefusal::answered(
                "InvalidSubnetID.Malformed",
                Some("Invalid id: 'not-a-subnet'".to_string()),
                Some(400),
            ),
            sentence: "Invalid id: 'not-a-subnet'",
            code: "InvalidSubnetID.Malformed",
            status: 400,
        },
        Relayed {
            case: "a security group id EC2 holds no group for",
            refusal: state::NetworkRefusal::answered(
                "InvalidGroup.NotFound",
                Some("The security group 'sg-0f1e2d3c4b5a69780' does not exist".to_string()),
                Some(400),
            ),
            sentence: "The security group 'sg-0f1e2d3c4b5a69780' does not exist",
            code: "InvalidGroup.NotFound",
            status: 400,
        },
        Relayed {
            case: "a value that is no security group id at all",
            refusal: state::NetworkRefusal::answered(
                "InvalidGroupId.Malformed",
                Some("Invalid id: 'not-a-group'".to_string()),
                Some(400),
            ),
            sentence: "Invalid id: 'not-a-group'",
            code: "InvalidGroupId.Malformed",
            status: 400,
        },
        Relayed {
            // A fault EC2 answered no sentence of its own for still states what it can: the
            // code it answered. A client refusal is the status the clause then states.
            case: "a fault EC2 worded no sentence for",
            refusal: state::NetworkRefusal::new("InvalidGroup.NotFound"),
            sentence: "InvalidGroup.NotFound",
            code: "InvalidGroup.NotFound",
            status: 400,
        },
        Relayed {
            case: "a fault that reached this service with a status of its own",
            refusal: state::NetworkRefusal::answered(
                "InvalidSubnetID.NotFound",
                Some("The subnet ID 'subnet-1a2b3c4d' does not exist".to_string()),
                Some(404),
            ),
            sentence: "The subnet ID 'subnet-1a2b3c4d' does not exist",
            code: "InvalidSubnetID.NotFound",
            status: 404,
        },
    ];

    let mut seen: Vec<String> = Vec::new();
    for row in &rows {
        let stated = state::relayed_network_refusal_message(&row.refusal);
        let head = format!(
            "{} (Service: AmazonEC2; Status Code: {}; Error Code: {}; Request ID: ",
            row.sentence, row.status, row.code
        );
        let Some(tail) = stated.strip_prefix(&head) else {
            panic!(
                "case {:?}: expected the fault relayed as {head:?}<request id>; Proxy: null), \
                 got {stated:?}",
                row.case
            );
        };
        let Some(request_id) = tail.strip_suffix("; Proxy: null)") else {
            panic!(
                "case {:?}: expected the relayed clause to close with the proxy, got \
                 {stated:?}",
                row.case
            );
        };
        assert!(
            is_request_id(request_id),
            "case {:?}: expected an 8-4-4-4-12 request id, got {request_id:?}",
            row.case
        );
        seen.push(request_id.to_string());
    }

    // One id per call: the clause names the attempt that was refused, so no two of them
    // carry the same id, and nothing about it comes from the fault.
    let mut sorted = seen.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(
        sorted.len(),
        seen.len(),
        "each relayed clause must name its own EC2 call, got {seen:?}"
    );
    let repeated = state::relayed_network_refusal_message(&rows[0].refusal);
    assert_ne!(
        repeated,
        state::relayed_network_refusal_message(&rows[0].refusal),
        "relaying the same fault twice must name two calls, got {repeated:?} twice"
    );
}

/// With no EC2 to ask, this service learned nothing to refuse a request on: both operations
/// accept the placement and the association they are given, and neither answers a relayed
/// fault for a read that never happened.
#[tokio::test]
async fn nothing_is_relayed_where_there_was_no_ec2_to_ask() {
    let router = emulator();
    let created = call(
        &router,
        REGION,
        "CreateLoadBalancer",
        &[
            param("LoadBalancerName", "relayed-fault-elb"),
            param("Listeners.member.1.Protocol", "HTTP"),
            param("Listeners.member.1.LoadBalancerPort", "80"),
            param("Listeners.member.1.InstancePort", "80"),
            param("Subnets.member.1", "subnet-0f1e2d3c4b5a69780"),
        ],
    )
    .await;
    assert_eq!(
        created.status, 200,
        "the fixture must be created, got {} with {}",
        created.status, created.body
    );

    for (case, action, params) in [
        (
            "an attach where no EC2 can be asked about the subnet",
            "AttachLoadBalancerToSubnets",
            vec![
                param("LoadBalancerName", "relayed-fault-elb"),
                param("Subnets.member.1", "subnet-1a2b3c4d"),
            ],
        ),
        (
            "an apply where no EC2 can be asked about the group",
            "ApplySecurityGroupsToLoadBalancer",
            vec![
                param("LoadBalancerName", "relayed-fault-elb"),
                param("SecurityGroups.member.1", "sg-1a2b3c4d"),
            ],
        ),
    ] {
        let answered = call(&router, REGION, action, &params).await;
        assert_eq!(
            answered.status, 200,
            "case {case:?}: nothing is refused on silence, got {} with {}",
            answered.status, answered.body
        );
        assert!(
            !answered.body.contains("Service: AmazonEC2"),
            "case {case:?}: no EC2 fault can be relayed for a read that never happened, got {}",
            answered.body
        );
    }
}
