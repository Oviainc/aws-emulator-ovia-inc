// SPDX-License-Identifier: CC0-1.0

//! Every output a deployed stack declares carries a RESOLVED value, end to end over the
//! awsQuery wire.
//!
//! The rule under test: once a stack's deployment has completed, `DescribeStacks` reports
//! each output the template declares (and whose `Condition` holds) with its `OutputValue`
//! — the declared `Value` evaluated against the stack's parameters, the pseudo
//! parameters and the resources the template declares. The functions a template composes
//! those from are evaluated, not dropped: `Fn::GetAZs`, `Fn::Cidr`, `Fn::Base64`,
//! `Fn::Length`, `Fn::ToJsonString` (both of which the `AWS::LanguageExtensions` transform
//! defines, so the rows using them declare it), `Fn::If` and the condition functions; a
//! `Ref` to a
//! list-typed parameter is a list to every function that reads it; and `Fn::GetAtt`
//! answers for a resource of ANY type — from the type's published return values, from the
//! property the template declared where the attribute IS that property, and from
//! CloudFormation's uniform `Arn`/`Id`/`Name` naming otherwise.
//!
//! Only an expression whose input this emulator does not hold is left without a value (a
//! `Ref` to a resource a false `Condition` kept out of the stack, a `Fn::ImportValue` of
//! an export no stack published), and those rows assert exactly that.
//!
//! Every case drives the real request path (form-encoded body in, XML envelope out)
//! through the wire harness `stack_lifecycle.rs` builds and the `Output` reader
//! `stack_outputs.rs` builds, so what it asserts is what a client reading the stack it
//! just deployed sees. The table spans the class the rule generalises over: both template
//! spellings, each function, both address families of `Fn::Cidr` and a size past its
//! bound, list- and scalar-typed parameters, conditions that hold and conditions that do
//! not, and resource types the return-value catalogue carries and types it does not. The
//! issue's own shape — a VPC, its subnets and an Auto Scaling group, whose outputs publish
//! what the stack created — is the last row.

use crate::stack_lifecycle::{
    create_stack, describe_stack, is_service_assigned_id, member, router,
    wait_for_the_transition, xml_text, ACCOUNT_ID, REGION,
};
use crate::stack_outputs::{output_for, wire_outputs};
use service_com_amazonaws_cloudformation_cloudformation::state::status;

/// What one declared output must report.
#[derive(Debug, Clone)]
enum Expect {
    /// Exactly this `OutputValue`.
    Value(String),
    /// The physical id CloudFormation generates for a resource the template does not
    /// name — `<stack name>-<logical id>-<unique id>` — behind `prefix` (empty for the
    /// bare id, the ARN's head where the value is an ARN). No request fixes the unique
    /// segment, so the rule is the FORM.
    Generated {
        prefix: String,
        logical_id: &'static str,
    },
    /// The output is reported, and carries no `OutputValue`: the expression names
    /// something this emulator does not hold.
    NoValue,
    /// An identifier the OWNING service assigns, of the published form `<prefix>-` plus
    /// [`crate::harness::SERVICE_ID_HEX_DIGITS`] lowercase hexadecimal characters: a
    /// resource of an Amazon EC2 VPC-network type reports one of those rather than a name,
    /// because EC2 takes no name on its create. No request fixes the hexadecimal part, so
    /// the rule is the FORM. `prefix` carries the ARN's head where the value is an ARN built
    /// on such an identifier, exactly as [`Expect::Generated`]'s does.
    ServiceAssigned { prefix: String },
    /// A value of this FORM, for one the OWNING service builds out of an identifier it
    /// minted that is not a plain `<prefix>-<hex>` id: an Auto Scaling group's ARN carries
    /// the group's own resource id between its `autoScalingGroup` and
    /// `autoScalingGroupName/<name>` segments, and no request fixes that id.
    Form {
        described: &'static str,
        check: fn(&str) -> bool,
    },
    /// The output is not reported at all, because its `Condition` evaluates false.
    Absent,
}

/// How many hexadecimal characters the unique segment of a generated physical id carries.
const GENERATED_ID_HEX_DIGITS: usize = 12;

