// SPDX-License-Identifier: CC0-1.0

//! What a REQUESTED rollback does with the PHYSICAL resources it gives up, end to end over
//! the awsQuery wire.
//!
//! RULE under test: **when a rollback gives up a resource the failed operation had
//! provisioned — `RollbackStack` discarding what a stack with no last known stable state had
//! made, and the revert of a failed update removing what that update had added — what becomes
//! of each PHYSICAL resource is that resource's own `DeletionPolicy`: `Retain` leaves it in
//! the service that owns it and the stack records its deletion as skipped, and any other
//! policy deletes it; unless the request asks for `RetainExceptOnCreate`, under which a
//! resource the failed operation created is deleted whatever its policy states.**
//!
//! `DeletionPolicy` is the attribute a template preserves a resource with, and
//! `RetainExceptOnCreate` is the one member documented to override it: "when set to true,
//! newly created resources are deleted when the operation rolls back. This includes newly
//! created resources marked with a deletion policy of Retain. Default: false"
//! (`smithy_query(query="operation",operation="RollbackStack")`). The per-resource statuses
//! are the model's own (`smithy_query(query="shape",shape="ResourceStatus")`).
//!
//! The rows differ along what the rule generalises over: the POLICY declared (`Retain`,
//! `Snapshot`, `Delete` stated outright, and a declaration stating none), the
//! `RetainExceptOnCreate` the rollback request carries (absent, `false` — the documented
//! default stated outright — and `true`), and whether the stack has a last known stable state
//! to go back to, which is what decides whether the rollback discards every resource or only
//! the ones the failed update added. Every row declares its policy on resources whose owning
//! services really hold them (an `AWS::SSM::Parameter` in Systems Manager and an
//! `AWS::SNS::Topic` in Amazon SNS), and each one is read back through that service's own
//! wire — so what the rows assert is what a client sees rather than a status word the stack
//! published.

use serde_json::{json, Value};
use service_com_amazonaws_cloudformation_cloudformation::state::{
    deletion_policy, resource_status, status,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::stack_lifecycle::{
    call, create_stack, member, router, sns, wait_for_the_transition, xml_blocks, xml_text,
};
use crate::stack_resources::sns_arn;
use crate::update_rollback_recovery::{
    parameter, parameter_value, ssm, status_of, template, template_with_an_unsignalled_gate,
    WAITED_OUT,
};

/// What the rollback left of a resource it gave up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fate {
    /// The physical resource is still in the service that owns it, and the stack recorded
    /// its deletion as skipped.
    Kept,
    /// The physical resource is gone, and the stack recorded its deletion.
    Deleted,
}

/// One row: a failed create whose resources carry the row's policy, and what the rollback it
/// is asked for leaves of them.
struct Row {
    case: &'static str,
    stack: &'static str,
    /// The `DeletionPolicy` declared on each of the row's resources, or `None` for a
    /// declaration stating none.
    policy: Option<&'static str>,
    /// `RetainExceptOnCreate`, exactly as the `RollbackStack` request carries it, or `None`
    /// for a request that names it nowhere.
    retain_except_on_create: Option<&'static str>,
    expected: Fate,
}

/// The class the rule covers: any policy a declaration can state, under any
/// `RetainExceptOnCreate` a rollback can be asked for.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "Retain, with no RetainExceptOnCreate member at all",
            stack: "rbretain-default",
            policy: Some(deletion_policy::RETAIN),
            retain_except_on_create: None,
            expected: Fate::Kept,
        },
        Row {
            case: "Retain, with the documented RetainExceptOnCreate default stated outright",
            stack: "rbretain-false",
            policy: Some(deletion_policy::RETAIN),
            retain_except_on_create: Some("false"),
            expected: Fate::Kept,
        },
        Row {
            case: "Retain, with RetainExceptOnCreate asked for",
            stack: "rbretain-true",
            policy: Some(deletion_policy::RETAIN),
            retain_except_on_create: Some("true"),
            expected: Fate::Deleted,
        },
        Row {
            case: "Snapshot, on types whose owning services take no snapshot on the way out",
            stack: "rbretain-snapshot",
            policy: Some(deletion_policy::SNAPSHOT),
            retain_except_on_create: None,
            expected: Fate::Deleted,
        },
        Row {
            case: "Delete stated outright",
            stack: "rbretain-delete",
            policy: Some(deletion_policy::DELETE),
            retain_except_on_create: None,
            expected: Fate::Deleted,
        },
        Row {
            case: "no policy at all, which is the Delete default",
            stack: "rbretain-none",
            policy: None,
            retain_except_on_create: None,
            expected: Fate::Deleted,
        },
        Row {
            case: "no policy at all, with RetainExceptOnCreate asked for",
            stack: "rbretain-none-retained",
            policy: None,
            retain_except_on_create: Some("true"),
            expected: Fate::Deleted,
        },
    ]
}

