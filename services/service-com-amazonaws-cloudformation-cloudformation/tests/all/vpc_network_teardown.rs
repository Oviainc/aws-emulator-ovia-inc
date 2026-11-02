// SPDX-License-Identifier: CC0-1.0

//! A teardown reaches a terminal status whatever ORDER the template declared the network in.
//!
//! RULE: **a stack's deletion asks each resource's owning service to take it away in an order
//! that service accepts, so the stack reaches `DELETE_COMPLETE` from the clock alone** — and
//! Amazon EC2 is what makes that order observable, because it refuses to delete a VPC while
//! anything still sits in it ("The vpc '<id>' has dependencies and cannot be deleted."): a
//! teardown that asked for the VPC before its subnet would leave the stack short of a terminal
//! status and a client's poll loop waiting for ever.
//!
//! `stack_teardown_events.rs` pins that rule, and the per-resource events behind it, over a
//! network whose template declares the VPC FIRST. The one axis left is the other order — a
//! generated template can emit the VPC after everything that sits in it — which is the order
//! that puts the VPC's own deletion first in a teardown walking the declaration backwards.

use service_com_amazonaws_ec2_amazonec2::state::{
    INVALID_SUBNET_ID_NOT_FOUND, INVALID_VPC_ID_NOT_FOUND,
};

use crate::stack_lifecycle::{
    call, deploy, ec2, member, router, wait_for_the_transition, xml_text,
};

/// The network declared VPC LAST: the subnet, its route table and the association come before
/// the VPC they all sit in.
const VPC_LAST: &str = r#"{"Resources":{
    "ZSubnet":{"Type":"AWS::EC2::Subnet","Properties":{"VpcId":{"Ref":"ZVpc"},
      "CidrBlock":"10.30.0.0/18","AvailabilityZone":"us-east-1a",
      "MapPublicIpOnLaunch":true}},
    "ZRouteTable":{"Type":"AWS::EC2::RouteTable","Properties":{"VpcId":{"Ref":"ZVpc"}}},
    "ZAssociation":{"Type":"AWS::EC2::SubnetRouteTableAssociation","Properties":{
      "SubnetId":{"Ref":"ZSubnet"},"RouteTableId":{"Ref":"ZRouteTable"}}},
    "ZVpc":{"Type":"AWS::EC2::VPC","Properties":{"CidrBlock":"10.30.0.0/16"}}},
  "Outputs":{
    "VpcId":{"Value":{"Ref":"ZVpc"}},
    "SubnetId":{"Value":{"Ref":"ZSubnet"}}}}"#;

/// The stack this suite deploys.
const STACK: &str = "vpc-declared-last-teardown";

#[tokio::test]
async fn a_network_declared_vpc_last_still_reaches_delete_complete() {
    let case = "a network whose template declares the VPC last";
    let router = router();
    deploy(&router, STACK, VPC_LAST).await;

    let (status, described) = call(&router, "DescribeStacks", &[member("StackName", STACK)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeStacks must answer 200, got {status} {described}"
    );
    let id = xml_text(&described, "StackId").unwrap_or_default();
    let published = crate::stack_lifecycle::xml_texts(&described, "OutputValue");
    let vpc_id = published
        .iter()
        .find(|value| value.starts_with("vpc-"))
        .cloned()
        .unwrap_or_default();
    let subnet_id = published
        .iter()
        .find(|value| value.starts_with("subnet-"))
        .cloned()
        .unwrap_or_default();
    assert!(
        !vpc_id.is_empty() && !subnet_id.is_empty(),
        "case {case:?}: expected the stack to publish the ids EC2 assigned, got {published:?}"
    );

    let (status, body) = call(&router, "DeleteStack", &[member("StackName", STACK)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DeleteStack must answer 200, got {status} {body}"
    );
    // ONE span is what a stack operation owes, and the status is terminal from there on: a
    // client's poll stops rather than waiting on a resource the owning service refused.
    wait_for_the_transition().await;
    for read in 1..=2 {
        let (status, reported) = call(&router, "DescribeStacks", &[member("StackName", &id)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeStacks({id}) must answer 200, got {status} {reported}"
        );
        assert_eq!(
            xml_text(&reported, "StackStatus").as_deref(),
            Some("DELETE_COMPLETE"),
            "case {case:?}: read {read} expected the teardown to have reached DELETE_COMPLETE, \
             got {reported}"
        );
    }

    // The teardown really asked EC2 for each resource, in an order it accepted: a subnet EC2
    // still held would have refused the VPC's own deletion.
    for (action, member_name, id, code) in [
        (
            "DescribeSubnets",
            "SubnetId.1",
            &subnet_id,
            INVALID_SUBNET_ID_NOT_FOUND,
        ),
        ("DescribeVpcs", "VpcId.1", &vpc_id, INVALID_VPC_ID_NOT_FOUND),
    ] {
        let (status, refused) = ec2(&router, action, &[(member_name, id)]).await;
        assert_eq!(
            (status, xml_text(&refused, "Code")),
            (400, Some(code.to_string())),
            "case {case:?}: expected {action} on the deleted {id} to be refused with {code}, \
             got {status} {refused}"
        );
    }
}
