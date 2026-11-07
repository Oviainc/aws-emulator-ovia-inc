// SPDX-License-Identifier: CC0-1.0

//! What a change set PREVIEWS, end to end over the awsQuery wire.
//!
//! The rule every case here holds the service to: **a change set is computed by comparing
//! the stack's REVISION — its template together with the parameter values in force — with
//! the revision the request submits, with every intrinsic function evaluated before the two
//! are compared; each resource whose deployed value would differ is reported with one
//! `ResourceChangeDetail` per attribute or property that differs, naming the entity the new
//! value is built from (`ChangeSource`/`CausingEntity`), whether that property's change
//! recreates the resource (`Target.RequiresRecreation`) and whether CloudFormation can
//! determine the new value now (`Evaluation`); `Replacement` and `PolicyAction` are read off
//! those details.** A change set that computes no change is the only one that fails.
//!
//! And for creating one: **a change-set name a stack already carries is answered by
//! comparing what the request submits with what the stored change set carries — the same
//! attributes name the same change set, differing ones cannot be created under that name and
//! the refusal names the attribute that differs — while a request that would CREATE a stack
//! that already exists is a `ValidationError` about the request, not a change-set name
//! collision.**
//!
//! The recorded traces this suite was written for are ONE row of each table below: the rows
//! span the kinds of edit a change set can carry (a parameter value, a property the template
//! states outright, the property a resource takes its NAME from, an attribute beside the
//! properties, a resource added, removed or retyped, a reference to a resource the plan
//! recreates) and the attributes a re-created name can differ in. Every read goes through
//! the real request path (form-encoded body in, XML envelope out), so what is asserted is
//! what a client sees.

use std::sync::Arc;

use service_com_amazonaws_cloudformation_cloudformation::state::change_action;
use service_com_amazonaws_cloudformation_cloudformation::state::change_plan::{
    change_source, evaluation, policy_action, replacement, requires_recreation,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::change_set_lifecycle::settle_the_store;
use crate::stack_lifecycle::{
    call, create_stack, member, router_with_engine, store, xml_blocks, xml_text, xml_texts,
};

/// The template every preview case starts from: two resources, one naming itself from a
/// parameter and the other reading an attribute off the first.
///
/// `TopicName` is the property `AWS::SNS::Topic` takes its physical name from and
/// `AlarmName` the one `AWS::CloudWatch::Alarm` does (CloudFormation's own convention: the
/// type's last segment plus `Name`), so an edit to either recreates its resource while an
/// edit to `DisplayName` does not. The reader is an ALARM because the rows below deploy this
/// one template to SEVERAL stacks of one account: this emulator provisions nothing for that
/// type, so no owning service is asked to hold two resources of the same name.
fn base_template() -> String {
    r#"{"Parameters":{"TopicName":{"Type":"String","Default":"first-topic"},
         "Label":{"Type":"String","Default":"first-label"}},
       "Resources":{
         "Topic":{"Type":"AWS::SNS::Topic","Properties":{
            "TopicName":{"Ref":"TopicName"},"DisplayName":{"Ref":"Label"}}},
         "Reader":{"Type":"AWS::CloudWatch::Alarm","Properties":{
            "AlarmName":"reader-alarm",
            "Tags":[{"Key":"topic","Value":{"Fn::GetAtt":["Topic","TopicArn"]}}]}}}}"#
        .to_string()
}

/// `base_template` with `replacement` spliced in for `original`, which is how a row states
/// "the same template, with this one edit".
fn edited(original: &str, replacement: &str) -> String {
    let template = base_template();
    assert!(
        template.contains(original),
        "the base template must carry {original:?} for a row to edit it"
    );
    template.replace(original, replacement)
}

/// The inner text of every non-nested `<tag>…</tag>` block, in document order: the shared
/// scanner, named for how this suite reads nested lists.
fn blocks(xml: &str, tag: &str) -> Vec<String> {
    xml_blocks(xml, tag)
}

