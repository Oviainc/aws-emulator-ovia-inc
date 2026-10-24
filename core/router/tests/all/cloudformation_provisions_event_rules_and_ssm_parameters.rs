// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack that declares an `AWS::Events::Rule` or an `AWS::SSM::Parameter`
//! really creates them in Amazon EventBridge and in AWS Systems Manager.
//!
//! The rule under test: **a stack that reaches a successful terminal state has created each
//! declared resource through the owning service's own create API, so every one of them is
//! readable through that service's own read — and deleting the stack deletes it there.**
//! Three crates have to agree for that to hold (CloudFormation makes the calls; EventBridge
//! and Systems Manager serve them), so the suite lives here rather than in any one of them.
//!
//! The rows differ along what the rule generalises over: whether the template NAMES the
//! resource or leaves the name to CloudFormation, whether the rule is driven by a
//! `ScheduleExpression` (`cron` and `rate`) or by an `EventPattern` document, which of the
//! three `State` values it declares, how many targets it carries (none, one, two) and
//! whether one of them is an ECS task with an awsvpc network configuration and a task tag
//! list, which parameter `Type` and `Tier` the parameter declares, whether it carries a tag
//! map, whether its name is a hierarchy path or a plain name, which intrinsic each output
//! is written with (`Ref`, `Fn::GetAtt`, `Fn::Sub`), whether the template is JSON or YAML,
//! and which Region the stack is deployed in.
//!
//! Every expected value is computed here from what the row declares, never read back out of
//! the answer being checked — except each resource's physical id, which is what the stack
//! reports for the resource and what the owning service is then asked to resolve.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The awsQuery form encoder and the XML element readers this crate writes ONCE
// (`tests/all/query_wire.rs`).
use super::query_wire::{elements, form_encode, opt_element};

/// Long enough for the stack's clock-owed span (`STATUS_TRANSITION_DELAY`, 500 ms) to come
/// due, so the stack reports the template it deployed.
const PAST_STACK_SPAN: Duration = Duration::from_millis(900);

/// The account id every ARN this emulator mints carries.
const ACCOUNT_ID: &str = "000000000000";

