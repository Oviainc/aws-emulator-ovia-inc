// SPDX-License-Identifier: CC0-1.0

//! A stack's `Outputs`, end to end over the awsQuery wire.
//!
//! The rule under test: for a stack whose template declares an `Outputs` section,
//! `DescribeStacks` reports one `Output` per declared output, carrying the declared key,
//! description and export name, and an `OutputValue` that is the declared `Value` with
//! CloudFormation's intrinsic functions resolved against the stack's parameters, the
//! identifiers of the resources the template declares, and the pseudo parameters.
//!
//! Every case drives the real request path (form-encoded body in, XML envelope out)
//! through the wire harness `stack_lifecycle.rs` builds, so what it asserts is what a
//! client reading a deployed stack sees. The table spans the class the rule generalises
//! over: both template spellings (JSON and YAML, long-form and short-form intrinsics),
//! several resource types, each intrinsic function this service evaluates, and the count
//! of declared outputs. The issue's own case (a YAML `AWS::SNS::Topic` template whose one
//! output is `!Ref` of the topic) is the first row.

use crate::stack_lifecycle::{
    call, create_stack, describe_stack, expected_status, member, router, wait_for_the_transition,
    xml_text, xml_unescape, ACCOUNT_ID, REGION,
};
use service_com_amazonaws_cloudformation_cloudformation::state::template::MAX_OUTPUTS_PER_TEMPLATE;
use service_com_amazonaws_cloudformation_cloudformation::state::{
    status, TEMPLATE_BODY_MAX_BYTES,
};

/// One `Output` as the response carries it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct WireOutput {
    key: Option<String>,
    value: Option<String>,
    description: Option<String>,
    export_name: Option<String>,
}

/// Every `Output` in the first `Stack` of a `DescribeStacks` response, in wire order.
///
/// Read member by member rather than tag by tag, so an output's key stays paired with
/// its own value (and so a declared output that carries NO `OutputValue` is visible as
/// such instead of silently borrowing the next one's).
fn wire_outputs(xml: &str) -> Vec<WireOutput> {
    let Some(start) = xml.find("<Outputs>") else {
        return Vec::new();
    };
    let after = &xml[start + "<Outputs>".len()..];
    let end = after.find("</Outputs>").unwrap_or(after.len());
    let block = &after[..end];

    let mut outputs = Vec::new();
    let mut rest = block;
    while let Some(open) = rest.find("<member>") {
        let after_open = &rest[open + "<member>".len()..];
        let Some(close) = after_open.find("</member>") else {
            break;
        };
        let entry = &after_open[..close];
        outputs.push(WireOutput {
            key: element(entry, "OutputKey"),
            value: element(entry, "OutputValue"),
            description: element(entry, "Description"),
            export_name: element(entry, "ExportName"),
        });
        rest = &after_open[close + "</member>".len()..];
    }
    outputs
}

/// One element's text inside a single `Output` member.
fn element(entry: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = entry.find(&open)?;
    let after = &entry[start + open.len()..];
    let end = after.find(&close)?;
    Some(xml_unescape(&after[..end]))
}

/// The `OutputValue` the response carries for one key, and whether the key is there at
/// all.
fn output_for<'a>(outputs: &'a [WireOutput], key: &str) -> Option<&'a WireOutput> {
    outputs
        .iter()
        .find(|output| output.key.as_deref() == Some(key))
}

/// What one declared output must report.
#[derive(Debug, Clone)]
enum Expected {
    /// Exactly this `OutputValue`.
    Value(String),
    /// The output is reported (its key is there) but carries no `OutputValue`, because
    /// this service cannot evaluate the declared expression.
    NoValue,
}

