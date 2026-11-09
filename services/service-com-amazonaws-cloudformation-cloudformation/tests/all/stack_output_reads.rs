// SPDX-License-Identifier: CC0-1.0

//! `Fn::GetStackOutput`: reading ONE OUTPUT of another stack, end to end over the awsQuery
//! wire.
//!
//! The rule under test: a template may read an output of another stack of the same account
//! and Region by naming the stack and the output (`{"StackName": …, "OutputName": …}`, the
//! two members the function takes) — the output needs no `Export` — and
//! the value a deployment reads is kept by the reading stack, so a later deployment of the
//! stack that publishes the output does not change what the reader reports until the reader
//! is itself deployed again. A reference naming an output no stack of that Region carries
//! has no value, so the deployment fails with CloudFormation's own reason rather than
//! deploying a blank.
//!
//! The table spans the class the rule generalises over: the stack addressed by name and by
//! stack id; an output that is exported and one that is not; a literal value and one a
//! resource's own identity decides; the reference standing alone, nested inside another
//! intrinsic function, and with its stack and output key named by a parameter; both template
//! spellings (JSON and YAML); the reference in an output value and in a resource property
//! the owning service is asked for; and a reference that resolves to nothing. Every case
//! drives the real request path (form-encoded body in, XML envelope out).

use smithy_server_runtime::MultiServiceRouter;

use crate::stack_lifecycle::{
    call, create_stack, member, router, sns, wait_for_the_transition, xml_blocks, xml_text,
    xml_texts,
};
use service_com_amazonaws_cloudformation_cloudformation::state::stack_outputs::{
    no_output_named, Reference,
};

/// The stack the cases read outputs off, and the outputs it publishes: a plain one that is
/// NOT exported, one that is, and one whose value is the identity of a resource the stack
/// created.
const PRODUCER: &str = "output-producer";

/// The value the producer's unexported output carries.
const PLAIN_VALUE: &str = "plain-output-value";

/// The value its exported output carries, under the export name below.
const EXPORTED_VALUE: &str = "exported-output-value";

/// The producer's template.
fn producer_template(plain: &str) -> String {
    format!(
        r#"{{"Resources":{{"Handle":{{"Type":"AWS::CloudFormation::WaitConditionHandle"}}}},
            "Outputs":{{
              "Plain":{{"Value":"{plain}"}},
              "Shared":{{"Value":"{EXPORTED_VALUE}","Export":{{"Name":"read-shared"}}}},
              "FromResource":{{"Value":{{"Ref":"Handle"}}}}}}}}"#
    )
}

