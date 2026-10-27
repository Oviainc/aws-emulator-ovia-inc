// SPDX-License-Identifier: CC0-1.0

//! What `UpdateStack` answers for a stack whose DELETION an owning service REFUSED, end to
//! end over the awsQuery wire.
//!
//! RULE under test: **a stack whose last operation left it in a status that holds no
//! configuration an update could change — a deletion that could not be carried out among
//! them — refuses `UpdateStack`, and the UPDATE change set that PLANS such an update, with
//! HTTP 400 `ValidationError` reading `Stack:<stack-id> is in <StackStatus> state and can
//! not be updated.`, where the stack id is the one the request resolved and the state name
//! is the stack's CURRENT status; and the refusal changes nothing — the stack keeps that
//! status, its stored template and its parameters, records no update event, stores no change
//! set, and the resource the deletion could not remove keeps the configuration it has in the
//! service that owns it.**
//!
//! `UpdateStack` models only `InsufficientCapabilitiesException` and
//! `TokenAlreadyExistsException` (Smithy, this crate's model), so a state refusal arrives as
//! the awsQuery common error `ValidationError` at 400 rather than as an operation-specific
//! shape; `CreateChangeSet` models `AlreadyExistsException`, `InsufficientCapabilitiesException`
//! and `LimitExceededException`, none of which is a state refusal, so the same holds for it.
//!
//! `stack_status_accepts_an_update.rs` spans which `StackStatus` values take an update, each
//! row seeded in the store. This suite drives the one a client really arrives at — a stack
//! an owning service refused to delete — through the wire alone, and pins what the refusal
//! LEAVES BEHIND, which a seeded status cannot show; its second case carries the same status
//! table over to the PLAN of an update, which is accepted exactly where the update is.
//! The rows of the first case differ along what the rule
//! generalises over: which service refused the deletion and so which resource type and
//! logical id the stack is stuck on, whether that deletion walk had already removed another
//! of the stack's resources, how the refused update names the stack (by name, and by the
//! stack id the message has to carry either way), where its template comes from (a submitted
//! body, or the stack's own template reused) and whether it restates a parameter with
//! `UsePreviousValue`.

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use crate::change_set_lifecycle::create_change_set;
use crate::stack_lifecycle::{
    call, create_stack, describe_stack, get_template, member, rds, router, router_with_engine,
    store, wait_for_the_transition, xml_text, xml_texts, REGION,
};
use crate::stack_resource_provisioning::s3;
use service_com_amazonaws_cloudformation_cloudformation::state::{
    resource_status, stack_cannot_be_updated, status,
};

/// How the refused update names the stack: either form resolves the same stack, and the
/// refusal names its stack id whichever was sent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Address {
    ByName,
    ById,
}

/// Where the refused update's template would come from — the two sources an `UpdateStack`
/// this emulator serves can name.
#[derive(Clone, Debug)]
enum Source {
    /// A submitted `TemplateBody`, carrying a real change.
    Body(String),
    /// `UsePreviousTemplate`: the body the stack already holds.
    ThePreviousTemplate,
}

/// One stack whose deletion is refused, and the update that is then asked of it.
struct Row {
    case: &'static str,
    stack: &'static str,
    /// The template the stack is created with.
    template: String,
    /// An object to store in this bucket before the delete, which is what makes Amazon S3
    /// refuse to remove it.
    hold_an_object_in: Option<&'static str>,
    address: Address,
    source: Source,
    /// A parameter the refused update restates with `UsePreviousValue`.
    restated: Option<&'static str>,
    /// The logical id whose deletion the owning service refused.
    refused: &'static str,
    /// A DB instance whose deletion protection must still be on once the update is refused.
    protected_instance: Option<&'static str>,
    /// A bucket Amazon S3 must still hold once the update is refused.
    surviving_bucket: Option<&'static str>,
}

