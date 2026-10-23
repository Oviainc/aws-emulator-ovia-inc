// SPDX-License-Identifier: CC0-1.0

//! What a stack's declared CONTAINER-PLATFORM resources report, end to end over the
//! awsQuery wire.
//!
//! The rule under test: for a stack whose template declares Amazon ECR, Amazon ECS or
//! Elastic Load Balancing v2 resources, every declared output carries an `OutputValue` —
//! the physical id that resource type's "Return values" documents `Ref` as returning, or
//! the attribute `Fn::GetAtt` names, each in the FORM the owning service publishes it in.
//! That is what the standard `{o["OutputKey"]: o["OutputValue"]}` read needs: an output
//! reported with its key and NO value raises `KeyError` there, and an output whose value
//! is a bare name is not something a client can address a load balancer or a target group
//! by.
//!
//! Every case drives the real request path (form-encoded body in, XML envelope out)
//! through the wire harness `stack_lifecycle.rs` builds. The table spans the class the
//! rule generalises over: both template spellings (YAML with short-form intrinsics, JSON
//! with long-form ones), resources the template NAMES and resources it leaves
//! CloudFormation to name, each type in the catalogue, and — for the load balancer, whose
//! published identifiers depend on its own properties — both schemes and two of the three
//! types.
//!
//! The one type in these templates this emulator also CREATES in the service that owns it
//! (`AWS::ECS::Cluster`, in Amazon ECS) is exercised by
//! `core/router/tests/all/cloudformation_provisions_container_platform_resources.rs`,
//! which mounts both services: a crate-local suite can only see CloudFormation's side of
//! that call.

use crate::stack_lifecycle::{
    create_stack, describe_stack, member, router, wait_for_the_transition, xml_text, ACCOUNT_ID,
    REGION,
};
use crate::stack_outputs::{output_for, wire_outputs, WireOutput};
use service_com_amazonaws_cloudformation_cloudformation::state::status;

/// What one declared output must report.
enum Expected {
    /// Exactly this `OutputValue`.
    Value(String),
    /// A value of this FORM: for an identifier the OWNING SERVICE assigns — which no
    /// request fixes, so no row can spell it — the rule is the documented form.
    Form {
        described: String,
        check: Box<dyn Fn(&str) -> bool>,
    },
}

