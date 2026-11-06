// SPDX-License-Identifier: CC0-1.0

//! Template parameters whose VALUE lives in AWS Systems Manager Parameter Store, end to end
//! over the awsQuery wire.
//!
//! RULE: a parameter the template declares with a type of the form
//! `AWS::SSM::Parameter::Value<<type>>` carries the NAME of a Parameter Store parameter
//! rather than a value — "the parameter value is the name of a Systems Manager parameter,
//! and the value that CloudFormation uses is the value of that parameter" — so the
//! deployment reads that parameter and every expression the template builds from the
//! parameter (a `Ref`, an `Fn::Sub`, a resource property) carries the VALUE the store holds;
//! the type inside the angle brackets is what that value is read AS, so a `List<…>` or a
//! `CommaDelimitedList` inside them makes it a list to every function that reads one. The
//! stack reports the name as the parameter's `ParameterValue` and the value as its
//! `ResolvedValue`. A name the store does not answer for leaves the name itself the value in
//! force: one unanswered read is no evidence about the parameter.
//!
//! The rows differ along what the rule generalises over: the inner type (a single string, a
//! list of strings, a comma-delimited list, an AWS-specific type), whether the name comes
//! from the template's `Default` or from the request, the template spelling (YAML and JSON),
//! the type that names a parameter KEY without resolving it, and a name the store does not
//! answer for. The store is written through Parameter Store's OWN wire on the same engine, so
//! what the deployment reads is what `PutParameter` wrote.

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cloudformation_cloudformation::state::status;

use crate::dynamic_references::{put_parameter, ssm};
use crate::change_set_lifecycle::{create_creating_change_set, execute_change_set};
use crate::harness::{
    call, create_stack, member, router, wait_for_the_transition, xml_blocks, xml_text, xml_texts,
    REGION,
};
use crate::stack_lifecycle::{describe_stack, ACCOUNT_ID};
use crate::stack_outputs::{output_for, wire_outputs};

/// One parameter the stack reports, as the response carries it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct WireParameter {
    key: Option<String>,
    value: Option<String>,
    resolved_value: Option<String>,
}

/// Every `Parameter` the first `Stack` of a `DescribeStacks` response reports, in wire
/// order.
///
/// Read member by member — inside the `Parameters` block alone — so a parameter's key stays
/// paired with its own value and with its own `ResolvedValue`, and a parameter that carries
/// NO `ResolvedValue` is visible as such instead of borrowing the next one's.
fn wire_parameters(xml: &str) -> Vec<WireParameter> {
    let Some(block) = xml_blocks(xml, "Parameters").into_iter().next() else {
        return Vec::new();
    };
    xml_blocks(&block, "member")
        .into_iter()
        .map(|entry| WireParameter {
            key: xml_text(&entry, "ParameterKey"),
            value: xml_text(&entry, "ParameterValue"),
            resolved_value: xml_text(&entry, "ResolvedValue"),
        })
        .collect()
}

/// The `Parameters` members of a `CreateStack`/`UpdateStack` request, as an SDK sends them.
fn request_parameters(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .enumerate()
        .flat_map(|(index, (key, value))| {
            let position = index + 1;
            vec![
                member(
                    &format!("Parameters.member.{position}.ParameterKey"),
                    key,
                ),
                member(
                    &format!("Parameters.member.{position}.ParameterValue"),
                    value,
                ),
            ]
        })
        .collect()
}

/// One parameter to write into Parameter Store before a row deploys: its name, the type it
/// is held under and its value.
struct Held {
    name: &'static str,
    parameter_type: &'static str,
    value: &'static str,
}

