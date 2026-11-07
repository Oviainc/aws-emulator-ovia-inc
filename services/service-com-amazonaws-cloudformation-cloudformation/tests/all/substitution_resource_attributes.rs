// SPDX-License-Identifier: CC0-1.0

//! A DOTTED `Fn::Sub` reference — `${Name.Attribute}` — held to what it is, end to end over
//! the awsQuery wire.
//!
//! RULE: `Fn::Sub` resolves a variable with a dot as a RESOURCE ATTRIBUTE ("`${Logical.Attr}`
//! is the same as `Fn::GetAtt`"), so a dotted reference whose first segment names no resource
//! of the template references no attribute at all: CloudFormation refuses the template with
//! `Template error: instance of Fn::Sub references invalid resource attribute <the dotted
//! reference>`, naming the function instance rather than a section, exactly as it does for the
//! `Fn::GetAtt` spelling of the same mistake. Every operation that READS a submitted template
//! is held to it — the ones that deploy it and the one that only summarises it — so a template
//! this service accepts is one that can be deployed.
//!
//! The rows differ along what the rule generalises over: how many segments the reference has
//! (a two-part one naming nothing, a three-part one naming a MAPPING path), whether the
//! variable is supplied by the substitution's own variable map, whether the name is a declared
//! resource, a declared parameter or a pseudo parameter, and the `${!Literal}` escape — plus
//! the operation the template is submitted to.

use serde_json::json;

use crate::harness::{call, member, router, xml_text};

/// A template whose one resource's `Tags` carry the `Fn::Sub` under test, beside a `Mappings`
/// section and a parameter a reference may name.
fn template(substitution: serde_json::Value) -> String {
    json!({
        "Parameters": {"Stage": {"Type": "String", "Default": "alpha"}},
        "Mappings": {"StageConfig": {"alpha": {"LambdaAccount1": "000000000000"}}},
        "Resources": {
            "Store": {"Type": "AWS::S3::Bucket", "Properties": {
                "BucketName": "substitution-attributes-store",
                "Tags": [{"Key": "policy", "Value": substitution}]
            }},
            "Queue": {"Type": "AWS::SQS::Queue", "Properties": {}}
        }
    })
    .to_string()
}

/// One row: the `Fn::Sub` the template carries, and the reference the refusal must name — or
/// `None` where the template is one CloudFormation accepts.
struct Row {
    case: &'static str,
    substitution: serde_json::Value,
    invalid_reference: Option<&'static str>,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a three-part reference naming a mapping value",
            substitution: json!({
                "Fn::Sub": "arn:aws:iam::${StageConfig.alpha.LambdaAccount1}:root"
            }),
            invalid_reference: Some("StageConfig.alpha.LambdaAccount1"),
        },
        Row {
            case: "a two-part reference naming no resource",
            substitution: json!({"Fn::Sub": "${NoSuchResource.Arn}"}),
            invalid_reference: Some("NoSuchResource.Arn"),
        },
        Row {
            case: "a dotted reference naming a PARAMETER, which is no resource",
            substitution: json!({"Fn::Sub": "${Stage.Value}"}),
            invalid_reference: Some("Stage.Value"),
        },
        Row {
            case: "a dotted reference naming a resource of the template",
            substitution: json!({"Fn::Sub": "${Queue.Arn}"}),
            invalid_reference: None,
        },
        Row {
            case: "a dotted reference the substitution's own variable map supplies",
            substitution: json!({
                "Fn::Sub": ["${StageConfig.alpha.LambdaAccount1}", {
                    "StageConfig.alpha.LambdaAccount1": "000000000000"
                }]
            }),
            invalid_reference: None,
        },
        Row {
            case: "a variable with no dot at all, naming a parameter",
            substitution: json!({"Fn::Sub": "${Stage}-tag"}),
            invalid_reference: None,
        },
        Row {
            case: "a pseudo parameter, which carries no dot either",
            substitution: json!({"Fn::Sub": "${AWS::Region}-tag"}),
            invalid_reference: None,
        },
        Row {
            case: "the literal escape, which is no reference",
            substitution: json!({"Fn::Sub": "${!StageConfig.alpha.LambdaAccount1}"}),
            invalid_reference: None,
        },
    ]
}

#[tokio::test]
async fn a_dotted_substitution_reference_must_name_a_resource_attribute() {
    let router = router();
    for row in rows() {
        let case = row.case;
        let body = template(row.substitution.clone());
        // Both the operation that only SUMMARISES a template and the one that validates it
        // for deployment are held to the rule.
        for (action, members) in [
            (
                "GetTemplateSummary",
                vec![member("TemplateBody", &body)],
            ),
            (
                "ValidateTemplate",
                vec![member("TemplateBody", &body)],
            ),
        ] {
            let (code, answer) = call(&router, action, &members).await;
            match row.invalid_reference {
                Some(reference) => {
                    assert_eq!(
                        code, 400,
                        "case {case:?}: {action} must refuse the template, got {code} {answer}"
                    );
                    assert_eq!(
                        xml_text(&answer, "Code").as_deref(),
                        Some("ValidationError"),
                        "case {case:?}: {action} must answer a ValidationError, got {answer}"
                    );
                    let expected = format!(
                        "Template error: instance of Fn::Sub references invalid resource \
                         attribute {reference}"
                    );
                    assert_eq!(
                        xml_text(&answer, "Message").as_deref(),
                        Some(expected.as_str()),
                        "case {case:?}: {action} must name the reference, got {answer}"
                    );
                }
                None => assert_eq!(
                    code, 200,
                    "case {case:?}: {action} must accept the template, got {code} {answer}"
                ),
            }
        }
    }
}
