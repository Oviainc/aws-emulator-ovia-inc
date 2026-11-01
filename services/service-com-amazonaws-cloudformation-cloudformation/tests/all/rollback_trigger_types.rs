// SPDX-License-Identifier: CC0-1.0

//! The resource types a `RollbackConfiguration`'s triggers may name.
//!
//! The rule every case here holds the service to: **a stack operation that names a
//! rollback trigger whose `Type` is not one of the CloudWatch alarm resource types
//! `RollbackTrigger$Type` documents is refused synchronously with `ValidationError` and the
//! message `Rollback Trigger Type not supported`, changing nothing — while a trigger naming
//! one of those types is accepted and read back from the store.**
//!
//! Every row drives the real awsQuery request path (form body in, XML envelope out) through
//! the shared harness, so what is asserted is what a client sees. The recorded trace this
//! suite was written for is ONE refused row of the first table: the rows span the supported
//! pair, unsupported CloudWatch and non-CloudWatch types, a near-miss spelling, the case of
//! the letters, the trigger's position in the list and the operation the configuration is
//! supplied on.

use service_com_amazonaws_cloudformation_cloudformation::state::ROLLBACK_TRIGGER_TYPES;

use crate::change_set_lifecycle::{create_change_set, describe_change_set};
use crate::harness::{
    call, create_stack, member, router, wait_for_the_transition, xml_blocks, xml_text, xml_texts,
};
use crate::stack_lifecycle::describe_stack;

/// The refusal CloudFormation words for a trigger type it does not support. It carries no
/// variable part, so it is the same text whatever type the request named.
const NOT_SUPPORTED: &str = "Rollback Trigger Type not supported";

/// A template every row shares, so the only thing a row varies is the trigger type.
const TEMPLATE: &str = r#"{"Description":"rollback triggers"}"#;

/// The ARN a case's trigger points at. A rollback trigger names a CloudWatch alarm; the
/// account id is the one this emulator signs for.
const ALARM_ARN: &str = "arn:aws:cloudwatch:us-east-1:000000000000:alarm:probe";

/// The template a change-set case plans from: `CreateChangeSet` is held to a template that
/// declares at least one resource, so the rows that reach that check carry one. The topic's
/// owning service is mounted beside CloudFormation by the shared harness.
const PLANNED_TEMPLATE: &str = r#"{"Resources":{"Topic":{"Type":"AWS::SNS::Topic"}}}"#;

/// The members that spell one `RollbackConfiguration` with a trigger per entry of `types`.
fn rollback_triggers(types: &[&str]) -> Vec<(String, String)> {
    let mut members = vec![member("RollbackConfiguration.MonitoringTimeInMinutes", "5")];
    for (index, trigger_type) in types.iter().enumerate() {
        let n = index + 1;
        members.push(member(
            &format!("RollbackConfiguration.RollbackTriggers.member.{n}.Arn"),
            ALARM_ARN,
        ));
        members.push(member(
            &format!("RollbackConfiguration.RollbackTriggers.member.{n}.Type"),
            trigger_type,
        ));
    }
    members
}

/// The trigger types the response reports inside its `RollbackTriggers` block, in wire
/// order: the block is read first so a `Type` another element carries cannot be swept up.
fn reported_trigger_types(xml: &str) -> Vec<String> {
    match xml_blocks(xml, "RollbackTriggers").into_iter().next() {
        Some(block) => xml_texts(&block, "Type"),
        None => Vec::new(),
    }
}

/// Assert that `body` is the refusal a rollback trigger with an unsupported type is owed.
fn assert_the_type_refusal(case: &str, status: u16, body: &str) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected the call to be refused, got {status} {body}"
    );
    assert_eq!(
        xml_text(body, "Code").as_deref(),
        Some("ValidationError"),
        "case {case:?}: expected a ValidationError, got {body}"
    );
    assert_eq!(
        xml_text(body, "Message").as_deref(),
        Some(NOT_SUPPORTED),
        "case {case:?}: expected the message {NOT_SUPPORTED:?}, got {body}"
    );
}

/// One create request's trigger types, and whether CloudFormation can monitor them all.
struct CreateCase {
    case: &'static str,
    types: Vec<&'static str>,
    accepted: bool,
}

