// SPDX-License-Identifier: CC0-1.0

//! The security groups a stack declares, read back through AMAZON EC2's own wire.
//!
//! RULE: a stack that reports `CREATE_COMPLETE` has created each declared resource in the
//! service that OWNS it, so the physical resource id it publishes for one — the value a
//! `Ref`, an `Fn::GetAtt` or an output carries — is the identifier that service assigned to a
//! resource it really holds, with the configuration the template declared, and the stack's
//! deletion takes that resource away again.
//!
//! Amazon EC2 is the owning service here: `CreateSecurityGroup` takes no identifier and
//! assigns the `sg-…` one "Ref returns the ID of the security group" names, so
//! `DescribeSecurityGroups` is what says whether the group a completed stack claims exists.
//! A group id nothing was created for is refused with [`INVALID_GROUP_NOT_FOUND`], which is
//! also what a deleted one is refused with — the other side of the same rule.
//!
//! The rules the group holds are part of that configuration, in BOTH directions: "when you
//! create a security group, it has no inbound rules", so its inbound rules are the ones the
//! declaration asked for; a new group carries the one all-traffic outbound rule EC2 gives it
//! ("by default, a security group includes an outbound rule that allows all outbound
//! traffic"), and a declaration that asks for outbound rules of its own asks for THOSE and
//! no others — so a group whose declaration names any outbound rule no longer carries that
//! one, and a declaration of the all-traffic rule itself leaves exactly one such rule rather
//! than a duplicate of the rule the group already held.
//!
//! The rows differ along what the rule generalises over: whether the template NAMES the group
//! or leaves CloudFormation to generate the name, whether it declares the VPC or leaves the
//! group in the Region's default one, how many inline rules it declares and in which
//! direction, which kind of source each rule names (an IPv4 block, an IPv6 block, another
//! group of the same stack), whether a rule carries a description, and the template spelling
//! (YAML with short-form intrinsics, JSON).

use service_com_amazonaws_ec2_amazonec2::state::INVALID_GROUP_NOT_FOUND;
use smithy_server_runtime::MultiServiceRouter;

use crate::stack_lifecycle::{
    acknowledging_iam, call, create_stack, describe_stack, ec2, is_service_assigned_id, member,
    router, wait_for_the_transition, xml_blocks, xml_text, xml_texts, SERVICE_ID_HEX_DIGITS,
};

/// The rules one direction of a group must carry, as the SETS Amazon EC2 reports them in:
/// the protocols, the port bounds, the IPv4 and IPv6 blocks, the ids of the groups the rules
/// reference and the descriptions they carry.
///
/// Sets rather than a per-rule structure because that is what the aggregate
/// `ipPermissions` / `ipPermissionsEgress` members of a described group report, and a rule
/// that was dropped, mis-directed or stripped of its source or its description is missing
/// from them.
#[derive(Debug)]
struct ExpectedRules {
    protocols: &'static [&'static str],
    ports: &'static [&'static str],
    ipv4_blocks: &'static [&'static str],
    ipv6_blocks: &'static [&'static str],
    /// The logical ids of the groups of the same stack the rules reference, whose own
    /// physical ids the referenced rules must carry.
    referenced: &'static [&'static str],
    descriptions: &'static [&'static str],
}

/// What a row expects of the group the stack declared.
struct GroupCase {
    case: &'static str,
    stack: &'static str,
    template: &'static str,
    /// The logical id of the `AWS::EC2::SecurityGroup` under test.
    logical_id: &'static str,
    /// The `GroupName` the template declared, or `None` where it declared none and
    /// CloudFormation generates the `<stack name>-<logical id>-<unique id>` one.
    group_name: Option<&'static str>,
    description: &'static str,
    /// Whether the template declares the VPC (as the `AWS::EC2::VPC` of this logical id) or
    /// leaves the group in the Region's default VPC.
    vpc: Option<&'static str>,
    inbound: ExpectedRules,
    /// The outbound rules the group must hold. A declaration that names none leaves the
    /// all-traffic rule Amazon EC2 gives a new group ([`THE_ALL_TRAFFIC_RULE`]); one that
    /// names any asks for exactly those, since CloudFormation removes that default rule, so
    /// the comparison is exact in both cases
    /// (`core/router/tests/all/cloudformation_ec2_ebs_instance_estate.rs` holds the same half
    /// across services).
    outbound: ExpectedRules,
}