/// A DB instance declaration whose deletion protection is `protected` — Amazon RDS refuses
/// `DeleteDBInstance` while it is on, which is what leaves the stack's deletion failed.
///
/// The instance class comes from a template PARAMETER so a row can restate that parameter
/// with `UsePreviousValue`, the form a client recovering such a stack sends.
fn protected_instance_template(identifier: &str, protected: bool) -> String {
    json!({
        "Parameters": {"InstanceClass": {"Type": "String", "Default": "db.t3.micro"}},
        "Resources": {
            "Db": {
                "Type": "AWS::RDS::DBInstance",
                "Properties": {
                    "DBInstanceIdentifier": identifier,
                    "Engine": "mysql",
                    "DBInstanceClass": {"Ref": "InstanceClass"},
                    "DeletionProtection": protected,
                },
            },
        },
    })
    .to_string()
}

/// The same protected instance beside a topic the deletion walk removes before it reaches
/// the instance: a stack whose refused deletion already took one resource with it.
fn protected_instance_and_topic_template(
    identifier: &str,
    topic: &str,
    protected: bool,
) -> String {
    json!({
        "Resources": {
            "Database": {
                "Type": "AWS::RDS::DBInstance",
                "Properties": {
                    "DBInstanceIdentifier": identifier,
                    "Engine": "postgres",
                    "DBInstanceClass": "db.t3.micro",
                    "DeletionProtection": protected,
                },
            },
            "Topic": {"Type": "AWS::SNS::Topic", "Properties": {"TopicName": topic}},
        },
    })
    .to_string()
}

/// A bucket declaration, optionally with versioning on — the change a row's refused update
/// would make.
fn bucket_template(bucket: &str, versioned: bool) -> String {
    let mut properties = json!({"BucketName": bucket});
    if versioned {
        properties["VersioningConfiguration"] = json!({"Status": "Enabled"});
    }
    json!({"Resources": {"Archive": {"Type": "AWS::S3::Bucket", "Properties": properties}}})
        .to_string()
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "Amazon RDS refuses a protected DB instance; the update names the stack \
                   by its id, submits a body that would lift the protection and restates a \
                   parameter with UsePreviousValue",
            stack: "refused-delete-then-update-db",
            template: protected_instance_template("cfn-refused-update-protected", true),
            hold_an_object_in: None,
            address: Address::ById,
            source: Source::Body(protected_instance_template(
                "cfn-refused-update-protected",
                false,
            )),
            restated: Some("InstanceClass"),
            refused: "Db",
            protected_instance: Some("cfn-refused-update-protected"),
            surviving_bucket: None,
        },
        Row {
            case: "the deletion walk removed the stack's topic before Amazon RDS refused \
                   the instance; the update names the stack by name and submits a body",
            stack: "refused-delete-then-update-db-and-topic",
            template: protected_instance_and_topic_template(
                "cfn-refused-update-protected-2",
                "cfn-refused-update-topic",
                true,
            ),
            hold_an_object_in: None,
            address: Address::ByName,
            source: Source::Body(protected_instance_and_topic_template(
                "cfn-refused-update-protected-2",
                "cfn-refused-update-topic",
                false,
            )),
            restated: None,
            refused: "Database",
            protected_instance: Some("cfn-refused-update-protected-2"),
            surviving_bucket: None,
        },
        Row {
            case: "Amazon S3 refuses a bucket that still holds an object; the update names \
                   the stack by name and reuses the stack's own template",
            stack: "refused-delete-then-update-bucket",
            template: bucket_template("cfn-refused-update-archive", false),
            hold_an_object_in: Some("cfn-refused-update-archive"),
            address: Address::ByName,
            source: Source::ThePreviousTemplate,
            restated: None,
            refused: "Archive",
            protected_instance: None,
            surviving_bucket: Some("cfn-refused-update-archive"),
        },
        Row {
            case: "the same bucket refusal, with the update naming the stack by its id and \
                   submitting a body that would turn versioning on",
            stack: "refused-delete-then-update-bucket-body",
            template: bucket_template("cfn-refused-update-archive-2", false),
            hold_an_object_in: Some("cfn-refused-update-archive-2"),
            address: Address::ById,
            source: Source::Body(bucket_template("cfn-refused-update-archive-2", true)),
            restated: None,
            refused: "Archive",
            protected_instance: None,
            surviving_bucket: Some("cfn-refused-update-archive-2"),
        },
    ]
}