#[tokio::test]
async fn create_stack_weighs_every_rollback_trigger_type_against_the_supported_pair() {
    let cases = vec![
        CreateCase {
            case: "the alarm type",
            types: vec!["AWS::CloudWatch::Alarm"],
            accepted: true,
        },
        CreateCase {
            case: "the composite alarm type",
            types: vec!["AWS::CloudWatch::CompositeAlarm"],
            accepted: true,
        },
        CreateCase {
            case: "both supported types, one trigger each",
            types: vec!["AWS::CloudWatch::Alarm", "AWS::CloudWatch::CompositeAlarm"],
            accepted: true,
        },
        CreateCase {
            case: "no triggers at all",
            types: vec![],
            accepted: true,
        },
        CreateCase {
            case: "another CloudWatch type that is not an alarm",
            types: vec!["AWS::CloudWatch::Metric"],
            accepted: false,
        },
        CreateCase {
            case: "a CloudWatch dashboard",
            types: vec!["AWS::CloudWatch::Dashboard"],
            accepted: false,
        },
        CreateCase {
            case: "a type belonging to another service",
            types: vec!["AWS::SQS::Queue"],
            accepted: false,
        },
        CreateCase {
            case: "a near-miss spelling of the alarm type",
            types: vec!["AWS::CloudWatch::Alarms"],
            accepted: false,
        },
        CreateCase {
            case: "the alarm type in the wrong case",
            types: vec!["aws::cloudwatch::alarm"],
            accepted: false,
        },
        CreateCase {
            case: "a bare resource name with no namespace",
            types: vec!["Alarm"],
            accepted: false,
        },
        CreateCase {
            case: "an unsupported type AFTER a supported one",
            types: vec!["AWS::CloudWatch::Alarm", "AWS::CloudWatch::Metric"],
            accepted: false,
        },
        CreateCase {
            case: "an unsupported type BEFORE a supported one",
            types: vec!["AWS::CloudWatch::Metric", "AWS::CloudWatch::Alarm"],
            accepted: false,
        },
    ];

    let router = router();
    for (index, case) in cases.iter().enumerate() {
        let stack_name = format!("rbt-create-{index}");
        let (status, body) =
            create_stack(&router, &stack_name, TEMPLATE, &rollback_triggers(&case.types)).await;
        if case.accepted {
            assert_eq!(
                status, 200,
                "case {:?}: the create must be accepted, got {status} {body}",
                case.case
            );
            let (status, described) = describe_stack(&router, &stack_name).await;
            assert_eq!(
                status, 200,
                "case {:?}: expected DescribeStacks to succeed, got {status} {described}",
                case.case
            );
            assert_eq!(
                reported_trigger_types(&described),
                case.types
                    .iter()
                    .map(|trigger_type| trigger_type.to_string())
                    .collect::<Vec<_>>(),
                "case {:?}: the stack reports the triggers it was created with, got \
                 {described}",
                case.case
            );
        } else {
            assert_the_type_refusal(case.case, status, &body);
            // "nothing was created": the name the refused create used names no stack, so
            // reading it back is itself refused.
            let (status, described) = describe_stack(&router, &stack_name).await;
            assert_eq!(
                status, 400,
                "case {:?}: a refused create leaves no stack behind, got {status} {described}",
                case.case
            );
            assert_eq!(
                xml_text(&described, "Code").as_deref(),
                Some("ValidationError"),
                "case {:?}: expected DescribeStacks to refuse an absent stack, got {described}",
                case.case
            );
        }
    }
}

/// Every type the service holds to be supported is really accepted, so the pair and the
/// refusal cannot drift apart. The catalog comes from `RollbackTrigger$Type`'s
/// documentation ("Specify either AWS::CloudWatch::Alarm or AWS::CloudWatch::CompositeAlarm
/// resource types"), read with
/// `smithy_query(query="doc", shape="RollbackTrigger", member="Type")`.
#[tokio::test]
async fn every_supported_trigger_type_is_accepted_and_read_back() {
    assert_eq!(
        ROLLBACK_TRIGGER_TYPES.to_vec(),
        vec!["AWS::CloudWatch::Alarm", "AWS::CloudWatch::CompositeAlarm"],
        "the supported pair is the one RollbackTrigger$Type documents, got {ROLLBACK_TRIGGER_TYPES:?}"
    );

    let router = router();
    for (index, trigger_type) in ROLLBACK_TRIGGER_TYPES.iter().enumerate() {
        let stack_name = format!("rbt-supported-{index}");
        let (status, body) =
            create_stack(&router, &stack_name, TEMPLATE, &rollback_triggers(&[trigger_type])).await;
        assert_eq!(
            status, 200,
            "case {trigger_type:?}: a supported trigger type must be accepted, got {status} \
             {body}"
        );
        let (_, described) = describe_stack(&router, &stack_name).await;
        assert_eq!(
            reported_trigger_types(&described),
            vec![trigger_type.to_string()],
            "case {trigger_type:?}: the stack reports the trigger it was created with, got \
             {described}"
        );
    }
}

