// SPDX-License-Identifier: CC0-1.0

//! The VPC and the subnets a stack declares, read back through AMAZON EC2's own wire.
//!
//! RULE: a stack that reports `CREATE_COMPLETE` has created each declared resource in the
//! service that OWNS it, and the physical resource id it publishes for one is the identifier
//! that service assigned — so every id a `Ref`, an `Fn::GetAtt` or an output of such a stack
//! carries is one the owning service answers a read for, with the configuration the template
//! declared, and one the stack's deletion takes away again.
//!
//! Amazon EC2 is the owning service here: it takes no name on `CreateVpc` or `CreateSubnet`
//! and assigns each resource an id of its own published form, so `DescribeVpcs` and
//! `DescribeSubnets` are what say whether the resources a completed stack claims really
//! exist. A subnet id nothing was created for is refused with
//! [`INVALID_SUBNET_ID_NOT_FOUND`], which is also what a deleted one is refused with — the
//! other side of the same rule.
//!
//! The rows differ along what the rule generalises over: how many subnets the stack
//! declares, the template spelling (YAML with short-form intrinsics, JSON), the VPC block
//! size, how each subnet's Availability Zone is decided (a zone named outright, one selected
//! out of `Fn::GetAZs`, none at all), the launch-time and DNS switches the declaration asks
//! for, and whether the subnet ids leave the stack one per output or joined into one.