/// A resource the rollback gives up, and how the service that OWNS it is asked whether the
/// resource is still there.
#[derive(Debug, Clone)]
enum Subject {
    /// A parameter of Parameter Store, addressed by the name the template gave it.
    Parameter(String),
    /// A topic of Amazon SNS, addressed by the ARN its name resolves to.
    Topic(String),
}

/// The `AWS::SNS::Topic` declaration these rows deploy beside the parameter, so the rule is
/// asserted on a second type, owned by a second service.
fn topic(name: &str) -> Value {
    json!({"Type": "AWS::SNS::Topic", "Properties": {"TopicName": name}})
}

/// `declaration` carrying the row's `DeletionPolicy`, or exactly as it is for a row that
/// states none.
fn with_policy(declaration: Value, policy: Option<&str>) -> Value {
    let mut declaration = declaration;
    if let Some(policy) = policy {
        declaration
            .as_object_mut()
            .expect("a resource declaration is a JSON object")
            .insert("DeletionPolicy".to_string(), json!(policy));
    }
    declaration
}

/// Whether the service that owns `subject` still holds it, with the answer it gave.
async fn still_there(router: &MultiServiceRouter, subject: &Subject) -> (bool, String) {
    match subject {
        Subject::Parameter(name) => {
            let (http_status, body) = ssm(router, "GetParameter", &json!({"Name": name})).await;
            (http_status == 200, body.to_string())
        }
        Subject::Topic(name) => {
            let (http_status, body) =
                sns(router, "GetTopicAttributes", &[("TopicArn", &sns_arn(name))]).await;
            (http_status == 200, body)
        }
    }
}

/// Every `ResourceStatus` the stack's event stream carries for `logical_id`, which is what
/// says whether the rollback skipped that resource's deletion or carried it out.
async fn event_statuses(
    router: &MultiServiceRouter,
    case: &str,
    stack: &str,
    logical_id: &str,
) -> Vec<String> {
    let (http_status, body) =
        call(router, "DescribeStackEvents", &[member("StackName", stack)]).await;
    assert_eq!(
        http_status, 200,
        "case {case:?}: expected DescribeStackEvents to succeed, got {http_status} {body}"
    );
    xml_blocks(&body, "member")
        .iter()
        .filter(|entry| xml_text(entry, "LogicalResourceId").as_deref() == Some(logical_id))
        .filter_map(|entry| xml_text(entry, "ResourceStatus"))
        .collect()
}

