// SPDX-License-Identifier: CC0-1.0

//! The VALUE every stack output carries, including the outputs an `Fn::ForEach` expansion
//! emits, end to end over the awsQuery wire.
//!
//! The rule under test: **a stack reports every output its template declares with BOTH
//! `Output$OutputKey` and the `Output$OutputValue` its `Value` expression resolved to
//! against the resources the deployment created** — "The value associated with the output"
//! — and an output an `Fn::ForEach` block emits is such an output like any other: one entry
//! per collection item, keyed by the loop's generated output key and valued by that item's
//! resolved expression. A consumer of a stack's outputs reads the value out of the entry it
//! finds the key in, so an entry that carries a key and no value hands it nothing to address
//! the resources the stack just created with.
//!
//! The table spans what the rule generalises over: how many items the loop's collection holds
//! (one, two, three, twenty-four), how that collection is spelled (a reference to a
//! list-typed parameter's `Default`, the same reference resolved from the submitting
//! request's own `ParameterValue`, a list written out in the template), how many loops one
//! `Outputs` section carries (one and two), what the `Value` expression is (a `Ref` to the
//! expanded logical id, an `Fn::GetAtt` on it, and an `Fn::Sub` over the loop variable and
//! the stack's own name), which resource type the value is read off — `Ref` answers an
//! `AWS::SNS::Topic`'s topic ARN and an `AWS::S3::Bucket`'s bucket name, as the
//! CloudFormation resource reference publishes for each type — and an output declared
//! OUTSIDE any loop beside the expanded ones.
//!
//! A value that names a resource is asserted against the `PhysicalResourceId` that resource
//! reports through `DescribeStackResource`, so what the rows pin is that the output really
//! resolved to the resource the deployment created rather than to any particular text.

use crate::harness::{
    call, create_stack, member, router, wait_for_the_transition, xml_blocks, xml_text,
};
use service_com_amazonaws_cloudformation_cloudformation::state::{status, CAPABILITY_AUTO_EXPAND};
use smithy_server_runtime::MultiServiceRouter;

/// The twenty-four slots the widest row's collection holds.
const SLOTS: [&str; 24] = [
    "a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l", "m", "n", "o", "p", "q", "r", "s",
    "t", "u", "v", "w", "x",
];

/// What one output's value must be.
enum Expected {
    /// The `PhysicalResourceId` of the resource under this logical id, which must also start
    /// with the head the resource reference publishes for `Ref` on its type where that
    /// identity has one (an ARN does; a name is the name).
    Resource {
        logical_id: String,
        head: Option<&'static str>,
    },
    /// Text the expression composes, which names no resource identity.
    Text(String),
}

/// The head of an `AWS::SNS::Topic`'s topic ARN — "Ref returns the topic ARN" — in the
/// Region these requests are signed for.
const TOPIC_ARN_HEAD: Option<&str> = Some("arn:aws:sns:us-east-1:");

/// An `AWS::S3::Bucket`'s `Ref` is "the bucket name", which has no head of its own: the row
/// pins it by the round trip against the bucket's own `PhysicalResourceId`.
const BUCKET_NAME_HAS_NO_HEAD: Option<&str> = None;

/// One row: a template, the parameter value the request supplies for it, and the outputs the
/// deployed stack must report.
struct OutputRow {
    case: &'static str,
    stack_name: &'static str,
    template: String,
    supplied: Option<(String, String)>,
    outputs: Vec<(String, Expected)>,
}

/// The twenty-four-resource row: the loop that fans a `CommaDelimitedList` parameter out into
/// one topic per slot, with an `Outputs` loop over the same collection whose value is a `Ref`
/// to the topic of that slot, and one output declared outside the loop whose value is a `Ref`
/// to a resource of another type.
fn fanned_resources() -> OutputRow {
    let template = format!(
        r#"{{"Transform":"AWS::LanguageExtensions",
          "Parameters":{{
            "Ids":{{"Type":"CommaDelimitedList","Default":"{slots}"}},
            "Prefix":{{"Type":"String","Default":"foreach-values-fanned"}}}},
          "Resources":{{
            "Fn::ForEach::Fanned":["Slot",{{"Ref":"Ids"}},
              {{"T${{Slot}}":{{"Type":"AWS::SNS::Topic",
                "Properties":{{"TopicName":{{"Fn::Sub":"${{Prefix}}-${{Slot}}"}}}}}}}}],
            "Sentinel":{{"Type":"AWS::S3::Bucket",
              "Properties":{{"BucketName":"foreach-values-fanned-sentinel"}}}}}},
          "Outputs":{{
            "Fn::ForEach::Ids":["Slot",{{"Ref":"Ids"}},
              {{"Id${{Slot}}":{{"Value":{{"Ref":{{"Fn::Sub":"T${{Slot}}"}}}}}}}}],
            "SentinelName":{{"Value":{{"Ref":"Sentinel"}}}}}}}}"#,
        slots = SLOTS.join(",")
    );
    let mut outputs: Vec<(String, Expected)> = SLOTS
        .iter()
        .map(|slot| {
            (
                format!("Id{slot}"),
                Expected::Resource {
                    logical_id: format!("T{slot}"),
                    head: TOPIC_ARN_HEAD,
                },
            )
        })
        .collect();
    outputs.push((
        "SentinelName".to_string(),
        Expected::Resource {
            logical_id: "Sentinel".to_string(),
            head: BUCKET_NAME_HAS_NO_HEAD,
        },
    ));
    OutputRow {
        case: "twenty-four topics fanned out of a list parameter, each output a Ref to its \
               topic, beside an output declared outside the loop",
        stack_name: "foreach-values-fanned",
        template,
        supplied: None,
        outputs,
    }
}