/// One row of the outputs table: a template, the parameters the create supplies, and
/// what each declared output must report.
struct OutputCase {
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

fn expect(pairs: &[(&'static str, String)]) -> Vec<(&'static str, Expected)> {
    pairs
        .iter()
        .map(|(key, value)| (*key, Expected::Value(value.clone())))
        .collect()
}

fn sns_arn(name: &str) -> String {
    format!("arn:aws:sns:{REGION}:{ACCOUNT_ID}:{name}")
}

fn queue_url(name: &str) -> String {
    format!("https://sqs.{REGION}.amazonaws.com/{ACCOUNT_ID}/{name}")
}

/// The class the rule covers. Each row differs from the others along what the rule
/// generalises over: the template's spelling, the intrinsic function the output uses, the
/// resource type the identifier comes from, and how many outputs are declared.
fn output_cases() -> Vec<OutputCase> {
    vec![
        OutputCase {
            // The issue's own case, in the YAML spelling it was recorded in.
            case: "yaml, an sns topic named by a parameter, output is !Ref of the topic",
            stack_name: "probe-yaml",
            template: concat!(
                "Parameters:\n",
                "  TopicName:\n",
                "    Type: String\n",
                "Resources:\n",
                "  ProbeTopic:\n",
                "    Type: AWS::SNS::Topic\n",
                "    Properties:\n",
                "      TopicName: !Ref TopicName\n",
                "Outputs:\n",
                "  TopicArn:\n",
                "    Value: !Ref ProbeTopic\n",
            )
            .to_string(),
            parameters: params(&[("TopicName", "probe-topic-yaml")]),
            expected: expect(&[("TopicArn", sns_arn("probe-topic-yaml"))]),
        },
        OutputCase {
            case: "json, the same template with long-form Ref",
            stack_name: "probe-json",
            template: r#"{"Parameters":{"TopicName":{"Type":"String"}},
                 "Resources":{"ProbeTopic":{"Type":"AWS::SNS::Topic",
                   "Properties":{"TopicName":{"Ref":"TopicName"}}}},
                 "Outputs":{"TopicArn":{"Value":{"Ref":"ProbeTopic"}}}}"#
                .to_string(),
            parameters: params(&[("TopicName", "probe-topic-json")]),
            expected: expect(&[("TopicArn", sns_arn("probe-topic-json"))]),
        },
        OutputCase {
            case: "yaml, a topic name taken from the template's own parameter Default",
            stack_name: "probe-default",
            template: concat!(
                "Parameters:\n",
                "  TopicName:\n",
                "    Type: String\n",
                "    Default: defaulted-topic\n",
                "Resources:\n",
                "  T:\n",
                "    Type: AWS::SNS::Topic\n",
                "    Properties:\n",
                "      TopicName: !Ref TopicName\n",
                "Outputs:\n",
                "  TopicArn:\n",
                "    Value: !Ref T\n",
            )
            .to_string(),
            parameters: vec![],
            expected: expect(&[("TopicArn", sns_arn("defaulted-topic"))]),
        },
        OutputCase {
            case: "Fn::GetAtt in both spellings, on a topic's two attributes",
            stack_name: "getatt",
            template: concat!(
                "Resources:\n",
                "  T:\n",
                "    Type: AWS::SNS::Topic\n",
                "    Properties:\n",
                "      TopicName: attributed-topic\n",
                "Outputs:\n",
                "  ShortForm:\n",
                "    Value: !GetAtt T.TopicArn\n",
                "  LongForm:\n",
                "    Value:\n",
                "      Fn::GetAtt: [T, TopicName]\n",
            )
            .to_string(),
            parameters: vec![],
            expected: expect(&[
                ("ShortForm", sns_arn("attributed-topic")),
                ("LongForm", "attributed-topic".to_string()),
            ]),
        },
        OutputCase {
            // A queue's Ref is documented as the queue URL, not its ARN: the rule is the
            // resource type's documented return value, not "an ARN".
            case: "a queue, whose Ref is the queue url and whose Arn is an attribute",
            stack_name: "queue",
            template: r#"{"Resources":{"Q":{"Type":"AWS::SQS::Queue",
                   "Properties":{"QueueName":"probe-queue"}}},
                 "Outputs":{"QueueUrl":{"Value":{"Ref":"Q"}},
                   "QueueArn":{"Value":{"Fn::GetAtt":["Q","Arn"]}},
                   "QueueName":{"Value":{"Fn::GetAtt":["Q","QueueName"]}}}}"#
                .to_string(),
            parameters: vec![],
            expected: expect(&[
                ("QueueUrl", queue_url("probe-queue")),
                (
                    "QueueArn",
                    format!("arn:aws:sqs:{REGION}:{ACCOUNT_ID}:probe-queue"),
                ),
                ("QueueName", "probe-queue".to_string()),
            ]),
        },
        OutputCase {
            // Two more catalogued types the issue never cited, with the two shapes a
            // documented return value takes: a bare name and a service ARN.
            case: "a bucket and a table: Ref is the name, Arn is the attribute",
            stack_name: "storage",
            template: r#"{"Resources":{"B":{"Type":"AWS::S3::Bucket",
                   "Properties":{"BucketName":"probe-bucket"}},
                   "D":{"Type":"AWS::DynamoDB::Table","Properties":{"TableName":"probe-table"}}},
                 "Outputs":{"BucketName":{"Value":{"Ref":"B"}},
                   "BucketArn":{"Value":{"Fn::GetAtt":["B","Arn"]}},
                   "TableName":{"Value":{"Ref":"D"}},
                   "TableArn":{"Value":{"Fn::GetAtt":["D","Arn"]}}}}"#
                .to_string(),
            parameters: vec![],
            expected: expect(&[
                ("BucketName", "probe-bucket".to_string()),
                ("BucketArn", "arn:aws:s3:::probe-bucket".to_string()),
                ("TableName", "probe-table".to_string()),
                (
                    "TableArn",
                    format!("arn:aws:dynamodb:{REGION}:{ACCOUNT_ID}:table/probe-table"),
                ),
            ]),
        },
        OutputCase {
            // The resource types the EventBridge Scheduler issue's templates declare:
            // neither is in the return-value catalogue, so `Ref` answers the physical
            // name — the name the template declared — and `Fn::GetAtt` for an attribute
            // nothing publishes is reported without a value rather than with a guess.
            case: "an uncatalogued type: Ref is its physical name, GetAtt has no value",
            stack_name: "scheduler",
            template: r#"{"Resources":{
                   "G":{"Type":"AWS::Scheduler::ScheduleGroup","Properties":{"Name":"probe-group"}},
                   "S":{"Type":"AWS::Scheduler::Schedule","Properties":{"Name":"probe-schedule",
                     "GroupName":{"Ref":"G"}}}},
                 "Outputs":{"GroupName":{"Value":{"Ref":"G"}},
                   "ScheduleName":{"Value":{"Ref":"S"}},
                   "ScheduleArn":{"Value":{"Fn::GetAtt":["S","Arn"]}}}}"#
                .to_string(),
            parameters: vec![],
            expected: vec![
                ("GroupName", Expected::Value("probe-group".to_string())),
                ("ScheduleName", Expected::Value("probe-schedule".to_string())),
                ("ScheduleArn", Expected::NoValue),
            ],
        },
        OutputCase {
            case: "the pseudo parameters a Ref resolves without any resource",
            stack_name: "pseudo",
            template: concat!(
                "Outputs:\n",
                "  Region:\n",
                "    Value: !Ref AWS::Region\n",
                "  Account:\n",
                "    Value: !Ref AWS::AccountId\n",
                "  Name:\n",
                "    Value: !Ref AWS::StackName\n",
                "  Partition:\n",
                "    Value: !Ref AWS::Partition\n",
                "  Suffix:\n",
                "    Value: !Ref AWS::URLSuffix\n",
            )
            .to_string(),
            parameters: vec![],
            expected: expect(&[
                ("Region", REGION.to_string()),
                ("Account", ACCOUNT_ID.to_string()),
                ("Name", "pseudo".to_string()),
                ("Partition", "aws".to_string()),
                ("Suffix", "amazonaws.com".to_string()),
            ]),
        },
        OutputCase {
            case: "Fn::Join and Fn::Sub over a parameter, an attribute and a literal",
            stack_name: "composed",
            template: concat!(
                "Parameters:\n",
                "  Stage:\n",
                "    Type: String\n",
                "Resources:\n",
                "  T:\n",
                "    Type: AWS::SNS::Topic\n",
                "    Properties:\n",
                "      TopicName: !Sub \"${Stage}-events\"\n",
                "Outputs:\n",
                "  Joined:\n",
                "    Value: !Join [\"-\", [!Ref Stage, !Ref \"AWS::Region\", !GetAtt T.TopicName]]\n",
                "  Substituted:\n",
                "    Value: !Sub \"${Stage}/${T.TopicArn}/${!Literal}\"\n",
            )
            .to_string(),
            parameters: params(&[("Stage", "prod")]),
            expected: expect(&[
                ("Joined", format!("prod-{REGION}-prod-events")),
                (
                    "Substituted",
                    format!("prod/{}/${{Literal}}", sns_arn("prod-events")),
                ),
            ]),
        },
        OutputCase {
            case: "Fn::Select over Fn::Split, and Fn::FindInMap over the Mappings section",
            stack_name: "listish",
            template: r#"{"Mappings":{"Sizes":{"small":{"Queue":"probe-small"}}},
                 "Outputs":{"Middle":{"Value":{"Fn::Select":["1",{"Fn::Split":[",","a,b,c"]}]}},
                   "Mapped":{"Value":{"Fn::FindInMap":["Sizes","small","Queue"]}}}}"#
                .to_string(),
            parameters: vec![],
            expected: expect(&[
                ("Middle", "b".to_string()),
                ("Mapped", "probe-small".to_string()),
            ]),
        },
        OutputCase {
            case: "an output declaring a description and an export name",
            stack_name: "exported",
            template: concat!(
                "Resources:\n",
                "  T:\n",
                "    Type: AWS::SNS::Topic\n",
                "    Properties:\n",
                "      TopicName: exported-topic\n",
                "Outputs:\n",
                "  TopicArn:\n",
                "    Description: the probe topic\n",
                "    Value: !Ref T\n",
                "    Export:\n",
                "      Name: !Sub \"${AWS::StackName}-TopicArn\"\n",
            )
            .to_string(),
            parameters: vec![],
            expected: expect(&[("TopicArn", sns_arn("exported-topic"))]),
        },
        OutputCase {
            // The count dimension: ten declared outputs, every one reported.
            case: "ten declared outputs",
            stack_name: "many",
            template: {
                let mut body = String::from("Outputs:\n");
                for index in 1..=10 {
                    body.push_str(&format!(
                        "  Out{index}:\n    Value: !Join [\"-\", [\"o\", \"{index}\"]]\n"
                    ));
                }
                body
            },
            parameters: vec![],
            expected: (1..=10)
                .map(|index| {
                    let key: &'static str = [
                        "Out1", "Out2", "Out3", "Out4", "Out5", "Out6", "Out7", "Out8", "Out9",
                        "Out10",
                    ][index - 1];
                    (key, Expected::Value(format!("o-{index}")))
                })
                .collect(),
        },
        OutputCase {
            case: "outputs this service cannot evaluate are reported without a value",
            stack_name: "unresolved",
            template: r#"{"Parameters":{"Unsupplied":{"Type":"String"}},
                 "Outputs":{"Imported":{"Value":{"Fn::ImportValue":"other-stack-export"}},
                   "Dangling":{"Value":{"Ref":"NoSuchThing"}},
                   "Missing":{"Value":{"Ref":"Unsupplied"}},
                   "Attribute":{"Value":{"Fn::GetAtt":["NoSuchThing","Arn"]}}}}"#
                .to_string(),
            parameters: vec![],
            expected: vec![
                ("Imported", Expected::NoValue),
                ("Dangling", Expected::NoValue),
                ("Missing", Expected::NoValue),
                ("Attribute", Expected::NoValue),
            ],
        },
    ]
}