/// One row: a template, the parameters the create supplies, and what each output must
/// report.
struct Case {
    case: &'static str,
    stack_name: &'static str,
    template: String,
    parameters: Vec<(String, String)>,
    expected: Vec<(&'static str, Expect)>,
}

fn params(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .enumerate()
        .flat_map(|(index, (key, value))| {
            let position = index + 1;
            vec![
                (
                    format!("Parameters.member.{position}.ParameterKey"),
                    (*key).to_string(),
                ),
                (
                    format!("Parameters.member.{position}.ParameterValue"),
                    (*value).to_string(),
                ),
            ]
        })
        .collect()
}

/// The Availability Zones of a Region, named `<region>` + a letter from `a`: the naming
/// every service of this emulator models the account's network with (see
/// `services/common/src/network.rs`) and what EC2's `DescribeAvailabilityZones` reports.
fn zones(region: &str) -> Vec<String> {
    ["a", "b", "c"]
        .iter()
        .map(|letter| format!("{region}{letter}"))
        .collect()
}

fn sns_arn(name: &str) -> String {
    format!("arn:aws:sns:{REGION}:{ACCOUNT_ID}:{name}")
}

fn value(key: &'static str, text: impl Into<String>) -> (&'static str, Expect) {
    (key, Expect::Value(text.into()))
}

fn cases() -> Vec<Case> {
    let local = zones(REGION).join(",");
    vec![
        Case {
            case: "the Availability Zones a Region has, through Fn::GetAZs",
            stack_name: "zones",
            template: r#"Transform: AWS::LanguageExtensions
Outputs:
  FirstZone:
    Value: !Select [0, !GetAZs ""]
  LastZone:
    Value: !Select [2, !GetAZs ""]
  AllZones:
    Value: !Join [",", !GetAZs ""]
  StackRegionZones:
    Value: !Join [",", !GetAZs {Ref: "AWS::Region"}]
  NamedRegionZones:
    Value: !Join [",", !GetAZs "eu-west-2"]
  ZoneCount:
    Value:
      Fn::Length: !GetAZs ""
"#
            .to_string(),
            parameters: vec![],
            expected: vec![
                value("FirstZone", format!("{REGION}a")),
                value("LastZone", format!("{REGION}c")),
                value("AllZones", local.clone()),
                // "" and the Region pseudo parameter name the same Region.
                value("StackRegionZones", local.clone()),
                // A Region the stack does not live in: the zones are that Region's.
                value("NamedRegionZones", zones("eu-west-2").join(",")),
                value("ZoneCount", "3"),
            ],
        },
        Case {
            case: "Fn::Cidr carves subnets out of a block, in both address families",
            stack_name: "cidr",
            template: r#"{"Transform":"AWS::LanguageExtensions","Outputs":{
                   "FirstV4":{"Value":{"Fn::Select":["0",{"Fn::Cidr":["10.0.0.0/16",3,8]}]}},
                   "SecondV4":{"Value":{"Fn::Select":["1",{"Fn::Cidr":["10.0.0.0/16",3,8]}]}},
                   "ThirdV4":{"Value":{"Fn::Select":["2",{"Fn::Cidr":["10.0.0.0/16",3,8]}]}},
                   "JoinedV4":{"Value":{"Fn::Join":[",",{"Fn::Cidr":["192.168.0.0/24",2,4]}]}},
                   "CountV4":{"Value":{"Fn::Length":{"Fn::Cidr":["10.0.0.0/16",3,8]}}},
                   "SecondV6":{"Value":{"Fn::Select":["1",{"Fn::Cidr":["2001:db8::/32",2,64]}]}},
                   "WholeBlock":{"Value":{"Fn::Select":["0",{"Fn::Cidr":["10.1.0.0/16",1,16]}]}},
                   "WholeV6Block":{"Value":{"Fn::Select":["0",{"Fn::Cidr":["::/0",1,128]}]}},
                   "TooLarge":{"Value":{"Fn::Select":["0",{"Fn::Cidr":["10.0.0.0/24",2,16]}]}},
                   "TooMany":{"Value":{"Fn::Select":["0",{"Fn::Cidr":["10.0.0.0/24",5,6]}]}}}}"#
                .to_string(),
            parameters: vec![],
            expected: vec![
                // "specifying a value of 8 … will create a CIDR with a mask of /24":
                // 32 - 8 subnet bits, carved in order from the block's own network.
                value("FirstV4", "10.0.0.0/24"),
                value("SecondV4", "10.0.1.0/24"),
                value("ThirdV4", "10.0.2.0/24"),
                value("JoinedV4", "192.168.0.0/28,192.168.0.16/28"),
                value("CountV4", "3"),
                value("SecondV6", "2001:db8:0:1::/64"),
                // The bound where the subnet is the whole block, in both families.
                value("WholeBlock", "10.1.0.0/16"),
                value("WholeV6Block", "::/0"),
                // Past the bounds on either side: a subnet LARGER than the block, and
                // more subnets than the block holds (a /24 holds four /26s, not five).
                ("TooLarge", Expect::NoValue),
                ("TooMany", Expect::NoValue),
            ],
        },
        Case {
            case: "a list-typed parameter is a list to every function that reads it",
            stack_name: "typed-parameters",
            template: r#"{"Transform":"AWS::LanguageExtensions","Parameters":{
                   "SubnetIds":{"Type":"List<AWS::EC2::Subnet::Id>"},
                   "Stages":{"Type":"CommaDelimitedList"},
                   "Single":{"Type":"String"}},
                 "Outputs":{
                   "FirstSubnet":{"Value":{"Fn::Select":["0",{"Ref":"SubnetIds"}]}},
                   "SubnetCount":{"Value":{"Fn::Length":{"Ref":"SubnetIds"}}},
                   "JoinedStages":{"Value":{"Fn::Join":["|",{"Ref":"Stages"}]}},
                   "SecondStage":{"Value":{"Fn::Select":["1",{"Ref":"Stages"}]}},
                   "Scalar":{"Value":{"Ref":"Single"}},
                   "ScalarAsList":{"Value":{"Fn::Join":[",",{"Ref":"Single"}]}}}}"#
                .to_string(),
            parameters: params(&[
                ("SubnetIds", "subnet-00000000000000001,subnet-00000000000000002"),
                // "An array of literal strings that are separated by commas": the
                // spaces around an element are not part of it.
                ("Stages", "dev, prod"),
                ("Single", "one"),
            ]),
            expected: vec![
                value("FirstSubnet", "subnet-00000000000000001"),
                value("SubnetCount", "2"),
                value("JoinedStages", "dev|prod"),
                value("SecondStage", "prod"),
                value("Scalar", "one"),
                // A String parameter is one value, not a one-element list.
                ("ScalarAsList", Expect::NoValue),
            ],
        },
        Case {
            case: "conditions decide which resources and which outputs a stack has",
            stack_name: "conditional",
            template: r#"Parameters:
  Stage:
    Type: String
    Default: prod
Conditions:
  IsProd: !Equals [!Ref Stage, "prod"]
  IsNotProd: !Not [!Condition IsProd]
  EitherWay: !Or [!Condition IsProd, !Condition IsNotProd]
  BothWays: !And [!Condition IsProd, !Condition EitherWay]
Resources:
  ProdTopic:
    Type: AWS::SNS::Topic
    Condition: IsProd
    Properties:
      TopicName: prod-topic
  TestTopic:
    Type: AWS::SNS::Topic
    Condition: IsNotProd
    Properties:
      TopicName: test-topic
Outputs:
  Chosen:
    Value: !If [IsProd, !Ref ProdTopic, "none"]
  Negated:
    Value: !If [IsNotProd, "test", "prod"]
  Either:
    Value: !If [EitherWay, "yes", "no"]
  Both:
    Value: !If [BothWays, "yes", "no"]
  NotCreated:
    Value: !Ref TestTopic
  OnlyInProd:
    Condition: IsProd
    Value: reported
  OnlyInTest:
    Condition: IsNotProd
    Value: never
"#
            .to_string(),
            parameters: vec![],
            expected: vec![
                value("Chosen", sns_arn("prod-topic")),
                value("Negated", "prod"),
                value("Either", "yes"),
                value("Both", "yes"),
                // The resource its condition excluded is not in the stack, so there is
                // no physical id for `Ref` to answer.
                ("NotCreated", Expect::NoValue),
                value("OnlyInProd", "reported"),
                ("OnlyInTest", Expect::Absent),
            ],
        },
        Case {
            case: "Fn::Base64 and Fn::ToJsonString over resolved expressions",
            stack_name: "encoded",
            template: r#"{"Transform":"AWS::LanguageExtensions","Outputs":{
                   "Literal":{"Value":{"Fn::Base64":"probe"}},
                   "Composed":{"Value":{"Fn::Base64":{"Fn::Sub":"${AWS::Region}"}}},
                   "Json":{"Value":{"Fn::ToJsonString":[
                     {"Fn::Select":["0",{"Fn::GetAZs":""}]},"literal",2]}}}}"#
                .to_string(),
            parameters: vec![],
            expected: vec![
                // The Base64 of "probe" and of "us-east-1", computed by hand so the row
                // does not re-run the implementation.
                value("Literal", "cHJvYmU="),
                value("Composed", "dXMtZWFzdC0x"),
                value("Json", format!("[\"{REGION}a\",\"literal\",2]")),
            ],
        },
        Case {
            // The convention never contradicts a type the catalogue carries: the ARN it
            // publishes is the ARN its conventional names answer with.
            case: "a catalogued type's conventional Arn is the ARN it publishes",
            stack_name: "catalogued",
            template: r#"{"Resources":{
                   "T":{"Type":"AWS::SNS::Topic","Properties":{"TopicName":"probe-topic"}},
                   "D":{"Type":"AWS::DynamoDB::Table","Properties":{
                     "TableName":"probe-table","BillingMode":"PAY_PER_REQUEST",
                     "KeySchema":[{"AttributeName":"pk","KeyType":"HASH"}],
                     "AttributeDefinitions":[{"AttributeName":"pk","AttributeType":"S"}]}}},
                 "Outputs":{
                   "TopicPublished":{"Value":{"Fn::GetAtt":["T","TopicArn"]}},
                   "TopicConventional":{"Value":{"Fn::GetAtt":["T","Arn"]}},
                   "TablePublished":{"Value":{"Fn::GetAtt":["D","Arn"]}},
                   "TableConventional":{"Value":{"Fn::GetAtt":["D","TableArn"]}}}}"#
                .to_string(),
            parameters: vec![],
            expected: vec![
                value("TopicPublished", sns_arn("probe-topic")),
                value("TopicConventional", sns_arn("probe-topic")),
                value(
                    "TablePublished",
                    format!("arn:aws:dynamodb:{REGION}:{ACCOUNT_ID}:table/probe-table"),
                ),
                value(
                    "TableConventional",
                    format!("arn:aws:dynamodb:{REGION}:{ACCOUNT_ID}:table/probe-table"),
                ),
            ],
        },
        Case {
            // The issue's own shape: a VPC, two subnets derived from the Availability
            // Zones and a block, a launch template and an Auto Scaling group, whose
            // outputs are how the client that deployed the stack learns what it got.
            case: "a vpc, its subnets and an auto scaling group: every output resolves",
            stack_name: "asg-net",
            template: r#"Parameters:
  VpcCidr:
    Type: String
    Default: 10.20.0.0/16
  ImageId:
    Type: String
Resources:
  Vpc:
    Type: AWS::EC2::VPC
    Properties:
      CidrBlock: !Ref VpcCidr
  SubnetA:
    Type: AWS::EC2::Subnet
    Properties:
      VpcId: !Ref Vpc
      AvailabilityZone: !Select [0, !GetAZs ""]
      CidrBlock: !Select [0, !Cidr [!Ref VpcCidr, 3, 8]]
  SubnetB:
    Type: AWS::EC2::Subnet
    Properties:
      VpcId: !Ref Vpc
      AvailabilityZone: !Select [1, !GetAZs ""]
      CidrBlock: !Select [1, !Cidr [!Ref VpcCidr, 3, 8]]
  Lt:
    Type: AWS::EC2::LaunchTemplate
    Properties:
      LaunchTemplateName: probe-lt
      LaunchTemplateData:
        ImageId: !Ref ImageId
        InstanceType: t3.micro
  Asg:
    Type: AWS::AutoScaling::AutoScalingGroup
    Properties:
      AutoScalingGroupName: probe-asg
      MinSize: "2"
      MaxSize: "4"
      DesiredCapacity: "2"
      LaunchTemplate:
        LaunchTemplateId: !Ref Lt
        Version: !GetAtt Lt.LatestVersionNumber
      VPCZoneIdentifier:
        - !Ref SubnetA
        - !Ref SubnetB
  Notice:
    Type: AWS::SNS::Topic
Outputs:
  AsgName:
    Value: !Ref Asg
  AsgArn:
    Value: !GetAtt Asg.Arn
  AsgDesired:
    Value: !GetAtt Asg.DesiredCapacity
  LaunchTemplateName:
    Value: !Ref Lt
  LaunchTemplateVersion:
    Value: !GetAtt Lt.LatestVersionNumber
  VpcId:
    Value: !Ref Vpc
  VpcCidrBlock:
    Value: !GetAtt Vpc.CidrBlock
  SubnetVpc:
    Value: !GetAtt SubnetA.VpcId
  SubnetIds:
    Value: !Join [",", [!Ref SubnetA, !Ref SubnetB]]
  SubnetZones:
    Value: !Join [",", [!GetAtt SubnetA.AvailabilityZone, !GetAtt SubnetB.AvailabilityZone]]
  SubnetCidrs:
    Value: !Join [",", [!GetAtt SubnetA.CidrBlock, !GetAtt SubnetB.CidrBlock]]
  SubnetArn:
    Value: !GetAtt SubnetA.Arn
  NoticeArn:
    Value: !Ref Notice
"#
            .to_string(),
            parameters: params(&[("ImageId", "ami-00000000000000001")]),
            expected: vec![
                // The template NAMES the group, so that is its physical id.
                value("AsgName", "probe-asg"),
                // The group's ARN is the one Amazon EC2 Auto Scaling built for the group it
                // created, which carries the resource id THAT service minted beside the
                // group's name — not a path these naming rules could compute.
                (
                    "AsgArn",
                    Expect::Form {
                        described: "the ARN Amazon EC2 Auto Scaling built for the group: \
                                    arn:aws:autoscaling:<region>:<account>:autoScalingGroup:\
                                    <group id>:autoScalingGroupName/probe-asg",
                        check: is_auto_scaling_group_arn,
                    },
                ),
                // An attribute that is the property the template declared for it.
                value("AsgDesired", "2"),
                // "Ref returns the launch template ID", the `lt-` identifier Amazon EC2
                // assigned the template the stack created there — never the name the
                // template declared for it.
                (
                    "LaunchTemplateName",
                    Expect::ServiceAssigned {
                        prefix: "lt".to_string(),
                    },
                ),
                // The version EC2 reported for the one version `CreateLaunchTemplate`
                // creates.
                value("LaunchTemplateVersion", "1"),
                // The VPC the template does not name reports the `vpc-` identifier its
                // type's `Ref` returns, and the subnet's `VpcId` attribute is that same id.
                (
                    "VpcId",
                    Expect::ServiceAssigned {
                        prefix: "vpc".to_string(),
                    },
                ),
                (
                    "SubnetVpc",
                    Expect::ServiceAssigned {
                        prefix: "vpc".to_string(),
                    },
                ),
                value("VpcCidrBlock", "10.20.0.0/16"),
                value("SubnetZones", format!("{REGION}a,{REGION}b")),
                value("SubnetCidrs", "10.20.0.0/24,10.20.1.0/24"),
                // The subnet's ARN is the one Amazon EC2 reported for the subnet it
                // created, which carries that service's own identifier for it.
                (
                    "SubnetArn",
                    Expect::ServiceAssigned {
                        prefix: format!("arn:aws:ec2:{REGION}:{ACCOUNT_ID}:subnet/subnet"),
                    },
                ),
                // The topic the template does NOT name is the other half of the identity
                // rule: CloudFormation names it itself, and its ARN carries that generated
                // name.
                (
                    "NoticeArn",
                    Expect::Generated {
                        prefix: format!("arn:aws:sns:{REGION}:{ACCOUNT_ID}:"),
                        logical_id: "Notice",
                    },
                ),
            ],
        },
    ]
}