/// One row: a template, the parameters the request supplies, what each output must report
/// and what the stack must report for each of its own parameters.
struct Case {
    case: &'static str,
    stack_name: &'static str,
    held: Vec<Held>,
    template: String,
    parameters: Vec<(&'static str, &'static str)>,
    /// `(OutputKey, OutputValue)`.
    outputs: Vec<(&'static str, String)>,
    /// `(ParameterKey, ParameterValue, ResolvedValue)`.
    reported: Vec<(&'static str, &'static str, Option<&'static str>)>,
}

fn sns_arn(name: &str) -> String {
    format!("arn:aws:sns:{REGION}:{ACCOUNT_ID}:{name}")
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            case: "a String parameter, named by the template's own Default",
            stack_name: "ssm-typed-string",
            held: vec![Held {
                name: "/probe/bootstrap-version",
                parameter_type: "String",
                value: "21",
            }],
            // YAML, short-form intrinsics. The topic's NAME is built from the parameter, so
            // the resource the stack really creates in Amazon SNS is named from the value
            // the store holds rather than from the name that addressed it.
            template: r#"Parameters:
  BootstrapVersion:
    Type: AWS::SSM::Parameter::Value<String>
    Default: /probe/bootstrap-version
Resources:
  Notice:
    Type: AWS::SNS::Topic
    Properties:
      TopicName: !Sub "probe-${BootstrapVersion}"
Outputs:
  ResolvedBootstrapVersion:
    Value: !Ref BootstrapVersion
  Composed:
    Value: !Sub "v${BootstrapVersion}"
  NoticeArn:
    Value: !Ref Notice
"#
            .to_string(),
            parameters: vec![],
            outputs: vec![
                ("ResolvedBootstrapVersion", "21".to_string()),
                ("Composed", "v21".to_string()),
                ("NoticeArn", sns_arn("probe-21")),
            ],
            reported: vec![(
                "BootstrapVersion",
                "/probe/bootstrap-version",
                Some("21"),
            )],
        },
        Case {
            case: "a String parameter, named by the request instead of the Default",
            stack_name: "ssm-typed-supplied",
            held: vec![
                Held {
                    name: "/probe/default-version",
                    parameter_type: "String",
                    value: "7",
                },
                Held {
                    name: "/probe/requested-version",
                    parameter_type: "String",
                    value: "42",
                },
            ],
            template: r#"Parameters:
  Version:
    Type: AWS::SSM::Parameter::Value<String>
    Default: /probe/default-version
Resources:
  Marker:
    Type: AWS::CloudFormation::WaitConditionHandle
Outputs:
  Resolved:
    Value: !Ref Version
"#
            .to_string(),
            parameters: vec![("Version", "/probe/requested-version")],
            // The request's own name wins over the declared Default, and the value is that
            // parameter's.
            outputs: vec![("Resolved", "42".to_string())],
            reported: vec![("Version", "/probe/requested-version", Some("42"))],
        },
        Case {
            case: "a List<String> inner type is a list to every function that reads it",
            stack_name: "ssm-typed-list",
            held: vec![Held {
                name: "/probe/subnets",
                parameter_type: "StringList",
                value: "subnet-00000000000000001,subnet-00000000000000002",
            }],
            // JSON, long-form intrinsics.
            template: r#"{"Parameters":{
                   "Subnets":{"Type":"AWS::SSM::Parameter::Value<List<String>>",
                              "Default":"/probe/subnets"}},
                 "Resources":{"Marker":{"Type":"AWS::CloudFormation::WaitConditionHandle"}},
                 "Outputs":{
                   "First":{"Value":{"Fn::Select":["0",{"Ref":"Subnets"}]}},
                   "Second":{"Value":{"Fn::Select":["1",{"Ref":"Subnets"}]}},
                   "Joined":{"Value":{"Fn::Join":["|",{"Ref":"Subnets"}]}}}}"#
                .to_string(),
            parameters: vec![],
            outputs: vec![
                ("First", "subnet-00000000000000001".to_string()),
                ("Second", "subnet-00000000000000002".to_string()),
                (
                    "Joined",
                    "subnet-00000000000000001|subnet-00000000000000002".to_string(),
                ),
            ],
            reported: vec![(
                "Subnets",
                "/probe/subnets",
                Some("subnet-00000000000000001,subnet-00000000000000002"),
            )],
        },
        Case {
            case: "a CommaDelimitedList inner type is a list too",
            stack_name: "ssm-typed-comma-list",
            held: vec![Held {
                name: "/probe/stages",
                parameter_type: "StringList",
                // "An array of literal strings that are separated by commas": the spaces
                // around an element are not part of it.
                value: "dev, prod",
            }],
            template: r#"{"Parameters":{
                   "Stages":{"Type":"AWS::SSM::Parameter::Value<CommaDelimitedList>",
                             "Default":"/probe/stages"}},
                 "Resources":{"Marker":{"Type":"AWS::CloudFormation::WaitConditionHandle"}},
                 "Outputs":{
                   "Second":{"Value":{"Fn::Select":["1",{"Ref":"Stages"}]}},
                   "Joined":{"Value":{"Fn::Join":["+",{"Ref":"Stages"}]}}}}"#
                .to_string(),
            parameters: vec![],
            outputs: vec![
                ("Second", "prod".to_string()),
                ("Joined", "dev+prod".to_string()),
            ],
            reported: vec![("Stages", "/probe/stages", Some("dev, prod"))],
        },
        Case {
            case: "an AWS-specific inner type resolves to one value",
            stack_name: "ssm-typed-aws-specific",
            held: vec![Held {
                name: "/probe/ami",
                parameter_type: "String",
                value: "ami-00000000000000001",
            }],
            template: r#"Parameters:
  ImageId:
    Type: AWS::SSM::Parameter::Value<AWS::EC2::Image::Id>
    Default: /probe/ami
Resources:
  Marker:
    Type: AWS::CloudFormation::WaitConditionHandle
Outputs:
  Resolved:
    Value: !Ref ImageId
"#
            .to_string(),
            parameters: vec![],
            outputs: vec![("Resolved", "ami-00000000000000001".to_string())],
            reported: vec![("ImageId", "/probe/ami", Some("ami-00000000000000001"))],
        },
        Case {
            // The complement: "AWS::SSM::Parameter::Name — the name of a Systems Manager
            // parameter key" is not a VALUE type, so nothing is read for it and a `Ref`
            // answers the key itself.
            case: "a parameter-name type is answered as the key it names",
            stack_name: "ssm-typed-name",
            held: vec![Held {
                name: "/probe/named-key",
                parameter_type: "String",
                value: "never-substituted",
            }],
            template: r#"Parameters:
  Key:
    Type: AWS::SSM::Parameter::Name
    Default: /probe/named-key
Resources:
  Marker:
    Type: AWS::CloudFormation::WaitConditionHandle
Outputs:
  Resolved:
    Value: !Ref Key
"#
            .to_string(),
            parameters: vec![],
            outputs: vec![("Resolved", "/probe/named-key".to_string())],
            reported: vec![("Key", "/probe/named-key", None)],
        },
    ]
}