/// One row: a template, and what each of its outputs must report.
struct Case {
    case: &'static str,
    stack_name: &'static str,
    template: String,
    parameters: Vec<(String, String)>,
    expected: Vec<(&'static str, Expected)>,
}

fn params(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .enumerate()
        .flat_map(|(index, (key, value))| {
            let position = index + 1;
            vec![
                member(&format!("Parameters.member.{position}.ParameterKey"), key),
                member(
                    &format!("Parameters.member.{position}.ParameterValue"),
                    value,
                ),
            ]
        })
        .collect()
}

fn value(key: &'static str, value: String) -> (&'static str, Expected) {
    (key, Expected::Value(value))
}

fn form(
    key: &'static str,
    described: impl Into<String>,
    check: impl Fn(&str) -> bool + 'static,
) -> (&'static str, Expected) {
    (
        key,
        Expected::Form {
            described: described.into(),
            check: Box::new(check),
        },
    )
}

/// `arn:aws:ecr:<region>:<account>:repository/<name>`, the ARN
/// `Fn::GetAtt <repository>.Arn` reports.
fn repository_arn(name: &str) -> String {
    format!("arn:aws:ecr:{REGION}:{ACCOUNT_ID}:repository/{name}")
}

/// `<account>.dkr.ecr.<region>.amazonaws.com/<name>` — "The format is
/// `aws_account_id.dkr.ecr.region.amazonaws.com/repository-name`", the URI
/// `Fn::GetAtt <repository>.RepositoryUri` reports and a container image reference is
/// built on.
fn repository_uri(name: &str) -> String {
    format!("{ACCOUNT_ID}.dkr.ecr.{REGION}.amazonaws.com/{name}")
}

/// `arn:aws:ecs:<region>:<account>:task-definition/<family>:1` — what `Ref` of an
/// `AWS::ECS::TaskDefinition` returns for the FIRST revision of a family ("the first task
/// definition that you registered to a family is given a revision of 1").
fn task_definition_arn(family: &str) -> String {
    format!("arn:aws:ecs:{REGION}:{ACCOUNT_ID}:task-definition/{family}:1")
}

/// Whether `value` is the 16 lowercase hexadecimal characters Elastic Load Balancing
/// assigns a load balancer or a target group as its id
/// (`.../app/my-load-balancer/50dc6c495c0c9188`).
fn is_elb_id(value: &str) -> bool {
    value.len() == 16
        && value
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
}

/// Whether `value` is the ARN of the target group named `name`:
/// `arn:aws:elasticloadbalancing:<region>:<account>:targetgroup/<name>/<id>`.
fn is_target_group_arn(value: &str, name: &str) -> bool {
    value
        .strip_prefix(&format!(
            "arn:aws:elasticloadbalancing:{REGION}:{ACCOUNT_ID}:targetgroup/{name}/"
        ))
        .is_some_and(is_elb_id)
}

/// Whether `value` is the ARN of the `type_code` load balancer named `name`:
/// `arn:aws:elasticloadbalancing:<region>:<account>:loadbalancer/<type>/<name>/<id>`.
fn is_load_balancer_arn(value: &str, type_code: &str, name: &str) -> bool {
    value
        .strip_prefix(&format!(
            "arn:aws:elasticloadbalancing:{REGION}:{ACCOUNT_ID}:loadbalancer/{type_code}/{name}/"
        ))
        .is_some_and(is_elb_id)
}

/// Whether `value` is the DNS name of the load balancer named `name`:
/// `<name>-<id>.<region>.elb.amazonaws.com`, prefixed `internal-` for an internal one.
fn is_dns_name(value: &str, name: &str, internal: bool) -> bool {
    let prefix = if internal { "internal-" } else { "" };
    value
        .strip_prefix(&format!("{prefix}{name}-"))
        .and_then(|rest| rest.strip_suffix(&format!(".{REGION}.elb.amazonaws.com")))
        .is_some_and(is_elb_id)
}

/// Whether `value` is the `<type>/<name>/<id>` path a load balancer's
/// `LoadBalancerFullName` reports — the resource part of its own ARN.
fn is_load_balancer_full_name(value: &str, type_code: &str, name: &str) -> bool {
    value
        .strip_prefix(&format!("{type_code}/{name}/"))
        .is_some_and(is_elb_id)
}

/// Whether `value` is the `targetgroup/<name>/<id>` path a target group's
/// `TargetGroupFullName` reports.
fn is_target_group_full_name(value: &str, name: &str) -> bool {
    value
        .strip_prefix(&format!("targetgroup/{name}/"))
        .is_some_and(is_elb_id)
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            // The issue's own stack, in the YAML spelling with short-form intrinsics: the
            // registry, the task definition, the target group and the internet-facing
            // application load balancer an ECS service runs behind, every resource NAMED
            // by the template through a parameter, and the three outputs the issue's
            // deploy helper reads.
            case: "yaml, a named ecr + ecs + elbv2 application stack",
            stack_name: "shop-platform",
            template: concat!(
                "Parameters:\n",
                "  AppName:\n",
                "    Type: String\n",
                "Resources:\n",
                "  AppRepository:\n",
                "    Type: AWS::ECR::Repository\n",
                "    Properties:\n",
                "      RepositoryName: !Sub \"${AppName}-images\"\n",
                "      ImageTagMutability: MUTABLE\n",
                "  TargetGroup:\n",
                "    Type: AWS::ElasticLoadBalancingV2::TargetGroup\n",
                "    Properties:\n",
                "      Name: !Sub \"${AppName}-tg\"\n",
                "      Port: 80\n",
                "      Protocol: HTTP\n",
                "      TargetType: ip\n",
                "      VpcId: vpc-0123456789abcdef0\n",
                "  LoadBalancer:\n",
                "    Type: AWS::ElasticLoadBalancingV2::LoadBalancer\n",
                "    Properties:\n",
                "      Name: !Sub \"${AppName}-alb\"\n",
                "      Scheme: internet-facing\n",
                "      Type: application\n",
                "      Subnets:\n",
                "        - subnet-0123456789abcdef0\n",
                "        - subnet-abcdef0123456789a\n",
                "  TaskDefinition:\n",
                "    Type: AWS::ECS::TaskDefinition\n",
                "    Properties:\n",
                "      Family: !Sub \"${AppName}-task\"\n",
                "      Cpu: \"256\"\n",
                "      Memory: \"512\"\n",
                "      NetworkMode: awsvpc\n",
                "      ContainerDefinitions:\n",
                "        - Name: app\n",
                "          Image: !Sub \"${AppRepository.RepositoryUri}:v1\"\n",
                "          Essential: true\n",
                "Outputs:\n",
                "  RepositoryUri:\n",
                "    Value: !GetAtt AppRepository.RepositoryUri\n",
                "  RepositoryArn:\n",
                "    Value: !GetAtt AppRepository.Arn\n",
                "  RepositoryName:\n",
                "    Value: !Ref AppRepository\n",
                "  TaskDefinitionArn:\n",
                "    Value: !Ref TaskDefinition\n",
                "  TargetGroupArn:\n",
                "    Value: !Ref TargetGroup\n",
                "  TargetGroupFullName:\n",
                "    Value: !GetAtt TargetGroup.TargetGroupFullName\n",
                "  LoadBalancerArn:\n",
                "    Value: !Ref LoadBalancer\n",
                "  LoadBalancerDnsName:\n",
                "    Value: !GetAtt LoadBalancer.DNSName\n",
                "  LoadBalancerFullName:\n",
                "    Value: !GetAtt LoadBalancer.LoadBalancerFullName\n",
                "  ContainerImage:\n",
                "    Value: !Sub \"${AppRepository.RepositoryUri}:v1\"\n",
            )
            .to_string(),
            parameters: params(&[("AppName", "shop")]),
            expected: vec![
                value("RepositoryUri", repository_uri("shop-images")),
                value("RepositoryArn", repository_arn("shop-images")),
                value("RepositoryName", "shop-images".to_string()),
                value("TaskDefinitionArn", task_definition_arn("shop-task")),
                form(
                    "TargetGroupArn",
                    "the target group's own ARN, targetgroup/shop-tg/<id>",
                    |value| is_target_group_arn(value, "shop-tg"),
                ),
                form(
                    "TargetGroupFullName",
                    "targetgroup/shop-tg/<id>",
                    |value| is_target_group_full_name(value, "shop-tg"),
                ),
                form(
                    "LoadBalancerArn",
                    "the load balancer's own ARN, loadbalancer/app/shop-alb/<id>",
                    |value| is_load_balancer_arn(value, "app", "shop-alb"),
                ),
                form(
                    "LoadBalancerDnsName",
                    "shop-alb-<id>.us-east-1.elb.amazonaws.com",
                    |value| is_dns_name(value, "shop-alb", false),
                ),
                form("LoadBalancerFullName", "app/shop-alb/<id>", |value| {
                    is_load_balancer_full_name(value, "app", "shop-alb")
                }),
                value(
                    "ContainerImage",
                    format!("{}:v1", repository_uri("shop-images")),
                ),
            ],
        },
        Case {
            // The same class with the variables flipped: the JSON spelling with long-form
            // intrinsics, an INTERNAL NETWORK load balancer (whose published ARN segment
            // is `net` and whose DNS name carries the `internal-` prefix), and no resource
            // named by the template — so each takes CloudFormation's generated
            // `<stack>-<logical id>-<unique id>` name, which is what the identifiers are
            // then built on.
            case: "json, an internal network load balancer and unnamed resources",
            stack_name: "warehouse",
            template: r#"{"Resources":{
                 "Balancer":{"Type":"AWS::ElasticLoadBalancingV2::LoadBalancer","Properties":{
                   "Name":"warehouse-internal",
                   "Scheme":"internal","Type":"network",
                   "Subnets":["subnet-0123456789abcdef0"]}},
                 "Group":{"Type":"AWS::ElasticLoadBalancingV2::TargetGroup","Properties":{
                   "Name":"warehouse-tg","Port":8080,"Protocol":"TCP","TargetType":"ip",
                   "VpcId":"vpc-0123456789abcdef0"}},
                 "Registry":{"Type":"AWS::ECR::Repository"},
                 "Task":{"Type":"AWS::ECS::TaskDefinition","Properties":{
                   "Cpu":"512","Memory":"1024","NetworkMode":"awsvpc",
                   "ContainerDefinitions":[{"Name":"web","Image":"public.ecr.aws/nginx:1",
                     "Essential":true}]}}},
               "Outputs":{
                 "LoadBalancerArn":{"Value":{"Ref":"Balancer"}},
                 "LoadBalancerDnsName":{"Value":{"Fn::GetAtt":["Balancer","DNSName"]}},
                 "LoadBalancerName":{"Value":{"Fn::GetAtt":["Balancer","LoadBalancerName"]}},
                 "TargetGroupArn":{"Value":{"Ref":"Group"}},
                 "TargetGroupName":{"Value":{"Fn::GetAtt":["Group","TargetGroupName"]}},
                 "RepositoryUri":{"Value":{"Fn::GetAtt":["Registry","RepositoryUri"]}},
                 "TaskDefinitionArn":{"Value":{"Fn::GetAtt":["Task","TaskDefinitionArn"]}}}}"#
                .to_string(),
            parameters: vec![],
            expected: vec![
                form(
                    "LoadBalancerArn",
                    "loadbalancer/net/warehouse-internal/<id>",
                    |value| is_load_balancer_arn(value, "net", "warehouse-internal"),
                ),
                form(
                    "LoadBalancerDnsName",
                    "internal-warehouse-internal-<id>.us-east-1.elb.amazonaws.com",
                    |value| is_dns_name(value, "warehouse-internal", true),
                ),
                value("LoadBalancerName", "warehouse-internal".to_string()),
                form(
                    "TargetGroupArn",
                    "targetgroup/warehouse-tg/<id>",
                    |value| is_target_group_arn(value, "warehouse-tg"),
                ),
                value("TargetGroupName", "warehouse-tg".to_string()),
                // Neither the registry nor the task definition is named by the template,
                // so both are built on CloudFormation's generated physical name — a
                // repository's lower-cased ("Repository names … can only contain lower
                // case letters"), a task definition's as its family.
                form(
                    "RepositoryUri",
                    "<account>.dkr.ecr.us-east-1.amazonaws.com/warehouse-registry-<id>",
                    |value| {
                        value
                            .strip_prefix(&format!(
                                "{ACCOUNT_ID}.dkr.ecr.{REGION}.amazonaws.com/warehouse-registry-"
                            ))
                            .is_some_and(|id| {
                                !id.is_empty() && id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
                            })
                    },
                ),
                form(
                    "TaskDefinitionArn",
                    "arn:aws:ecs:us-east-1:<account>:task-definition/warehouse-Task-<id>:1",
                    |value| {
                        value
                            .strip_prefix(&format!(
                                "arn:aws:ecs:{REGION}:{ACCOUNT_ID}:task-definition/warehouse-Task-"
                            ))
                            .is_some_and(|rest| rest.ends_with(":1"))
                    },
                ),
            ],
        },
    ]
}