#[tokio::test]
async fn declared_outputs_are_evaluated_and_reported_by_describe_stacks() {
    let router = router();

    for case in output_cases() {
        let (status_code, body) = create_stack(
            &router,
            case.stack_name,
            &case.template,
            &case.parameters,
        )
        .await;
        assert_eq!(
            status_code, 200,
            "case {:?}: expected CreateStack to succeed, got {status_code} {body}",
            case.case
        );
        wait_for_the_transition().await;

        let (status_code, body) = describe_stack(&router, case.stack_name).await;
        assert_eq!(
            status_code, 200,
            "case {:?}: expected DescribeStacks to succeed, got {status_code} {body}",
            case.case
        );
        assert_eq!(
            xml_text(&body, "StackStatus").as_deref(),
            Some(status::CREATE_COMPLETE),
            "case {:?}: the stack must have reached CREATE_COMPLETE, got {body}",
            case.case
        );

        let outputs = wire_outputs(&body);
        assert_eq!(
            outputs.len(),
            case.expected.len(),
            "case {:?}: expected one Output per declared output ({} of them), got {outputs:?} \
             from {body}",
            case.case,
            case.expected.len()
        );
        for (key, expected) in &case.expected {
            let Some(output) = output_for(&outputs, key) else {
                panic!(
                    "case {:?}: no Output with OutputKey {key:?}; got {outputs:?}",
                    case.case
                );
            };
            match expected {
                Expected::Value(value) => assert_eq!(
                    output.value.as_deref(),
                    Some(value.as_str()),
                    "case {:?}: output {key:?} must carry the evaluated value {value:?}, got {:?}",
                    case.case,
                    output.value
                ),
                Expected::NoValue => assert!(
                    output.value.is_none(),
                    "case {:?}: output {key:?} is not evaluable, so it must carry no \
                     OutputValue, got {:?}",
                    case.case,
                    output.value
                ),
            }
        }
    }
}