/// The table, each row's expected outputs in `OutputKey` order — the order
/// [`reported_outputs`] reads them in, since the protocol fixes no order for `Stack$Outputs`.
fn output_rows() -> Vec<OutputRow> {
    let mut rows = untidy_output_rows();
    for row in &mut rows {
        row.outputs.sort_by(|left, right| left.0.cmp(&right.0));
    }
    rows
}

/// The rows as each is written, before [`output_rows`] puts their outputs in key order.
fn untidy_output_rows() -> Vec<OutputRow> {
    vec![
        fanned_resources(),
        OutputRow {
            case: "three topics over a collection written out in the template, each output an \
                   Fn::GetAtt on its topic",
            stack_name: "foreach-values-getatt",
            template: r#"{"Transform":"AWS::LanguageExtensions",
              "Resources":{
                "Fn::ForEach::Topics":["Slot",["one","two","three"],
                  {"T${Slot}":{"Type":"AWS::SNS::Topic",
                    "Properties":{"TopicName":{"Fn::Sub":"foreach-values-getatt-${Slot}"}}}}]},
              "Outputs":{
                "Fn::ForEach::Arns":["Slot",["one","two","three"],
                  {"Arn${Slot}":{"Value":{"Fn::GetAtt":[{"Fn::Sub":"T${Slot}"},"TopicArn"]}}}]}}"#
                .to_string(),
            supplied: None,
            outputs: ["one", "two", "three"]
                .iter()
                .map(|slot| {
                    (
                        format!("Arn{slot}"),
                        Expected::Resource {
                            logical_id: format!("T{slot}"),
                            head: TOPIC_ARN_HEAD,
                        },
                    )
                })
                .collect(),
        },
        OutputRow {
            case: "a one-item collection the request supplies, whose output value is composed \
                   text rather than a resource identity",
            stack_name: "foreach-values-supplied",
            template: r#"{"Transform":"AWS::LanguageExtensions",
              "Parameters":{"Ids":{"Type":"CommaDelimitedList","Default":"unused"}},
              "Resources":{"Sentinel":{"Type":"AWS::SNS::Topic",
                "Properties":{"TopicName":"foreach-values-supplied-sentinel"}}},
              "Outputs":{
                "Fn::ForEach::Names":["Slot",{"Ref":"Ids"},
                  {"Name${Slot}":{"Value":{"Fn::Sub":"${AWS::StackName}-${Slot}"}}}]}}"#
                .to_string(),
            supplied: Some(("Ids".to_string(), "solo".to_string())),
            outputs: vec![(
                "Namesolo".to_string(),
                Expected::Text("foreach-values-supplied-solo".to_string()),
            )],
        },
        OutputRow {
            case: "two loops in one Outputs section over the same two-item collection, one \
                   reading each bucket's name and one its ARN",
            stack_name: "foreach-values-two-loops",
            template: r#"{"Transform":"AWS::LanguageExtensions",
              "Parameters":{"Ids":{"Type":"CommaDelimitedList","Default":"left,right"}},
              "Resources":{
                "Fn::ForEach::Buckets":["Slot",{"Ref":"Ids"},
                  {"B${Slot}":{"Type":"AWS::S3::Bucket",
                    "Properties":{"BucketName":{"Fn::Sub":"foreach-values-two-${Slot}"}}}}]},
              "Outputs":{
                "Fn::ForEach::Names":["Slot",{"Ref":"Ids"},
                  {"Name${Slot}":{"Value":{"Ref":{"Fn::Sub":"B${Slot}"}}}}],
                "Fn::ForEach::Arns":["Slot",{"Ref":"Ids"},
                  {"Arn${Slot}":{"Value":{"Fn::GetAtt":[{"Fn::Sub":"B${Slot}"},"Arn"]}}}]}}"#
                .to_string(),
            supplied: None,
            outputs: vec![
                // "An Amazon S3 bucket ARN carries neither a region nor an account id"
                // (the resource reference's `Arn` return value for this type).
                (
                    "Arnleft".to_string(),
                    Expected::Text("arn:aws:s3:::foreach-values-two-left".to_string()),
                ),
                (
                    "Arnright".to_string(),
                    Expected::Text("arn:aws:s3:::foreach-values-two-right".to_string()),
                ),
                (
                    "Nameleft".to_string(),
                    Expected::Resource {
                        logical_id: "Bleft".to_string(),
                        head: BUCKET_NAME_HAS_NO_HEAD,
                    },
                ),
                (
                    "Nameright".to_string(),
                    Expected::Resource {
                        logical_id: "Bright".to_string(),
                        head: BUCKET_NAME_HAS_NO_HEAD,
                    },
                ),
            ],
        },
    ]
}