/// One `ResourceChangeDetail` as the response carries it, flattened for comparison.
#[derive(Debug, PartialEq, Eq)]
struct Detail {
    attribute: Option<String>,
    name: Option<String>,
    requires_recreation: Option<String>,
    evaluation: Option<String>,
    change_source: Option<String>,
    causing_entity: Option<String>,
}

/// The expected form of one detail, as a row spells it.
fn detail(
    attribute: &str,
    name: Option<&str>,
    requires_recreation: &str,
    evaluation: &str,
    change_source: &str,
    causing_entity: Option<&str>,
) -> Detail {
    Detail {
        attribute: Some(attribute.to_string()),
        name: name.map(ToString::to_string),
        requires_recreation: Some(requires_recreation.to_string()),
        evaluation: Some(evaluation.to_string()),
        change_source: Some(change_source.to_string()),
        causing_entity: causing_entity.map(ToString::to_string),
    }
}

/// One `ResourceChange` as the response carries it.
#[derive(Debug)]
struct ReportedChange {
    action: Option<String>,
    logical_resource_id: Option<String>,
    resource_type: Option<String>,
    replacement: Option<String>,
    policy_action: Option<String>,
    scope: Vec<String>,
    details: Vec<Detail>,
}

/// Every `ResourceChange` a `DescribeChangeSet` response carries, in response order.
fn reported_changes(xml: &str) -> Vec<ReportedChange> {
    blocks(xml, "ResourceChange")
        .iter()
        .map(|change| ReportedChange {
            action: xml_text(change, "Action"),
            logical_resource_id: xml_text(change, "LogicalResourceId"),
            resource_type: xml_text(change, "ResourceType"),
            replacement: xml_text(change, "Replacement"),
            policy_action: xml_text(change, "PolicyAction"),
            scope: blocks(change, "Scope")
                .first()
                .map(|scope| xml_texts(scope, "member"))
                .unwrap_or_default(),
            details: blocks(change, "Details")
                .first()
                .map(|details| {
                    blocks(details, "member")
                        .iter()
                        .map(|entry| Detail {
                            attribute: xml_text(entry, "Attribute"),
                            name: xml_text(entry, "Name"),
                            requires_recreation: xml_text(entry, "RequiresRecreation"),
                            evaluation: xml_text(entry, "Evaluation"),
                            change_source: xml_text(entry, "ChangeSource"),
                            causing_entity: xml_text(entry, "CausingEntity"),
                        })
                        .collect()
                })
                .unwrap_or_default(),
        })
        .collect()
}

/// One expected change of a row.
struct ExpectedChange {
    action: &'static str,
    logical_resource_id: &'static str,
    resource_type: &'static str,
    replacement: Option<&'static str>,
    policy_action: Option<&'static str>,
    scope: Vec<&'static str>,
    details: Vec<Detail>,
}

/// One row of the preview table: an edit to the base revision, and the plan it must answer.
struct PreviewCase {
    case: &'static str,
    stack: &'static str,
    /// The template the change set submits; `None` reuses the stack's (`UsePreviousTemplate`).
    template: Option<String>,
    /// The `(key, value)` parameters the change set submits; empty leaves the stack's in
    /// force.
    parameters: Vec<(&'static str, &'static str)>,
    changes: Vec<ExpectedChange>,
}