/// The `(OutputKey, OutputValue)` pairs one stack reports through `DescribeStacks`.
async fn outputs_of(
    case: &str,
    router: &MultiServiceRouter,
    stack_name: &str,
) -> Vec<(String, String)> {
    let (status, body) = call(router, "DescribeStacks", &[member("StackName", stack_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected DescribeStacks to succeed, got {status} {body}"
    );
    let keys = xml_texts(&body, "OutputKey");
    let values = xml_texts(&body, "OutputValue");
    assert_eq!(
        keys.len(),
        values.len(),
        "case {case:?}: every reported output must carry an OutputValue, got keys {keys:?} and \
         values {values:?}"
    );
    keys.into_iter().zip(values).collect()
}

/// The value one of a stack's outputs carries, or `None` when the stack reports no such
/// output.
async fn output_of(
    case: &str,
    router: &MultiServiceRouter,
    stack_name: &str,
    key: &str,
) -> Option<String> {
    outputs_of(case, router, stack_name)
        .await
        .into_iter()
        .find(|(reported, _)| reported == key)
        .map(|(_, value)| value)
}

/// The `StackStatus` and `StackStatusReason` one stack reports.
async fn status_of(router: &MultiServiceRouter, stack_name: &str) -> (String, String) {
    let (status, body) = call(router, "DescribeStacks", &[member("StackName", stack_name)]).await;
    assert_eq!(
        status, 200,
        "expected DescribeStacks({stack_name}) to succeed, got {status} {body}"
    );
    (
        xml_text(&body, "StackStatus").unwrap_or_default(),
        xml_text(&body, "StackStatusReason").unwrap_or_default(),
    )
}

/// Deploy the producer and answer its stack id, once its outputs are published.
async fn deploy_producer(router: &MultiServiceRouter, name: &str, plain: &str) -> String {
    let (status, body) = create_stack(router, name, &producer_template(plain), &[]).await;
    assert_eq!(
        status, 200,
        "expected CreateStack for the producer to succeed, got {status} {body}"
    );
    let stack_id = xml_text(&body, "StackId").expect("CreateStack answers a StackId");
    wait_for_the_transition().await;
    stack_id
}

/// One row of the table: the reading stack, the output it reports and the value that output
/// must carry.
struct ReadCase {
    case: &'static str,
    stack_name: &'static str,
    template: String,
    parameters: Vec<(String, String)>,
    output_key: &'static str,
    expected: String,
}

/// The rows, built against the producer's identity (`stack_id`) and the value its
/// resource-derived output reports (`handle_value`).
fn read_cases(stack_id: &str, handle_value: &str) -> Vec<ReadCase> {
    vec![
        ReadCase {
            case: "an output reads an UNEXPORTED output by stack name",
            stack_name: "reads-unexported",
            template: format!(
                r#"{{"Resources":{{"H":{{"Type":"AWS::CloudFormation::WaitConditionHandle"}}}},
                   "Outputs":{{"Read":{{"Value":{{"Fn::GetStackOutput":
                     {{"StackName":"{PRODUCER}","OutputName":"Plain"}}}}}}}}}}"#
            ),
            parameters: Vec::new(),
            output_key: "Read",
            expected: PLAIN_VALUE.to_string(),
        },
        ReadCase {
            case: "the producing stack addressed by its stack id",
            stack_name: "reads-by-stack-id",
            template: format!(
                r#"{{"Resources":{{"H":{{"Type":"AWS::CloudFormation::WaitConditionHandle"}}}},
                   "Outputs":{{"Read":{{"Value":{{"Fn::GetStackOutput":
                     {{"StackName":"{stack_id}","OutputName":"Plain"}}}}}}}}}}"#
            ),
            parameters: Vec::new(),
            output_key: "Read",
            expected: PLAIN_VALUE.to_string(),
        },
        ReadCase {
            case: "an EXPORTED output is readable the same way",
            stack_name: "reads-exported",
            template: format!(
                r#"{{"Resources":{{"H":{{"Type":"AWS::CloudFormation::WaitConditionHandle"}}}},
                   "Outputs":{{"Read":{{"Value":{{"Fn::GetStackOutput":
                     {{"StackName":"{PRODUCER}","OutputName":"Shared"}}}}}}}}}}"#
            ),
            parameters: Vec::new(),
            output_key: "Read",
            expected: EXPORTED_VALUE.to_string(),
        },
        ReadCase {
            case: "an output whose value is a resource's own identity",
            stack_name: "reads-resource-identity",
            template: format!(
                r#"{{"Resources":{{"H":{{"Type":"AWS::CloudFormation::WaitConditionHandle"}}}},
                   "Outputs":{{"Read":{{"Value":{{"Fn::GetStackOutput":
                     {{"StackName":"{PRODUCER}","OutputName":"FromResource"}}}}}}}}}}"#
            ),
            parameters: Vec::new(),
            output_key: "Read",
            expected: handle_value.to_string(),
        },
        ReadCase {
            case: "the reference nested inside another intrinsic function",
            stack_name: "reads-nested",
            template: format!(
                r#"{{"Resources":{{"H":{{"Type":"AWS::CloudFormation::WaitConditionHandle"}}}},
                   "Outputs":{{"Read":{{"Value":{{"Fn::Join":["/",
                     [{{"Fn::GetStackOutput":{{"StackName":"{PRODUCER}",
                       "OutputName":"Plain"}}}},"tail"]]}}}}}}}}"#
            ),
            parameters: Vec::new(),
            output_key: "Read",
            expected: format!("{PLAIN_VALUE}/tail"),
        },
        ReadCase {
            case: "the stack and the output key named by parameters",
            stack_name: "reads-by-parameter",
            template: r#"{"Parameters":{"Which":{"Type":"String"},"Key":{"Type":"String"}},
                 "Resources":{"H":{"Type":"AWS::CloudFormation::WaitConditionHandle"}},
                 "Outputs":{"Read":{"Value":{"Fn::GetStackOutput":
                   {"StackName":{"Ref":"Which"},"OutputName":{"Fn::Sub":"${Key}"}}}}}}"#
                .to_string(),
            parameters: vec![
                member("Parameters.member.1.ParameterKey", "Which"),
                member("Parameters.member.1.ParameterValue", PRODUCER),
                member("Parameters.member.2.ParameterKey", "Key"),
                member("Parameters.member.2.ParameterValue", "Shared"),
            ],
            output_key: "Read",
            expected: EXPORTED_VALUE.to_string(),
        },
        ReadCase {
            case: "the yaml spelling of the same read",
            stack_name: "reads-in-yaml",
            template: format!(
                "Resources:\n  H:\n    Type: AWS::CloudFormation::WaitConditionHandle\n\
                 Outputs:\n  Read:\n    Value:\n      Fn::GetStackOutput:\n        \
                 StackName: {PRODUCER}\n        OutputName: Plain\n"
            ),
            parameters: Vec::new(),
            output_key: "Read",
            expected: PLAIN_VALUE.to_string(),
        },
    ]
}