/// The declared `Description` and `Export.Name` travel with the output, and the export
/// name is itself an evaluated expression.
#[tokio::test]
async fn an_outputs_description_and_export_name_are_reported() {
    let router = router();
    let template = concat!(
        "Resources:\n",
        "  T:\n",
        "    Type: AWS::SNS::Topic\n",
        "    Properties:\n",
        "      TopicName: described-topic\n",
        "Outputs:\n",
        "  TopicArn:\n",
        "    Description: the probe topic\n",
        "    Value: !Ref T\n",
        "    Export:\n",
        "      Name: !Sub \"${AWS::StackName}-TopicArn\"\n",
    );
    create_stack(&router, "described", template, &[]).await;
    wait_for_the_transition().await;

    let (_, body) = describe_stack(&router, "described").await;
    let outputs = wire_outputs(&body);
    let output = output_for(&outputs, "TopicArn").expect("the declared output");
    assert_eq!(
        output,
        &WireOutput {
            key: Some("TopicArn".to_string()),
            value: Some(sns_arn("described-topic")),
            description: Some("the probe topic".to_string()),
            export_name: Some("described-TopicArn".to_string()),
        },
        "an output must report its declared description and its evaluated export name, \
         got {body}"
    );
}

/// A template with no `Outputs` section reports no outputs at all — the member is absent
/// rather than an empty list, and nothing is invented for a stack that declares none.
#[tokio::test]
async fn a_template_declaring_no_outputs_reports_none() {
    let router = router();
    let cases: Vec<(&str, &str, &str)> = vec![
        ("no Outputs section at all", "silent-json", r#"{"Resources":{"T":{"Type":"AWS::SNS::Topic"}}}"#),
        (
            "no Outputs section, yaml",
            "silent-yaml",
            "Resources:\n  T:\n    Type: AWS::SNS::Topic\n",
        ),
        ("an empty template", "silent-empty", "{}"),
        (
            "a body that is neither json nor yaml",
            "silent-unreadable",
            "\tResources: [unclosed\n  - : :\n",
        ),
    ];

    for (case, name, template) in cases {
        let (status_code, body) = create_stack(&router, name, template, &[]).await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected CreateStack to succeed, got {status_code} {body}"
        );
        wait_for_the_transition().await;
        let (_, body) = describe_stack(&router, name).await;
        assert!(
            !body.contains("<Outputs>"),
            "case {case:?}: a stack declaring no outputs must report no Outputs, got {body}"
        );
    }
}