fn preview_cases() -> Vec<PreviewCase> {
    vec![
        // The recorded trace's own case: the change set edits ONLY a parameter value, and
        // the parameter is the one a resource takes its name from.
        PreviewCase {
            case: "a parameter the resource takes its name from",
            stack: "preview-name-parameter",
            template: None,
            parameters: vec![("TopicName", "second-topic"), ("Label", "first-label")],
            changes: vec![
                // The reader does not change itself: it reads an attribute off the topic,
                // and the topic is being recreated, so the value it will read changes.
                ExpectedChange {
                    action: change_action::MODIFY,
                    logical_resource_id: "Reader",
                    resource_type: "AWS::CloudWatch::Alarm",
                    replacement: Some(replacement::FALSE),
                    policy_action: None,
                    scope: vec!["Properties"],
                    details: vec![detail(
                        "Properties",
                        Some("Tags"),
                        requires_recreation::NEVER,
                        evaluation::STATIC,
                        change_source::RESOURCE_ATTRIBUTE,
                        Some("Topic.TopicArn"),
                    )],
                },
                ExpectedChange {
                    action: change_action::MODIFY,
                    logical_resource_id: "Topic",
                    resource_type: "AWS::SNS::Topic",
                    replacement: Some(replacement::TRUE),
                    policy_action: Some(policy_action::REPLACE_AND_DELETE),
                    scope: vec!["Properties"],
                    details: vec![
                        detail(
                            "Properties",
                            Some("TopicName"),
                            requires_recreation::ALWAYS,
                            evaluation::STATIC,
                            change_source::PARAMETER_REFERENCE,
                            Some("TopicName"),
                        ),
                        detail(
                            "Properties",
                            Some("TopicName"),
                            requires_recreation::ALWAYS,
                            evaluation::DYNAMIC,
                            change_source::DIRECT_MODIFICATION,
                            None,
                        ),
                    ],
                },
            ],
        },
        PreviewCase {
            case: "a parameter a property that is not the resource's name reads",
            stack: "preview-other-parameter",
            template: None,
            parameters: vec![("TopicName", "first-topic"), ("Label", "second-label")],
            changes: vec![ExpectedChange {
                action: change_action::MODIFY,
                logical_resource_id: "Topic",
                resource_type: "AWS::SNS::Topic",
                replacement: Some(replacement::FALSE),
                policy_action: None,
                scope: vec!["Properties"],
                details: vec![
                    detail(
                        "Properties",
                        Some("DisplayName"),
                        requires_recreation::NEVER,
                        evaluation::STATIC,
                        change_source::PARAMETER_REFERENCE,
                        Some("Label"),
                    ),
                    detail(
                        "Properties",
                        Some("DisplayName"),
                        requires_recreation::NEVER,
                        evaluation::DYNAMIC,
                        change_source::DIRECT_MODIFICATION,
                        None,
                    ),
                ],
            }],
        },
        PreviewCase {
            case: "a property the template states outright",
            stack: "preview-literal-edit",
            template: Some(edited(r#""DisplayName":{"Ref":"Label"}"#, r#""DisplayName":"stated""#)),
            parameters: Vec::new(),
            changes: vec![ExpectedChange {
                action: change_action::MODIFY,
                logical_resource_id: "Topic",
                resource_type: "AWS::SNS::Topic",
                replacement: Some(replacement::FALSE),
                policy_action: None,
                scope: vec!["Properties"],
                details: vec![detail(
                    "Properties",
                    Some("DisplayName"),
                    requires_recreation::NEVER,
                    evaluation::STATIC,
                    change_source::DIRECT_MODIFICATION,
                    None,
                )],
            }],
        },
        PreviewCase {
            case: "the name property stated outright, which recreates the resource",
            stack: "preview-literal-name",
            template: Some(edited(r#""AlarmName":"reader-alarm""#, r#""AlarmName":"reader-alarm-2""#)),
            parameters: Vec::new(),
            changes: vec![ExpectedChange {
                action: change_action::MODIFY,
                logical_resource_id: "Reader",
                resource_type: "AWS::CloudWatch::Alarm",
                replacement: Some(replacement::TRUE),
                policy_action: Some(policy_action::REPLACE_AND_DELETE),
                scope: vec!["Properties"],
                details: vec![detail(
                    "Properties",
                    Some("AlarmName"),
                    requires_recreation::ALWAYS,
                    evaluation::STATIC,
                    change_source::DIRECT_MODIFICATION,
                    None,
                )],
            }],
        },
        // "With the UpdateReplacePolicy attribute you can specify how CloudFormation handles
        // the [old] resource when the resource is replaced": the catalogue is Delete (the
        // default, the row above), Retain and Snapshot — one row each.
        PreviewCase {
            case: "a replaced resource whose UpdateReplacePolicy retains the old one",
            stack: "preview-retain",
            template: Some(edited(
                r#""Reader":{"Type":"AWS::CloudWatch::Alarm","Properties":{
            "AlarmName":"reader-alarm","#,
                r#""Reader":{"Type":"AWS::CloudWatch::Alarm","UpdateReplacePolicy":"Retain","Properties":{
            "AlarmName":"renamed-alarm","#,
            )),
            parameters: Vec::new(),
            changes: vec![ExpectedChange {
                action: change_action::MODIFY,
                logical_resource_id: "Reader",
                resource_type: "AWS::CloudWatch::Alarm",
                replacement: Some(replacement::TRUE),
                policy_action: Some(policy_action::REPLACE_AND_RETAIN),
                scope: vec!["Properties", "UpdateReplacePolicy"],
                details: vec![
                    detail(
                        "Properties",
                        Some("AlarmName"),
                        requires_recreation::ALWAYS,
                        evaluation::STATIC,
                        change_source::DIRECT_MODIFICATION,
                        None,
                    ),
                    detail(
                        "UpdateReplacePolicy",
                        None,
                        requires_recreation::NEVER,
                        evaluation::STATIC,
                        change_source::DIRECT_MODIFICATION,
                        None,
                    ),
                ],
            }],
        },
        PreviewCase {
            case: "a replaced resource whose UpdateReplacePolicy snapshots the old one",
            stack: "preview-snapshot",
            template: Some(edited(
                r#""Reader":{"Type":"AWS::CloudWatch::Alarm","Properties":{
            "AlarmName":"reader-alarm","#,
                r#""Reader":{"Type":"AWS::CloudWatch::Alarm","UpdateReplacePolicy":"Snapshot","Properties":{
            "AlarmName":"renamed-alarm","#,
            )),
            parameters: Vec::new(),
            changes: vec![ExpectedChange {
                action: change_action::MODIFY,
                logical_resource_id: "Reader",
                resource_type: "AWS::CloudWatch::Alarm",
                replacement: Some(replacement::TRUE),
                policy_action: Some(policy_action::REPLACE_AND_SNAPSHOT),
                scope: vec!["Properties", "UpdateReplacePolicy"],
                details: vec![
                    detail(
                        "Properties",
                        Some("AlarmName"),
                        requires_recreation::ALWAYS,
                        evaluation::STATIC,
                        change_source::DIRECT_MODIFICATION,
                        None,
                    ),
                    detail(
                        "UpdateReplacePolicy",
                        None,
                        requires_recreation::NEVER,
                        evaluation::STATIC,
                        change_source::DIRECT_MODIFICATION,
                        None,
                    ),
                ],
            }],
        },
        // An attribute beside the properties is one of the model's other `ResourceAttribute`
        // values, and recreating a resource is a property-level judgement: a `Metadata` block
        // never does it.
        PreviewCase {
            case: "an attribute beside the properties",
            stack: "preview-metadata",
            template: Some(edited(
                r#""Topic":{"Type":"AWS::SNS::Topic","Properties":{"#,
                r#""Topic":{"Type":"AWS::SNS::Topic","Metadata":{"owner":"platform"},"Properties":{"#,
            )),
            parameters: Vec::new(),
            changes: vec![ExpectedChange {
                action: change_action::MODIFY,
                logical_resource_id: "Topic",
                resource_type: "AWS::SNS::Topic",
                replacement: Some(replacement::FALSE),
                policy_action: None,
                scope: vec!["Metadata"],
                details: vec![detail(
                    "Metadata",
                    None,
                    requires_recreation::NEVER,
                    evaluation::STATIC,
                    change_source::DIRECT_MODIFICATION,
                    None,
                )],
            }],
        },
        // An addition and a removal change the whole resource rather than an attribute of
        // it, so neither carries a detail, a scope or a replacement.
        PreviewCase {
            case: "a resource added and a resource removed",
            stack: "preview-add-remove",
            template: Some(
                r#"{"Parameters":{"TopicName":{"Type":"String","Default":"first-topic"},
                     "Label":{"Type":"String","Default":"first-label"}},
                   "Resources":{
                     "Topic":{"Type":"AWS::SNS::Topic","Properties":{
                        "TopicName":{"Ref":"TopicName"},"DisplayName":{"Ref":"Label"}}},
                     "Queue":{"Type":"AWS::SQS::Queue"}}}"#
                    .to_string(),
            ),
            parameters: Vec::new(),
            changes: vec![
                ExpectedChange {
                    action: change_action::ADD,
                    logical_resource_id: "Queue",
                    resource_type: "AWS::SQS::Queue",
                    replacement: None,
                    policy_action: None,
                    scope: Vec::new(),
                    details: Vec::new(),
                },
                ExpectedChange {
                    action: change_action::REMOVE,
                    logical_resource_id: "Reader",
                    resource_type: "AWS::CloudWatch::Alarm",
                    replacement: None,
                    policy_action: None,
                    scope: Vec::new(),
                    details: Vec::new(),
                },
            ],
        },
        // A resource whose TYPE changed cannot be modified into place.
        PreviewCase {
            case: "a resource whose type changed",
            stack: "preview-retyped",
            template: Some(edited(
                r#""Reader":{"Type":"AWS::CloudWatch::Alarm""#,
                r#""Reader":{"Type":"AWS::SQS::Queue""#,
            )),
            parameters: Vec::new(),
            changes: vec![
                ExpectedChange {
                    action: change_action::REMOVE,
                    logical_resource_id: "Reader",
                    resource_type: "AWS::CloudWatch::Alarm",
                    replacement: None,
                    policy_action: None,
                    scope: Vec::new(),
                    details: Vec::new(),
                },
                ExpectedChange {
                    action: change_action::ADD,
                    logical_resource_id: "Reader",
                    resource_type: "AWS::SQS::Queue",
                    replacement: None,
                    policy_action: None,
                    scope: Vec::new(),
                    details: Vec::new(),
                },
            ],
        },
    ]
}

/// Pull every pending transition into the past and let a read materialise it, so what the
/// cases assert is the emulator's own settling rather than the wall clock — the seam
/// `change_set_lifecycle` owns and documents.
async fn settle(router: &MultiServiceRouter, engine: &Arc<engine::InMemoryEngine>) {
    settle_the_store(router, engine).await;
}

/// The members that submit one row's parameters.
fn parameter_members(parameters: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut members = Vec::new();
    for (position, (key, value)) in parameters.iter().enumerate() {
        let position = position + 1;
        members.push(member(
            &format!("Parameters.member.{position}.ParameterKey"),
            key,
        ));
        members.push(member(
            &format!("Parameters.member.{position}.ParameterValue"),
            value,
        ));
    }
    members
}

#[tokio::test]
async fn a_change_set_reports_the_detail_behind_every_change_it_plans() {
    let (router, engine) = router_with_engine();
    for case in preview_cases() {
        let name = case.case;
        let (code, body) = create_stack(&router, case.stack, &base_template(), &[]).await;
        assert_eq!(
            code, 200,
            "case {name:?}: the stack the change set is for must be created, got {code} {body}"
        );
        settle(&router, &engine).await;

        let mut members = vec![
            member("StackName", case.stack),
            member("ChangeSetName", "preview"),
        ];
        match &case.template {
            Some(template) => members.push(member("TemplateBody", template)),
            None => members.push(member("UsePreviousTemplate", "true")),
        }
        members.extend(parameter_members(&case.parameters));
        let (code, body) = call(&router, "CreateChangeSet", &members).await;
        assert_eq!(
            code, 200,
            "case {name:?}: AWS creates this change set, got {code} {body}"
        );
        settle(&router, &engine).await;

        let (code, body) = call(
            &router,
            "DescribeChangeSet",
            &[
                member("ChangeSetName", "preview"),
                member("StackName", case.stack),
            ],
        )
        .await;
        assert_eq!(
            code, 200,
            "case {name:?}: the change set must be describable, got {code} {body}"
        );
        // A change set that computed a plan is CREATE_COMPLETE and executable; only one
        // that computed nothing fails.
        assert_eq!(
            xml_text(&body, "Status").as_deref(),
            Some("CREATE_COMPLETE"),
            "case {name:?}: a change set with changes completes, got {body}"
        );
        assert_eq!(
            xml_text(&body, "ExecutionStatus").as_deref(),
            Some("AVAILABLE"),
            "case {name:?}: a completed change set is executable, got {body}"
        );

        let reported = reported_changes(&body);
        assert_eq!(
            reported.len(),
            case.changes.len(),
            "case {name:?}: expected {} change(s), got {reported:?}",
            case.changes.len()
        );
        for (expected, actual) in case.changes.iter().zip(&reported) {
            let at = expected.logical_resource_id;
            assert_eq!(
                actual.action.as_deref(),
                Some(expected.action),
                "case {name:?} resource {at:?}: expected action {:?}, got {actual:?}",
                expected.action
            );
            assert_eq!(
                actual.logical_resource_id.as_deref(),
                Some(expected.logical_resource_id),
                "case {name:?}: expected the change of {at:?}, got {actual:?}"
            );
            assert_eq!(
                actual.resource_type.as_deref(),
                Some(expected.resource_type),
                "case {name:?} resource {at:?}: expected type {:?}, got {actual:?}",
                expected.resource_type
            );
            assert_eq!(
                actual.replacement.as_deref(),
                expected.replacement,
                "case {name:?} resource {at:?}: expected Replacement {:?}, got {actual:?}",
                expected.replacement
            );
            assert_eq!(
                actual.policy_action.as_deref(),
                expected.policy_action,
                "case {name:?} resource {at:?}: expected PolicyAction {:?}, got {actual:?}",
                expected.policy_action
            );
            assert_eq!(
                actual.scope,
                expected
                    .scope
                    .iter()
                    .map(|entry| (*entry).to_string())
                    .collect::<Vec<_>>(),
                "case {name:?} resource {at:?}: expected Scope {:?}, got {actual:?}",
                expected.scope
            );
            assert_eq!(
                actual.details, expected.details,
                "case {name:?} resource {at:?}: expected Details {:?}, got {actual:?}",
                expected.details
            );
        }
    }
}

#[tokio::test]
async fn a_change_set_that_would_deploy_the_same_revision_computes_nothing() {
    let (router, engine) = router_with_engine();
    let (code, body) = create_stack(&router, "unchanged", &base_template(), &[]).await;
    assert_eq!(code, 200, "the stack must be created, got {code} {body}");
    settle(&router, &engine).await;

    // The stack's own template and its own parameter values, submitted again: the two
    // revisions deploy the same resources, so there is nothing to plan.
    let (code, body) = call(
        &router,
        "CreateChangeSet",
        &[
            member("StackName", "unchanged"),
            member("ChangeSetName", "same"),
            member("UsePreviousTemplate", "true"),
            member("Parameters.member.1.ParameterKey", "TopicName"),
            member("Parameters.member.1.UsePreviousValue", "true"),
            member("Parameters.member.2.ParameterKey", "Label"),
            member("Parameters.member.2.UsePreviousValue", "true"),
        ],
    )
    .await;
    assert_eq!(code, 200, "CreateChangeSet succeeds, got {code} {body}");
    settle(&router, &engine).await;
    let (code, body) = call(
        &router,
        "DescribeChangeSet",
        &[
            member("ChangeSetName", "same"),
            member("StackName", "unchanged"),
        ],
    )
    .await;
    assert_eq!(code, 200, "the change set is describable, got {code} {body}");
    assert_eq!(
        xml_text(&body, "Status").as_deref(),
        Some("FAILED"),
        "case \"the same revision\": a change set with no change fails, got {body}"
    );
    assert!(
        reported_changes(&body).is_empty(),
        "case \"the same revision\": it reports no change, got {body}"
    );
}

/// One row of the name-reuse table: the attribute the second request states differently,
/// the members that state it, and the attribute the refusal must name — `None` for a
/// request that submits the SAME change set, which is answered with the one that is there.
struct ReuseCase {
    case: &'static str,
    members: Vec<(String, String)>,
    mismatch: Option<&'static str>,
}

fn reuse_cases(template: &str) -> Vec<ReuseCase> {
    vec![
        ReuseCase {
            case: "the very same attributes",
            members: vec![
                member("TemplateBody", template),
                member("Parameters.member.1.ParameterKey", "TopicName"),
                member("Parameters.member.1.ParameterValue", "second-topic"),
                member("Parameters.member.2.ParameterKey", "Label"),
                member("Parameters.member.2.ParameterValue", "first-label"),
                member("Description", "the plan"),
            ],
            mismatch: None,
        },
        ReuseCase {
            case: "the same parameters in the other order",
            members: vec![
                member("TemplateBody", template),
                member("Parameters.member.1.ParameterKey", "Label"),
                member("Parameters.member.1.ParameterValue", "first-label"),
                member("Parameters.member.2.ParameterKey", "TopicName"),
                member("Parameters.member.2.ParameterValue", "second-topic"),
                member("Description", "the plan"),
            ],
            mismatch: None,
        },
        ReuseCase {
            case: "a different parameter value",
            members: vec![
                member("TemplateBody", template),
                member("Parameters.member.1.ParameterKey", "TopicName"),
                member("Parameters.member.1.ParameterValue", "third-topic"),
                member("Parameters.member.2.ParameterKey", "Label"),
                member("Parameters.member.2.ParameterValue", "first-label"),
                member("Description", "the plan"),
            ],
            mismatch: Some("Parameters"),
        },
        ReuseCase {
            case: "a different description",
            members: vec![
                member("TemplateBody", template),
                member("Parameters.member.1.ParameterKey", "TopicName"),
                member("Parameters.member.1.ParameterValue", "second-topic"),
                member("Parameters.member.2.ParameterKey", "Label"),
                member("Parameters.member.2.ParameterValue", "first-label"),
                member("Description", "another plan"),
            ],
            mismatch: Some("Description"),
        },
        ReuseCase {
            case: "a different template body",
            members: vec![
                member(
                    "TemplateBody",
                    &edited(r#""DisplayName":{"Ref":"Label"}"#, r#""DisplayName":"stated""#),
                ),
                member("Parameters.member.1.ParameterKey", "TopicName"),
                member("Parameters.member.1.ParameterValue", "second-topic"),
                member("Parameters.member.2.ParameterKey", "Label"),
                member("Parameters.member.2.ParameterValue", "first-label"),
                member("Description", "the plan"),
            ],
            mismatch: Some("Template"),
        },
        ReuseCase {
            case: "a capability the stored change set does not carry",
            members: vec![
                member("TemplateBody", template),
                member("Parameters.member.1.ParameterKey", "TopicName"),
                member("Parameters.member.1.ParameterValue", "second-topic"),
                member("Parameters.member.2.ParameterKey", "Label"),
                member("Parameters.member.2.ParameterValue", "first-label"),
                member("Description", "the plan"),
                member("Capabilities.member.1", "CAPABILITY_IAM"),
            ],
            mismatch: Some("Capabilities"),
        },
        ReuseCase {
            case: "a tag the stored change set does not carry",
            members: vec![
                member("TemplateBody", template),
                member("Parameters.member.1.ParameterKey", "TopicName"),
                member("Parameters.member.1.ParameterValue", "second-topic"),
                member("Parameters.member.2.ParameterKey", "Label"),
                member("Parameters.member.2.ParameterValue", "first-label"),
                member("Description", "the plan"),
                member("Tags.member.1.Key", "env"),
                member("Tags.member.1.Value", "dev"),
            ],
            mismatch: Some("Tags"),
        },
        ReuseCase {
            case: "a notification topic the stored change set does not carry",
            members: vec![
                member("TemplateBody", template),
                member("Parameters.member.1.ParameterKey", "TopicName"),
                member("Parameters.member.1.ParameterValue", "second-topic"),
                member("Parameters.member.2.ParameterKey", "Label"),
                member("Parameters.member.2.ParameterValue", "first-label"),
                member("Description", "the plan"),
                member(
                    "NotificationARNs.member.1",
                    "arn:aws:sns:us-east-1:000000000000:alerts",
                ),
            ],
            mismatch: Some("NotificationARNs"),
        },
    ]
}

#[tokio::test]
async fn re_creating_a_change_set_name_compares_the_attributes_submitted_under_it() {
    let (router, engine) = router_with_engine();
    let template = base_template();
    let (code, body) = create_stack(&router, "reused", &template, &[]).await;
    assert_eq!(code, 200, "the stack must be created, got {code} {body}");
    settle(&router, &engine).await;

    let stored = vec![
        member("StackName", "reused"),
        member("ChangeSetName", "the-set"),
        member("TemplateBody", &template),
        member("Parameters.member.1.ParameterKey", "TopicName"),
        member("Parameters.member.1.ParameterValue", "second-topic"),
        member("Parameters.member.2.ParameterKey", "Label"),
        member("Parameters.member.2.ParameterValue", "first-label"),
        member("Description", "the plan"),
    ];
    let (code, body) = call(&router, "CreateChangeSet", &stored).await;
    assert_eq!(code, 200, "the first create succeeds, got {code} {body}");
    let first_id = xml_text(&body, "Id").expect("the created change set's id");
    settle(&router, &engine).await;

    for case in reuse_cases(&template) {
        let name = case.case;
        let mut members = vec![
            member("StackName", "reused"),
            member("ChangeSetName", "the-set"),
        ];
        members.extend(case.members);
        let (code, body) = call(&router, "CreateChangeSet", &members).await;
        match case.mismatch {
            None => {
                assert_eq!(
                    code, 200,
                    "case {name:?}: re-submitting the same change set is the change set that \
                     is there, got {code} {body}"
                );
                assert_eq!(
                    xml_text(&body, "Id").as_deref(),
                    Some(first_id.as_str()),
                    "case {name:?}: it answers the stored change set's id, got {body}"
                );
            }
            Some(attribute) => {
                assert_eq!(
                    code, 400,
                    "case {name:?}: a change set of that name with other attributes cannot \
                     be created, got {code} {body}"
                );
                assert_eq!(
                    xml_text(&body, "Code").as_deref(),
                    Some("AlreadyExistsException"),
                    "case {name:?}: expected the AlreadyExistsException error, got {body}"
                );
                let message = xml_text(&body, "Message").unwrap_or_default();
                assert!(
                    message.contains(attribute) && message.contains("the-set"),
                    "case {name:?}: the message must name the change set and the attribute \
                     {attribute:?}, got {message:?}"
                );
            }
        }
    }

    // Every refused and every replayed create left the ONE change set the stack carries.
    let held = store(&engine);
    let names: Vec<String> = held
        .lock()
        .change_sets
        .iter()
        .map(|change_set| change_set.change_set_name.clone())
        .collect();
    assert_eq!(
        names,
        vec!["the-set".to_string()],
        "the stack carries exactly one change set, got {names:?}"
    );
}

#[tokio::test]
async fn creating_a_change_set_for_a_stack_that_exists_is_a_request_error() {
    let (router, engine) = router_with_engine();
    let template = base_template();
    let (code, body) = create_stack(&router, "already-there", &template, &[]).await;
    assert_eq!(code, 200, "the stack must be created, got {code} {body}");
    settle(&router, &engine).await;

    // "You can't use … the CREATE type to create a change set for an existing stack": the
    // REQUEST cannot be satisfied, which is a ValidationError naming the stack and the
    // change set — not the change-set name collision `AlreadyExistsException` reports.
    let (code, body) = call(
        &router,
        "CreateChangeSet",
        &[
            member("StackName", "already-there"),
            member("ChangeSetName", "cs"),
            member("ChangeSetType", "CREATE"),
            member("TemplateBody", &template),
        ],
    )
    .await;
    assert_eq!(code, 400, "expected HTTP 400, got {code} {body}");
    assert_eq!(
        xml_text(&body, "Code").as_deref(),
        Some("ValidationError"),
        "expected the ValidationError error code, got {body}"
    );
    let message = xml_text(&body, "Message").unwrap_or_default();
    assert!(
        message.contains("already-there") && message.contains("cs"),
        "the message must name the stack and the change set, got {message:?}"
    );
    // The refusal created nothing.
    let held = store(&engine);
    assert!(
        held.lock().change_sets.is_empty(),
        "a refused create must leave no change set"
    );
}