/// Every `ResourceStatus` the stack's event stream carries, newest first.
async fn event_statuses(router: &MultiServiceRouter, stack: &str) -> Vec<String> {
    let (code, body) = call(
        router,
        "DescribeStackEvents",
        &[member("StackName", stack)],
    )
    .await;
    assert_eq!(
        code, 200,
        "DescribeStackEvents({stack}) must answer 200, got {code} {body}"
    );
    xml_texts(&body, "ResourceStatus")
}

/// The `ResourceStatus` the stack reports for one of its resources.
async fn resource_report(router: &MultiServiceRouter, stack: &str, logical_id: &str) -> String {
    let (code, body) = call(
        router,
        "DescribeStackResources",
        &[
            member("StackName", stack),
            member("LogicalResourceId", logical_id),
        ],
    )
    .await;
    assert_eq!(
        code, 200,
        "DescribeStackResources({stack}, {logical_id}) must answer 200, got {code} {body}"
    );
    xml_text(&body, "ResourceStatus").unwrap_or_default()
}

/// The `TemplateBody` the stack holds, as `GetTemplate` serves it.
async fn stored_template(router: &MultiServiceRouter, stack: &str) -> String {
    let (code, body) = get_template(router, stack, None).await;
    assert_eq!(
        code, 200,
        "GetTemplate({stack}) must answer 200, got {code} {body}"
    );
    xml_text(&body, "TemplateBody").unwrap_or_default()
}