/// Write every parameter a row needs into Parameter Store, through that service's own wire.
async fn hold(router: &MultiServiceRouter, case: &str, held: &[Held]) {
    for parameter in held {
        put_parameter(
            router,
            case,
            parameter.name,
            parameter.parameter_type,
            parameter.value,
        )
        .await;
    }
}

#[tokio::test]
async fn an_ssm_typed_parameter_is_deployed_as_the_value_parameter_store_holds() {
    let router = router();

    for case in cases() {
        hold(&router, case.case, &case.held).await;

        let (status_code, body) = create_stack(
            &router,
            case.stack_name,
            &case.template,
            &request_parameters(&case.parameters),
        )
        .await;
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
            "case {:?}: the stack must have reached CREATE_COMPLETE before its outputs are \
             read, got {body}",
            case.case
        );

        let outputs = wire_outputs(&body);
        for (key, expected) in &case.outputs {
            let found = output_for(&outputs, key);
            assert_eq!(
                found.and_then(|output| output.value.as_deref()),
                Some(expected.as_str()),
                "case {:?}: output {key:?} must carry {expected:?}, got {found:?}",
                case.case
            );
        }

        // What the stack reports for its own parameters: the NAME as `ParameterValue` and
        // the value the store holds as `ResolvedValue`.
        let reported = wire_parameters(&body);
        for (key, value, resolved) in &case.reported {
            let found = reported.iter().find(|parameter| {
                parameter.key.as_deref() == Some(*key)
            });
            assert_eq!(
                found.map(|parameter| (parameter.value.as_deref(), parameter.resolved_value.as_deref())),
                Some((Some(*value), *resolved)),
                "case {:?}: parameter {key:?} must report the name {value:?} and the \
                 resolved value {resolved:?}, got {found:?}",
                case.case
            );
        }
    }
}