#[tokio::test]
async fn a_requested_rollback_gives_up_each_resource_as_its_deletion_policy_states() {
    for row in rows() {
        let router = router();
        let parameter_name = format!("/{}/p", row.stack);
        let topic_name = format!("{}-topic", row.stack);
        let template = template_with_an_unsignalled_gate(json!({
            "AParam": with_policy(parameter(&parameter_name, "provisioned"), row.policy),
            "BTopic": with_policy(topic(&topic_name), row.policy),
        }));

        // `DisableRollback` is what makes the failing create PRESERVE what it had already
        // provisioned, which is the state a requested rollback exists to clear up.
        let (http_status, body) = create_stack(
            &router,
            row.stack,
            &template,
            &[member("DisableRollback", "true")],
        )
        .await;
        assert_eq!(
            http_status, 200,
            "case {:?}: expected CreateStack to succeed, got {http_status} {body}",
            row.case
        );

        // The wait condition nothing signals runs out and fails the create with no call of
        // the client's.
        tokio::time::sleep(WAITED_OUT).await;
        let failed = status_of(&router, row.stack).await;
        assert_eq!(
            failed,
            status::CREATE_FAILED,
            "case {:?}: expected the preserved create to leave the stack in {}, got {failed:?}",
            row.case,
            status::CREATE_FAILED
        );

        let subjects = [
            ("AParam", Subject::Parameter(parameter_name)),
            ("BTopic", Subject::Topic(topic_name)),
        ];
        // A row that asserts a resource was KEPT has to start from one that really existed.
        for (logical_id, subject) in &subjects {
            let (there, answered) = still_there(&router, subject).await;
            assert!(
                there,
                "case {:?}: expected {logical_id} to exist before the rollback, got {answered}",
                row.case
            );
        }

        let mut members = vec![member("StackName", row.stack)];
        if let Some(asked) = row.retain_except_on_create {
            members.push(member("RetainExceptOnCreate", asked));
        }
        let (http_status, body) = call(&router, "RollbackStack", &members).await;
        assert_eq!(
            http_status, 200,
            "case {:?}: expected RollbackStack to succeed, got {http_status} {body}",
            row.case
        );

        // Past the rollback's own terminal status, and past the removal an owning service
        // owes a resource it has put into a deleting state.
        wait_for_the_transition().await;
        wait_for_the_transition().await;
        let settled = status_of(&router, row.stack).await;
        assert_eq!(
            settled,
            status::ROLLBACK_COMPLETE,
            "case {:?}: expected the rollback to settle the stack at {}, got {settled:?}",
            row.case,
            status::ROLLBACK_COMPLETE
        );

        for (logical_id, subject) in &subjects {
            let (there, answered) = still_there(&router, subject).await;
            assert_eq!(
                there,
                row.expected == Fate::Kept,
                "case {:?}: expected {logical_id} to be kept = {}, got {answered}",
                row.case,
                row.expected == Fate::Kept
            );
            let (expected, unexpected) = match row.expected {
                Fate::Kept => (
                    resource_status::DELETE_SKIPPED,
                    resource_status::DELETE_COMPLETE,
                ),
                Fate::Deleted => (
                    resource_status::DELETE_COMPLETE,
                    resource_status::DELETE_SKIPPED,
                ),
            };
            let statuses = event_statuses(&router, row.case, row.stack, logical_id).await;
            assert!(
                statuses.iter().any(|status| status == expected),
                "case {:?}: expected a {expected} event for {logical_id}, got {statuses:?}",
                row.case
            );
            assert!(
                !statuses.iter().any(|status| status == unexpected),
                "case {:?}: expected no {unexpected} event for {logical_id}, got {statuses:?}",
                row.case
            );
        }
    }
}

/// One row of the revert table: a failed UPDATE that ADDED a resource carrying the row's
/// policy, which the rollback of that update removes again.
struct AddedRow {
    case: &'static str,
    stack: &'static str,
    /// The `DeletionPolicy` declared on the resource the failed update added, or `None` for
    /// a declaration stating none.
    policy: Option<&'static str>,
    /// `RetainExceptOnCreate`, exactly as the `RollbackStack` request carries it.
    retain_except_on_create: Option<&'static str>,
    expected: Fate,
}

/// The same class, where the rollback has a last known stable state to go back to: the
/// resource it gives up is the one the failed update had newly created.
fn added_rows() -> Vec<AddedRow> {
    vec![
        AddedRow {
            case: "Retain on a resource the failed update added, rolled back as asked",
            stack: "rbadded-default",
            policy: Some(deletion_policy::RETAIN),
            retain_except_on_create: None,
            expected: Fate::Kept,
        },
        AddedRow {
            case: "the same resource, with RetainExceptOnCreate asked for",
            stack: "rbadded-retained",
            policy: Some(deletion_policy::RETAIN),
            retain_except_on_create: Some("true"),
            expected: Fate::Deleted,
        },
        AddedRow {
            case: "no policy at all on the added resource, which is the Delete default",
            stack: "rbadded-none",
            policy: None,
            retain_except_on_create: None,
            expected: Fate::Deleted,
        },
    ]
}