#[tokio::test]
async fn a_stack_whose_deletion_was_refused_refuses_an_update_and_keeps_everything() {
    for row in rows() {
        let router = router();
        let (code, created) = create_stack(&router, row.stack, &row.template, &[]).await;
        assert_eq!(
            code, 200,
            "case {:?}: CreateStack must answer 200, got {code} {created}",
            row.case
        );
        // Twice: the stack's own terminal status and the status an owning service owes the
        // resource it is bringing up are two clocks, and both have to be past due before
        // the delete is asked for.
        wait_for_the_transition().await;
        wait_for_the_transition().await;
        let (code, described) = describe_stack(&router, row.stack).await;
        assert_eq!(
            xml_text(&described, "StackStatus").as_deref(),
            Some(status::CREATE_COMPLETE),
            "case {:?}: the stack must be created before the delete, got {code} {described}",
            row.case
        );
        let stack_id = xml_text(&described, "StackId").unwrap_or_default();
        assert!(
            !stack_id.is_empty(),
            "case {:?}: expected the created stack to report a StackId, got {described}",
            row.case
        );

        if let Some(bucket) = row.hold_an_object_in {
            let stored = s3(
                &router,
                "PUT",
                &format!("/{bucket}/an-object"),
                b"the object that keeps the bucket from being deleted",
            )
            .await;
            assert_eq!(
                stored.status, 200,
                "case {:?}: the object must be stored, got {} {}",
                row.case, stored.status, stored.body
            );
        }

        let (code, deleted) =
            call(&router, "DeleteStack", &[member("StackName", row.stack)]).await;
        assert_eq!(
            code, 200,
            "case {:?}: DeleteStack must answer 200, got {code} {deleted}",
            row.case
        );
        wait_for_the_transition().await;
        wait_for_the_transition().await;
        let (code, described) = describe_stack(&router, row.stack).await;
        assert_eq!(
            xml_text(&described, "StackStatus").as_deref(),
            Some(status::DELETE_FAILED),
            "case {:?}: a deletion the owning service refused must leave the stack in {}, \
             got {code} {described}",
            row.case,
            status::DELETE_FAILED
        );
        let reason_before = xml_text(&described, "StackStatusReason").unwrap_or_default();
        let template_before = stored_template(&router, row.stack).await;
        let parameters_before = xml_texts(&described, "ParameterValue");
        let events_before = event_statuses(&router, row.stack).await;

        // The update a client sends next, naming the stack as this row names it.
        let named = match row.address {
            Address::ByName => row.stack.to_string(),
            Address::ById => stack_id.clone(),
        };
        let mut members = vec![member("StackName", &named)];
        match &row.source {
            Source::Body(body) => members.push(member("TemplateBody", body)),
            Source::ThePreviousTemplate => members.push(member("UsePreviousTemplate", "true")),
        }
        if let Some(parameter) = row.restated {
            members.push(member("Parameters.member.1.ParameterKey", parameter));
            members.push(member("Parameters.member.1.UsePreviousValue", "true"));
        }
        let (code, updated) = call(&router, "UpdateStack", &members).await;

        assert_eq!(
            code, 400,
            "case {:?}: a stack in {} must refuse an update, got {code} {updated}",
            row.case,
            status::DELETE_FAILED
        );
        assert_eq!(
            xml_text(&updated, "Code").as_deref(),
            Some("ValidationError"),
            "case {:?}: expected a ValidationError, got {updated}",
            row.case
        );
        // The message's variable parts come from the stack this request resolved and from
        // its CURRENT status — the stack id even where the request named the stack by name.
        let expected = stack_cannot_be_updated(&stack_id, status::DELETE_FAILED);
        assert_eq!(
            xml_text(&updated, "Message").as_deref(),
            Some(expected.as_str()),
            "case {:?}: expected the refusal to read {expected:?}, got {updated}",
            row.case
        );
        assert!(
            xml_text(&updated, "StackId").is_none(),
            "case {:?}: a refused update must not answer a StackId, got {updated}",
            row.case
        );

        // The PLAN of that same update is refused the same way: an UPDATE change set is an
        // update of the stack as it stands, so it is accepted exactly where the update is.
        let mut plan = vec![member("ChangeSetType", "UPDATE")];
        match &row.source {
            Source::Body(body) => plan.push(member("TemplateBody", body)),
            Source::ThePreviousTemplate => plan.push(member("UsePreviousTemplate", "true")),
        }
        let (code, planned) = create_change_set(&router, &named, "the-plan", &plan).await;
        assert_eq!(
            code, 400,
            "case {:?}: a stack in {} must refuse an UPDATE change set, got {code} {planned}",
            row.case,
            status::DELETE_FAILED
        );
        assert_eq!(
            xml_text(&planned, "Code").as_deref(),
            Some("ValidationError"),
            "case {:?}: expected a ValidationError for the plan, got {planned}",
            row.case
        );
        assert_eq!(
            xml_text(&planned, "Message").as_deref(),
            Some(expected.as_str()),
            "case {:?}: expected the plan's refusal to read {expected:?}, got {planned}",
            row.case
        );
        let (code, listed) = call(
            &router,
            "ListChangeSets",
            &[member("StackName", row.stack)],
        )
        .await;
        assert_eq!(
            code, 200,
            "case {:?}: ListChangeSets must answer 200, got {code} {listed}",
            row.case
        );
        assert!(
            xml_texts(&listed, "ChangeSetName").is_empty(),
            "case {:?}: a refused plan must not be stored, got {listed}",
            row.case
        );

        // Nothing about the stack moved: the status, its reason, the template it holds and
        // its parameters are what the refused deletion left, and no update event was
        // recorded.
        let (code, after) = describe_stack(&router, row.stack).await;
        assert_eq!(
            code, 200,
            "case {:?}: DescribeStacks must answer 200, got {code} {after}",
            row.case
        );
        assert_eq!(
            xml_text(&after, "StackStatus").as_deref(),
            Some(status::DELETE_FAILED),
            "case {:?}: expected the refused update to leave the stack in {}, got {after}",
            row.case,
            status::DELETE_FAILED
        );
        assert_eq!(
            xml_text(&after, "StackStatusReason").unwrap_or_default(),
            reason_before,
            "case {:?}: expected the stack's reason to be the one the refused deletion \
             left, got {after}",
            row.case
        );
        assert_eq!(
            xml_text(&after, "LastUpdatedTime"),
            None,
            "case {:?}: a refused update must not record an update time, got {after}",
            row.case
        );
        assert_eq!(
            xml_texts(&after, "ParameterValue"),
            parameters_before,
            "case {:?}: expected the stack's parameters to be unchanged, got {after}",
            row.case
        );
        assert_eq!(
            stored_template(&router, row.stack).await,
            template_before,
            "case {:?}: expected the stack to keep the template it held",
            row.case
        );
        let events_after = event_statuses(&router, row.stack).await;
        assert_eq!(
            events_after, events_before,
            "case {:?}: a refused update must record no event, got {events_after:?}",
            row.case
        );
        assert_eq!(
            resource_report(&router, row.stack, row.refused).await,
            resource_status::DELETE_FAILED,
            "case {:?}: expected {:?} to still report the delete the owning service \
             refused",
            row.case,
            row.refused
        );

        // And nothing in the services that OWN the resources moved either: the instance
        // whose protection the update would have lifted still has it, and the bucket the
        // update would have reconfigured is still the one S3 holds.
        if let Some(identifier) = row.protected_instance {
            let (code, instances) = rds(
                &router,
                "DescribeDBInstances",
                &[("DBInstanceIdentifier", identifier)],
            )
            .await;
            assert_eq!(
                code, 200,
                "case {:?}: the protected instance must survive the refused update, got \
                 {code} {instances}",
                row.case
            );
            assert_eq!(
                xml_text(&instances, "DeletionProtection").as_deref(),
                Some("true"),
                "case {:?}: expected the refused update to leave deletion protection on, \
                 got {instances}",
                row.case
            );
        }
        if let Some(bucket) = row.surviving_bucket {
            let head = s3(&router, "HEAD", &format!("/{bucket}"), b"").await;
            assert_eq!(
                head.status, 200,
                "case {:?}: expected Amazon S3 to still hold the bucket, got {} {}",
                row.case, head.status, head.body
            );
        }
    }
}

