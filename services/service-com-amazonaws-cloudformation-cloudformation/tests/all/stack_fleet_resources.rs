// SPDX-License-Identifier: CC0-1.0

//! The FLEET resources a stack declares — an `AWS::EC2::LaunchTemplate` and the
//! `AWS::AutoScaling::AutoScalingGroup` that launches from one — read back through the wire
//! of the service that OWNS each, beside the VPC defaults a template reads off a VPC.
//!
//! RULE: every `Fn::GetAtt` attribute a resource type publishes as a return value resolves,
//! and its value — like the physical id the type's `Ref` returns — is the one the service
//! that owns the resource answers its own read with. So an output over such an attribute
//! carries an `OutputValue` (never a key with no value), the identifier it carries is one
//! that service accepts, and a stack that reports `CREATE_COMPLETE` is one whose resources
//! those services really hold.
//!
//! Three families carry the rule here, each with an attribute no uniform id/ARN/name
//! convention could compute:
//!
//! * a launch template's `LaunchTemplateId`, `LatestVersionNumber` and
//!   `DefaultVersionNumber`, which are Amazon EC2's — it assigns the `lt-` id and keeps the
//!   version numbers;
//! * an Auto Scaling group's own existence and ARN, which are Amazon EC2 Auto Scaling's: the
//!   zones it places instances in follow from the SUBNETS the declaration names, which that
//!   service resolves against the account's own subnets;
//! * a VPC's `DefaultSecurityGroup` and `DefaultNetworkAcl`, which are the ids of the two
//!   resources EC2 creates WITH the VPC and reports under its own filters.
//!
//! The rows differ along what the rule generalises over: whether the template NAMES the
//! resource or leaves CloudFormation to name it, the shape of the launch data (scalars, a
//! nested structure, a LIST member, tag specifications), how an Auto Scaling group states
//! where it launches (subnets in two zones, one subnet, zones named outright), and how many
//! VPCs one stack declares.

use smithy_server_runtime::MultiServiceRouter;

use crate::stack_lifecycle::{
    autoscaling, call, deploy, ec2, ec2_tag_value as tag_value, is_service_assigned_id, member,
    router, xml_text, xml_texts,
};
use crate::stack_outputs::wire_outputs;

/// The AMI a launch template's data names. The id form is Amazon EC2's own
/// (`ami-` plus 17 hexadecimal characters), which is what that service reads an image id as.
const IMAGE_ID: &str = "ami-00000000000000001";