/// Whether `value` is the ARN Amazon EC2 Auto Scaling builds for the group named
/// `probe-asg`: `arn:aws:autoscaling:<region>:<account>:autoScalingGroup:<group id>:
/// autoScalingGroupName/<name>`, the form that service's own `AutoScalingGroupARN` carries.
/// The group id between the two segments is one that service minted, so the rule is the FORM
/// of the ARN and the name at the end of it.
fn is_auto_scaling_group_arn(value: &str) -> bool {
    let head = format!("arn:aws:autoscaling:{REGION}:{ACCOUNT_ID}:autoScalingGroup:");
    value.strip_prefix(&head).is_some_and(|rest| {
        rest.split_once(":autoScalingGroupName/")
            .is_some_and(|(group_id, name)| !group_id.is_empty() && name == "probe-asg")
    })
}

/// Whether `value` is a generated physical id of `logical_id` in `stack_name`, behind
/// `prefix`: `<stack name>-<logical id>-<unique id>`, the unique segment being
/// hexadecimal.
fn is_generated(value: &str, prefix: &str, stack_name: &str, logical_id: &str) -> bool {
    value
        .strip_prefix(&format!("{prefix}{stack_name}-{logical_id}-"))
        .is_some_and(|unique| {
            unique.len() == GENERATED_ID_HEX_DIGITS
                && unique.chars().all(|c| c.is_ascii_hexdigit())
        })
}