/// No rules in a direction: what a row expects of the side its template declares nothing
/// for.
const NO_RULES: ExpectedRules = ExpectedRules {
    protocols: &[],
    ports: &[],
    ipv4_blocks: &[],
    ipv6_blocks: &[],
    referenced: &[],
    descriptions: &[],
};

/// The one outbound rule Amazon EC2 gives every new security group: the all-protocols rule
/// over every IPv4 address ("by default, a security group includes an outbound rule that
/// allows all outbound traffic"). What a row expects outbound where its template declares
/// no outbound rule of its own.
const THE_ALL_TRAFFIC_RULE: ExpectedRules = ExpectedRules {
    protocols: &["-1"],
    ports: &[],
    ipv4_blocks: &["0.0.0.0/0"],
    ipv6_blocks: &[],
    referenced: &[],
    descriptions: &[],
};

const GROUP_CASES: &[GroupCase] = &[
    GroupCase {
        case: "a named group in the stack's own VPC with two inbound IPv4 rules, one \
               described, in YAML",
        stack: "group-named-in-own-vpc",
        template: concat!(
            "Resources:\n",
            "  Net:\n",
            "    Type: AWS::EC2::VPC\n",
            "    Properties:\n",
            "      CidrBlock: 10.20.0.0/16\n",
            "  Primary:\n",
            "    Type: AWS::EC2::SecurityGroup\n",
            "    Properties:\n",
            "      GroupName: primary-node-access\n",
            "      GroupDescription: primary node access\n",
            "      VpcId: !Ref Net\n",
            "      SecurityGroupIngress:\n",
            "        - IpProtocol: tcp\n",
            "          FromPort: 22\n",
            "          ToPort: 22\n",
            "          CidrIp: 10.20.0.0/16\n",
            "          Description: ssh from the network\n",
            "        - IpProtocol: tcp\n",
            "          FromPort: 8443\n",
            "          ToPort: 8443\n",
            "          CidrIp: 10.21.0.0/16\n",
            "Outputs:\n",
            "  GroupRef:\n",
            "    Value: !Ref Primary\n",
            "  GroupIdAttribute:\n",
            "    Value: !GetAtt Primary.GroupId\n",
            "  GroupVpcAttribute:\n",
            "    Value: !GetAtt Primary.VpcId\n",
            "  VpcId:\n",
            "    Value: !Ref Net\n",
        ),
        logical_id: "Primary",
        group_name: Some("primary-node-access"),
        description: "primary node access",
        vpc: Some("Net"),
        inbound: ExpectedRules {
            protocols: &["tcp", "tcp"],
            ports: &["22", "22", "8443", "8443"],
            ipv4_blocks: &["10.20.0.0/16", "10.21.0.0/16"],
            ipv6_blocks: &[],
            referenced: &[],
            descriptions: &["ssh from the network"],
        },
        outbound: THE_ALL_TRAFFIC_RULE,
    },
    GroupCase {
        case: "an unnamed group in the Region's default VPC whose inbound rule references \
               another group of the same stack, in JSON",
        stack: "group-referencing-another",
        template: r#"{"Resources":{
            "Primary":{"Type":"AWS::EC2::SecurityGroup","Properties":{
              "GroupDescription":"the primary node"}},
            "Core":{"Type":"AWS::EC2::SecurityGroup","Properties":{
              "GroupDescription":"the core nodes",
              "SecurityGroupIngress":[{"IpProtocol":"tcp","FromPort":0,"ToPort":65535,
                "SourceSecurityGroupId":{"Fn::GetAtt":["Primary","GroupId"]},
                "Description":"everything from the primary"}]}}},
          "Outputs":{
            "GroupRef":{"Value":{"Ref":"Core"}},
            "GroupIdAttribute":{"Value":{"Fn::GetAtt":["Core","GroupId"]}}}}"#,
        logical_id: "Core",
        group_name: None,
        description: "the core nodes",
        vpc: None,
        inbound: ExpectedRules {
            protocols: &["tcp"],
            ports: &["0", "65535"],
            ipv4_blocks: &[],
            ipv6_blocks: &[],
            referenced: &["Primary"],
            descriptions: &["everything from the primary"],
        },
        outbound: THE_ALL_TRAFFIC_RULE,
    },
    GroupCase {
        case: "an unnamed group in the stack's own VPC with an all-protocol IPv6 inbound \
               rule and a described outbound rule, in YAML",
        stack: "group-ipv6-and-egress",
        template: concat!(
            "Resources:\n",
            "  Net:\n",
            "    Type: AWS::EC2::VPC\n",
            "    Properties:\n",
            "      CidrBlock: 10.30.0.0/16\n",
            "  Edge:\n",
            "    Type: AWS::EC2::SecurityGroup\n",
            "    Properties:\n",
            "      GroupDescription: the edge group\n",
            "      VpcId: !Ref Net\n",
            "      SecurityGroupIngress:\n",
            "        - IpProtocol: \"-1\"\n",
            "          CidrIpv6: 2001:db8::/32\n",
            "      SecurityGroupEgress:\n",
            "        - IpProtocol: udp\n",
            "          FromPort: 53\n",
            "          ToPort: 53\n",
            "          CidrIp: 10.31.0.0/16\n",
            "          Description: dns out of the network\n",
            "Outputs:\n",
            "  GroupRef:\n",
            "    Value: !Ref Edge\n",
            "  GroupIdAttribute:\n",
            "    Value: !GetAtt Edge.GroupId\n",
            "  GroupVpcAttribute:\n",
            "    Value: !GetAtt Edge.VpcId\n",
            "  VpcId:\n",
            "    Value: !Ref Net\n",
        ),
        logical_id: "Edge",
        group_name: None,
        description: "the edge group",
        vpc: Some("Net"),
        inbound: ExpectedRules {
            protocols: &["-1"],
            ports: &[],
            ipv4_blocks: &[],
            ipv6_blocks: &["2001:db8::/32"],
            referenced: &[],
            descriptions: &[],
        },
        outbound: ExpectedRules {
            protocols: &["udp"],
            ports: &["53", "53"],
            ipv4_blocks: &["10.31.0.0/16"],
            ipv6_blocks: &[],
            referenced: &[],
            descriptions: &["dns out of the network"],
        },
    },
    GroupCase {
        case: "a named group declaring no rule at all, in the Region's default VPC, in JSON",
        stack: "group-with-no-rules",
        template: r#"{"Resources":{
            "Bare":{"Type":"AWS::EC2::SecurityGroup","Properties":{
              "GroupName":"bare-group","GroupDescription":"no rules declared",
              "Tags":[{"Key":"Tier","Value":"diagnostics"}]}}},
          "Outputs":{
            "GroupRef":{"Value":{"Ref":"Bare"}},
            "GroupIdAttribute":{"Value":{"Fn::GetAtt":["Bare","GroupId"]}}}}"#,
        logical_id: "Bare",
        group_name: Some("bare-group"),
        description: "no rules declared",
        vpc: None,
        inbound: NO_RULES,
        outbound: THE_ALL_TRAFFIC_RULE,
    },
    GroupCase {
        // The issue's own shape: the declaration asks for the very rule Amazon EC2 gives a
        // new group, so the group ends up holding exactly one such rule — authorizing it
        // beside the rule already there would be the duplicate that service refuses.
        case: "a group whose only declared rule is the all-traffic outbound one, in YAML",
        stack: "group-all-traffic-egress",
        template: concat!(
            "Resources:\n",
            "  Net:\n",
            "    Type: AWS::EC2::VPC\n",
            "    Properties:\n",
            "      CidrBlock: 10.40.0.0/16\n",
            "  Instance:\n",
            "    Type: AWS::EC2::SecurityGroup\n",
            "    Properties:\n",
            "      GroupDescription: Security group for EC2 instance\n",
            "      VpcId: !Ref Net\n",
            "      SecurityGroupEgress:\n",
            "        - IpProtocol: \"-1\"\n",
            "          CidrIp: 0.0.0.0/0\n",
            "Outputs:\n",
            "  GroupRef:\n",
            "    Value: !Ref Instance\n",
            "  GroupIdAttribute:\n",
            "    Value: !GetAtt Instance.GroupId\n",
            "  GroupVpcAttribute:\n",
            "    Value: !GetAtt Instance.VpcId\n",
            "  VpcId:\n",
            "    Value: !Ref Net\n",
        ),
        logical_id: "Instance",
        group_name: None,
        description: "Security group for EC2 instance",
        vpc: Some("Net"),
        inbound: NO_RULES,
        outbound: THE_ALL_TRAFFIC_RULE,
    },
    GroupCase {
        // Two NARROW outbound rules: the all-traffic rule the group was given is gone, so
        // the group lets out only what the template allowed.
        case: "a group declaring two narrow outbound rules and one inbound rule, in JSON",
        stack: "group-narrow-egress",
        template: r#"{"Resources":{
            "Net":{"Type":"AWS::EC2::VPC","Properties":{"CidrBlock":"10.50.0.0/16"}},
            "Edge":{"Type":"AWS::EC2::SecurityGroup","Properties":{
              "GroupDescription":"the egress-restricted group",
              "VpcId":{"Ref":"Net"},
              "SecurityGroupIngress":[{"IpProtocol":"tcp","FromPort":443,"ToPort":443,
                "CidrIp":"10.50.0.0/16"}],
              "SecurityGroupEgress":[
                {"IpProtocol":"tcp","FromPort":443,"ToPort":443,"CidrIp":"10.51.0.0/16",
                 "Description":"https out of the network"},
                {"IpProtocol":"udp","FromPort":53,"ToPort":53,
                 "CidrIpv6":"2001:db8:2::/48"}]}}},
          "Outputs":{
            "GroupRef":{"Value":{"Ref":"Edge"}},
            "GroupIdAttribute":{"Value":{"Fn::GetAtt":["Edge","GroupId"]}},
            "GroupVpcAttribute":{"Value":{"Fn::GetAtt":["Edge","VpcId"]}},
            "VpcId":{"Value":{"Ref":"Net"}}}}"#,
        logical_id: "Edge",
        group_name: None,
        description: "the egress-restricted group",
        vpc: Some("Net"),
        inbound: ExpectedRules {
            protocols: &["tcp"],
            ports: &["443", "443"],
            ipv4_blocks: &["10.50.0.0/16"],
            ipv6_blocks: &[],
            referenced: &[],
            descriptions: &[],
        },
        outbound: ExpectedRules {
            protocols: &["tcp", "udp"],
            ports: &["443", "443", "53", "53"],
            ipv4_blocks: &["10.51.0.0/16"],
            ipv6_blocks: &["2001:db8:2::/48"],
            referenced: &[],
            descriptions: &["https out of the network"],
        },
    },
    GroupCase {
        // The shape an end-to-end estate deploys: the group sits in a stack that also
        // declares the network around it (an internet gateway, a route table, a route, a
        // subnet association), an instance role and a volume, so the row pins that the
        // group's own create is what the stack's completion depends on — and that the id it
        // publishes resolves in Amazon EC2 for as long as the stack exists.
        case: "a group in a whole network estate whose declaration asks for the all-traffic \
               outbound rule, in YAML",
        stack: "group-in-a-network-estate",
        template: concat!(
            "Resources:\n",
            "  Net:\n",
            "    Type: AWS::EC2::VPC\n",
            "    Properties:\n",
            "      CidrBlock: 10.60.0.0/16\n",
            "      EnableDnsSupport: true\n",
            "      EnableDnsHostnames: true\n",
            "  Igw:\n",
            "    Type: AWS::EC2::InternetGateway\n",
            "  IgwAttachment:\n",
            "    Type: AWS::EC2::VPCGatewayAttachment\n",
            "    Properties:\n",
            "      VpcId: !Ref Net\n",
            "      InternetGatewayId: !Ref Igw\n",
            "  PublicSubnet:\n",
            "    Type: AWS::EC2::Subnet\n",
            "    Properties:\n",
            "      VpcId: !Ref Net\n",
            "      CidrBlock: 10.60.0.0/24\n",
            "      AvailabilityZone: us-east-1a\n",
            "      MapPublicIpOnLaunch: true\n",
            "  PublicRouteTable:\n",
            "    Type: AWS::EC2::RouteTable\n",
            "    Properties:\n",
            "      VpcId: !Ref Net\n",
            "  PublicDefaultRoute:\n",
            "    Type: AWS::EC2::Route\n",
            "    DependsOn: IgwAttachment\n",
            "    Properties:\n",
            "      RouteTableId: !Ref PublicRouteTable\n",
            "      DestinationCidrBlock: 0.0.0.0/0\n",
            "      GatewayId: !Ref Igw\n",
            "  PublicSubnetAssociation:\n",
            "    Type: AWS::EC2::SubnetRouteTableAssociation\n",
            "    Properties:\n",
            "      SubnetId: !Ref PublicSubnet\n",
            "      RouteTableId: !Ref PublicRouteTable\n",
            "  Instance:\n",
            "    Type: AWS::EC2::SecurityGroup\n",
            "    Properties:\n",
            "      GroupDescription: Security group for EC2 instance\n",
            "      VpcId: !Ref Net\n",
            "      SecurityGroupEgress:\n",
            "        - IpProtocol: \"-1\"\n",
            "          CidrIp: 0.0.0.0/0\n",
            "      Tags:\n",
            "        - Key: Name\n",
            "          Value: the instance group\n",
            "  InstanceRole:\n",
            "    Type: AWS::IAM::Role\n",
            "    Properties:\n",
            "      AssumeRolePolicyDocument:\n",
            "        Version: '2012-10-17'\n",
            "        Statement:\n",
            "          - Effect: Allow\n",
            "            Principal:\n",
            "              Service: ec2.amazonaws.com\n",
            "            Action: sts:AssumeRole\n",
            "Outputs:\n",
            "  GroupRef:\n",
            "    Value: !Ref Instance\n",
            "  GroupIdAttribute:\n",
            "    Value: !GetAtt Instance.GroupId\n",
            "  GroupVpcAttribute:\n",
            "    Value: !GetAtt Instance.VpcId\n",
            "  VpcId:\n",
            "    Value: !Ref Net\n",
        ),
        logical_id: "Instance",
        group_name: None,
        description: "Security group for EC2 instance",
        vpc: Some("Net"),
        inbound: NO_RULES,
        outbound: THE_ALL_TRAFFIC_RULE,
    },
];