/// Every output of a deployed stack, as the key-to-value map a client reads them as — built
/// through the per-member reader, so an output that carries NO `OutputValue` is visible as
/// such rather than borrowing the next one's.
async fn outputs(
    case: &str,
    router: &MultiServiceRouter,
    stack: &str,
) -> Vec<(String, Option<String>)> {
    let (status, body) = call(router, "DescribeStacks", &[member("StackName", stack)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeStacks({stack}) must answer 200, got {status} {body}"
    );
    wire_outputs(&body)
        .into_iter()
        .map(|output| (output.key.unwrap_or_default(), output.value))
        .collect()
}

/// One output's `OutputValue`, asserted PRESENT: the half of the rule a client reads a
/// deployed stack through, since an output reported with a key and no value is one it cannot
/// use at all.
fn output(case: &str, outputs: &[(String, Option<String>)], key: &str) -> String {
    let found = outputs
        .iter()
        .find(|(name, _)| name == key)
        .unwrap_or_else(|| panic!("case {case:?}: no output named {key:?}, got {outputs:?}"));
    found
        .1
        .clone()
        .unwrap_or_else(|| panic!("case {case:?}: output {key:?} carries no OutputValue"))
}

/// One launch template a row declares: how its launch data is spelled, and what the resource
/// EC2 ends up holding must report.
struct LaunchTemplateCase {
    case: &'static str,
    stack: &'static str,
    template: &'static str,
    /// The `LaunchTemplateName` the declaration states, or `None` where CloudFormation names
    /// the template itself.
    declared_name: Option<&'static str>,
    /// A member of the launch DATA the row sends, as the element EC2 reports it under and the
    /// value it must carry: this is what says the data reached EC2 as the document the
    /// template declared.
    reported_data: &'static [(&'static str, &'static str)],
    /// A tag the row expects on the launch template itself, from a `TagSpecifications` entry
    /// naming the `launch-template` resource type.
    reported_tag: Option<(&'static str, &'static str)>,
}

const LAUNCH_TEMPLATE_CASES: &[LaunchTemplateCase] = &[
    LaunchTemplateCase {
        case: "a launch template the declaration names, with scalar launch data",
        stack: "fleet-lt-named",
        template: concat!(
            "Resources:\n",
            "  Lt:\n",
            "    Type: AWS::EC2::LaunchTemplate\n",
            "    Properties:\n",
            "      LaunchTemplateName: fleet-named-template\n",
            "      LaunchTemplateData:\n",
            "        ImageId: ami-00000000000000001\n",
            "        InstanceType: t3.micro\n",
            "Outputs:\n",
            "  Reference:\n",
            "    Value: !Ref Lt\n",
            "  Identifier:\n",
            "    Value: !GetAtt Lt.LaunchTemplateId\n",
            "  Latest:\n",
            "    Value: !GetAtt Lt.LatestVersionNumber\n",
            "  Default:\n",
            "    Value: !GetAtt Lt.DefaultVersionNumber\n",
        ),
        declared_name: Some("fleet-named-template"),
        reported_data: &[("imageId", IMAGE_ID), ("instanceType", "t3.micro")],
        reported_tag: None,
    },
    LaunchTemplateCase {
        case: "a launch template CloudFormation names, whose launch data carries a nested \
               structure",
        stack: "fleet-lt-generated",
        template: concat!(
            "Resources:\n",
            "  Lt:\n",
            "    Type: AWS::EC2::LaunchTemplate\n",
            "    Properties:\n",
            "      LaunchTemplateData:\n",
            "        ImageId: ami-00000000000000001\n",
            "        InstanceType: t3.small\n",
            "        Monitoring:\n",
            "          Enabled: true\n",
            "Outputs:\n",
            "  Reference:\n",
            "    Value: !Ref Lt\n",
            "  Identifier:\n",
            "    Value: !GetAtt Lt.LaunchTemplateId\n",
            "  Latest:\n",
            "    Value: !GetAtt Lt.LatestVersionNumber\n",
            "  Default:\n",
            "    Value: !GetAtt Lt.DefaultVersionNumber\n",
        ),
        declared_name: None,
        reported_data: &[("instanceType", "t3.small"), ("enabled", "true")],
        reported_tag: None,
    },
    LaunchTemplateCase {
        case: "launch data carrying a LIST member and tag specifications of its own",
        stack: "fleet-lt-lists",
        template: concat!(
            "Resources:\n",
            "  Vpc:\n",
            "    Type: AWS::EC2::VPC\n",
            "    Properties:\n",
            "      CidrBlock: 10.140.0.0/16\n",
            "  Sg:\n",
            "    Type: AWS::EC2::SecurityGroup\n",
            "    Properties:\n",
            "      GroupDescription: the group the launch data names\n",
            "      VpcId: !Ref Vpc\n",
            "  Lt:\n",
            "    Type: AWS::EC2::LaunchTemplate\n",
            "    Properties:\n",
            "      TagSpecifications:\n",
            "        - ResourceType: launch-template\n",
            "          Tags:\n",
            "            - Key: Fleet\n",
            "              Value: lists\n",
            "      LaunchTemplateData:\n",
            "        ImageId: ami-00000000000000001\n",
            "        InstanceType: t3.micro\n",
            "        SecurityGroupIds:\n",
            "          - !Ref Sg\n",
            "        TagSpecifications:\n",
            "          - ResourceType: instance\n",
            "            Tags:\n",
            "              - Key: Launched\n",
            "                Value: from-the-template\n",
            "Outputs:\n",
            "  Reference:\n",
            "    Value: !Ref Lt\n",
            "  Identifier:\n",
            "    Value: !GetAtt Lt.LaunchTemplateId\n",
            "  Latest:\n",
            "    Value: !GetAtt Lt.LatestVersionNumber\n",
            "  Default:\n",
            "    Value: !GetAtt Lt.DefaultVersionNumber\n",
            "  GroupId:\n",
            "    Value: !GetAtt Sg.GroupId\n",
        ),
        declared_name: None,
        reported_data: &[("imageId", IMAGE_ID)],
        reported_tag: Some(("Fleet", "lists")),
    },
];

#[tokio::test]
async fn a_launch_templates_published_return_values_are_the_ones_ec2_assigned() {
    for row in LAUNCH_TEMPLATE_CASES {
        let case = row.case;
        let router = router();
        deploy(&router, row.stack, row.template).await;

        let published = outputs(case, &router, row.stack).await;
        let reference = output(case, &published, "Reference");
        // "Ref returns the launch template ID", the `lt-` form Amazon EC2 assigns — never the
        // name the declaration or CloudFormation decided.
        assert!(
            is_service_assigned_id(&reference, "lt"),
            "case {case:?}: expected Ref to carry the lt- id EC2 assigned, got {reference:?}"
        );
        for attribute in ["Identifier", "Latest", "Default"] {
            let value = output(case, &published, attribute);
            assert!(
                !value.is_empty(),
                "case {case:?}: output {attribute:?} must carry the value the type publishes, \
                 got {value:?}"
            );
        }
        assert_eq!(
            output(case, &published, "Identifier"),
            reference,
            "case {case:?}: the LaunchTemplateId attribute and Ref must be the same id"
        );
        // The one version `CreateLaunchTemplate` creates is version 1, and it is both the
        // latest and the default.
        assert_eq!(
            (
                output(case, &published, "Latest"),
                output(case, &published, "Default")
            ),
            ("1".to_string(), "1".to_string()),
            "case {case:?}: expected the first version to be the latest and the default"
        );

        // Amazon EC2 holds that launch template, under the name the declaration asked for or
        // the one CloudFormation generated for it.
        let (status, described) = ec2(
            &router,
            "DescribeLaunchTemplates",
            &[("LaunchTemplateId.1", &reference)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: EC2 must report the launch template {reference:?} the stack \
             created, got {status} {described}"
        );
        assert_eq!(
            xml_text(&described, "launchTemplateId").as_deref(),
            Some(reference.as_str()),
            "case {case:?}: expected the read to report that template, got {described}"
        );
        let reported_name = xml_text(&described, "launchTemplateName").unwrap_or_default();
        match row.declared_name {
            Some(declared) => assert_eq!(
                reported_name, declared,
                "case {case:?}: expected the declared name, got {described}"
            ),
            None => assert!(
                reported_name.starts_with(&format!("{}-Lt-", row.stack)),
                "case {case:?}: expected the name CloudFormation generated \
                 ({}-Lt-<unique id>), got {reported_name:?}",
                row.stack
            ),
        }
        assert_eq!(
            xml_text(&described, "latestVersionNumber").as_deref(),
            Some("1"),
            "case {case:?}: expected EC2 to report version 1 as the latest, got {described}"
        );
        if let Some((key, value)) = row.reported_tag {
            assert_eq!(
                tag_value(&described, key).as_deref(),
                Some(value),
                "case {case:?}: expected the launch template to carry the declared tag \
                 {key}={value}, got {described}"
            );
        }
        // The provenance CloudFormation records on every resource it creates reached EC2,
        // which only a real create could carry.
        assert_eq!(
            tag_value(&described, "aws:cloudformation:logical-id").as_deref(),
            Some("Lt"),
            "case {case:?}: expected the template to carry the stack's logical-id tag, got \
             {described}"
        );

        // The launch DATA reached EC2 as the document the template declared: the version EC2
        // keeps reports the members the row sent, lists and nested structures included.
        let (status, versions) = ec2(
            &router,
            "DescribeLaunchTemplateVersions",
            &[("LaunchTemplateId", &reference), ("Versions.1", "1")],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: EC2 must report the template's version, got {status} {versions}"
        );
        for (element, expected) in row.reported_data {
            assert_eq!(
                xml_text(&versions, element).as_deref(),
                Some(*expected),
                "case {case:?}: expected the stored launch data to carry {element}={expected}, \
                 got {versions}"
            );
        }
        if row.stack == "fleet-lt-lists" {
            // The LIST member of the launch data is the one ec2Query flattens under its own
            // element name, so the group id the declaration named is in the stored version —
            // reported inside that member's own set.
            let group_id = output(case, &published, "GroupId");
            let stored = versions
                .split("<securityGroupIdSet>")
                .nth(1)
                .unwrap_or_default();
            assert!(
                xml_texts(stored, "item").contains(&group_id),
                "case {case:?}: expected the stored launch data to name the security group \
                 {group_id:?}, got {versions}"
            );
        }
    }
}

/// One Auto Scaling group a row declares: how it states where it launches, and the zones that
/// service must end up placing it in.
struct GroupCase {
    case: &'static str,
    stack: &'static str,
    template: &'static str,
    /// The zones Amazon EC2 Auto Scaling must report for the group, sorted.
    zones: &'static [&'static str],
    /// The sizes the group must report: `(MinSize, MaxSize, DesiredCapacity)`.
    sizes: (&'static str, &'static str, &'static str),
}

/// The subnets, launch template and outputs every group row shares, so each row's template
/// differs only in how the group states where it launches.
macro_rules! group_template {
    ($group:expr) => {
        concat!(
            "Resources:\n",
            "  Vpc:\n",
            "    Type: AWS::EC2::VPC\n",
            "    Properties:\n",
            "      CidrBlock: 10.141.0.0/16\n",
            "  SubnetA:\n",
            "    Type: AWS::EC2::Subnet\n",
            "    Properties:\n",
            "      VpcId: !Ref Vpc\n",
            "      CidrBlock: 10.141.1.0/24\n",
            "      AvailabilityZone: !Select [0, !GetAZs \"\"]\n",
            "  SubnetB:\n",
            "    Type: AWS::EC2::Subnet\n",
            "    Properties:\n",
            "      VpcId: !Ref Vpc\n",
            "      CidrBlock: 10.141.2.0/24\n",
            "      AvailabilityZone: !Select [1, !GetAZs \"\"]\n",
            "  Lt:\n",
            "    Type: AWS::EC2::LaunchTemplate\n",
            "    Properties:\n",
            "      LaunchTemplateData:\n",
            "        ImageId: ami-00000000000000001\n",
            "        InstanceType: t3.micro\n",
            $group,
            "Outputs:\n",
            "  GroupName:\n",
            "    Value: !Ref Asg\n",
            "  GroupArn:\n",
            "    Value: !GetAtt Asg.Arn\n",
            "  SubnetIds:\n",
            "    Value: !Join [\",\", [!Ref SubnetA, !Ref SubnetB]]\n",
        )
    };
}

const GROUP_CASES: &[GroupCase] = &[
    GroupCase {
        case: "a group whose zones follow from the two subnets its VPCZoneIdentifier names",
        stack: "fleet-asg-subnets",
        template: group_template!(concat!(
            "  Asg:\n",
            "    Type: AWS::AutoScaling::AutoScalingGroup\n",
            "    Properties:\n",
            "      MinSize: \"0\"\n",
            "      MaxSize: \"4\"\n",
            "      DesiredCapacity: \"0\"\n",
            "      Cooldown: \"120\"\n",
            "      VPCZoneIdentifier:\n",
            "        - !Ref SubnetA\n",
            "        - !Ref SubnetB\n",
            "      Tags:\n",
            "        - Key: Fleet\n",
            "          Value: subnets\n",
            "          PropagateAtLaunch: true\n",
            "      LaunchTemplate:\n",
            "        LaunchTemplateId: !Ref Lt\n",
            "        Version: !GetAtt Lt.LatestVersionNumber\n",
        )),
        zones: &["us-east-1a", "us-east-1b"],
        sizes: ("0", "4", "0"),
    },
    GroupCase {
        case: "a group in ONE subnet, which is one zone",
        stack: "fleet-asg-one-subnet",
        template: group_template!(concat!(
            "  Asg:\n",
            "    Type: AWS::AutoScaling::AutoScalingGroup\n",
            "    Properties:\n",
            "      AutoScalingGroupName: fleet-one-subnet\n",
            "      MinSize: \"1\"\n",
            "      MaxSize: \"3\"\n",
            "      DesiredCapacity: \"1\"\n",
            "      VPCZoneIdentifier:\n",
            "        - !Ref SubnetB\n",
            "      LaunchTemplate:\n",
            "        LaunchTemplateId: !Ref Lt\n",
            "        Version: !GetAtt Lt.LatestVersionNumber\n",
        )),
        zones: &["us-east-1b"],
        sizes: ("1", "3", "1"),
    },
    GroupCase {
        case: "a group that names its zones outright, with no subnets at all",
        stack: "fleet-asg-zones",
        template: group_template!(concat!(
            "  Asg:\n",
            "    Type: AWS::AutoScaling::AutoScalingGroup\n",
            "    Properties:\n",
            "      MinSize: \"0\"\n",
            "      MaxSize: \"1\"\n",
            "      AvailabilityZones:\n",
            "        - us-east-1a\n",
            "        - us-east-1c\n",
            "      HealthCheckType: EC2\n",
            "      LaunchTemplate:\n",
            "        LaunchTemplateId: !Ref Lt\n",
            "        Version: !GetAtt Lt.LatestVersionNumber\n",
        )),
        zones: &["us-east-1a", "us-east-1c"],
        sizes: ("0", "1", "0"),
    },
];

#[tokio::test]
async fn an_auto_scaling_group_a_stack_creates_is_the_group_that_service_reports() {
    for row in GROUP_CASES {
        let case = row.case;
        let router = router();
        deploy(&router, row.stack, row.template).await;

        let published = outputs(case, &router, row.stack).await;
        let name = output(case, &published, "GroupName");
        let arn = output(case, &published, "GroupArn");

        // Amazon EC2 Auto Scaling holds the group the stack reported created, under the name
        // the type's `Ref` returns.
        let (status, described) = autoscaling(
            &router,
            "DescribeAutoScalingGroups",
            &[("AutoScalingGroupNames.member.1", &name)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: Auto Scaling must report the group {name:?} the stack created, got \
             {status} {described}"
        );
        assert_eq!(
            xml_text(&described, "AutoScalingGroupName").as_deref(),
            Some(name.as_str()),
            "case {case:?}: expected the read to report that group, got {described}"
        );
        // The ARN the stack publishes is the one that service built for the group.
        assert_eq!(
            xml_text(&described, "AutoScalingGroupARN").as_deref(),
            Some(arn.as_str()),
            "case {case:?}: expected the stack's ARN output to be the group's own, got \
             {described}"
        );

        // The zones the group places instances in: the zones of the SUBNETS its declaration
        // named, or the zones it named outright. Read INSIDE the group's own
        // `AvailabilityZones` block, so a `<member>` of another list the answer carries
        // cannot be swept up with them.
        let zone_block = described
            .split("<AvailabilityZones>")
            .nth(1)
            .and_then(|rest| rest.split("</AvailabilityZones>").next())
            .unwrap_or_default();
        let mut zones = xml_texts(zone_block, "member");
        zones.sort();
        assert_eq!(
            zones, row.zones,
            "case {case:?}: expected the group to be in {:?}, got {described}",
            row.zones
        );
        assert_eq!(
            (
                xml_text(&described, "MinSize").unwrap_or_default(),
                xml_text(&described, "MaxSize").unwrap_or_default(),
                xml_text(&described, "DesiredCapacity").unwrap_or_default(),
            ),
            (
                row.sizes.0.to_string(),
                row.sizes.1.to_string(),
                row.sizes.2.to_string()
            ),
            "case {case:?}: expected the declared sizes, got {described}"
        );
        // Where the declaration named subnets, the group holds the subnet ids the stack
        // published for them — the ids EC2 assigned, as one comma-separated value.
        if row.stack != "fleet-asg-zones" {
            let subnets = xml_text(&described, "VPCZoneIdentifier").unwrap_or_default();
            for subnet_id in subnets.split(',').filter(|id| !id.is_empty()) {
                assert!(
                    is_service_assigned_id(subnet_id, "subnet"),
                    "case {case:?}: expected the group's subnets to be the ids EC2 assigned, \
                     got {subnets:?}"
                );
                let (status, _) =
                    ec2(&router, "DescribeSubnets", &[("SubnetId.1", subnet_id)]).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: EC2 must hold the subnet {subnet_id:?} the group launches \
                     into"
                );
            }
        }
        // A declared tag reaches the group with the `PropagateAtLaunch` its declaration
        // states, which is a member of an Auto Scaling group's tag alone.
        if row.stack == "fleet-asg-subnets" {
            assert_eq!(
                autoscaling_tag(&described, "Fleet"),
                Some(("subnets".to_string(), "true".to_string())),
                "case {case:?}: expected the group to carry the declared tag with its \
                 PropagateAtLaunch, got {described}"
            );
        }
    }
}

#[tokio::test]
async fn a_vpc_reports_the_default_group_and_acl_ec2_created_with_it() {
    let case = "two VPCs in one stack, each reporting its own defaults";
    let router = router();
    let template = concat!(
        "Resources:\n",
        "  First:\n",
        "    Type: AWS::EC2::VPC\n",
        "    Properties:\n",
        "      CidrBlock: 10.142.0.0/16\n",
        "  Second:\n",
        "    Type: AWS::EC2::VPC\n",
        "    Properties:\n",
        "      CidrBlock: 10.143.0.0/16\n",
        "Outputs:\n",
        "  FirstId:\n",
        "    Value: !Ref First\n",
        "  FirstGroup:\n",
        "    Value: !GetAtt First.DefaultSecurityGroup\n",
        "  FirstAcl:\n",
        "    Value: !GetAtt First.DefaultNetworkAcl\n",
        "  SecondId:\n",
        "    Value: !Ref Second\n",
        "  SecondGroup:\n",
        "    Value: !GetAtt Second.DefaultSecurityGroup\n",
        "  SecondAcl:\n",
        "    Value: !GetAtt Second.DefaultNetworkAcl\n",
    );
    deploy(&router, "fleet-vpc-defaults", template).await;
    let published = outputs(case, &router, "fleet-vpc-defaults").await;

    for (vpc_output, group_output, acl_output) in [
        ("FirstId", "FirstGroup", "FirstAcl"),
        ("SecondId", "SecondGroup", "SecondAcl"),
    ] {
        let case = format!("{case} / {vpc_output}");
        let vpc_id = output(&case, &published, vpc_output);
        let group_id = output(&case, &published, group_output);
        let acl_id = output(&case, &published, acl_output);

        // The group the attribute names is the `default` group Amazon EC2 created WITH the
        // VPC, read through that service's own filters.
        let (status, groups) = ec2(
            &router,
            "DescribeSecurityGroups",
            &[
                ("Filter.1.Name", "vpc-id"),
                ("Filter.1.Value.1", &vpc_id),
                ("Filter.2.Name", "group-name"),
                ("Filter.2.Value.1", "default"),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: EC2 must report the VPC's default group, got {status} {groups}"
        );
        assert_eq!(
            xml_text(&groups, "groupId").as_deref(),
            Some(group_id.as_str()),
            "case {case:?}: expected the DefaultSecurityGroup attribute to be the id of the \
             group EC2 created with the VPC, got {groups}"
        );
        assert!(
            is_service_assigned_id(&group_id, "sg"),
            "case {case:?}: expected an sg- id EC2 assigned, got {group_id:?}"
        );

        // And the ACL is the one EC2 reports as that VPC's default.
        let (status, acls) = ec2(
            &router,
            "DescribeNetworkAcls",
            &[
                ("Filter.1.Name", "vpc-id"),
                ("Filter.1.Value.1", &vpc_id),
                ("Filter.2.Name", "default"),
                ("Filter.2.Value.1", "true"),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: EC2 must report the VPC's default network ACL, got {status} {acls}"
        );
        assert_eq!(
            xml_text(&acls, "networkAclId").as_deref(),
            Some(acl_id.as_str()),
            "case {case:?}: expected the DefaultNetworkAcl attribute to be the id of the ACL \
             EC2 created with the VPC, got {acls}"
        );
    }

    // Two VPCs are two sets of defaults: neither reports the other's.
    assert_ne!(
        output(case, &published, "FirstGroup"),
        output(case, &published, "SecondGroup"),
        "case {case:?}: each VPC must report its OWN default security group"
    );
    assert_ne!(
        output(case, &published, "FirstAcl"),
        output(case, &published, "SecondAcl"),
        "case {case:?}: each VPC must report its OWN default network ACL"
    );
}

#[tokio::test]
async fn deleting_the_stack_takes_the_fleet_resources_away_again() {
    let case = "a group and the launch template it launches from, deleted with the stack";
    let router = router();
    deploy(&router, "fleet-delete", GROUP_CASES[0].template).await;
    let published = outputs(case, &router, "fleet-delete").await;
    let name = output(case, &published, "GroupName");
    let (_, described) = call(
        &router,
        "DescribeStackResources",
        &[
            member("StackName", "fleet-delete"),
            member("LogicalResourceId", "Lt"),
        ],
    )
    .await;
    let launch_template_id = xml_text(&described, "PhysicalResourceId").unwrap_or_default();

    let (status, body) = call(
        &router,
        "DeleteStack",
        &[member("StackName", "fleet-delete")],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DeleteStack must answer 200, got {status} {body}"
    );
    crate::harness::wait_for_the_transition().await;

    // The group is gone from Amazon EC2 Auto Scaling: a read of it reports none.
    let (_, groups) = autoscaling(
        &router,
        "DescribeAutoScalingGroups",
        &[("AutoScalingGroupNames.member.1", &name)],
    )
    .await;
    assert_eq!(
        xml_text(&groups, "AutoScalingGroupName"),
        None,
        "case {case:?}: the deleted stack's group must be gone from Auto Scaling, got {groups}"
    );
    // And the launch template is gone from Amazon EC2, which no longer reports a template
    // under the id it assigned: the describe is refused rather than answering one. (WHICH
    // refusal that read classifies an id it holds no template for as is its own handler's
    // rule, so the case asserts the refusal and that no template is reported.)
    let (status, templates) = ec2(
        &router,
        "DescribeLaunchTemplates",
        &[("LaunchTemplateId.1", &launch_template_id)],
    )
    .await;
    assert_eq!(
        (status, xml_text(&templates, "launchTemplateId")),
        (400, None),
        "case {case:?}: the deleted stack's launch template must be gone from EC2, got \
         {status} {templates}"
    );
}

/// The `(Value, PropagateAtLaunch)` an Auto Scaling group reports for the tag keyed `key`,
/// read off the `Tags` its describe answers with.
fn autoscaling_tag(body: &str, key: &str) -> Option<(String, String)> {
    let tags = body.split("<Tags>").nth(1).unwrap_or_default();
    xml_texts(tags, "Key")
        .into_iter()
        .zip(
            xml_texts(tags, "Value")
                .into_iter()
                .zip(xml_texts(tags, "PropagateAtLaunch")),
        )
        .find(|(held, _)| held == key)
        .map(|(_, pair)| pair)
}