/// Every declared output reports its resolved value.
#[tokio::test]
async fn a_container_platform_stacks_outputs_all_carry_their_resolved_value() {
    let router = router();
    for case in cases() {
        let (status, body) =
            create_stack(&router, case.stack_name, &case.template, &case.parameters).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected CreateStack to answer 200, got {status} {body}",
            case.case
        );
        wait_for_the_transition().await;

        let (status, xml) = describe_stack(&router, case.stack_name).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected DescribeStacks to answer 200, got {status} {xml}",
            case.case
        );
        assert_eq!(
            xml_text(&xml, "StackStatus").as_deref(),
            Some(status::CREATE_COMPLETE),
            "case {:?}: expected the stack to have completed; status reason {:?}",
            case.case,
            xml_text(&xml, "StackStatusReason")
        );

        let outputs = wire_outputs(&xml);
        assert_eq!(
            outputs.len(),
            case.expected.len(),
            "case {:?}: expected one output per declared output, got {outputs:?}",
            case.case
        );
        for (key, expected) in &case.expected {
            let reported = reported_value(case.case, &outputs, key);
            match expected {
                Expected::Value(wanted) => assert_eq!(
                    &reported, wanted,
                    "case {:?}: output {key:?}",
                    case.case
                ),
                Expected::Form { described, check } => assert!(
                    check(&reported),
                    "case {:?}: expected output {key:?} to be {described}, got {reported:?}",
                    case.case
                ),
            }
        }
    }
}