/// A resource the template does not name takes the physical name CloudFormation
/// generates, `<stack name>-<logical id>-<unique id>`, and that name is STABLE: the same
/// read twice, and the same resource after an unrelated update, report the same id.
#[tokio::test]
async fn an_unnamed_resource_refs_its_generated_physical_name() {
    let router = router();
    let template = concat!(
        "Resources:\n",
        "  Unnamed:\n",
        "    Type: AWS::SNS::Topic\n",
        "Outputs:\n",
        "  TopicArn:\n",
        "    Value: !Ref Unnamed\n",
    );
    create_stack(&router, "generated", template, &[]).await;
    wait_for_the_transition().await;

    let (_, body) = describe_stack(&router, "generated").await;
    let first = output_for(&wire_outputs(&body), "TopicArn")
        .and_then(|output| output.value.clone())
        .unwrap_or_else(|| panic!("expected an evaluated TopicArn, got {body}"));
    let prefix = format!("{}:generated-Unnamed-", sns_arn("").trim_end_matches(':'));
    assert!(
        first.starts_with(&prefix),
        "an unnamed resource's Ref must be built from its generated name {prefix:?}, got \
         {first:?}"
    );
    let unique = &first[prefix.len()..];
    assert!(
        unique.len() == 12 && unique.chars().all(|c| c.is_ascii_hexdigit()),
        "the generated name's unique segment must be a fixed-width id, got {unique:?}"
    );

    let (_, body) = describe_stack(&router, "generated").await;
    let second = output_for(&wire_outputs(&body), "TopicArn").and_then(|o| o.value.clone());
    assert_eq!(
        second.as_deref(),
        Some(first.as_str()),
        "reading the stack again must report the same physical id, got {body}"
    );
}

/// `{"Ref": "AWS::StackId"}` resolves to the stack's own id, which the same response
/// reports as `StackId` — the output is computed from the stack, not from the request.
#[tokio::test]
async fn the_stack_id_pseudo_parameter_resolves_to_this_stacks_id() {
    let router = router();
    create_stack(
        &router,
        "identified",
        "Outputs:\n  Self:\n    Value: !Ref AWS::StackId\n",
        &[],
    )
    .await;
    wait_for_the_transition().await;

    let (_, body) = describe_stack(&router, "identified").await;
    let stack_id = xml_text(&body, "StackId").expect("the stack's id");
    let output = output_for(&wire_outputs(&body), "Self").and_then(|o| o.value.clone());
    assert_eq!(
        output.as_deref(),
        Some(stack_id.as_str()),
        "Ref AWS::StackId must resolve to the stack's own id {stack_id:?}, got {body}"
    );
}

/// Outputs are the RESULT of the operation that deploys the template, so they are
/// reported once that operation has completed: a read taken while the create is still in
/// progress carries none.
#[tokio::test]
async fn outputs_appear_only_once_the_create_has_completed() {
    let router = router();
    let template = concat!(
        "Resources:\n",
        "  T:\n",
        "    Type: AWS::SNS::Topic\n",
        "    Properties:\n",
        "      TopicName: pending-topic\n",
        "Outputs:\n",
        "  TopicArn:\n",
        "    Value: !Ref T\n",
    );
    let started = std::time::Instant::now();
    create_stack(&router, "pending", template, &[]).await;

    let (_, body) = describe_stack(&router, "pending").await;
    let in_progress = expected_status(
        started,
        status::CREATE_IN_PROGRESS,
        status::CREATE_COMPLETE,
    ) == status::CREATE_IN_PROGRESS;
    if in_progress {
        assert_eq!(
            xml_text(&body, "StackStatus").as_deref(),
            Some(status::CREATE_IN_PROGRESS),
            "the read was taken before the transition was due, got {body}"
        );
        assert!(
            wire_outputs(&body).is_empty(),
            "a stack whose create is still in progress must report no outputs, got {body}"
        );
    }

    wait_for_the_transition().await;
    let (_, body) = describe_stack(&router, "pending").await;
    let output = output_for(&wire_outputs(&body), "TopicArn").and_then(|o| o.value.clone());
    assert_eq!(
        output.as_deref(),
        Some(sns_arn("pending-topic").as_str()),
        "once the create has completed the declared output must carry its value, got {body}"
    );
}