/// The emulator with the three services this suite exercises, mounted on ONE engine exactly
/// as `core/router/src/lib.rs` mounts them. Request authentication is off, as in every other
/// suite here; the in-process callback CloudFormation makes signs its request either way
/// (the credential scope is what routes it).
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudformation_cloudformation::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_eventbridge_awsevents::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(service_com_amazonaws_ssm_amazonssm::EmulatedService::new(
        engine,
    ));
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service` in `region`, which
/// is what routes the request and what each handler reads its Region from. The signature is
/// never checked: this router runs with request authentication off.
fn authorization(service: &str, region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{region}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// Call one CloudFormation operation over its awsQuery wire, signed for `region`.
async fn cloudformation(
    router: &MultiServiceRouter,
    region: &str,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2010-05-15");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("cloudformation.{region}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("cloudformation", region))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = String::from_utf8_lossy(response.body()).to_string();
    (status, payload)
}

/// Call one awsJson1_1 operation of a service addressed by its signing name and
/// `X-Amz-Target` prefix.
async fn json_rpc(
    router: &MultiServiceRouter,
    signing_name: &str,
    target_prefix: &str,
    region: &str,
    operation: &str,
    request: &Value,
) -> (u16, Value) {
    let http_request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_name}.{region}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{target_prefix}.{operation}"))
        .header("authorization", authorization(signing_name, region))
        .body(Bytes::from(request.to_string()))
        .expect("a well-formed request");
    let response = router.dispatch(http_request).await;
    let status = response.status().as_u16();
    let payload: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, payload)
}

/// Call one Amazon EventBridge operation (`sigv4Name: events`, service shape `AWSEvents`).
async fn events(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    request: &Value,
) -> (u16, Value) {
    json_rpc(router, "events", "AWSEvents", region, operation, request).await
}

/// Call one AWS Systems Manager operation (`sigv4Name: ssm`, service shape `AmazonSSM`).
async fn ssm(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    request: &Value,
) -> (u16, Value) {
    json_rpc(router, "ssm", "AmazonSSM", region, operation, request).await
}

/// A `DescribeStacks` answer's `Outputs`, as the key/value map a client reads it as.
fn outputs_of(xml: &str) -> BTreeMap<String, String> {
    elements(xml, "OutputKey")
        .into_iter()
        .zip(elements(xml, "OutputValue"))
        .collect()
}

/// A `DescribeStackResources` answer as `logical id -> physical resource id`: each
/// `member` carries exactly one of each, so the two document-ordered lists pair up.
fn physical_ids(xml: &str) -> BTreeMap<String, String> {
    elements(xml, "LogicalResourceId")
        .into_iter()
        .zip(elements(xml, "PhysicalResourceId"))
        .collect()
}

/// The error code an awsJson answer carries, as a client branches on it.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .map(|code| code.rsplit('#').next().unwrap_or(code).to_string())
        .unwrap_or_default()
}

/// One row of the table: a template, and what the rule says the two resources it declares
/// must then be in the services that own them.
struct Row {
    case: &'static str,
    /// The Region the stack is deployed in, and so the Region both resources live in.
    region: &'static str,
    /// The stack's name, unique across this suite.
    stack_name: &'static str,
    /// The template body, in the form the row writes it (JSON or YAML).
    template: fn() -> String,
    /// The rule's logical id, and the name the template declares for it — `None` where
    /// CloudFormation generates the name.
    rule_logical_id: &'static str,
    declared_rule_name: Option<&'static str>,
    /// What `DescribeRule` must report for the rule the row declares.
    expected_schedule: Option<&'static str>,
    /// The event pattern as a DOCUMENT; compared against the text EventBridge holds parsed
    /// back, because key order and spacing are the serialiser's choice.
    expected_pattern: Option<&'static str>,
    expected_state: &'static str,
    expected_description: Option<&'static str>,
    /// The ids of the targets `ListTargetsByRule` must report, in declaration order.
    expected_target_ids: &'static [&'static str],
    /// The awsvpc subnets the one ECS target declares, where the row declares one.
    expected_subnets: &'static [&'static str],
    /// The task tags that target's `TagList` declares.
    expected_task_tags: &'static [(&'static str, &'static str)],
    /// The parameter's logical id, and the name the template declares for it.
    parameter_logical_id: &'static str,
    declared_parameter_name: Option<&'static str>,
    expected_parameter_type: &'static str,
    expected_parameter_value: &'static str,
}

/// The ECS cluster a scheduled rule's target invokes, in `region`.
fn target_cluster_arn(region: &str) -> String {
    format!("arn:aws:ecs:{region}:{ACCOUNT_ID}:cluster/a-cluster")
}

/// The task definition that target runs, in `region`.
fn task_definition_arn(region: &str) -> String {
    format!("arn:aws:ecs:{region}:{ACCOUNT_ID}:task-definition/a-task:1")
}

/// The role the target assumes. IAM is global, so the ARN carries no Region.
const TARGET_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/a-target-role";

const ROWS: &[Row] = &[
    Row {
        case: "a named cron rule with an ECS RunTask target, and a hierarchy parameter",
        region: "us-west-2",
        stack_name: "events-ssm-cron",
        template: || {
            json!({
                "AWSTemplateFormatVersion": "2010-09-09",
                "Description": "a scheduled ECS task and a bootstrap parameter",
                "Resources": {
                    "MonthlyRule": {
                        "Type": "AWS::Events::Rule",
                        "Properties": {
                            "Name": "events-ssm-cron-monthly",
                            "Description": "runs a task once a month",
                            "ScheduleExpression": "cron(05 23 27 * ? *)",
                            "State": "ENABLED",
                            "Targets": [{
                                "Id": "EcsTask",
                                "Arn": target_cluster_arn("us-west-2"),
                                "RoleArn": TARGET_ROLE_ARN,
                                "Input": "{\"detail\":\"scheduled\"}",
                                "EcsParameters": {
                                    "TaskDefinitionArn": task_definition_arn("us-west-2"),
                                    "TaskCount": 1,
                                    "LaunchType": "FARGATE",
                                    "NetworkConfiguration": {
                                        "AwsVpcConfiguration": {
                                            "Subnets": ["subnet-11111111", "subnet-22222222"],
                                            "AssignPublicIp": "ENABLED"
                                        }
                                    },
                                    "TagList": [{ "Key": "Owner", "Value": "platform" }]
                                }
                            }]
                        }
                    },
                    "BootstrapVersion": {
                        "Type": "AWS::SSM::Parameter",
                        "Properties": {
                            "Name": "/cdk-bootstrap/hnb659fds/version",
                            "Type": "String",
                            "Value": "28",
                            "Description": "the bootstrap stack version",
                            "Tags": { "Owner": "platform" }
                        }
                    }
                },
                "Outputs": {
                    "RuleName": { "Value": { "Ref": "MonthlyRule" } },
                    "RuleArn": { "Value": { "Fn::GetAtt": ["MonthlyRule", "Arn"] } },
                    "ParameterName": { "Value": { "Ref": "BootstrapVersion" } },
                    "ParameterType": {
                        "Value": { "Fn::GetAtt": ["BootstrapVersion", "Type"] }
                    },
                    "ParameterValue": {
                        "Value": { "Fn::GetAtt": ["BootstrapVersion", "Value"] }
                    }
                }
            })
            .to_string()
        },
        rule_logical_id: "MonthlyRule",
        declared_rule_name: Some("events-ssm-cron-monthly"),
        expected_schedule: Some("cron(05 23 27 * ? *)"),
        expected_pattern: None,
        expected_state: "ENABLED",
        expected_description: Some("runs a task once a month"),
        expected_target_ids: &["EcsTask"],
        expected_subnets: &["subnet-11111111", "subnet-22222222"],
        expected_task_tags: &[("Owner", "platform")],
        parameter_logical_id: "BootstrapVersion",
        declared_parameter_name: Some("/cdk-bootstrap/hnb659fds/version"),
        expected_parameter_type: "String",
        expected_parameter_value: "28",
    },
    Row {
        case: "a YAML rate rule with no State and no targets, and a StringList parameter, \
               both named by CloudFormation",
        region: "eu-west-1",
        stack_name: "events-ssm-rate",
        template: || {
            [
                "Resources:",
                "  Heartbeat:",
                "    Type: AWS::Events::Rule",
                "    Properties:",
                "      ScheduleExpression: rate(5 minutes)",
                "  Settings:",
                "    Type: AWS::SSM::Parameter",
                "    Properties:",
                "      Type: StringList",
                "      Value: alpha,beta,gamma",
                "Outputs:",
                "  RuleName:",
                "    Value: !Ref Heartbeat",
                "  RuleArn:",
                "    Value: !GetAtt Heartbeat.Arn",
                "  ParameterName:",
                "    Value: !Ref Settings",
                "  ParameterType:",
                "    Value: !GetAtt Settings.Type",
                "  ParameterValue:",
                "    Value: !GetAtt Settings.Value",
                "",
            ]
            .join("\n")
        },
        rule_logical_id: "Heartbeat",
        declared_rule_name: None,
        expected_schedule: Some("rate(5 minutes)"),
        expected_pattern: None,
        // "Rules are enabled by default, or based on value of the state" (`PutRule`): a
        // declaration naming no State gets EventBridge's own default, not a value this
        // emulator invents.
        expected_state: "ENABLED",
        expected_description: None,
        expected_target_ids: &[],
        expected_subnets: &[],
        expected_task_tags: &[],
        parameter_logical_id: "Settings",
        declared_parameter_name: None,
        expected_parameter_type: "StringList",
        expected_parameter_value: "alpha,beta,gamma",
    },
    Row {
        case: "a disabled event-pattern rule with two targets, and a plain-named parameter",
        region: "us-east-1",
        stack_name: "events-ssm-pattern",
        template: || {
            json!({
                "Resources": {
                    "StateChanges": {
                        "Type": "AWS::Events::Rule",
                        "Properties": {
                            "EventPattern": {
                                "source": ["aws.ecs"],
                                "detail-type": ["ECS Task State Change"]
                            },
                            "State": "DISABLED",
                            "Targets": [
                                {
                                    "Id": "First",
                                    "Arn": target_cluster_arn("us-east-1"),
                                    "InputPath": "$.detail"
                                },
                                {
                                    "Id": "Second",
                                    "Arn": target_cluster_arn("us-east-1"),
                                    "RetryPolicy": { "MaximumRetryAttempts": 2 }
                                }
                            ]
                        }
                    },
                    "Threshold": {
                        "Type": "AWS::SSM::Parameter",
                        "Properties": {
                            "Name": "plain-threshold",
                            "Type": "String",
                            "Value": "7",
                            "AllowedPattern": "^[0-9]+$",
                            "Tier": "Standard"
                        }
                    }
                },
                "Outputs": {
                    "RuleName": { "Value": { "Ref": "StateChanges" } },
                    "RuleArn": { "Value": { "Fn::GetAtt": ["StateChanges", "Arn"] } },
                    "ParameterName": { "Value": { "Ref": "Threshold" } },
                    "ParameterType": { "Value": { "Fn::GetAtt": ["Threshold", "Type"] } },
                    "ParameterValue": { "Value": { "Fn::GetAtt": ["Threshold", "Value"] } }
                }
            })
            .to_string()
        },
        rule_logical_id: "StateChanges",
        declared_rule_name: None,
        expected_schedule: None,
        expected_pattern: Some(
            r#"{"source":["aws.ecs"],"detail-type":["ECS Task State Change"]}"#,
        ),
        expected_state: "DISABLED",
        expected_description: None,
        expected_target_ids: &["First", "Second"],
        expected_subnets: &[],
        expected_task_tags: &[],
        parameter_logical_id: "Threshold",
        declared_parameter_name: Some("plain-threshold"),
        expected_parameter_type: "String",
        expected_parameter_value: "7",
    },
    Row {
        case: "a rule in the third State the enum admits, with an Fn::Sub output",
        region: "ap-northeast-1",
        stack_name: "events-ssm-cloudtrail",
        template: || {
            json!({
                "Resources": {
                    "Management": {
                        "Type": "AWS::Events::Rule",
                        "Properties": {
                            "Name": "events-ssm-management",
                            "EventPattern": { "source": ["aws.cloudtrail"] },
                            "State": "ENABLED_WITH_ALL_CLOUDTRAIL_MANAGEMENT_EVENTS"
                        }
                    },
                    "Audit": {
                        "Type": "AWS::SSM::Parameter",
                        "Properties": {
                            "Name": "/audit/enabled",
                            "Type": "String",
                            "Value": "true"
                        }
                    }
                },
                "Outputs": {
                    "RuleName": { "Value": { "Ref": "Management" } },
                    "RuleArn": { "Value": { "Fn::GetAtt": ["Management", "Arn"] } },
                    "ParameterName": {
                        "Value": {
                            "Fn::Sub": ["${Name}", { "Name": { "Ref": "Audit" } }]
                        }
                    },
                    "ParameterType": { "Value": { "Fn::GetAtt": ["Audit", "Type"] } },
                    "ParameterValue": { "Value": { "Fn::GetAtt": ["Audit", "Value"] } }
                }
            })
            .to_string()
        },
        rule_logical_id: "Management",
        declared_rule_name: Some("events-ssm-management"),
        expected_schedule: None,
        expected_pattern: Some(r#"{"source":["aws.cloudtrail"]}"#),
        expected_state: "ENABLED_WITH_ALL_CLOUDTRAIL_MANAGEMENT_EVENTS",
        expected_description: None,
        expected_target_ids: &[],
        expected_subnets: &[],
        expected_task_tags: &[],
        parameter_logical_id: "Audit",
        declared_parameter_name: Some("/audit/enabled"),
        expected_parameter_type: "String",
        expected_parameter_value: "true",
    },
];

/// Every row: the stack completes, the rule the template declared is one
/// `events DescribeRule` answers with on the `default` bus, the parameter is one
/// `ssm GetParameter` answers with, and deleting the stack deletes both there.
#[tokio::test]
async fn a_stack_provisions_the_event_rules_and_parameters_its_template_declares() {
    let router = build_router();

    for row in ROWS {
        let case = row.case;
        let region = row.region;
        let stack_name = row.stack_name;
        let template = (row.template)();

        let (status, body) = cloudformation(
            &router,
            region,
            "CreateStack",
            &[("StackName", stack_name), ("TemplateBody", &template)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
        );

        tokio::time::sleep(PAST_STACK_SPAN).await;
        let (status, described) =
            cloudformation(&router, region, "DescribeStacks", &[("StackName", stack_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeStacks, got {status} with body \
             {described}"
        );
        assert_eq!(
            opt_element(&described, "StackStatus").as_deref(),
            Some("CREATE_COMPLETE"),
            "case {case:?}: expected the stack to reach CREATE_COMPLETE, got {described}"
        );

        let (status, resources) = cloudformation(
            &router,
            region,
            "DescribeStackResources",
            &[("StackName", stack_name)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeStackResources, got {status} with \
             body {resources}"
        );
        let identifiers = physical_ids(&resources);
        let rule_name = identifiers
            .get(row.rule_logical_id)
            .cloned()
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: no physical id for {:?} in {resources}",
                    row.rule_logical_id
                )
            });
        let parameter_name = identifiers
            .get(row.parameter_logical_id)
            .cloned()
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: no physical id for {:?} in {resources}",
                    row.parameter_logical_id
                )
            });

        // A declared name is the physical id; a name CloudFormation generated is the
        // `<stack name>-<logical id>-<unique id>` form, inside the owning service's bound.
        for (what, declared, actual, bound) in [
            ("the rule", row.declared_rule_name, &rule_name, 64_usize),
            (
                "the parameter",
                row.declared_parameter_name,
                &parameter_name,
                2048,
            ),
        ] {
            match declared {
                Some(declared) => assert_eq!(
                    actual, declared,
                    "case {case:?}: expected {what}'s physical id to be the declared name \
                     {declared:?}, got {actual:?}"
                ),
                None => {
                    assert!(
                        actual.starts_with(&format!("{stack_name}-")),
                        "case {case:?}: expected {what}'s generated name to start with the \
                         stack name, got {actual:?}"
                    );
                    assert!(
                        actual.chars().count() <= bound,
                        "case {case:?}: expected {what}'s generated name within {bound} \
                         characters, got {actual:?}"
                    );
                }
            }
        }

        // EventBridge itself resolves the rule the stack reported, on the `default` bus.
        let (status, rule) = events(
            &router,
            region,
            "DescribeRule",
            &json!({ "Name": rule_name }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeRule on the name the stack \
             reported, got {status} with body {rule}"
        );
        assert_eq!(
            rule.get("Name").and_then(Value::as_str),
            Some(rule_name.as_str()),
            "case {case:?}: expected DescribeRule to report the rule's name, got {rule}"
        );
        assert_eq!(
            rule.get("EventBusName").and_then(Value::as_str),
            Some("default"),
            "case {case:?}: expected the rule on the default bus, got {rule}"
        );
        assert_eq!(
            rule.get("ScheduleExpression").and_then(Value::as_str),
            row.expected_schedule,
            "case {case:?}: expected ScheduleExpression {:?}, got {rule}",
            row.expected_schedule
        );
        assert_eq!(
            rule.get("State").and_then(Value::as_str),
            Some(row.expected_state),
            "case {case:?}: expected State {:?}, got {rule}",
            row.expected_state
        );
        assert_eq!(
            rule.get("Description").and_then(Value::as_str),
            row.expected_description,
            "case {case:?}: expected Description {:?}, got {rule}",
            row.expected_description
        );
        match row.expected_pattern {
            Some(expected) => {
                let expected: Value =
                    serde_json::from_str(expected).expect("a well-formed expected pattern");
                let reported: Value = rule
                    .get("EventPattern")
                    .and_then(Value::as_str)
                    .map(|text| serde_json::from_str(text).unwrap_or(Value::Null))
                    .unwrap_or(Value::Null);
                assert_eq!(
                    reported, expected,
                    "case {case:?}: expected the declared event pattern document \
                     {expected:?}, got {reported:?} in {rule}"
                );
            }
            None => assert!(
                rule.get("EventPattern").is_none(),
                "case {case:?}: expected no EventPattern, got {rule}"
            ),
        }

        // The ARN the stack publishes for the rule is the ARN EventBridge holds for it, and
        // the form that service mints for a rule on the default bus.
        let outputs = outputs_of(&described);
        let rule_arn = rule
            .get("Arn")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            rule_arn,
            format!("arn:aws:events:{region}:{ACCOUNT_ID}:rule/{rule_name}"),
            "case {case:?}: expected the rule ARN EventBridge mints, got {rule_arn:?}"
        );
        for (key, expected) in [
            ("RuleName", &rule_name),
            ("RuleArn", &rule_arn),
            ("ParameterName", &parameter_name),
        ] {
            assert_eq!(
                outputs.get(key),
                Some(expected),
                "case {case:?}: expected output {key:?} to be {expected:?}, got {:?}",
                outputs.get(key)
            );
        }
        for (key, expected) in [
            ("ParameterType", row.expected_parameter_type),
            ("ParameterValue", row.expected_parameter_value),
        ] {
            assert_eq!(
                outputs.get(key).map(String::as_str),
                Some(expected),
                "case {case:?}: expected output {key:?} to be {expected:?}, got {:?}",
                outputs.get(key)
            );
        }

        // The declared targets are installed on that rule, in declaration order.
        let (status, listed) = events(
            &router,
            region,
            "ListTargetsByRule",
            &json!({ "Rule": rule_name }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from ListTargetsByRule, got {status} with body \
             {listed}"
        );
        let installed = listed
            .get("Targets")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let installed_ids: Vec<&str> = installed
            .iter()
            .filter_map(|target| target.get("Id").and_then(Value::as_str))
            .collect();
        assert_eq!(
            installed_ids, row.expected_target_ids,
            "case {case:?}: expected the declared target ids {:?}, got {listed}",
            row.expected_target_ids
        );
        if !row.expected_subnets.is_empty() {
            let awsvpc = installed
                .first()
                .and_then(|target| target.get("EcsParameters"))
                .and_then(|ecs| ecs.get("NetworkConfiguration"))
                .and_then(|network| network.get("awsvpcConfiguration"))
                .cloned()
                .unwrap_or(Value::Null);
            let subnets: Vec<&str> = awsvpc
                .get("Subnets")
                .and_then(Value::as_array)
                .map(|subnets| subnets.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            assert_eq!(
                subnets, row.expected_subnets,
                "case {case:?}: expected the declared awsvpc subnets {:?}, got {listed}",
                row.expected_subnets
            );
            let task_tags: Vec<(&str, &str)> = installed
                .first()
                .and_then(|target| target.get("EcsParameters"))
                .and_then(|ecs| ecs.get("Tags"))
                .and_then(Value::as_array)
                .map(|tags| {
                    tags.iter()
                        .filter_map(|tag| {
                            Some((
                                tag.get("Key").and_then(Value::as_str)?,
                                tag.get("Value").and_then(Value::as_str)?,
                            ))
                        })
                        .collect()
                })
                .unwrap_or_default();
            assert_eq!(
                task_tags, row.expected_task_tags,
                "case {case:?}: expected the declared task tag list {:?}, got {listed}",
                row.expected_task_tags
            );
        }

        // And Systems Manager resolves the parameter the stack reported.
        let (status, read) = ssm(
            &router,
            region,
            "GetParameter",
            &json!({ "Name": parameter_name }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from GetParameter on the name the stack \
             reported, got {status} with body {read}"
        );
        let parameter = read.get("Parameter").cloned().unwrap_or(Value::Null);
        for (member, expected) in [
            ("Type", row.expected_parameter_type),
            ("Value", row.expected_parameter_value),
        ] {
            assert_eq!(
                parameter.get(member).and_then(Value::as_str),
                Some(expected),
                "case {case:?}: expected the parameter's {member} to be {expected:?}, got \
                 {parameter}"
            );
        }
        assert_eq!(
            parameter.get("Version").and_then(Value::as_i64),
            Some(1),
            "case {case:?}: expected the create to leave version 1, got {parameter}"
        );

        // Deleting the stack deletes both resources in the services that hold them.
        let (status, body) =
            cloudformation(&router, region, "DeleteStack", &[("StackName", stack_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DeleteStack, got {status} with body {body}"
        );
        let (status, gone) = events(
            &router,
            region,
            "DescribeRule",
            &json!({ "Name": rule_name }),
        )
        .await;
        assert_eq!(
            (status, error_code(&gone).as_str()),
            (400, "ResourceNotFoundException"),
            "case {case:?}: expected DescribeRule to answer 400 ResourceNotFoundException \
             once the stack deleted the rule, got {status} with body {gone}"
        );
        let (status, gone) = ssm(
            &router,
            region,
            "GetParameter",
            &json!({ "Name": parameter_name }),
        )
        .await;
        assert_eq!(
            (status, error_code(&gone).as_str()),
            (400, "ParameterNotFound"),
            "case {case:?}: expected GetParameter to answer 400 ParameterNotFound once the \
             stack deleted the parameter, got {status} with body {gone}"
        );
    }
}

/// A resource an owning service — or, for an UPSERT create, the probe that stands in for
/// its refusal — will not create is not reported as created: a second stack declaring a
/// name the owning service already holds fails, naming the resource, and what the first
/// stack created is untouched.
#[tokio::test]
async fn a_resource_whose_name_is_already_held_fails_the_stack() {
    let router = build_router();
    let region = "us-east-2";
    let parameter_name = "shared-parameter";
    let rule_name = "shared-rule";
    let template = json!({
        "Resources": {
            "Param": {
                "Type": "AWS::SSM::Parameter",
                "Properties": { "Name": parameter_name, "Type": "String", "Value": "first" }
            }
        }
    })
    .to_string();
    let rule_template = json!({
        "Resources": {
            "Rule": {
                "Type": "AWS::Events::Rule",
                "Properties": {
                    "Name": rule_name,
                    "ScheduleExpression": "rate(5 minutes)",
                    "State": "ENABLED"
                }
            }
        }
    })
    .to_string();

    // (the case, the two stack names, the template both deploy, the logical id the failure
    // must name)
    let cases: [(&str, [&str; 2], &str, &str); 2] = [
        (
            "two stacks declaring one parameter name",
            ["parameter-first", "parameter-second"],
            &template,
            "Param",
        ),
        (
            "two stacks declaring one rule name",
            ["rule-first", "rule-second"],
            &rule_template,
            "Rule",
        ),
    ];

    for (case, stack_names, body, logical_id) in cases {
        for stack_name in stack_names {
            let (status, answered) = cloudformation(
                &router,
                region,
                "CreateStack",
                &[("StackName", stack_name), ("TemplateBody", body)],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 from CreateStack for {stack_name:?}, got \
                 {status} with body {answered}"
            );
        }
        tokio::time::sleep(PAST_STACK_SPAN).await;

        let (_, first) =
            cloudformation(&router, region, "DescribeStacks", &[("StackName", stack_names[0])])
                .await;
        assert_eq!(
            opt_element(&first, "StackStatus").as_deref(),
            Some("CREATE_COMPLETE"),
            "case {case:?}: expected the first stack to complete, got {first}"
        );

        let (_, second) =
            cloudformation(&router, region, "DescribeStacks", &[("StackName", stack_names[1])])
                .await;
        assert_eq!(
            opt_element(&second, "StackStatus").as_deref(),
            Some("CREATE_FAILED"),
            "case {case:?}: expected the second stack to fail rather than report a resource \
             nothing created, got {second}"
        );
        let reason = opt_element(&second, "StackStatusReason").unwrap_or_default();
        assert!(
            reason.contains(logical_id),
            "case {case:?}: expected the failure to name the resource that failed, got \
             {reason:?}"
        );
    }

    // What the first stacks created is exactly as they left it.
    let (status, read) = ssm(
        &router,
        region,
        "GetParameter",
        &json!({ "Name": parameter_name }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"the first stack's parameter\": expected it to still resolve, got {status} with \
         body {read}"
    );
    assert_eq!(
        read.get("Parameter")
            .and_then(|parameter| parameter.get("Value"))
            .and_then(Value::as_str),
        Some("first"),
        "case \"the first stack's parameter\": expected its own value, got {read}"
    );
    let (status, rule) = events(
        &router,
        region,
        "DescribeRule",
        &json!({ "Name": rule_name }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"the first stack's rule\": expected it to still resolve, got {status} with body \
         {rule}"
    );
    assert_eq!(
        rule.get("ScheduleExpression").and_then(Value::as_str),
        Some("rate(5 minutes)"),
        "case \"the first stack's rule\": expected its own schedule, got {rule}"
    );
}

/// An update that changes what the template declares is APPLIED through each owning
/// service's own update path: the parameter gets a new version carrying the new value, the
/// rule's schedule is replaced, and a target the template no longer declares is removed.
#[tokio::test]
async fn an_update_applies_the_changed_declaration_in_place() {
    let router = build_router();
    let case = "a redeploy that changes the rule's schedule, its targets and the value";
    let region = "ca-central-1";
    let stack_name = "events-ssm-update";
    let rule_name = "events-ssm-update-rule";
    let parameter_name = "/update/version";

    // (the targets the deployment declares, the schedule, the parameter's value)
    let template = |targets: &[&str], schedule: &str, value: &str| {
        let targets: Vec<Value> = targets
            .iter()
            .map(|id| {
                json!({
                    "Id": id,
                    "Arn": target_cluster_arn(region)
                })
            })
            .collect();
        json!({
            "Resources": {
                "Rule": {
                    "Type": "AWS::Events::Rule",
                    "Properties": {
                        "Name": rule_name,
                        "ScheduleExpression": schedule,
                        "State": "ENABLED",
                        "Targets": targets
                    }
                },
                "Version": {
                    "Type": "AWS::SSM::Parameter",
                    "Properties": {
                        "Name": parameter_name,
                        "Type": "String",
                        "Value": value
                    }
                }
            }
        })
        .to_string()
    };

    let created = template(&["Keep", "Drop"], "rate(5 minutes)", "28");
    let (status, body) = cloudformation(
        &router,
        region,
        "CreateStack",
        &[("StackName", stack_name), ("TemplateBody", &created)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
    );
    tokio::time::sleep(PAST_STACK_SPAN).await;

    let updated = template(&["Keep"], "rate(10 minutes)", "29");
    let (status, body) = cloudformation(
        &router,
        region,
        "UpdateStack",
        &[("StackName", stack_name), ("TemplateBody", &updated)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from UpdateStack, got {status} with body {body}"
    );
    tokio::time::sleep(PAST_STACK_SPAN).await;

    let (_, described) =
        cloudformation(&router, region, "DescribeStacks", &[("StackName", stack_name)]).await;
    assert_eq!(
        opt_element(&described, "StackStatus").as_deref(),
        Some("UPDATE_COMPLETE"),
        "case {case:?}: expected the update to complete, got {described}"
    );

    let (status, rule) = events(
        &router,
        region,
        "DescribeRule",
        &json!({ "Name": rule_name }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeRule, got {status} with body {rule}"
    );
    assert_eq!(
        rule.get("ScheduleExpression").and_then(Value::as_str),
        Some("rate(10 minutes)"),
        "case {case:?}: expected the declared schedule to be in force, got {rule}"
    );

    let (status, listed) = events(
        &router,
        region,
        "ListTargetsByRule",
        &json!({ "Rule": rule_name }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from ListTargetsByRule, got {status} with body \
         {listed}"
    );
    let installed_ids: Vec<&str> = listed
        .get("Targets")
        .and_then(Value::as_array)
        .map(|targets| {
            targets
                .iter()
                .filter_map(|target| target.get("Id").and_then(Value::as_str))
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        installed_ids,
        vec!["Keep"],
        "case {case:?}: expected only the target the update still declares, got {listed}"
    );

    let (status, read) = ssm(
        &router,
        region,
        "GetParameter",
        &json!({ "Name": parameter_name }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from GetParameter, got {status} with body {read}"
    );
    let parameter = read.get("Parameter").cloned().unwrap_or(Value::Null);
    assert_eq!(
        parameter.get("Value").and_then(Value::as_str),
        Some("29"),
        "case {case:?}: expected the updated value, got {parameter}"
    );
    assert_eq!(
        parameter.get("Version").and_then(Value::as_i64),
        Some(2),
        "case {case:?}: expected the overwrite to append a version, got {parameter}"
    );
}