#[tokio::test]
async fn a_stack_output_read_resolves_to_the_value_the_named_stack_carries() {
    let router = router();
    let stack_id = deploy_producer(&router, PRODUCER, PLAIN_VALUE).await;
    let handle_value = output_of("the producer", &router, PRODUCER, "FromResource")
        .await
        .expect("the producer reports the output built from its own resource");

    let cases = read_cases(&stack_id, &handle_value);
    for case in &cases {
        let (status, body) =
            create_stack(&router, case.stack_name, &case.template, &case.parameters).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected CreateStack to succeed, got {status} {body}",
            case.case
        );
    }
    wait_for_the_transition().await;

    for case in &cases {
        let (stack_status, reason) = status_of(&router, case.stack_name).await;
        assert_eq!(
            stack_status, "CREATE_COMPLETE",
            "case {:?}: expected the read to deploy, got {stack_status:?} ({reason:?})",
            case.case
        );
        let reported = output_of(case.case, &router, case.stack_name, case.output_key).await;
        assert_eq!(
            reported.as_deref(),
            Some(case.expected.as_str()),
            "case {:?}: expected the output {:?} to carry {:?}, got {reported:?}",
            case.case,
            case.output_key,
            case.expected
        );
    }
}

#[tokio::test]
async fn a_read_in_a_resource_property_reaches_the_service_that_owns_the_resource() {
    let router = router();
    deploy_producer(&router, PRODUCER, PLAIN_VALUE).await;

    let case = "a resource property reads another stack's output";
    let template = format!(
        r#"{{"Resources":{{"Topic":{{"Type":"AWS::SNS::Topic","Properties":{{
             "TopicName":"reading-topic",
             "DisplayName":{{"Fn::GetStackOutput":
               {{"StackName":"{PRODUCER}","OutputName":"Plain"}}}}}}}}}},
           "Outputs":{{"Arn":{{"Value":{{"Ref":"Topic"}}}}}}}}"#
    );
    let (status, body) = create_stack(&router, "reads-into-a-property", &template, &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected CreateStack to succeed, got {status} {body}"
    );
    wait_for_the_transition().await;

    let arn = output_of(case, &router, "reads-into-a-property", "Arn")
        .await
        .expect("the stack reports the topic it created");
    // The value the OWNING service was asked for: the property reached Amazon SNS resolved,
    // not as the reference's own text.
    let (status, attributes) = sns(&router, "GetTopicAttributes", &[("TopicArn", &arn)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected GetTopicAttributes({arn}) to succeed, got {status} {attributes}"
    );
    let display_name = xml_blocks(&attributes, "entry")
        .iter()
        .find(|entry| xml_text(entry, "key").as_deref() == Some("DisplayName"))
        .and_then(|entry| xml_text(entry, "value"));
    assert_eq!(
        display_name.as_deref(),
        Some(PLAIN_VALUE),
        "case {case:?}: expected the topic's DisplayName to carry the output's value, got \
         {display_name:?}"
    );
}