use service_com_amazonaws_ec2_amazonec2::state::{
    INVALID_SUBNET_ID_NOT_FOUND, INVALID_VPC_ID_NOT_FOUND,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::stack_lifecycle::{
    call, deploy, ec2, is_service_assigned_id, member, router, wait_for_the_transition, xml_text,
    xml_texts, SERVICE_ID_HEX_DIGITS,
};

/// One subnet a row declares: the logical id the template gives it, the block it asks for,
/// and the zone EC2 must place it in — `None` where the declaration names no zone at all,
/// for which "Amazon Web Services selects one for you" (`CreateSubnet$AvailabilityZone`),
/// the Region's first.
struct ExpectedSubnet {
    logical_id: &'static str,
    cidr_block: &'static str,
    availability_zone: Option<&'static str>,
}

/// One deployment of a network: the template, the VPC it declares and the subnets in it.
struct NetworkCase {
    case: &'static str,
    stack: &'static str,
    template: &'static str,
    /// The logical id of the `AWS::EC2::VPC`, and the block it declares.
    vpc: (&'static str, &'static str),
    subnets: &'static [ExpectedSubnet],
    /// The output the stack publishes the subnet ids under, and whether that output carries
    /// them joined with commas (`Fn::Join`) rather than one id per output.
    subnet_ids_output: &'static str,
}

/// The Region every call here is signed for, and whose zones `Fn::GetAZs` lists.
const REGION: &str = "us-east-1";

const NETWORK_CASES: &[NetworkCase] = &[
    NetworkCase {
        case: "three subnets across three zones selected out of Fn::GetAZs, joined into one \
               output, in YAML",
        stack: "ec2-network-three-zones",
        template: concat!(
            "Resources:\n",
            "  Vpc:\n",
            "    Type: AWS::EC2::VPC\n",
            "    Properties:\n",
            "      CidrBlock: 10.168.0.0/16\n",
            "      EnableDnsSupport: true\n",
            "      EnableDnsHostnames: true\n",
            "      Tags:\n",
            "        - Key: Name\n",
            "          Value: managed-directory\n",
            "  SubnetA:\n",
            "    Type: AWS::EC2::Subnet\n",
            "    Properties:\n",
            "      VpcId: !Ref Vpc\n",
            "      CidrBlock: 10.168.1.0/24\n",
            "      AvailabilityZone: !Select [0, !GetAZs \"\"]\n",
            "  SubnetB:\n",
            "    Type: AWS::EC2::Subnet\n",
            "    Properties:\n",
            "      VpcId: !Ref Vpc\n",
            "      CidrBlock: 10.168.2.0/24\n",
            "      AvailabilityZone: !Select [1, !GetAZs \"\"]\n",
            "      MapPublicIpOnLaunch: true\n",
            "  SubnetC:\n",
            "    Type: AWS::EC2::Subnet\n",
            "    Properties:\n",
            "      VpcId: !Ref Vpc\n",
            "      CidrBlock: 10.168.3.0/24\n",
            "      AvailabilityZone: !Select [2, !GetAZs \"\"]\n",
            "Outputs:\n",
            "  VpcId:\n",
            "    Value: !Ref Vpc\n",
            "  SubnetIds:\n",
            "    Value: !Join [\",\", [!Ref SubnetA, !Ref SubnetB, !Ref SubnetC]]\n",
        ),
        vpc: ("Vpc", "10.168.0.0/16"),
        subnets: &[
            ExpectedSubnet {
                logical_id: "SubnetA",
                cidr_block: "10.168.1.0/24",
                availability_zone: Some("us-east-1a"),
            },
            ExpectedSubnet {
                logical_id: "SubnetB",
                cidr_block: "10.168.2.0/24",
                availability_zone: Some("us-east-1b"),
            },
            ExpectedSubnet {
                logical_id: "SubnetC",
                cidr_block: "10.168.3.0/24",
                availability_zone: Some("us-east-1c"),
            },
        ],
        subnet_ids_output: "SubnetIds",
    },
    NetworkCase {
        case: "one subnet with no zone declared, in a /20 VPC, in JSON",
        stack: "ec2-network-one-subnet",
        template: r#"{"Resources":{
            "Net":{"Type":"AWS::EC2::VPC","Properties":{
              "CidrBlock":"172.31.16.0/20","InstanceTenancy":"default"}},
            "Only":{"Type":"AWS::EC2::Subnet","Properties":{
              "VpcId":{"Ref":"Net"},"CidrBlock":"172.31.16.0/24",
              "Tags":[{"Key":"Tier","Value":"private"}]}}},
          "Outputs":{
            "VpcId":{"Value":{"Ref":"Net"}},
            "SubnetIds":{"Value":{"Ref":"Only"}}}}"#,
        vpc: ("Net", "172.31.16.0/20"),
        subnets: &[ExpectedSubnet {
            logical_id: "Only",
            cidr_block: "172.31.16.0/24",
            availability_zone: None,
        }],
        subnet_ids_output: "SubnetIds",
    },
    NetworkCase {
        case: "two subnets with zones named outright and a dedicated-tenancy VPC, in YAML",
        stack: "ec2-network-named-zones",
        template: concat!(
            "Resources:\n",
            "  TenantVpc:\n",
            "    Type: AWS::EC2::VPC\n",
            "    Properties:\n",
            "      CidrBlock: 192.168.0.0/18\n",
            "      InstanceTenancy: dedicated\n",
            "  East:\n",
            "    Type: AWS::EC2::Subnet\n",
            "    Properties:\n",
            "      VpcId: !Ref TenantVpc\n",
            "      CidrBlock: 192.168.4.0/22\n",
            "      AvailabilityZone: us-east-1a\n",
            "  West:\n",
            "    Type: AWS::EC2::Subnet\n",
            "    Properties:\n",
            "      VpcId: !Ref TenantVpc\n",
            "      CidrBlock: 192.168.8.0/22\n",
            "      AvailabilityZone: us-east-1c\n",
            "Outputs:\n",
            "  VpcId:\n",
            "    Value: !Ref TenantVpc\n",
            "  SubnetIds:\n",
            "    Value: !Join [\",\", [!Ref East, !Ref West]]\n",
        ),
        vpc: ("TenantVpc", "192.168.0.0/18"),
        subnets: &[
            ExpectedSubnet {
                logical_id: "East",
                cidr_block: "192.168.4.0/22",
                availability_zone: Some("us-east-1a"),
            },
            ExpectedSubnet {
                logical_id: "West",
                cidr_block: "192.168.8.0/22",
                availability_zone: Some("us-east-1c"),
            },
        ],
        subnet_ids_output: "SubnetIds",
    },
];

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