/// Deploy one row's template and wait past the instant its create settles.
///
/// A create that did NOT complete carries the refusal the owning service answered on the
/// failing resource's own event, so that refusal is what this reports rather than the status
/// alone: the whole point of the rule under test is that the group reaches Amazon EC2.
async fn deploy(router: &MultiServiceRouter, row: &GroupCase) {
    let case = row.case;
    let (status, body) =
        create_stack(router, row.stack, row.template, &acknowledging_iam(&[])).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateStack must answer 200, got {status} {body}"
    );
    wait_for_the_transition().await;
    let (status, described) = describe_stack(router, row.stack).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeStacks must answer 200, got {status} {described}"
    );
    let reported = xml_text(&described, "StackStatus").unwrap_or_default();
    if reported != "CREATE_COMPLETE" {
        let (_, events) = call(
            router,
            "DescribeStackEvents",
            &[member("StackName", row.stack)],
        )
        .await;
        panic!(
            "case {case:?}: expected the create to complete, got {reported:?} because {:?}",
            xml_texts(&events, "ResourceStatusReason")
        );
    }
}

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

/// The stack's `Outputs` as the key-to-value pairs a client reads them as.
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

/// Every value of `member` inside ONE direction's aggregate permissions, sorted: the block
/// is taken first so a rule of the other direction cannot be swept up with it.
fn rule_values(described: &str, direction: &str, member: &str) -> Vec<String> {
    let mut found: Vec<String> = xml_blocks(described, direction)
        .iter()
        .flat_map(|block| xml_texts(block, member))
        .collect();
    found.sort();
    found
}