/// RULE (the complement): a name Parameter Store does not answer for leaves the NAME itself
/// the value in force — one unanswered read is no evidence about the parameter, so nothing
/// invents a value for it — and the stack deploys and reports no `ResolvedValue` for it.
///
/// The rows differ along how many names are unanswered and whether any of them IS held: one
/// missing name, two, and a template whose other SSM-typed parameter resolves fine, which is
/// the row that shows the two are decided per parameter.
#[tokio::test]
async fn a_name_parameter_store_does_not_hold_leaves_the_name_in_force() {
    let router = router();
    put_parameter(&router, "held", "/probe/held", "String", "9").await;

    /// `(case, template, [(OutputKey, OutputValue, ResolvedValue)])`.
    type Row = (
        &'static str,
        &'static str,
        Vec<(&'static str, &'static str, Option<&'static str>)>,
    );
    let rows: Vec<Row> = vec![
        (
            "one name nothing is held under",
            r#"{"Parameters":{"A":{"Type":"AWS::SSM::Parameter::Value<String>",
                                   "Default":"/probe/absent-one"}},
                "Resources":{"M":{"Type":"AWS::CloudFormation::WaitConditionHandle"}},
                "Outputs":{"A":{"Value":{"Ref":"A"}}}}"#,
            vec![("A", "/probe/absent-one", None)],
        ),
        (
            "two of them",
            r#"{"Parameters":{
                  "A":{"Type":"AWS::SSM::Parameter::Value<String>",
                       "Default":"/probe/absent-a"},
                  "B":{"Type":"AWS::SSM::Parameter::Value<String>",
                       "Default":"/probe/absent-b"}},
                "Resources":{"M":{"Type":"AWS::CloudFormation::WaitConditionHandle"}},
                "Outputs":{"A":{"Value":{"Ref":"A"}},"B":{"Value":{"Ref":"B"}}}}"#,
            vec![
                ("A", "/probe/absent-a", None),
                ("B", "/probe/absent-b", None),
            ],
        ),
        (
            "one unanswered name beside one the store does hold",
            r#"{"Parameters":{
                  "Held":{"Type":"AWS::SSM::Parameter::Value<String>",
                          "Default":"/probe/held"},
                  "Absent":{"Type":"AWS::SSM::Parameter::Value<String>",
                            "Default":"/probe/absent-c"}},
                "Resources":{"M":{"Type":"AWS::CloudFormation::WaitConditionHandle"}},
                "Outputs":{"Held":{"Value":{"Ref":"Held"}},
                           "Absent":{"Value":{"Ref":"Absent"}}}}"#,
            vec![
                ("Held", "9", Some("9")),
                ("Absent", "/probe/absent-c", None),
            ],
        ),
    ];

    for (index, (case, template, expected)) in rows.iter().enumerate() {
        let stack_name = format!("ssm-typed-absent-{index}");
        let (status_code, body) = create_stack(&router, &stack_name, template, &[]).await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: a name the store does not answer for is still deployed, got \
             {status_code}: {body}"
        );
        wait_for_the_transition().await;
        let (_, body) = describe_stack(&router, &stack_name).await;
        assert_eq!(
            xml_text(&body, "StackStatus").as_deref(),
            Some(status::CREATE_COMPLETE),
            "case {case:?}: the stack must complete, got {body}"
        );
        let outputs = wire_outputs(&body);
        let reported = wire_parameters(&body);
        for (key, value_in_force, resolved) in expected {
            let output = output_for(&outputs, key);
            assert_eq!(
                output.and_then(|output| output.value.as_deref()),
                Some(*value_in_force),
                "case {case:?}: output {key:?} must carry {value_in_force:?}, got {output:?}"
            );
            let found = reported
                .iter()
                .find(|parameter| parameter.key.as_deref() == Some(*key));
            assert_eq!(
                found.map(|parameter| parameter.resolved_value.as_deref()),
                Some(*resolved),
                "case {case:?}: parameter {key:?} must report the resolved value \
                 {resolved:?}, got {found:?}"
            );
        }
    }
}

