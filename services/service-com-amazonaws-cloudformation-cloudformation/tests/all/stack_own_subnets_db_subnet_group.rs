// SPDX-License-Identifier: CC0-1.0

//! A DB subnet group a stack declares over subnets the SAME stack declares, end to end.
//!
//! RULE: a resource of an Amazon EC2 VPC-network type is addressed by the identifier EC2
//! assigns it — "Ref returns the subnet ID", "Ref returns the ID of the VPC" — of that
//! service's published form (`<prefix>-` plus hexadecimal characters), never by a name
//! CloudFormation composed, so a property of another resource that `Ref`s one is handed a
//! value the owning service's own API takes. `AWS::RDS::DBSubnetGroup$SubnetIds` is such a
//! property: Amazon RDS resolves each entry as an EC2 subnet id and refuses anything else
//! (`InvalidSubnet`), so a stack that creates its own subnets and places a DB subnet group
//! in them completes, and the group RDS holds afterwards names exactly those ids.
//!
//! The rows differ along what the rule generalises over: the template spelling (YAML with
//! short-form intrinsics and JSON), how many subnets the group covers, and whether a DB
//! instance is placed in the group as well — the downstream resource that can only be
//! created if the group resolved.
//!
//! Both services are driven through one router over their own wires, so what is asserted is
//! what a client that deploys the template and then calls `DescribeDBSubnetGroups`
//! observes.

use service_com_amazonaws_cloudformation_cloudformation::state::status;
use smithy_server_runtime::MultiServiceRouter;

use crate::stack_lifecycle::{
    call, create_stack, describe_stack, member, rds, router, wait_for_the_transition, xml_text,
    xml_texts,
};

/// One deployment of a DB subnet group over the stack's own subnets.
struct SubnetGroupCase {
    case: &'static str,
    stack: &'static str,
    template: &'static str,
    /// The logical ids of the `AWS::EC2::Subnet` resources the group covers, in the order
    /// the group names them.
    subnets: &'static [&'static str],
    /// The logical id of the `AWS::RDS::DBSubnetGroup`.
    group: &'static str,
    /// The identifier of a DB instance the stack places in the group, if it declares one.
    instance: Option<&'static str>,
}

const SUBNET_GROUP_CASES: &[SubnetGroupCase] = &[
    SubnetGroupCase {
        case: "two subnets and a DB instance in the group, in YAML",
        stack: "stack-own-subnets-yaml",
        template: concat!(
            "Resources:\n",
            "  Vpc:\n",
            "    Type: AWS::EC2::VPC\n",
            "    Properties:\n",
            "      CidrBlock: 10.0.0.0/16\n",
            "  SubA:\n",
            "    Type: AWS::EC2::Subnet\n",
            "    Properties:\n",
            "      VpcId: !Ref Vpc\n",
            "      CidrBlock: 10.0.1.0/24\n",
            "      AvailabilityZone: !Select [0, !GetAZs \"\"]\n",
            "  SubB:\n",
            "    Type: AWS::EC2::Subnet\n",
            "    Properties:\n",
            "      VpcId: !Ref Vpc\n",
            "      CidrBlock: 10.0.2.0/24\n",
            "      AvailabilityZone: !Select [1, !GetAZs \"\"]\n",
            "  SubnetGroup:\n",
            "    Type: AWS::RDS::DBSubnetGroup\n",
            "    Properties:\n",
            "      DBSubnetGroupDescription: the stack's own subnets\n",
            "      SubnetIds:\n",
            "        - !Ref SubA\n",
            "        - !Ref SubB\n",
            "  Db:\n",
            "    Type: AWS::RDS::DBInstance\n",
            "    Properties:\n",
            "      DBInstanceIdentifier: cfn-own-subnets-db\n",
            "      Engine: mysql\n",
            "      DBInstanceClass: db.t3.micro\n",
            "      DBSubnetGroupName: !Ref SubnetGroup\n",
        ),
        subnets: &["SubA", "SubB"],
        group: "SubnetGroup",
        instance: Some("cfn-own-subnets-db"),
    },
    SubnetGroupCase {
        case: "three subnets and no instance, in JSON",
        stack: "stack-own-subnets-json",
        template: r#"{"Resources":{
            "Vpc":{"Type":"AWS::EC2::VPC","Properties":{"CidrBlock":"10.1.0.0/16"}},
            "One":{"Type":"AWS::EC2::Subnet","Properties":{
              "VpcId":{"Ref":"Vpc"},"CidrBlock":"10.1.1.0/24",
              "AvailabilityZone":{"Fn::Select":[0,{"Fn::GetAZs":""}]}}},
            "Two":{"Type":"AWS::EC2::Subnet","Properties":{
              "VpcId":{"Ref":"Vpc"},"CidrBlock":"10.1.2.0/24",
              "AvailabilityZone":{"Fn::Select":[1,{"Fn::GetAZs":""}]}}},
            "Three":{"Type":"AWS::EC2::Subnet","Properties":{
              "VpcId":{"Ref":"Vpc"},"CidrBlock":"10.1.3.0/24",
              "AvailabilityZone":{"Fn::Select":[2,{"Fn::GetAZs":""}]}}},
            "Group":{"Type":"AWS::RDS::DBSubnetGroup","Properties":{
              "DBSubnetGroupDescription":"three of the stack's own subnets",
              "SubnetIds":[{"Ref":"One"},{"Ref":"Two"},{"Ref":"Three"}]}}}}"#,
        subnets: &["One", "Two", "Three"],
        group: "Group",
        instance: None,
    },
];