/// The `OutputValue` the stack reports for `key` — which must be there: an output
/// reported with its key and no value is what the issue's
/// `{o["OutputKey"]: o["OutputValue"]}` read raises `KeyError` on.
fn reported_value(case: &str, outputs: &[WireOutput], key: &str) -> String {
    let output = output_for(outputs, key)
        .unwrap_or_else(|| panic!("case {case:?}: no output named {key:?}, got {outputs:?}"));
    let value = output.value.clone().unwrap_or_else(|| {
        panic!(
            "case {case:?}: output {key:?} carries no OutputValue at all, got {output:?} \
             (the standard {{o[\"OutputKey\"]: o[\"OutputValue\"]}} read raises KeyError here)"
        )
    });
    assert!(
        !value.is_empty(),
        "case {case:?}: output {key:?} carries an empty OutputValue"
    );
    assert!(
        !value.contains("${"),
        "case {case:?}: output {key:?} still carries an unresolved expression: {value:?}"
    );
    value
}

/// Two resources of one stack never carry the same assigned id, and the same resource
/// reports the same one on every read — the two invariants an identifier the OWNING
/// SERVICE assigns has to keep for a client to address the resource by it.
#[tokio::test]
async fn an_assigned_identifier_is_unique_per_resource_and_stable() {
    let router = router();
    let case = "two load balancers and two target groups in one stack";
    let template = concat!(
        "Resources:\n",
        "  FirstBalancer:\n",
        "    Type: AWS::ElasticLoadBalancingV2::LoadBalancer\n",
        "    Properties:\n",
        "      Name: shared-name\n",
        "      Subnets: [subnet-0123456789abcdef0]\n",
        "  SecondBalancer:\n",
        "    Type: AWS::ElasticLoadBalancingV2::LoadBalancer\n",
        "    Properties:\n",
        "      Name: second-alb\n",
        "      Subnets: [subnet-0123456789abcdef0]\n",
        "  FirstGroup:\n",
        // The same NAME as the first load balancer: Elastic Load Balancing keeps the two
        // name spaces apart, and the ids it assigns them are still distinct.
        "    Type: AWS::ElasticLoadBalancingV2::TargetGroup\n",
        "    Properties:\n",
        "      Name: shared-name\n",
        "      Port: 80\n",
        "      Protocol: HTTP\n",
        "      VpcId: vpc-0123456789abcdef0\n",
        "  SecondGroup:\n",
        "    Type: AWS::ElasticLoadBalancingV2::TargetGroup\n",
        "    Properties:\n",
        "      Name: second-tg\n",
        "      Port: 80\n",
        "      Protocol: HTTP\n",
        "      VpcId: vpc-0123456789abcdef0\n",
        "Outputs:\n",
        "  FirstBalancer:\n",
        "    Value: !Ref FirstBalancer\n",
        "  SecondBalancer:\n",
        "    Value: !Ref SecondBalancer\n",
        "  FirstGroup:\n",
        "    Value: !Ref FirstGroup\n",
        "  SecondGroup:\n",
        "    Value: !Ref SecondGroup\n",
    );
    let (status, body) = create_stack(&router, "identity-stack", template, &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected CreateStack to answer 200, got {status} {body}"
    );
    wait_for_the_transition().await;

    let (_, first_read) = describe_stack(&router, "identity-stack").await;
    let reported: Vec<String> = ["FirstBalancer", "SecondBalancer", "FirstGroup", "SecondGroup"]
        .iter()
        .map(|key| reported_value(case, &wire_outputs(&first_read), key))
        .collect();
    let mut unique = reported.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(
        unique.len(),
        reported.len(),
        "case {case:?}: expected four distinct identifiers, got {reported:?}"
    );
    // The two that share a NAME still carry different assigned ids: the ARN's last
    // segment is that id, and the two name spaces are distinct.
    let balancer_id = reported[0].rsplit('/').next().unwrap_or_default();
    let group_id = reported[2].rsplit('/').next().unwrap_or_default();
    assert_ne!(
        balancer_id, group_id,
        "case {case:?}: expected the load balancer and the target group named \
         \"shared-name\" to carry different assigned ids, got {reported:?}"
    );

    let (_, second_read) = describe_stack(&router, "identity-stack").await;
    let again: Vec<String> = ["FirstBalancer", "SecondBalancer", "FirstGroup", "SecondGroup"]
        .iter()
        .map(|key| reported_value(case, &wire_outputs(&second_read), key))
        .collect();
    assert_eq!(
        again, reported,
        "case {case:?}: expected the same identifiers on a second read"
    );
}