#[tokio::test]
async fn every_declared_output_of_a_deployed_stack_carries_its_resolved_value() {
    let router = router();

    for case in cases() {
        // A row declaring the language-extensions transform has to acknowledge it, and a
        // request may acknowledge a capability its template does not need — so every row
        // carries the same acknowledgement.
        let mut extra = case.parameters.clone();
        extra.push(member("Capabilities.member.1", "CAPABILITY_AUTO_EXPAND"));
        let (status_code, body) =
            create_stack(&router, case.stack_name, &case.template, &extra).await;
        assert_eq!(
            status_code, 200,
            "case {:?}: CreateStack must accept the template, got {status_code}: {body}",
            case.case
        );

        wait_for_the_transition().await;
        let (status_code, body) = describe_stack(&router, case.stack_name).await;
        assert_eq!(
            status_code, 200,
            "case {:?}: DescribeStacks must answer 200, got {status_code}: {body}",
            case.case
        );
        assert_eq!(
            xml_text(&body, "StackStatus").as_deref(),
            Some(status::CREATE_COMPLETE),
            "case {:?}: the stack must have reached CREATE_COMPLETE before its outputs \
             are read, got {body}",
            case.case
        );

        let outputs = wire_outputs(&body);
        for (key, expected) in &case.expected {
            if matches!(expected, Expect::Absent) {
                assert!(
                    output_for(&outputs, key).is_none(),
                    "case {:?}: output {key:?} is excluded by its Condition, so it must \
                     not be reported; got {outputs:?}",
                    case.case
                );
                continue;
            }
            let Some(output) = output_for(&outputs, key) else {
                panic!(
                    "case {:?}: no Output with OutputKey {key:?}; got {outputs:?}",
                    case.case
                );
            };
            match expected {
                Expect::Value(wanted) => assert_eq!(
                    output.value.as_deref(),
                    Some(wanted.as_str()),
                    "case {:?}: output {key:?} must carry {wanted:?}, got {:?}",
                    case.case,
                    output.value
                ),
                Expect::Generated { prefix, logical_id } => {
                    let actual = output.value.as_deref().unwrap_or_default();
                    assert!(
                        output.value.is_some()
                            && is_generated(actual, prefix, case.stack_name, logical_id),
                        "case {:?}: output {key:?} must carry \
                         {prefix}<stack>-{logical_id}-<unique id>, got {:?}",
                        case.case,
                        output.value
                    );
                }
                Expect::ServiceAssigned { prefix } => {
                    let actual = output.value.as_deref().unwrap_or_default();
                    assert!(
                        is_service_assigned_id(actual, prefix),
                        "case {:?}: output {key:?} must carry {prefix}-<17 hexadecimal \
                         characters>, got {:?}",
                        case.case,
                        output.value
                    );
                }
                Expect::Form { described, check } => {
                    let actual = output.value.as_deref().unwrap_or_default();
                    assert!(
                        output.value.is_some() && check(actual),
                        "case {:?}: output {key:?} must carry {described}, got {:?}",
                        case.case,
                        output.value
                    );
                }
                Expect::NoValue => assert!(
                    output.value.is_none(),
                    "case {:?}: output {key:?} names something this emulator does not \
                     hold, so it must carry no OutputValue, got {:?}",
                    case.case,
                    output.value
                ),
                Expect::Absent => unreachable!("handled above"),
            }
        }

        // The rule itself, over the WHOLE response rather than the keys the row spells:
        // no reported output may lack its value except the ones the row declares
        // unresolvable.
        let expected_without_value: Vec<&str> = case
            .expected
            .iter()
            .filter(|(_, expected)| matches!(expected, Expect::NoValue))
            .map(|(key, _)| *key)
            .collect();
        for output in &outputs {
            let key = output.key.clone().unwrap_or_default();
            if expected_without_value.contains(&key.as_str()) {
                continue;
            }
            assert!(
                output.value.is_some(),
                "case {:?}: every Output of a CREATE_COMPLETE stack must carry an \
                 OutputValue; {key:?} carries none ({outputs:?})",
                case.case
            );
        }
    }
}