/// An update deploys a new template, so the stack's outputs are those of the template it
/// now carries: a key the new template drops is gone, and a value a changed parameter
/// moves is re-evaluated. Until the update COMPLETES, the values in force are still the
/// ones the completed create left.
#[tokio::test]
async fn an_update_re_evaluates_the_outputs() {
    let router = router();
    let original = concat!(
        "Parameters:\n",
        "  TopicName:\n",
        "    Type: String\n",
        "Resources:\n",
        "  T:\n",
        "    Type: AWS::SNS::Topic\n",
        "    Properties:\n",
        "      TopicName: !Ref TopicName\n",
        "Outputs:\n",
        "  TopicArn:\n",
        "    Value: !Ref T\n",
        "  Dropped:\n",
        "    Value: !Ref AWS::Region\n",
    );
    create_stack(
        &router,
        "evolving",
        original,
        &params(&[("TopicName", "first-topic")]),
    )
    .await;
    wait_for_the_transition().await;

    let (_, body) = describe_stack(&router, "evolving").await;
    assert_eq!(
        output_for(&wire_outputs(&body), "TopicArn").and_then(|o| o.value.clone()),
        Some(sns_arn("first-topic")),
        "the created stack's output must carry the first topic's arn, got {body}"
    );

    let updated = concat!(
        "Parameters:\n",
        "  TopicName:\n",
        "    Type: String\n",
        "Resources:\n",
        "  T:\n",
        "    Type: AWS::SNS::Topic\n",
        "    Properties:\n",
        "      TopicName: !Ref TopicName\n",
        "Outputs:\n",
        "  TopicArn:\n",
        "    Value: !GetAtt T.TopicArn\n",
    );
    let mut members = vec![member("StackName", "evolving"), member("TemplateBody", updated)];
    members.extend(params(&[("TopicName", "second-topic")]));
    let started = std::time::Instant::now();
    let (status_code, body) = call(&router, "UpdateStack", &members).await;
    assert_eq!(
        status_code, 200,
        "expected UpdateStack to succeed, got {status_code} {body}"
    );

    // While the update is still in progress the stack reports the values the completed
    // create left, not the ones the update is about to install.
    let (_, body) = describe_stack(&router, "evolving").await;
    if expected_status(started, status::UPDATE_IN_PROGRESS, status::UPDATE_COMPLETE)
        == status::UPDATE_IN_PROGRESS
    {
        let outputs = wire_outputs(&body);
        assert_eq!(
            output_for(&outputs, "TopicArn").and_then(|o| o.value.clone()),
            Some(sns_arn("first-topic")),
            "an update in progress must still report the outputs in force, got {body}"
        );
        assert!(
            output_for(&outputs, "Dropped").is_some(),
            "an update in progress must still report the output it is about to drop, got \
             {body}"
        );
    }
    wait_for_the_transition().await;

    let (_, body) = describe_stack(&router, "evolving").await;
    let outputs = wire_outputs(&body);
    assert_eq!(
        output_for(&outputs, "TopicArn").and_then(|o| o.value.clone()),
        Some(sns_arn("second-topic")),
        "the update must re-evaluate the output against the new parameter, got {body}"
    );
    assert!(
        output_for(&outputs, "Dropped").is_none(),
        "an output the new template no longer declares must be gone, got {body}"
    );
}

/// A stack whose deletion has completed has no resources left, so it reports no output
/// values — it is still addressable by its stack id, and what it carries is the terminal
/// status, not stale identifiers.
#[tokio::test]
async fn a_deleted_stacks_outputs_are_gone() {
    let router = router();
    let template = concat!(
        "Resources:\n",
        "  T:\n",
        "    Type: AWS::SNS::Topic\n",
        "    Properties:\n",
        "      TopicName: doomed-topic\n",
        "Outputs:\n",
        "  TopicArn:\n",
        "    Value: !Ref T\n",
    );
    create_stack(&router, "doomed", template, &[]).await;
    wait_for_the_transition().await;

    let (_, body) = describe_stack(&router, "doomed").await;
    let stack_id = xml_text(&body, "StackId").expect("the stack's id");
    assert_eq!(
        output_for(&wire_outputs(&body), "TopicArn").and_then(|o| o.value.clone()),
        Some(sns_arn("doomed-topic")),
        "the live stack must report its output, got {body}"
    );

    call(&router, "DeleteStack", &[member("StackName", "doomed")]).await;
    wait_for_the_transition().await;

    let (status_code, body) = describe_stack(&router, &stack_id).await;
    assert_eq!(
        status_code, 200,
        "a deleted stack is still addressable by its id, got {status_code} {body}"
    );
    assert_eq!(
        xml_text(&body, "StackStatus").as_deref(),
        Some(status::DELETE_COMPLETE),
        "the stack must have reached DELETE_COMPLETE, got {body}"
    );
    assert!(
        wire_outputs(&body).is_empty(),
        "a stack whose deletion completed must report no outputs, got {body}"
    );
}