/// `UpdateStack` is held to the same rule, and a refused update leaves the configuration the
/// stack already carried.
#[tokio::test]
async fn update_stack_refuses_an_unsupported_trigger_type_and_changes_nothing() {
    let router = router();
    let (status, body) = create_stack(
        &router,
        "rbt-updated",
        TEMPLATE,
        &rollback_triggers(&["AWS::CloudWatch::Alarm"]),
    )
    .await;
    assert_eq!(
        status, 200,
        "the stack under test must be created, got {status} {body}"
    );
    wait_for_the_transition().await;

    // One row per shape of unsupported update, each asserted against the configuration the
    // stack keeps.
    let refused: Vec<(&str, Vec<&str>)> = vec![
        ("another CloudWatch type", vec!["AWS::CloudWatch::Metric"]),
        ("a type of another service", vec!["AWS::SNS::Topic"]),
        (
            "an unsupported type beside a supported one",
            vec!["AWS::CloudWatch::CompositeAlarm", "AWS::CloudWatch::Metric"],
        ),
    ];
    for (case, types) in refused {
        let mut members = vec![
            member("StackName", "rbt-updated"),
            member("TemplateBody", r#"{"Description":"revised"}"#),
        ];
        members.extend(rollback_triggers(&types));
        let (status, body) = call(&router, "UpdateStack", &members).await;
        assert_the_type_refusal(case, status, &body);

        let (_, described) = describe_stack(&router, "rbt-updated").await;
        assert_eq!(
            reported_trigger_types(&described),
            vec!["AWS::CloudWatch::Alarm".to_string()],
            "case {case:?}: a refused update leaves the stack's own triggers in place, got \
             {described}"
        );
        assert_eq!(
            xml_text(&described, "StackStatus").as_deref(),
            Some("CREATE_COMPLETE"),
            "case {case:?}: a refused update does not touch the stack's status, got {described}"
        );
    }

    // The same update with a supported type is carried out and read back.
    let mut members = vec![
        member("StackName", "rbt-updated"),
        member("TemplateBody", r#"{"Description":"revised"}"#),
    ];
    members.extend(rollback_triggers(&["AWS::CloudWatch::CompositeAlarm"]));
    let (status, body) = call(&router, "UpdateStack", &members).await;
    assert_eq!(
        status, 200,
        "case \"a supported type\": the update must be accepted, got {status} {body}"
    );
    wait_for_the_transition().await;
    let (_, described) = describe_stack(&router, "rbt-updated").await;
    assert_eq!(
        reported_trigger_types(&described),
        vec!["AWS::CloudWatch::CompositeAlarm".to_string()],
        "case \"a supported type\": the update's triggers are in force, got {described}"
    );
}

/// `CreateChangeSet` carries a `RollbackConfiguration` for the stack its execution would
/// configure, so it is held to the same rule — and a refused call leaves no change set.
#[tokio::test]
async fn create_change_set_refuses_an_unsupported_trigger_type() {
    let router = router();
    let (status, body) = create_stack(&router, "rbt-planned", TEMPLATE, &[]).await;
    assert_eq!(
        status, 200,
        "the stack the change sets plan for must be created, got {status} {body}"
    );
    wait_for_the_transition().await;

    let refused: Vec<(&str, Vec<&str>)> = vec![
        ("another CloudWatch type", vec!["AWS::CloudWatch::Metric"]),
        ("a type of another service", vec!["AWS::SQS::Queue"]),
    ];
    for (index, (case, types)) in refused.into_iter().enumerate() {
        let change_set_name = format!("rbt-bad-{index}");
        let mut members = vec![member("TemplateBody", PLANNED_TEMPLATE)];
        members.extend(rollback_triggers(&types));
        let (status, body) =
            create_change_set(&router, "rbt-planned", &change_set_name, &members).await;
        assert_the_type_refusal(case, status, &body);

        let (status, described) =
            describe_change_set(&router, &change_set_name, Some("rbt-planned")).await;
        assert_eq!(
            status, 404,
            "case {case:?}: a refused call leaves no change set behind, got {status} \
             {described}"
        );
        assert_eq!(
            xml_text(&described, "Code").as_deref(),
            Some("ChangeSetNotFound"),
            "case {case:?}: expected DescribeChangeSet to report the change set absent, got \
             {described}"
        );
    }

    let mut members = vec![member("TemplateBody", PLANNED_TEMPLATE)];
    members.extend(rollback_triggers(&["AWS::CloudWatch::Alarm"]));
    let (status, body) = create_change_set(&router, "rbt-planned", "rbt-good", &members).await;
    assert_eq!(
        status, 200,
        "case \"a supported type\": the change set must be created, got {status} {body}"
    );
    let (status, described) = describe_change_set(&router, "rbt-good", Some("rbt-planned")).await;
    assert_eq!(
        status, 200,
        "case \"a supported type\": expected DescribeChangeSet to succeed, got {status} \
         {described}"
    );
    assert_eq!(
        reported_trigger_types(&described),
        vec!["AWS::CloudWatch::Alarm".to_string()],
        "case \"a supported type\": the change set reports the triggers it was created with, \
         got {described}"
    );
}
