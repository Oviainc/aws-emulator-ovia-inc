// SPDX-License-Identifier: CC0-1.0

//! The identifiers a stack hands DOWNSTREAM for the Amazon EC2 VPC-network resources it
//! declares, end to end over the awsQuery wire.
//!
//! RULE: a declared resource of a type whose OWNING service assigns the identifier carries
//! an identifier of that service's own published form as its `PhysicalResourceId` — Amazon
//! EC2 takes no name on any of these creates and each type's "Return values" section says
//! "Ref returns the ID of the VPC" / "the subnet ID" / "the ID of the security group" — so
//! a property of another resource that `Ref`s one is handed a value the service that owns
//! THAT resource accepts, rather than the `<stack>-<logical id>-<unique id>` name
//! CloudFormation computes for a resource it names itself.
//!
//! The downstream service here is Amazon RDS, which resolves a DB subnet group's
//! `SubnetIds` and a DB instance's `VpcSecurityGroupIds` and refuses a value that is not an
//! EC2 identifier — so this suite is what shows the two halves agree: every declared
//! network resource's id is read back off the stack and matched against its type's form,
//! and the DB subnet group and DB instance the same stack places in them are read back
//! through RDS's own wire carrying exactly those ids.
//!
//! `crate::state::template`'s identity rules are the layer under test; the wire harness is
//! the one `stack_lifecycle.rs` owns.

use smithy_server_runtime::MultiServiceRouter;

use crate::stack_lifecycle::{
    call, deploy, member, rds, router, xml_text, xml_texts,
};

/// How many hexadecimal characters an Amazon EC2 identifier carries after its prefix: the
/// current (long) id form is 17 (`subnet-0123456789abcdef0`).
const EC2_ID_HEX_DIGITS: usize = 17;

/// A stack declaring its own VPC, two subnets in it and a security group, with an Amazon
/// RDS DB subnet group over `Ref`s of those subnets and a MySQL DB instance in that group
/// behind a `Ref` of that security group.
///
/// This is the shape an IaC deployment that brings its own network takes: nothing names a
/// subnet or a security group, so every id downstream comes from a `Ref`.
const OWN_NETWORK: &str = concat!(
    "Resources:\n",
    "  Vpc:\n",
    "    Type: AWS::EC2::VPC\n",
    "    Properties:\n",
    "      CidrBlock: 10.0.0.0/16\n",
    "  Subnet0:\n",
    "    Type: AWS::EC2::Subnet\n",
    "    Properties:\n",
    "      VpcId: !Ref Vpc\n",
    "      CidrBlock: 10.0.1.0/24\n",
    "      AvailabilityZone: us-east-1a\n",
    "  Subnet1:\n",
    "    Type: AWS::EC2::Subnet\n",
    "    Properties:\n",
    "      VpcId: !Ref Vpc\n",
    "      CidrBlock: 10.0.2.0/24\n",
    "      AvailabilityZone: us-east-1b\n",
    "  DbSecurityGroup:\n",
    "    Type: AWS::EC2::SecurityGroup\n",
    "    Properties:\n",
    "      GroupDescription: the database's own security group\n",
    "      VpcId: !Ref Vpc\n",
    "  DbSubnetGroup:\n",
    "    Type: AWS::RDS::DBSubnetGroup\n",
    "    Properties:\n",
    "      DBSubnetGroupName: cfn-own-network-subnets\n",
    "      DBSubnetGroupDescription: the stack's own subnets\n",
    "      SubnetIds:\n",
    "        - !Ref Subnet0\n",
    "        - !Ref Subnet1\n",
    "  Database:\n",
    "    Type: AWS::RDS::DBInstance\n",
    "    Properties:\n",
    "      DBInstanceIdentifier: cfn-own-network-db\n",
    "      Engine: mysql\n",
    "      DBInstanceClass: db.t3.micro\n",
    "      DBSubnetGroupName: !Ref DbSubnetGroup\n",
    "      VPCSecurityGroups:\n",
    "        - !Ref DbSecurityGroup\n",
);