/// One row of the status table: the status the stack reports when the plan is asked for, and
/// whether an update — and so the PLAN of one — is accepted from it.
struct StatusRow {
    case: &'static str,
    /// Unique per row, so each row drives its own stack.
    stack: &'static str,
    seeded: &'static str,
    accepted: bool,
}

/// The `StackStatus` values this service puts a stack in, as the model's `StackStatus` enum
/// lists them (read with `smithy_query`): the five an update is accepted from and the five
/// whose own failure another operation has to resolve first. The statuses of an operation
/// still IN PROGRESS are refused by the same rule, and `DELETE_COMPLETE` is a stack that no
/// longer exists and answers that it does not.
const STATUS_ROWS: &[StatusRow] = &[
    StatusRow {
        case: "a create that completed",
        stack: "plan-created",
        seeded: status::CREATE_COMPLETE,
        accepted: true,
    },
    StatusRow {
        case: "an update that completed",
        stack: "plan-updated",
        seeded: status::UPDATE_COMPLETE,
        accepted: true,
    },
    StatusRow {
        case: "an update whose rollback completed",
        stack: "plan-rolled-back",
        seeded: status::UPDATE_ROLLBACK_COMPLETE,
        accepted: true,
    },
    StatusRow {
        case: "a create that failed with its resources preserved",
        stack: "plan-create-failed",
        seeded: status::CREATE_FAILED,
        accepted: true,
    },
    StatusRow {
        case: "an update that failed with its resources preserved",
        stack: "plan-update-failed",
        seeded: status::UPDATE_FAILED,
        accepted: true,
    },
    StatusRow {
        case: "a deletion an owning service refused",
        stack: "plan-delete-failed",
        seeded: status::DELETE_FAILED,
        accepted: false,
    },
    StatusRow {
        case: "a create that was rolled back",
        stack: "plan-rollback-complete",
        seeded: status::ROLLBACK_COMPLETE,
        accepted: false,
    },
    StatusRow {
        case: "a rollback that could not remove what the create had made",
        stack: "plan-rollback-failed",
        seeded: status::ROLLBACK_FAILED,
        accepted: false,
    },
    StatusRow {
        case: "an update rollback that could not put a resource back",
        stack: "plan-update-rollback-failed",
        seeded: status::UPDATE_ROLLBACK_FAILED,
        accepted: false,
    },
    StatusRow {
        case: "a stack a CREATE change set brought into existence",
        stack: "plan-in-review",
        seeded: status::REVIEW_IN_PROGRESS,
        accepted: false,
    },
];