/// A template body built to explode the parse is read as NO template — the request is
/// answered, the process is still serving, and the stack simply carries no outputs.
///
/// Nesting is what matters here: the YAML loader descends once per level and cannot be
/// aborted from the event stream, so the nesting a YAML body can reach is bounded before
/// the parser sees it. Each row is a different way to buy depth cheaply.
#[tokio::test]
async fn a_template_body_built_to_explode_the_parse_is_read_as_no_template() {
    let router = router();
    // The documented `TemplateBody` maximum, so each body is as large as a caller can
    // make one.
    let body_max = TEMPLATE_BODY_MAX_BYTES;
    let cases: Vec<(&str, String, usize)> = vec![
        ("flow sequences, one byte per level", "[".repeat(body_max), 1),
        ("flow mappings, one byte per level", "{".repeat(body_max), 2),
        (
            "compact block sequences, two bytes per level",
            "- ".repeat(body_max / 2),
            3,
        ),
        (
            "indentation, one column per level",
            (0..300)
                .map(|level| format!("{}k{level}:\n", " ".repeat(level)))
                .collect(),
            4,
        ),
        (
            "well-formed json nested past what any template needs",
            format!("{}1{}", "[".repeat(2_000), "]".repeat(2_000)),
            5,
        ),
        (
            // Closers hidden inside scalars keep the running flow depth low while the
            // loader's own depth climbs; the opener COUNT is what bounds that.
            "flow openers interleaved with closers quoted inside scalars",
            format!("Outputs: {}", "[\"]\"".repeat(body_max / 8)),
            6,
        ),
        (
            // The worst case the opener count permits: every closer is quoted, so the
            // running depth never rises, and the loader really does descend once per
            // level. Just inside MAX_TEMPLATE_FLOW_OPENERS, so this is the deepest
            // document the guard lets through — and the service answers it.
            "the deepest nesting the opener count allows, every closer quoted",
            format!("Outputs: {}", "[\"]\",".repeat(2_000)),
            7,
        ),
        (
            "nesting just past what the reader builds, with outputs declared beneath it",
            format!(
                "Outputs:\n  Deep:\n    Value: {}\"x\"{}\n",
                "[".repeat(40),
                "]".repeat(40)
            ),
            8,
        ),
    ];

    for (case, template, index) in cases {
        let name = format!("exploding-{index}");
        let (status_code, body) = create_stack(&router, &name, &template, &[]).await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected CreateStack to be answered, got {status_code} {body}"
        );
        wait_for_the_transition().await;
        let (status_code, body) = describe_stack(&router, &name).await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected DescribeStacks to be answered, got {status_code} {body}"
        );
        assert!(
            wire_outputs(&body).is_empty(),
            "case {case:?}: a body this service will not read declares no outputs, got {body}"
        );
    }
}

/// A big, FLAT template is read like any other: nothing that bounds nesting may be
/// spent by a template that is merely large — many resources and outputs, hyphenated
/// names, long `Fn::Join` lines, a `Description` — in either spelling.
#[tokio::test]
async fn a_large_flat_template_is_read_in_full() {
    let router = router();

    // 400 resources and 100 outputs in the JSON spelling: well over a thousand braces and
    // brackets, and every one of them closed immediately.
    let resources: serde_json::Map<String, serde_json::Value> = (0..400)
        .map(|index| {
            (
                format!("R{index:03}"),
                serde_json::json!({
                    "Type": "AWS::SNS::Topic",
                    "Properties": {"TopicName": format!("flat-topic-{index:03}")}
                }),
            )
        })
        .collect();
    let outputs: serde_json::Map<String, serde_json::Value> = (0..100)
        .map(|index| {
            (
                format!("O{index:03}"),
                serde_json::json!({"Value": {"Ref": format!("R{index:03}")}}),
            )
        })
        .collect();
    let template = serde_json::json!({
        "Description": "a large flat template",
        "Resources": resources,
        "Outputs": outputs,
    })
    .to_string();
    assert!(
        template.len() < TEMPLATE_BODY_MAX_BYTES,
        "the fixture must be a body a caller may send, got {} bytes",
        template.len()
    );

    let (status_code, body) = create_stack(&router, "flat-json", &template, &[]).await;
    assert_eq!(
        status_code, 200,
        "expected CreateStack to succeed, got {status_code} {body}"
    );
    wait_for_the_transition().await;
    let (_, body) = describe_stack(&router, "flat-json").await;
    assert_eq!(
        xml_text(&body, "Description").as_deref(),
        Some("a large flat template"),
        "a large template still reports its Description, got {body}"
    );
    let outputs = wire_outputs(&body);
    assert_eq!(
        outputs.len(),
        100,
        "every declared output of a large template must be reported, got {} of them",
        outputs.len()
    );
    assert_eq!(
        output_for(&outputs, "O399").and_then(|o| o.value.clone()),
        None,
        "O399 is not declared by this template"
    );
    for key in ["O000", "O042", "O099"] {
        let index: usize = key[1..].parse().expect("the output's index");
        assert_eq!(
            output_for(&outputs, key).and_then(|o| o.value.clone()),
            Some(sns_arn(&format!("flat-topic-{index:03}"))),
            "output {key:?} must carry its topic's arn, got {body}"
        );
    }

    // The same shape in YAML — which DOES go through the pre-flight guard — with
    // hyphenated names and a long `Fn::Join` line carrying hundreds of hyphens.
    let mut yaml = String::from("Description: a large flat yaml template\nResources:\n");
    for index in 0..200 {
        yaml.push_str(&format!(
            "  R{index:03}:\n    Type: AWS::SNS::Topic\n    Properties:\n      TopicName: a-very-hyphenated-topic-name-{index:03}\n"
        ));
    }
    yaml.push_str("Outputs:\n  Joined:\n    Value: !Join [\"-\", [");
    yaml.push_str(
        &(0..100)
            .map(|index| format!("\"part-{index:03}\""))
            .collect::<Vec<String>>()
            .join(", "),
    );
    yaml.push_str("]]\n  First:\n    Value: !Ref R000\n");
    assert!(
        yaml.len() < TEMPLATE_BODY_MAX_BYTES,
        "the fixture must be a body a caller may send, got {} bytes",
        yaml.len()
    );

    let (status_code, body) = create_stack(&router, "flat-yaml", &yaml, &[]).await;
    assert_eq!(
        status_code, 200,
        "expected CreateStack to succeed, got {status_code} {body}"
    );
    wait_for_the_transition().await;
    let (_, body) = describe_stack(&router, "flat-yaml").await;
    let outputs = wire_outputs(&body);
    assert_eq!(
        output_for(&outputs, "First").and_then(|o| o.value.clone()),
        Some(sns_arn("a-very-hyphenated-topic-name-000")),
        "a hyphenated resource name must still be read, got {body}"
    );
    let joined = (0..100)
        .map(|index| format!("part-{index:03}"))
        .collect::<Vec<String>>()
        .join("-");
    assert_eq!(
        output_for(&outputs, "Joined").and_then(|o| o.value.clone()),
        Some(joined),
        "a long Fn::Join line full of hyphens must still be read, got {body}"
    );
    assert_eq!(
        xml_text(&body, "Description").as_deref(),
        Some("a large flat yaml template"),
        "a large yaml template still reports its Description, got {body}"
    );
}