/// The `PhysicalResourceId` the stack reports for one of its resources.
async fn physical_id(router: &MultiServiceRouter, stack: &str, logical_id: &str) -> String {
    let (status, body) = call(
        router,
        "DescribeStackResources",
        &[
            member("StackName", stack),
            member("LogicalResourceId", logical_id),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "DescribeStackResources({stack}, {logical_id}) must answer 200, got {status} {body}"
    );
    xml_text(&body, "PhysicalResourceId").unwrap_or_default()
}

/// Whether `value` is an Amazon EC2 identifier of the kind `prefix` names: `<prefix>-`
/// followed by [`EC2_ID_HEX_DIGITS`] lowercase hexadecimal characters.
fn is_ec2_id(value: &str, prefix: &str) -> bool {
    let Some(digits) = value.strip_prefix(&format!("{prefix}-")) else {
        return false;
    };
    digits.chars().count() == EC2_ID_HEX_DIGITS
        && digits
            .chars()
            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
}

/// Every declared EC2 network resource carries an id of its own type's published form, the
/// `Ref` of each one reaches the owning service as that id, and the DB subnet group and DB
/// instance the same stack places in them are really created in Amazon RDS over those ids.
#[tokio::test]
async fn a_declared_ec2_network_resource_hands_downstream_its_own_services_id() {
    let stack = "cfn-own-network";
    let router = router();
    deploy(&router, stack, OWN_NETWORK).await;

    // The class the rule generalises over: every EC2 network type this stack declares,
    // each with the prefix its own "Return values" section publishes.
    let mut ids = Vec::new();
    for (logical_id, prefix) in [
        ("Vpc", "vpc"),
        ("Subnet0", "subnet"),
        ("Subnet1", "subnet"),
        ("DbSecurityGroup", "sg"),
    ] {
        let case = format!("{logical_id} ({prefix})");
        let id = physical_id(&router, stack, logical_id).await;
        assert!(
            is_ec2_id(&id, prefix),
            "case {case:?}: expected {prefix}- followed by {EC2_ID_HEX_DIGITS} hexadecimal \
             characters, got {id:?}"
        );
        ids.push((logical_id, id));
    }
    // Two resources of one type are two different resources, so their ids differ.
    let subnet0 = ids[1].1.clone();
    let subnet1 = ids[2].1.clone();
    assert_ne!(
        subnet0, subnet1,
        "two declared subnets must carry different subnet ids, got {subnet0:?} twice"
    );
    let security_group = ids[3].1.clone();

    // Amazon RDS resolved those subnet ids: the group it holds reports exactly the ids the
    // stack minted, so the value the `Ref` substituted is one RDS accepted.
    let (status, body) = rds(
        &router,
        "DescribeDBSubnetGroups",
        &[("DBSubnetGroupName", "cfn-own-network-subnets")],
    )
    .await;
    assert_eq!(
        status, 200,
        "DescribeDBSubnetGroups must answer 200 for the group the stack created, got {status} \
         {body}"
    );
    let mut reported = xml_texts(&body, "SubnetIdentifier");
    reported.sort();
    let mut expected = vec![subnet0.clone(), subnet1.clone()];
    expected.sort();
    assert_eq!(
        reported, expected,
        "expected the group to hold the subnet ids the stack minted, got {body}"
    );

    // ...and the DB instance is in that group, behind the security group id the same stack
    // minted.
    let (status, body) = rds(
        &router,
        "DescribeDBInstances",
        &[("DBInstanceIdentifier", "cfn-own-network-db")],
    )
    .await;
    assert_eq!(
        status, 200,
        "DescribeDBInstances must answer 200 for the instance the stack created, got {status} \
         {body}"
    );
    assert_eq!(
        (
            xml_text(&body, "DBSubnetGroupName"),
            xml_text(&body, "VpcSecurityGroupId"),
        ),
        (
            Some("cfn-own-network-subnets".to_string()),
            Some(security_group.clone()),
        ),
        "expected the instance to report the group and the security group the stack minted, \
         got {body}"
    );
}