/// The stack's `Outputs` as the key-to-value map a client reads them as.
async fn outputs(router: &MultiServiceRouter, stack: &str) -> Vec<(String, String)> {
    let (status, body) = call(router, "DescribeStacks", &[member("StackName", stack)]).await;
    assert_eq!(
        status, 200,
        "DescribeStacks({stack}) must answer 200, got {status} {body}"
    );
    xml_texts(&body, "OutputKey")
        .into_iter()
        .zip(xml_texts(&body, "OutputValue"))
        .collect()
}

/// One output's value, or the empty string where the stack published no such output.
fn output(outputs: &[(String, String)], key: &str) -> String {
    outputs
        .iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.clone())
        .unwrap_or_default()
}

/// The tag value a described EC2 resource carries under `key`, read off the `tagSet` the
/// answer reports — the ONE reading of an EC2 tag set in this crate's suites.
use crate::harness::ec2_tag_value as tag_value;

#[tokio::test]
async fn the_vpc_and_subnets_a_stack_declares_are_created_in_ec2() {
    for row in NETWORK_CASES {
        let case = row.case;
        let router = router();
        deploy(&router, row.stack, row.template).await;

        let published = outputs(&router, row.stack).await;
        let vpc_id = output(&published, "VpcId");
        assert!(
            is_service_assigned_id(&vpc_id, "vpc"),
            "case {case:?}: expected the VpcId output to carry vpc- plus \
             {SERVICE_ID_HEX_DIGITS} hexadecimal characters, got {vpc_id:?}"
        );
        assert_eq!(
            physical_id(&router, row.stack, row.vpc.0).await,
            vpc_id,
            "case {case:?}: expected the VPC's physical id to be the id the output carries"
        );

        // Amazon EC2 holds that VPC, with the block the template declared, and it is up: a
        // stack reports a resource created only once its owning service does. The block and
        // the state are asserted through EC2's own documented filters (`cidr`, `state`), so
        // the read names the VPC's own members rather than any nested element of the same
        // name.
        let (status, described) = ec2(&router, "DescribeVpcs", &[("VpcId.1", &vpc_id)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: EC2 must report the VPC {vpc_id:?} the stack created, got \
             {status} {described}"
        );
        assert_eq!(
            xml_text(&described, "vpcId").as_deref(),
            Some(vpc_id.as_str()),
            "case {case:?}: expected the read to report that VPC, got {described}"
        );
        let (status, filtered) = ec2(
            &router,
            "DescribeVpcs",
            &[
                ("VpcId.1", &vpc_id),
                ("Filter.1.Name", "cidr"),
                ("Filter.1.Value.1", row.vpc.1),
                ("Filter.2.Name", "state"),
                ("Filter.2.Value.1", "available"),
            ],
        )
        .await;
        assert_eq!(
            (status, xml_text(&filtered, "vpcId")),
            (200, Some(vpc_id.clone())),
            "case {case:?}: expected the VPC to be available with the declared block {}, got \
             {status} {filtered}",
            row.vpc.1
        );
        // The provenance CloudFormation records on every resource it creates reached EC2,
        // which only a real create could carry.
        assert_eq!(
            tag_value(&described, "aws:cloudformation:logical-id"),
            Some(row.vpc.0.to_string()),
            "case {case:?}: expected the VPC to carry the stack's logical-id tag, got \
             {described}"
        );
        assert_eq!(
            tag_value(&described, "aws:cloudformation:stack-name"),
            Some(row.stack.to_string()),
            "case {case:?}: expected the VPC to carry the stack-name tag, got {described}"
        );

        // The subnet ids the stack published — joined into one output, or the one id a
        // single-subnet stack publishes — are the ids of the subnets it declared.
        let joined = output(&published, row.subnet_ids_output);
        let reported: Vec<&str> = joined.split(',').filter(|id| !id.is_empty()).collect();
        let mut declared_ids = Vec::new();
        for subnet in row.subnets {
            declared_ids.push(physical_id(&router, row.stack, subnet.logical_id).await);
        }
        assert_eq!(
            reported, declared_ids,
            "case {case:?}: expected the {} output to carry the subnets' own ids, got \
             {joined:?}",
            row.subnet_ids_output
        );

        for (subnet, subnet_id) in row.subnets.iter().zip(&declared_ids) {
            let case = format!("{case} / {}", subnet.logical_id);
            assert!(
                is_service_assigned_id(subnet_id, "subnet"),
                "case {case:?}: expected subnet- plus {SERVICE_ID_HEX_DIGITS} hexadecimal \
                 characters, got {subnet_id:?}"
            );
            let (status, described) =
                ec2(&router, "DescribeSubnets", &[("SubnetId.1", subnet_id)]).await;
            assert_eq!(
                status, 200,
                "case {case:?}: EC2 must report the subnet {subnet_id:?} the stack created, \
                 got {status} {described}"
            );
            assert_eq!(
                xml_text(&described, "vpcId").as_deref(),
                Some(vpc_id.as_str()),
                "case {case:?}: expected the subnet to sit in the stack's VPC, got {described}"
            );
            assert_eq!(
                xml_text(&described, "cidrBlock").as_deref(),
                Some(subnet.cidr_block),
                "case {case:?}: expected the declared CIDR block, got {described}"
            );
            // A zone the declaration named is the zone EC2 placed the subnet in; where it
            // named none, EC2 selected one of the Region's.
            let zone = xml_text(&described, "availabilityZone").unwrap_or_default();
            match subnet.availability_zone {
                Some(expected) => assert_eq!(
                    zone, expected,
                    "case {case:?}: expected the declared zone, got {described}"
                ),
                None => assert!(
                    zone.starts_with(REGION),
                    "case {case:?}: expected a zone of {REGION}, got {zone:?}"
                ),
            }
            assert_eq!(
                tag_value(&described, "aws:cloudformation:logical-id"),
                Some(subnet.logical_id.to_string()),
                "case {case:?}: expected the subnet to carry the stack's logical-id tag, \
                 got {described}"
            );
        }

        // The subnets span as many zones as the stack declared: a row's distinct zones stay
        // distinct in EC2.
        let mut zones = Vec::new();
        for subnet_id in &declared_ids {
            let (_, described) =
                ec2(&router, "DescribeSubnets", &[("SubnetId.1", subnet_id)]).await;
            zones.push(xml_text(&described, "availabilityZone").unwrap_or_default());
        }
        let mut distinct = zones.clone();
        distinct.sort();
        distinct.dedup();
        assert_eq!(
            distinct.len(),
            row.subnets.len(),
            "case {case:?}: expected the subnets to span {} zones, got {zones:?}",
            row.subnets.len()
        );

        // ...and deleting the stack takes them away in EC2, which is what the stack having
        // created them means. An id nothing holds a resource for is refused — the subnet
        // with `InvalidSubnetID.NotFound`, the VPC with `InvalidVpcID.NotFound`.
        let (status, body) = call(&router, "DeleteStack", &[member("StackName", row.stack)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DeleteStack must answer 200, got {status} {body}"
        );
        wait_for_the_transition().await;
        for subnet_id in &declared_ids {
            let (status, refused) =
                ec2(&router, "DescribeSubnets", &[("SubnetId.1", subnet_id)]).await;
            assert_eq!(
                (status, xml_text(&refused, "Code")),
                (400, Some(INVALID_SUBNET_ID_NOT_FOUND.to_string())),
                "case {case:?}: expected EC2 to refuse the deleted subnet {subnet_id:?}, got \
                 {status} {refused}"
            );
        }
        let (status, refused) = ec2(&router, "DescribeVpcs", &[("VpcId.1", &vpc_id)]).await;
        assert_eq!(
            (status, xml_text(&refused, "Code")),
            (400, Some(INVALID_VPC_ID_NOT_FOUND.to_string())),
            "case {case:?}: expected EC2 to refuse the deleted VPC {vpc_id:?}, got {status} \
             {refused}"
        );
    }
}