/// `Fn::Sub` resolves each variable ONCE however often its source names it, and the
/// deployment's work budget bounds the total: a chain of nested substitutions that would
/// re-evaluate its parts exponentially is answered — promptly — with no value rather than
/// with unbounded work.
#[tokio::test]
async fn a_substitution_chain_cannot_multiply_the_work_of_its_parts() {
    let router = router();

    /// One `Fn::Sub` per level, each source naming its variable `occurrences` times, the
    /// variable being the next level down. `body` is the innermost source.
    fn chain(levels: usize, occurrences: usize, innermost: &str) -> serde_json::Value {
        let source = "${a}".repeat(occurrences);
        let mut value = serde_json::Value::String(innermost.to_string());
        for _ in 0..levels {
            value = serde_json::json!({"Fn::Sub": [source.clone(), {"a": value}]});
        }
        value
    }

    let cases: Vec<(&str, serde_json::Value, Option<&str>)> = vec![
        (
            // Exponential WITHOUT memoisation: 8^16 evaluations. Each level resolves to
            // the empty string, so no size cap would ever trip.
            "sixteen levels, eight occurrences each, empty innermost",
            chain(16, 8, ""),
            Some(""),
        ),
        (
            // The same shape, but each level's text grows, so the evaluation is refused
            // by the value-size cap or the work budget — either way, with no value.
            "sixteen levels, eight occurrences each, growing text",
            chain(16, 8, "0123456789abcdef"),
            None,
        ),
        (
            "one level, one occurrence of a plain parameter-less literal",
            chain(1, 1, "x"),
            Some("x"),
        ),
    ];

    for (index, (case, value, expected)) in cases.into_iter().enumerate() {
        let template =
            serde_json::json!({"Outputs": {"Chained": {"Value": value}}}).to_string();
        let name = format!("chained-{index}");
        let started = std::time::Instant::now();
        let (status_code, body) = create_stack(&router, &name, &template, &[]).await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected CreateStack to succeed, got {status_code} {body}"
        );
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "case {case:?}: the deployment must be bounded work, took {:?}",
            started.elapsed()
        );
        wait_for_the_transition().await;
        let (_, body) = describe_stack(&router, &name).await;
        let outputs = wire_outputs(&body);
        let output = output_for(&outputs, "Chained")
            .unwrap_or_else(|| panic!("case {case:?}: the declared output, got {body}"));
        assert_eq!(
            output.value.as_deref(),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            output.value
        );
    }
}

/// A template past a documented per-template quota is not one CloudFormation accepts, so
/// this service reports no part of it rather than a truncated subset.
#[tokio::test]
async fn a_template_past_a_documented_quota_reports_no_outputs() {
    let router = router();
    let cases: Vec<(&str, usize, bool)> = vec![
        // The published bound and one past it.
        ("outputs at the documented quota", MAX_OUTPUTS_PER_TEMPLATE, true),
        (
            "one output past the documented quota",
            MAX_OUTPUTS_PER_TEMPLATE + 1,
            false,
        ),
    ];

    for (case, count, reported) in cases {
        let outputs: serde_json::Map<String, serde_json::Value> = (0..count)
            .map(|index| {
                (
                    format!("Out{index:04}"),
                    serde_json::json!({"Value": {"Ref": "AWS::Region"}}),
                )
            })
            .collect();
        let template = serde_json::json!({"Outputs": outputs}).to_string();
        let name = format!("quota-{count}");
        let (status_code, body) = create_stack(&router, &name, &template, &[]).await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected CreateStack to succeed, got {status_code} {body}"
        );
        wait_for_the_transition().await;
        let (_, body) = describe_stack(&router, &name).await;
        assert_eq!(
            !wire_outputs(&body).is_empty(),
            reported,
            "case {case:?}: expected outputs reported = {reported}, got {} of them",
            wire_outputs(&body).len()
        );
    }
}