/// The template a status row's stack is created with, and — with another value — the update
/// it is asked to plan.
fn planned_template(value: &str) -> String {
    json!({
        "Resources": {
            "Param": {
                "Type": "AWS::SSM::Parameter",
                "Properties": {"Type": "String", "Value": value},
            },
        },
    })
    .to_string()
}

#[tokio::test]
async fn an_update_change_set_is_refused_wherever_the_update_itself_is() {
    for row in STATUS_ROWS {
        let (router, engine) = router_with_engine();
        let (code, created) =
            create_stack(&router, row.stack, &planned_template("before"), &[]).await;
        assert_eq!(
            code, 200,
            "case {:?}: CreateStack must succeed, got {code} {created}",
            row.case
        );
        wait_for_the_transition().await;

        // The status the plan is asked for at, seeded directly: the operations that leave a
        // stack in most of these need an owning service to refuse something, which no wire
        // call of this row can make happen.
        let stack_id = {
            let state = store(&engine);
            let mut held = state.lock();
            // The create's own clock-owed completion is materialised FIRST, so the status
            // seeded below is not overwritten by the transition the next call settles.
            held.settle(std::time::SystemTime::now());
            let stack = held
                .resolve_mut(REGION, row.stack)
                .expect("the stack just created");
            stack.status = row.seeded.to_string();
            stack.stack_id.clone()
        };

        let (code, planned) = create_change_set(
            &router,
            row.stack,
            "the-plan",
            &[
                member("ChangeSetType", "UPDATE"),
                member("TemplateBody", &planned_template("after")),
            ],
        )
        .await;

        if row.accepted {
            assert_eq!(
                code, 200,
                "case {:?}: a stack in {} must take an UPDATE change set, got {code} \
                 {planned}",
                row.case, row.seeded
            );
            assert!(
                xml_text(&planned, "Id").is_some(),
                "case {:?}: an accepted plan must answer its own id, got {planned}",
                row.case
            );
            continue;
        }

        assert_eq!(
            code, 400,
            "case {:?}: a stack in {} must refuse an UPDATE change set, got {code} {planned}",
            row.case, row.seeded
        );
        assert_eq!(
            xml_text(&planned, "Code").as_deref(),
            Some("ValidationError"),
            "case {:?}: expected a ValidationError, got {planned}",
            row.case
        );
        let expected = stack_cannot_be_updated(&stack_id, row.seeded);
        assert_eq!(
            xml_text(&planned, "Message").as_deref(),
            Some(expected.as_str()),
            "case {:?}: expected the refusal to read {expected:?}, got {planned}",
            row.case
        );
        // A refused plan changes nothing: no change set is stored and the stack is still in
        // the status it was in.
        let (code, listed) = call(
            &router,
            "ListChangeSets",
            &[member("StackName", row.stack)],
        )
        .await;
        assert_eq!(
            code, 200,
            "case {:?}: ListChangeSets must answer 200, got {code} {listed}",
            row.case
        );
        assert!(
            xml_texts(&listed, "ChangeSetName").is_empty(),
            "case {:?}: a refused plan must not be stored, got {listed}",
            row.case
        );
        let (code, described) = describe_stack(&router, row.stack).await;
        assert_eq!(
            code, 200,
            "case {:?}: DescribeStacks must answer 200, got {code} {described}",
            row.case
        );
        assert_eq!(
            xml_text(&described, "StackStatus").as_deref(),
            Some(row.seeded),
            "case {:?}: expected the refused plan to leave the stack in {}, got {described}",
            row.case,
            row.seeded
        );
    }
}