/// `expected`, sorted, as the comparison [`rule_values`] is held to.
fn sorted(expected: &[&str]) -> Vec<String> {
    let mut sorted: Vec<String> = expected.iter().map(ToString::to_string).collect();
    sorted.sort();
    sorted
}

#[tokio::test]
async fn the_security_group_a_stack_declares_is_created_in_ec2() {
    for row in GROUP_CASES {
        let case = row.case;
        let router = router();
        deploy(&router, row).await;

        // The id the stack publishes for the group, from each of the three places it reports
        // one: they cannot disagree, and all three are Amazon EC2's own identifier form
        // rather than a name CloudFormation composed.
        let published = outputs(&router, row.stack).await;
        let group_id = output(&published, "GroupRef");
        assert!(
            is_service_assigned_id(&group_id, "sg"),
            "case {case:?}: expected the group's Ref to carry sg- plus \
             {SERVICE_ID_HEX_DIGITS} hexadecimal characters, got {group_id:?}"
        );
        assert_eq!(
            output(&published, "GroupIdAttribute"),
            group_id,
            "case {case:?}: expected Fn::GetAtt of GroupId to carry the id Ref answered \
             ({group_id:?}), got {published:?}"
        );
        assert_eq!(
            physical_id(&router, row.stack, row.logical_id).await,
            group_id,
            "case {case:?}: expected the group's physical id to be the id the output carries"
        );

        // Amazon EC2 holds that group, as the template declared it.
        let (status, described) =
            ec2(&router, "DescribeSecurityGroups", &[("GroupId.1", &group_id)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: EC2 must report the group {group_id:?} the stack created, got \
             {status} {described}"
        );
        assert_eq!(
            xml_text(&described, "groupId").as_deref(),
            Some(group_id.as_str()),
            "case {case:?}: expected the read to report that group, got {described}"
        );
        assert_eq!(
            xml_text(&described, "groupDescription").as_deref(),
            Some(row.description),
            "case {case:?}: expected the declared description, got {described}"
        );
        // The name the template asked for, or — where it asked for none — the
        // `<stack name>-<logical id>-<unique id>` one CloudFormation generates, which is the
        // name the group was really created under.
        let reported_name = xml_text(&described, "groupName").unwrap_or_default();
        match row.group_name {
            Some(declared) => assert_eq!(
                reported_name, declared,
                "case {case:?}: expected the declared group name, got {described}"
            ),
            None => assert!(
                reported_name.starts_with(&format!("{}-{}-", row.stack, row.logical_id)),
                "case {case:?}: expected a generated name of the form \
                 <stack>-<logical id>-<unique id>, got {reported_name:?}"
            ),
        }
        // The VPC the group sits in: the one the template declared, resolved to the id EC2
        // minted for that VPC, or the Region's default where the template declared none.
        let reported_vpc = xml_text(&described, "vpcId").unwrap_or_default();
        match row.vpc {
            Some(logical_id) => {
                let vpc_id = output(&published, "VpcId");
                assert_eq!(
                    vpc_id,
                    physical_id(&router, row.stack, logical_id).await,
                    "case {case:?}: expected the VpcId output to carry the VPC's own id"
                );
                assert_eq!(
                    reported_vpc, vpc_id,
                    "case {case:?}: expected the group in the stack's VPC {vpc_id:?}, got \
                     {described}"
                );
                assert_eq!(
                    output(&published, "GroupVpcAttribute"),
                    vpc_id,
                    "case {case:?}: expected Fn::GetAtt of VpcId to carry that same VPC id, \
                     got {published:?}"
                );
            }
            None => assert!(
                is_service_assigned_id(&reported_vpc, "vpc"),
                "case {case:?}: expected the group in a VPC of the Region, got \
                 {reported_vpc:?}"
            ),
        }
        // The provenance CloudFormation records on every resource it creates reached EC2,
        // which only a real create could carry.
        assert_eq!(
            tag_value(&described, "aws:cloudformation:logical-id"),
            Some(row.logical_id.to_string()),
            "case {case:?}: expected the group to carry the stack's logical-id tag, got \
             {described}"
        );
        assert_eq!(
            tag_value(&described, "aws:cloudformation:stack-name"),
            Some(row.stack.to_string()),
            "case {case:?}: expected the group to carry the stack-name tag, got {described}"
        );

        // The inline rules the declaration asked for are the rules EC2 holds on the group,
        // on the side the declaration put them: an inbound rule is the group's whole inbound
        // set, since "when you create a security group, it has no inbound rules".
        for (member, expected) in [
            ("ipProtocol", row.inbound.protocols),
            ("cidrIp", row.inbound.ipv4_blocks),
            ("cidrIpv6", row.inbound.ipv6_blocks),
            ("description", row.inbound.descriptions),
        ] {
            assert_eq!(
                rule_values(&described, "ipPermissions", member),
                sorted(expected),
                "case {case:?}: expected the inbound {member} values the declaration asked \
                 for, got {described}"
            );
        }
        // The port bounds of a rule are reported one `fromPort` and one `toPort` apiece, so
        // the two members are compared against the one list of bounds the row states.
        let mut inbound_ports = rule_values(&described, "ipPermissions", "fromPort");
        inbound_ports.extend(rule_values(&described, "ipPermissions", "toPort"));
        inbound_ports.sort();
        assert_eq!(
            inbound_ports,
            sorted(row.inbound.ports),
            "case {case:?}: expected the inbound port bounds the declaration asked for, got \
             {described}"
        );
        // A rule that references another group of the same stack carries THAT group's own
        // physical id, which is the hand-off a stack's resources make to each other.
        let mut referenced = Vec::new();
        for logical_id in row.inbound.referenced {
            referenced.push(physical_id(&router, row.stack, logical_id).await);
        }
        referenced.sort();
        assert_eq!(
            rule_values(&described, "groups", "groupId"),
            referenced,
            "case {case:?}: expected the inbound rules to reference the stack's own groups, \
             got {described}"
        );

        // The outbound rules are the group's whole outbound set: the all-traffic rule EC2
        // gave the new group where the declaration asked for no outbound rule, and exactly
        // the declared ones where it asked for any.
        for (member, expected) in [
            ("ipProtocol", row.outbound.protocols),
            ("cidrIp", row.outbound.ipv4_blocks),
            ("cidrIpv6", row.outbound.ipv6_blocks),
            ("description", row.outbound.descriptions),
        ] {
            assert_eq!(
                rule_values(&described, "ipPermissionsEgress", member),
                sorted(expected),
                "case {case:?}: expected the outbound {member} values the group must hold, \
                 got {described}"
            );
        }
        let mut outbound_ports = rule_values(&described, "ipPermissionsEgress", "fromPort");
        outbound_ports.extend(rule_values(&described, "ipPermissionsEgress", "toPort"));
        outbound_ports.sort();
        assert_eq!(
            outbound_ports,
            sorted(row.outbound.ports),
            "case {case:?}: expected the outbound port bounds the group must hold, got \
             {described}"
        );

        // ...and deleting the stack takes the group away in EC2, which is what the stack
        // having created it means. An id nothing holds a group for is refused with
        // `InvalidGroup.NotFound`.
        let (status, body) = call(&router, "DeleteStack", &[member("StackName", row.stack)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DeleteStack must answer 200, got {status} {body}"
        );
        wait_for_the_transition().await;
        let (status, refused) =
            ec2(&router, "DescribeSecurityGroups", &[("GroupId.1", &group_id)]).await;
        assert_eq!(
            (status, xml_text(&refused, "Code")),
            (400, Some(INVALID_GROUP_NOT_FOUND.to_string())),
            "case {case:?}: expected EC2 to refuse the deleted group {group_id:?}, got \
             {status} {refused}"
        );
    }
}