#[tokio::test]
async fn a_read_of_an_output_nothing_carries_fails_the_deployment_with_its_own_reason() {
    let router = router();
    deploy_producer(&router, PRODUCER, PLAIN_VALUE).await;

    // One row per way the reference can name nothing: an output the stack does not declare,
    // and a stack this Region does not hold.
    let rows = [
        (
            "an output the named stack does not declare",
            "reads-no-such-output",
            Reference::new(PRODUCER, "NoSuchOutput"),
        ),
        (
            "a stack this Region does not hold",
            "reads-no-such-stack",
            Reference::new("no-such-producer", "Plain"),
        ),
    ];
    for (case, stack_name, reference) in rows {
        let template = format!(
            r#"{{"Resources":{{"Topic":{{"Type":"AWS::SNS::Topic","Properties":{{
                 "DisplayName":{{"Fn::GetStackOutput":
                   {{"StackName":"{}","OutputName":"{}"}}}}}}}}}}}}"#,
            reference.stack, reference.output
        );
        let (status, body) = create_stack(&router, stack_name, &template, &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the CreateStack call itself to be accepted, got {status} \
             {body}"
        );
        wait_for_the_transition().await;
        wait_for_the_transition().await;
        let (stack_status, reason) = status_of(&router, stack_name).await;
        assert!(
            stack_status.starts_with("ROLLBACK") || stack_status == "CREATE_FAILED",
            "case {case:?}: expected the create to fail over the unresolvable read, got \
             {stack_status:?} ({reason:?})"
        );
        assert!(
            reason.contains(&no_output_named(&reference)),
            "case {case:?}: expected the reason to name the output that could not be read \
             ({:?}), got {reason:?}",
            no_output_named(&reference)
        );
    }
}

#[tokio::test]
async fn the_value_a_read_resolved_is_kept_until_the_reading_stack_is_deployed_again() {
    let router = router();
    deploy_producer(&router, PRODUCER, PLAIN_VALUE).await;

    let case = "the read is a snapshot of the value at the reader's own deployment";
    let template = format!(
        r#"{{"Resources":{{"H":{{"Type":"AWS::CloudFormation::WaitConditionHandle"}}}},
           "Outputs":{{"Read":{{"Value":{{"Fn::GetStackOutput":
             {{"StackName":"{PRODUCER}","OutputName":"Plain"}}}}}}}}}}"#
    );
    let (status, body) = create_stack(&router, "snapshot-reader", &template, &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected CreateStack to succeed, got {status} {body}"
    );
    wait_for_the_transition().await;
    assert_eq!(
        output_of(case, &router, "snapshot-reader", "Read").await,
        Some(PLAIN_VALUE.to_string()),
        "case {case:?}: expected the reader to resolve the producer's value"
    );

    // The producer publishes a NEW value for the same output.
    const REVISED: &str = "revised-output-value";
    let (status, body) = call(
        &router,
        "UpdateStack",
        &[
            member("StackName", PRODUCER),
            member("TemplateBody", &producer_template(REVISED)),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the producer's update to succeed, got {status} {body}"
    );
    wait_for_the_transition().await;
    assert_eq!(
        output_of(case, &router, PRODUCER, "Plain").await,
        Some(REVISED.to_string()),
        "case {case:?}: expected the producer to publish the revised value"
    );

    // The reader was not deployed again, so it still reports the value it read.
    assert_eq!(
        output_of(case, &router, "snapshot-reader", "Read").await,
        Some(PLAIN_VALUE.to_string()),
        "case {case:?}: expected the reader to keep the value its own deployment read"
    );

    // Deploying the reader again is what takes the new value.
    let (status, body) = call(
        &router,
        "UpdateStack",
        &[
            member("StackName", "snapshot-reader"),
            member(
                "TemplateBody",
                &format!(
                    r#"{{"Resources":{{"H":{{"Type":
                         "AWS::CloudFormation::WaitConditionHandle"}}}},
                       "Outputs":{{"Read":{{"Value":{{"Fn::GetStackOutput":
                         {{"StackName":"{PRODUCER}","OutputName":"Plain"}}}}}},
                       "Marker":{{"Value":"redeployed"}}}}}}"#
                ),
            ),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the reader's update to succeed, got {status} {body}"
    );
    wait_for_the_transition().await;
    assert_eq!(
        output_of(case, &router, "snapshot-reader", "Read").await,
        Some(REVISED.to_string()),
        "case {case:?}: expected the redeployed reader to read the revised value"
    );
}