/// RULE (the third deploying path): a change set is a PLAN, so an SSM-typed parameter it
/// carries is resolved AGAIN by the call that EXECUTES it — `ExecuteChangeSet` deploys the
/// template, so the value in force is the one Parameter Store holds then rather than the one
/// the plan was written against.
#[tokio::test]
async fn executing_a_change_set_deploys_the_value_the_store_holds() {
    let router = router();
    put_parameter(&router, "change set", "/probe/planned", "String", "5").await;

    let template = r#"Parameters:
  Version:
    Type: AWS::SSM::Parameter::Value<String>
    Default: /probe/planned
Resources:
  Marker:
    Type: AWS::CloudFormation::WaitConditionHandle
Outputs:
  Resolved:
    Value: !Ref Version
"#;

    let (status_code, body) =
        create_creating_change_set(&router, "ssm-typed-plan", "ssm-plan", template).await;
    assert_eq!(
        status_code, 200,
        "CreateChangeSet must accept the plan, got {status_code}: {body}"
    );
    wait_for_the_transition().await;
    // The parameter changes between the plan and its execution: what the execution
    // deploys is the value in force THEN, not the one the plan was written against.
    let (status_code, written) = ssm(
        &router,
        "PutParameter",
        &json!({"Name": "/probe/planned", "Type": "String", "Value": "6", "Overwrite": true}),
    )
    .await;
    assert_eq!(
        status_code, 200,
        "the parameter must be overwritten, got {status_code} {written}"
    );

    let (status_code, body) =
        execute_change_set(&router, "ssm-plan", Some("ssm-typed-plan")).await;
    assert_eq!(
        status_code, 200,
        "ExecuteChangeSet must be accepted, got {status_code}: {body}"
    );
    wait_for_the_transition().await;
    let (_, body) = describe_stack(&router, "ssm-typed-plan").await;
    assert_eq!(
        xml_text(&body, "StackStatus").as_deref(),
        Some(status::CREATE_COMPLETE),
        "the executed change set must have created the stack, got {body}"
    );
    let outputs = wire_outputs(&body);
    assert_eq!(
        output_for(&outputs, "Resolved").and_then(|output| output.value.as_deref()),
        Some("6"),
        "the execution must deploy the value the store holds, got {outputs:?}"
    );
    let reported = wire_parameters(&body);
    assert_eq!(
        reported
            .iter()
            .map(|parameter| (
                parameter.key.as_deref(),
                parameter.value.as_deref(),
                parameter.resolved_value.as_deref()
            ))
            .collect::<Vec<_>>(),
        vec![(Some("Version"), Some("/probe/planned"), Some("6"))],
        "the stack must report the name it addresses and the value in force, got {reported:?}"
    );

    // The complement on the same path: a plan whose SSM-typed parameter addresses a name the
    // store does not answer for is refused where it is PLANNED — CloudFormation resolves such
    // a parameter while it computes the change set — and no stack is created for it.
    let absent = r#"Parameters:
  Version:
    Type: AWS::SSM::Parameter::Value<String>
    Default: /probe/planned-absent
Resources:
  Marker:
    Type: AWS::CloudFormation::WaitConditionHandle
Outputs:
  Resolved:
    Value: !Ref Version
"#;
    let (status_code, body) = create_creating_change_set(
        &router,
        "ssm-typed-plan-absent",
        "ssm-plan-absent",
        absent,
    )
    .await;
    assert_eq!(
        status_code, 400,
        "CreateChangeSet must refuse a name the store does not answer for, got \
         {status_code}: {body}"
    );
    assert!(
        body.contains(
            "Unable to fetch parameters [/probe/planned-absent] from parameter store for \
             this account."
        ),
        "the refusal must name the parameter-store name it could not read, got {body}"
    );
    let (status_code, body) = describe_stack(&router, "ssm-typed-plan-absent").await;
    assert_eq!(
        status_code, 400,
        "the refused plan must have created no stack, got {status_code}: {body}"
    );
}