/// Every output a `DescribeStacks` response reports, as `(OutputKey, OutputValue)` sorted by
/// key — the value as an `Option`, so an entry that carries NO `OutputValue` member at all is
/// told apart from one whose value is empty text.
///
/// Read INSIDE the `Outputs` block, one `member` at a time, so a key and a value are paired by
/// the element that carries both and the `member`s of the response's other lists cannot be
/// swept up with them.
fn reported_outputs(xml: &str) -> Vec<(String, Option<String>)> {
    let mut outputs: Vec<(String, Option<String>)> = xml_blocks(xml, "Outputs")
        .into_iter()
        .flat_map(|block| xml_blocks(&block, "member"))
        .filter_map(|output| {
            let key = xml_text(&output, "OutputKey")?;
            Some((key, xml_text(&output, "OutputValue")))
        })
        .collect();
    outputs.sort();
    outputs
}

/// The `PhysicalResourceId` a stack reports for one of its resources.
async fn physical_id(
    case: &str,
    router: &MultiServiceRouter,
    stack: &str,
    logical_id: &str,
) -> String {
    let (http_status, body) = call(
        router,
        "DescribeStackResource",
        &[
            member("StackName", stack),
            member("LogicalResourceId", logical_id),
        ],
    )
    .await;
    assert_eq!(
        http_status, 200,
        "case {case:?}: expected {logical_id:?} to be described, got {http_status} {body}"
    );
    xml_text(&body, "PhysicalResourceId").unwrap_or_else(|| {
        panic!("case {case:?}: expected {logical_id:?} to carry a PhysicalResourceId, got {body}")
    })
}

#[tokio::test]
async fn every_output_a_stack_reports_carries_the_value_its_expression_resolved_to() {
    let router = router();
    let rows = output_rows();

    for row in &rows {
        let mut extra = vec![member("Capabilities.member.1", CAPABILITY_AUTO_EXPAND)];
        if let Some((key, value)) = &row.supplied {
            extra.push(member("Parameters.member.1.ParameterKey", key));
            extra.push(member("Parameters.member.1.ParameterValue", value));
        }
        let (http_status, body) = create_stack(&router, row.stack_name, &row.template, &extra).await;
        assert_eq!(
            http_status, 200,
            "case {:?}: expected CreateStack to accept the template, got {http_status} {body}",
            row.case
        );
    }

    wait_for_the_transition().await;

    for row in &rows {
        let case = row.case;
        let (http_status, described) = call(
            &router,
            "DescribeStacks",
            &[member("StackName", row.stack_name)],
        )
        .await;
        assert_eq!(
            http_status, 200,
            "case {case:?}: expected DescribeStacks to succeed, got {http_status} {described}"
        );
        assert_eq!(
            xml_text(&described, "StackStatus").as_deref(),
            Some(status::CREATE_COMPLETE),
            "case {case:?}: expected the template to deploy, got {described}"
        );

        let reported = reported_outputs(&described);
        let keys: Vec<&str> = reported.iter().map(|(key, _)| key.as_str()).collect();
        let expected_keys: Vec<&str> = row.outputs.iter().map(|(key, _)| key.as_str()).collect();
        assert_eq!(
            keys, expected_keys,
            "case {case:?}: expected one output per declared output, got {keys:?}"
        );

        for ((key, value), (_, expected)) in reported.iter().zip(row.outputs.iter()) {
            // The member itself: an output whose value the deployment resolved carries it,
            // and a client reading the entry by key finds a value there.
            let value = value.clone().unwrap_or_else(|| {
                panic!(
                    "case {case:?}: expected output {key:?} to carry an OutputValue member, \
                     got {reported:?}"
                )
            });
            match expected {
                Expected::Resource { logical_id, head } => {
                    let identity = physical_id(case, &router, row.stack_name, logical_id).await;
                    assert_eq!(
                        value, identity,
                        "case {case:?}: expected output {key:?} to be {logical_id:?}'s \
                         identity {identity:?}, got {value:?}"
                    );
                    if let Some(head) = head {
                        assert!(
                            value.starts_with(head),
                            "case {case:?}: expected output {key:?} to be the identity its \
                             type publishes (starting {head:?}), got {value:?}"
                        );
                    }
                }
                Expected::Text(text) => assert_eq!(
                    &value, text,
                    "case {case:?}: expected output {key:?} to be {text:?}, got {value:?}"
                ),
            }
        }
    }
}