/// The `PhysicalResourceId` the stack reports for one of its resources.
async fn physical_id(router: &MultiServiceRouter, stack: &str, logical_id: &str) -> String {
    let (status_code, body) = call(
        router,
        "DescribeStackResources",
        &[
            member("StackName", stack),
            member("LogicalResourceId", logical_id),
        ],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "DescribeStackResources({stack}, {logical_id}) must answer 200, got {status_code} \
         {body}"
    );
    xml_text(&body, "PhysicalResourceId").unwrap_or_default()
}

/// Whether `value` is an EC2 subnet id: the `subnet-` prefix and the 8 or 17 hexadecimal
/// characters of the two published id forms (the legacy short one and the current long one),
/// which is the form `AWS::RDS::DBSubnetGroup$SubnetIds` is resolved against.
fn is_ec2_subnet_id(value: &str) -> bool {
    let Some(hex) = value.strip_prefix("subnet-") else {
        return false;
    };
    matches!(hex.chars().count(), 8 | 17) && hex.chars().all(|c| c.is_ascii_hexdigit())
}

#[tokio::test]
async fn a_db_subnet_group_over_the_stacks_own_subnets_is_created_in_rds() {
    for row in SUBNET_GROUP_CASES {
        let router = router();
        let (status_code, created) = create_stack(&router, row.stack, row.template, &[]).await;
        assert_eq!(
            status_code, 200,
            "case {:?}: CreateStack must answer 200, got {status_code} {created}",
            row.case
        );
        // Twice: the stack's own terminal status and the status Amazon RDS owes a database it
        // is bringing up are two clocks.
        wait_for_the_transition().await;
        wait_for_the_transition().await;

        let (status_code, described) = describe_stack(&router, row.stack).await;
        assert_eq!(
            status_code, 200,
            "case {:?}: DescribeStacks must answer 200, got {status_code} {described}",
            row.case
        );
        assert_eq!(
            xml_text(&described, "StackStatus").as_deref(),
            Some(status::CREATE_COMPLETE),
            "case {:?}: expected the deployment to complete, got {described}",
            row.case
        );

        // Each subnet is reported under the identifier EC2 assigns, which is what the group's
        // `SubnetIds` were resolved from.
        let mut subnet_ids = Vec::new();
        for logical_id in row.subnets {
            let reported = physical_id(&router, row.stack, logical_id).await;
            assert!(
                is_ec2_subnet_id(&reported),
                "case {:?}: expected {logical_id} to carry an EC2 subnet id, got {reported:?}",
                row.case
            );
            subnet_ids.push(reported);
        }

        // Amazon RDS holds the group over exactly those ids: the stack's view and the owning
        // service's view of the group cannot disagree. The name is folded the way RDS stores
        // it ("This value is stored as a lowercase string"), which is the spelling RDS reports
        // it under.
        let group_name = physical_id(&router, row.stack, row.group).await;
        let stored_name = group_name.to_ascii_lowercase();
        let (status_code, group) = rds(
            &router,
            "DescribeDBSubnetGroups",
            &[("DBSubnetGroupName", group_name.as_str())],
        )
        .await;
        assert_eq!(
            status_code, 200,
            "case {:?}: RDS must report the group the stack created, got {status_code} {group}",
            row.case
        );
        assert_eq!(
            xml_texts(&group, "SubnetIdentifier"),
            subnet_ids,
            "case {:?}: expected the group to cover the subnets the stack created, got {group}",
            row.case
        );

        // And the resource that could only be created if the group resolved is there too.
        if let Some(identifier) = row.instance {
            let (status_code, instance) = rds(
                &router,
                "DescribeDBInstances",
                &[("DBInstanceIdentifier", identifier)],
            )
            .await;
            assert_eq!(
                status_code, 200,
                "case {:?}: RDS must report the instance placed in the group, got \
                 {status_code} {instance}",
                row.case
            );
            assert_eq!(
                xml_text(&instance, "DBSubnetGroupName").as_deref(),
                Some(stored_name.as_str()),
                "case {:?}: expected the instance to sit in the stack's group, got {instance}",
                row.case
            );
        }
    }
}