/// RULE (over time): the value in force on a stack is the one the store held when the
/// deployment ran, so an update re-reads the parameter and the stack then reports — and its
/// resources carry — the value the store holds NOW.
#[tokio::test]
async fn an_update_re_reads_the_parameter_the_stack_addresses() {
    let router = router();
    put_parameter(&router, "update", "/probe/rolling", "String", "1").await;

    let template = r#"Parameters:
  Version:
    Type: AWS::SSM::Parameter::Value<String>
    Default: /probe/rolling
Resources:
  Marker:
    Type: AWS::CloudFormation::WaitConditionHandle
Outputs:
  Resolved:
    Value: !Ref Version
"#;

    let (status_code, body) = create_stack(&router, "ssm-typed-update", template, &[]).await;
    assert_eq!(
        status_code, 200,
        "the create must be accepted, got {status_code}: {body}"
    );
    wait_for_the_transition().await;
    let (_, body) = describe_stack(&router, "ssm-typed-update").await;
    let outputs = wire_outputs(&body);
    assert_eq!(
        output_for(&outputs, "Resolved").and_then(|output| output.value.as_deref()),
        Some("1"),
        "the created stack must carry the value the store held, got {outputs:?}"
    );

    // The parameter changes in the store, through that service's own wire.
    let (status_code, written) = ssm(
        &router,
        "PutParameter",
        &json!({"Name": "/probe/rolling", "Type": "String", "Value": "2", "Overwrite": true}),
    )
    .await;
    assert_eq!(
        status_code, 200,
        "the parameter must be overwritten, got {status_code} {written}"
    );
    // Until the stack is updated it still carries what it was deployed with: a read does not
    // re-resolve a deployment that already happened.
    let (_, body) = describe_stack(&router, "ssm-typed-update").await;
    let outputs = wire_outputs(&body);
    assert_eq!(
        output_for(&outputs, "Resolved").and_then(|output| output.value.as_deref()),
        Some("1"),
        "a read must report the value the deployment resolved, got {outputs:?}"
    );

    let (status_code, body) = call(
        &router,
        "UpdateStack",
        &[
            member("StackName", "ssm-typed-update"),
            member("TemplateBody", template),
            member("Parameters.member.1.ParameterKey", "Version"),
            member("Parameters.member.1.ParameterValue", "/probe/rolling"),
        ],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "the update must be accepted, got {status_code}: {body}"
    );
    wait_for_the_transition().await;
    let (_, body) = describe_stack(&router, "ssm-typed-update").await;
    let outputs = wire_outputs(&body);
    assert_eq!(
        output_for(&outputs, "Resolved").and_then(|output| output.value.as_deref()),
        Some("2"),
        "the update must re-read the parameter, got {outputs:?}"
    );
    let reported = wire_parameters(&body);
    assert_eq!(
        reported
            .iter()
            .map(|parameter| (
                parameter.key.as_deref(),
                parameter.value.as_deref(),
                parameter.resolved_value.as_deref()
            ))
            .collect::<Vec<_>>(),
        vec![(Some("Version"), Some("/probe/rolling"), Some("2"))],
        "the updated stack must report the name it addresses and the value in force, got \
         {reported:?}"
    );
    // Nothing else on the response is disturbed by the resolution: the stack still reports
    // exactly one parameter.
    assert_eq!(
        xml_texts(&body, "ParameterKey"),
        vec!["Version".to_string()],
        "the stack must report its one parameter, got {body}"
    );
}
