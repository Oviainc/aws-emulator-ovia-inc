// SPDX-License-Identifier: CC0-1.0

//! What a stack reports for an `AWS::FIS::ExperimentTemplate` it declares, seen from
//! CloudFormation's side alone.
//!
//! The rule under test: **a resource type this emulator PROVISIONS is reported as created
//! only where the service that owns it created it, and the identifier the stack publishes
//! for it is the one that service minted.** An experiment template's id is minted by AWS
//! FIS — `Ref` of the type returns it and the type has no name property at all — so there
//! is no identifier CloudFormation could compute and pass off as the resource's: with no
//! FIS service mounted beside CloudFormation (the shape of "the call did not happen"), the
//! stack has to report `CREATE_FAILED` naming the resource and publish NO output value,
//! rather than a `CREATE_COMPLETE` stack whose `TemplateId` output names a template FIS
//! never created.
//!
//! The rows differ along what the rule generalises over: the template spelling (JSON with
//! long-form intrinsics, YAML with the short-form tags), how the output reads the resource
//! back (`Ref` and `Fn::GetAtt Id`), and how many experiment templates one template
//! declares.
//!
//! The half where FIS IS mounted and the create really happens — the stack completing, its
//! output carrying the id FIS minted, that id resolving in FIS, and the stack's deletion
//! deleting the template there — is
//! `core/router/tests/all/cloudformation_provisions_fis_experiment_templates.rs`: a
//! crate-local suite can only ever see one side of a two-crate call.

use serde_json::json;

use crate::stack_lifecycle::{create_stack, describe_stack, wait_for_the_transition, xml_text, xml_texts};

/// The IAM role every experiment template below declares. Any ARN serves: FIS stores
/// `roleArn` as it is given, and no row here reaches FIS at all.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/fis-experiment";

/// One row: a template declaring one or more experiment templates, and the logical ids the
/// stack's failure has to name.
struct Row {
    case: &'static str,
    stack_name: &'static str,
    template: String,
    /// The logical id of the resource whose create is attempted FIRST, which is the one
    /// the failure names (the walk stops at the first refusal).
    refused: &'static str,
}

fn rows() -> Vec<Row> {
    vec![
        // The issue's own template: one experiment template, with the id read back through
        // `Ref`, written in JSON with long-form intrinsics.
        Row {
            case: "one experiment template, Ref read back, JSON spelling",
            stack_name: "fis-json-stack",
            template: json!({
                "Resources": {
                    "FisTemplate": {
                        "Type": "AWS::FIS::ExperimentTemplate",
                        "Properties": {
                            "Description": "a wait experiment",
                            "RoleArn": ROLE_ARN,
                            "StopConditions": [{"Source": "none"}],
                            "Targets": {},
                            "Actions": {
                                "waitAction": {
                                    "ActionId": "aws:fis:wait",
                                    "Parameters": {"duration": "PT1M"}
                                }
                            }
                        }
                    }
                },
                "Outputs": {"TemplateId": {"Value": {"Ref": "FisTemplate"}}}
            })
            .to_string(),
            refused: "FisTemplate",
        },
        // The same declaration in YAML, with the short-form intrinsic tag, and the id read
        // back through `Fn::GetAtt <resource>.Id` instead of `Ref`.
        Row {
            case: "one experiment template, Fn::GetAtt Id read back, YAML spelling",
            stack_name: "fis-yaml-stack",
            template: format!(
                "Resources:\n  \
                   Experiment:\n    \
                     Type: AWS::FIS::ExperimentTemplate\n    \
                     Properties:\n      \
                       Description: a wait experiment\n      \
                       RoleArn: {ROLE_ARN}\n      \
                       StopConditions:\n        \
                         - Source: none\n      \
                       Targets: {{}}\n      \
                       Actions:\n        \
                         waitAction:\n          \
                           ActionId: aws:fis:wait\n          \
                           Parameters:\n            \
                             duration: PT1M\n\
                 Outputs:\n  \
                   TemplateId:\n    \
                     Value: !GetAtt Experiment.Id\n"
            ),
            refused: "Experiment",
        },
        // Two experiment templates in one stack: the first refusal stops the walk, so the
        // stack names it — and neither id is published.
        Row {
            case: "two experiment templates, both read back",
            stack_name: "fis-pair-stack",
            template: json!({
                "Resources": {
                    "First": {
                        "Type": "AWS::FIS::ExperimentTemplate",
                        "Properties": {
                            "Description": "the first experiment",
                            "RoleArn": ROLE_ARN,
                            "StopConditions": [{"Source": "none"}],
                            "Actions": {
                                "waitAction": {
                                    "ActionId": "aws:fis:wait",
                                    "Parameters": {"duration": "PT1M"}
                                }
                            }
                        }
                    },
                    "Second": {
                        "Type": "AWS::FIS::ExperimentTemplate",
                        "Properties": {
                            "Description": "the second experiment",
                            "RoleArn": ROLE_ARN,
                            "StopConditions": [{"Source": "none"}],
                            "Actions": {
                                "waitAction": {
                                    "ActionId": "aws:fis:wait",
                                    "Parameters": {"duration": "PT1M"}
                                }
                            }
                        }
                    }
                },
                "Outputs": {
                    "FirstId": {"Value": {"Ref": "First"}},
                    "SecondId": {"Value": {"Fn::GetAtt": ["Second", "Id"]}}
                }
            })
            .to_string(),
            refused: "First",
        },
    ]
}

/// A declared experiment template whose owning service never created it is not reported as
/// created: the stack fails, names the resource, and publishes no identifier for it.
#[tokio::test]
async fn an_experiment_template_fis_never_created_is_not_reported_as_created() {
    let router = crate::stack_lifecycle::router();

    for row in rows() {
        let (status, body) = create_stack(&router, row.stack_name, &row.template, &[]).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200 from CreateStack, got {status} with body {body}",
            row.case
        );
        wait_for_the_transition().await;
        let (status, body) = describe_stack(&router, row.stack_name).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200 from DescribeStacks, got {status} with body {body}",
            row.case
        );

        assert_eq!(
            xml_text(&body, "StackStatus").as_deref(),
            Some("CREATE_FAILED"),
            "case {:?}: expected the stack to fail rather than publish an experiment \
             template id FIS never minted, got {body}",
            row.case
        );
        let reason = xml_text(&body, "StackStatusReason").unwrap_or_default();
        assert!(
            reason.contains(row.refused),
            "case {:?}: expected the reason to name the resource that failed ({}), got \
             {reason:?}",
            row.case,
            row.refused
        );
        let values = xml_texts(&body, "OutputValue");
        assert!(
            values.is_empty(),
            "case {:?}: expected no output value for a resource nothing created, got \
             {values:?}",
            row.case
        );
    }
}