#[tokio::test]
async fn a_requested_revert_gives_up_what_the_failed_update_added_as_its_policy_states() {
    for row in added_rows() {
        let router = router();
        let kept_name = format!("/{}/kept", row.stack);
        let added_name = format!("{}-topic", row.stack);
        // The configuration the stack is stable in, which is the last known stable state
        // this rollback goes back to.
        let created = template(json!({"Kept": parameter(&kept_name, "stable")}));
        // ...and the update that ADDS a resource and is then failed by a wait condition
        // nothing signals, with what it provisioned preserved.
        let updated = template_with_an_unsignalled_gate(json!({
            "Kept": parameter(&kept_name, "stable"),
            "Added": with_policy(topic(&added_name), row.policy),
        }));

        let (http_status, body) = create_stack(&router, row.stack, &created, &[]).await;
        assert_eq!(
            http_status, 200,
            "case {:?}: expected CreateStack to succeed, got {http_status} {body}",
            row.case
        );
        wait_for_the_transition().await;
        let stable = status_of(&router, row.stack).await;
        assert_eq!(
            stable,
            status::CREATE_COMPLETE,
            "case {:?}: expected the create to complete before the update, got {stable:?}",
            row.case
        );

        let (http_status, body) = call(
            &router,
            "UpdateStack",
            &[
                member("StackName", row.stack),
                member("TemplateBody", &updated),
                member("DisableRollback", "true"),
            ],
        )
        .await;
        assert_eq!(
            http_status, 200,
            "case {:?}: expected UpdateStack to succeed, got {http_status} {body}",
            row.case
        );
        tokio::time::sleep(WAITED_OUT).await;
        let failed = status_of(&router, row.stack).await;
        assert_eq!(
            failed,
            status::UPDATE_FAILED,
            "case {:?}: expected the preserved update to leave the stack in {}, got {failed:?}",
            row.case,
            status::UPDATE_FAILED
        );
        let added = Subject::Topic(added_name);
        let (there, answered) = still_there(&router, &added).await;
        assert!(
            there,
            "case {:?}: expected the added resource to exist before the rollback, got \
             {answered}",
            row.case
        );

        let mut members = vec![member("StackName", row.stack)];
        if let Some(asked) = row.retain_except_on_create {
            members.push(member("RetainExceptOnCreate", asked));
        }
        let (http_status, body) = call(&router, "RollbackStack", &members).await;
        assert_eq!(
            http_status, 200,
            "case {:?}: expected RollbackStack to succeed, got {http_status} {body}",
            row.case
        );
        wait_for_the_transition().await;
        wait_for_the_transition().await;
        let settled = status_of(&router, row.stack).await;
        assert_eq!(
            settled,
            status::UPDATE_ROLLBACK_COMPLETE,
            "case {:?}: expected the rollback to settle the stack at {}, got {settled:?}",
            row.case,
            status::UPDATE_ROLLBACK_COMPLETE
        );

        // What the stack was stable in is PRESERVED: a rollback puts the configuration in
        // force before the failed update back, it does not give that resource up.
        let held = parameter_value(&router, &kept_name).await;
        assert_eq!(
            held.as_deref(),
            Some("stable"),
            "case {:?}: expected Parameter Store to still hold the stable value for \
             {kept_name:?}, got {held:?}",
            row.case
        );

        let (there, answered) = still_there(&router, &added).await;
        assert_eq!(
            there,
            row.expected == Fate::Kept,
            "case {:?}: expected the added resource to be kept = {}, got {answered}",
            row.case,
            row.expected == Fate::Kept
        );
        let (expected, unexpected) = match row.expected {
            Fate::Kept => (
                resource_status::DELETE_SKIPPED,
                resource_status::DELETE_COMPLETE,
            ),
            Fate::Deleted => (
                resource_status::DELETE_COMPLETE,
                resource_status::DELETE_SKIPPED,
            ),
        };
        let statuses = event_statuses(&router, row.case, row.stack, "Added").await;
        assert!(
            statuses.iter().any(|status| status == expected),
            "case {:?}: expected a {expected} event for the added resource, got {statuses:?}",
            row.case
        );
        assert!(
            !statuses.iter().any(|status| status == unexpected),
            "case {:?}: expected no {unexpected} event for the added resource, got \
             {statuses:?}",
            row.case
        );
    }
}
