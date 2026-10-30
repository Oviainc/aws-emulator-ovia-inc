// SPDX-License-Identifier: CC0-1.0

//! The `DeletionPolicy` resource attribute, end to end over the awsQuery wire.
//!
//! RULE under test: **when CloudFormation stops managing a resource — the stack is deleted,
//! or a deployment no longer declares the resource — what becomes of the PHYSICAL resource
//! is that resource's `DeletionPolicy`: `Retain` leaves it in the service that owns it and
//! reports its deletion `DELETE_SKIPPED`, `Snapshot` has that service take a final snapshot
//! of it before deleting it, and `Delete` — the default a declaration stating no policy gets
//! — deletes it.**
//!
//! That is the attribute's own documentation ("with the `DeletionPolicy` attribute you can
//! preserve, and in some cases, backup a resource when its stack is deleted"), and the
//! statuses are the model's own `ResourceStatus` values
//! (`smithy_query(query="shape", shape="ResourceStatus")`).
//!
//! The rows differ along what the rule generalises over: the POLICY (`Retain`, `Snapshot`,
//! `Delete` stated outright, and a declaration stating none), the resource TYPE it is
//! declared on (an `AWS::RDS::DBInstance`, the `AWS::RDS::DBParameterGroup` an instance of
//! the same stack uses, and an `AWS::SNS::Topic` — a type whose owning service takes no
//! snapshot on the way out), and where the resource sits in the teardown order, so a
//! resource declared beside it is still deleted either way.
//!
//! Every case drives the real request path for BOTH services through one router — the
//! CloudFormation calls over the awsQuery wire and the Amazon RDS and Amazon SNS calls as a
//! client's SDK makes them — so what is asserted is what a client that deploys by template
//! and then calls `DescribeDBInstances`, `DescribeDBSnapshots`, `DescribeDBParameters` or
//! `GetTopicAttributes` observes.

use smithy_server_runtime::MultiServiceRouter;

use crate::rds_stack_resources::group_settings;
use crate::stack_lifecycle::{
    call, create_stack, describe_stack, member, rds, resource_event_reason, router, sns,
    wait_for_the_transition, xml_blocks, xml_text,
};
use crate::stack_resources::sns_arn;
use service_com_amazonaws_cloudformation_cloudformation::state::{resource_status, status};

/// The resource a row declares its policy on, and how the service that OWNS it is asked
/// whether the resource is still there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Subject {
    /// A DB instance, addressed by the `DBInstanceIdentifier` the template names it.
    DbInstance(&'static str),
    /// A DB parameter group, addressed by its name, with the `(name, value)` parameter
    /// setting the template put on it — which a group that was KEPT must still report.
    ParameterGroup(&'static str, (&'static str, &'static str)),
    /// An SNS topic, addressed by the ARN its name resolves to.
    Topic(&'static str),
}

/// What the deletion left of the row's subject.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fate {
    /// The physical resource is still in its owning service, and the stack reported its
    /// deletion as skipped.
    Kept,
    /// The physical resource is gone, and the owning service holds no snapshot of it.
    Deleted,
    /// The physical resource is gone, and the owning service holds an available snapshot of
    /// it, taken on the way out.
    Snapshotted,
}

/// One stack deleted with a policy on one of its resources.
struct PolicyCase {
    case: &'static str,
    stack: &'static str,
    template: &'static str,
    /// The logical id the policy is declared on, whose own deletion event the row reads.
    logical_id: &'static str,
    subject: Subject,
    expected: Fate,
    /// The resources of the same stack that must be GONE afterwards whatever the policy
    /// says, so a retained or snapshotted resource is shown not to stop the teardown.
    gone: &'static [Subject],
}

const POLICY_CASES: &[PolicyCase] = &[
    PolicyCase {
        case: "Retain on a DB instance, declared before a topic",
        stack: "policy-retain-db",
        template: concat!(
            "Resources:\n",
            "  Db:\n",
            "    Type: AWS::RDS::DBInstance\n",
            "    DeletionPolicy: Retain\n",
            "    Properties:\n",
            "      DBInstanceIdentifier: policy-retain-db-instance\n",
            "      Engine: mysql\n",
            "      DBInstanceClass: db.t3.micro\n",
            "  Topic:\n",
            "    Type: AWS::SNS::Topic\n",
            "    Properties:\n",
            "      TopicName: policy-retain-db-topic\n",
        ),
        logical_id: "Db",
        subject: Subject::DbInstance("policy-retain-db-instance"),
        expected: Fate::Kept,
        gone: &[Subject::Topic("policy-retain-db-topic")],
    },
    PolicyCase {
        case: "Retain on the parameter group a deleted instance of the same stack uses",
        stack: "policy-retain-params",
        template: concat!(
            "Resources:\n",
            "  Params:\n",
            "    Type: AWS::RDS::DBParameterGroup\n",
            "    DeletionPolicy: Retain\n",
            "    Properties:\n",
            "      DBParameterGroupName: policy-retain-params-group\n",
            "      Family: mysql8.0\n",
            "      Description: kept by its deletion policy\n",
            "      Parameters:\n",
            "        max_connections: '77'\n",
            "  Db:\n",
            "    Type: AWS::RDS::DBInstance\n",
            "    Properties:\n",
            "      DBInstanceIdentifier: policy-retain-params-instance\n",
            "      Engine: mysql\n",
            "      DBInstanceClass: db.t3.micro\n",
            "      DBParameterGroupName: !Ref Params\n",
        ),
        logical_id: "Params",
        subject: Subject::ParameterGroup(
            "policy-retain-params-group",
            ("max_connections", "77"),
        ),
        expected: Fate::Kept,
        gone: &[Subject::DbInstance("policy-retain-params-instance")],
    },
    PolicyCase {
        case: "Snapshot on a DB instance, whose owning service takes the final snapshot",
        stack: "policy-snapshot-db",
        template: concat!(
            "Resources:\n",
            "  Db:\n",
            "    Type: AWS::RDS::DBInstance\n",
            "    DeletionPolicy: Snapshot\n",
            "    Properties:\n",
            "      DBInstanceIdentifier: policy-snapshot-db-instance\n",
            "      Engine: mysql\n",
            "      DBInstanceClass: db.t3.micro\n",
            "  Topic:\n",
            "    Type: AWS::SNS::Topic\n",
            "    Properties:\n",
            "      TopicName: policy-snapshot-db-topic\n",
        ),
        logical_id: "Db",
        subject: Subject::DbInstance("policy-snapshot-db-instance"),
        expected: Fate::Snapshotted,
        gone: &[Subject::Topic("policy-snapshot-db-topic")],
    },
    PolicyCase {
        case: "Delete stated outright on a DB instance",
        stack: "policy-delete-db",
        template: concat!(
            "Resources:\n",
            "  Db:\n",
            "    Type: AWS::RDS::DBInstance\n",
            "    DeletionPolicy: Delete\n",
            "    Properties:\n",
            "      DBInstanceIdentifier: policy-delete-db-instance\n",
            "      Engine: mysql\n",
            "      DBInstanceClass: db.t3.micro\n",
        ),
        logical_id: "Db",
        subject: Subject::DbInstance("policy-delete-db-instance"),
        expected: Fate::Deleted,
        gone: &[],
    },
    PolicyCase {
        case: "no policy at all on a DB instance, which is the Delete default",
        stack: "policy-default-db",
        template: concat!(
            "Resources:\n",
            "  Db:\n",
            "    Type: AWS::RDS::DBInstance\n",
            "    Properties:\n",
            "      DBInstanceIdentifier: policy-default-db-instance\n",
            "      Engine: mysql\n",
            "      DBInstanceClass: db.t3.micro\n",
        ),
        logical_id: "Db",
        subject: Subject::DbInstance("policy-default-db-instance"),
        expected: Fate::Deleted,
        gone: &[],
    },
    PolicyCase {
        case: "Snapshot on a type whose owning service takes no snapshot on the way out",
        stack: "policy-snapshot-topic",
        template: concat!(
            "Resources:\n",
            "  Topic:\n",
            "    Type: AWS::SNS::Topic\n",
            "    DeletionPolicy: Snapshot\n",
            "    Properties:\n",
            "      TopicName: policy-snapshot-topic-name\n",
        ),
        logical_id: "Topic",
        subject: Subject::Topic("policy-snapshot-topic-name"),
        expected: Fate::Deleted,
        gone: &[],
    },
    PolicyCase {
        case: "Retain on a topic declared between two resources that are deleted",
        stack: "policy-retain-topic",
        template: concat!(
            "Resources:\n",
            "  Before:\n",
            "    Type: AWS::SNS::Topic\n",
            "    Properties:\n",
            "      TopicName: policy-retain-before-topic\n",
            "  Kept:\n",
            "    Type: AWS::SNS::Topic\n",
            "    DeletionPolicy: Retain\n",
            "    Properties:\n",
            "      TopicName: policy-retain-kept-topic\n",
            "  After:\n",
            "    Type: AWS::SNS::Topic\n",
            "    Properties:\n",
            "      TopicName: policy-retain-after-topic\n",
        ),
        logical_id: "Kept",
        subject: Subject::Topic("policy-retain-kept-topic"),
        expected: Fate::Kept,
        gone: &[
            Subject::Topic("policy-retain-before-topic"),
            Subject::Topic("policy-retain-after-topic"),
        ],
    },
];

/// Whether the service that owns `subject` still holds it, with the body it answered.
async fn still_there(router: &MultiServiceRouter, subject: Subject) -> (bool, String) {
    match subject {
        Subject::DbInstance(identifier) => {
            let (status_code, body) = rds(
                router,
                "DescribeDBInstances",
                &[("DBInstanceIdentifier", identifier)],
            )
            .await;
            (status_code == 200, body)
        }
        Subject::ParameterGroup(group, _) => {
            let (status_code, body) = rds(
                router,
                "DescribeDBParameterGroups",
                &[("DBParameterGroupName", group)],
            )
            .await;
            (status_code == 200, body)
        }
        Subject::Topic(name) => {
            let (status_code, body) = sns(
                router,
                "GetTopicAttributes",
                &[("TopicArn", &sns_arn(name))],
            )
            .await;
            (status_code == 200, body)
        }
    }
}

/// The `(identifier, status)` of every MANUAL snapshot Amazon RDS holds of `instance`.
async fn manual_snapshots_of(
    router: &MultiServiceRouter,
    instance: &str,
) -> Vec<(String, String)> {
    let (status_code, body) = rds(
        router,
        "DescribeDBSnapshots",
        &[
            ("DBInstanceIdentifier", instance),
            ("SnapshotType", "manual"),
        ],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "DescribeDBSnapshots({instance}) must answer 200, got {status_code} {body}"
    );
    xml_blocks(&body, "DBSnapshot")
        .iter()
        .map(|entry| {
            (
                xml_text(entry, "DBSnapshotIdentifier").unwrap_or_default(),
                xml_text(entry, "Status").unwrap_or_default(),
            )
        })
        .collect()
}

/// `DeleteStack` for one stack, waited past both of the clocks a teardown runs on: the
/// stack's own terminal status, and the removal an owning service owes a resource it put in
/// a deleting state.
async fn delete_stack(router: &MultiServiceRouter, case: &str, stack: &str) {
    let (status_code, body) = call(router, "DeleteStack", &[member("StackName", stack)]).await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: DeleteStack must answer 200, got {status_code} {body}"
    );
    wait_for_the_transition().await;
    wait_for_the_transition().await;
}

/// Create the row's stack, wait past both clocks, and answer its stack id — which is how a
/// completed deletion's events are still readable.
async fn created(
    router: &MultiServiceRouter,
    case: &str,
    stack: &str,
    template: &str,
) -> String {
    let (status_code, created) = create_stack(router, stack, template, &[]).await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: CreateStack must answer 200, got {status_code} {created}"
    );
    wait_for_the_transition().await;
    wait_for_the_transition().await;
    let (status_code, described) = describe_stack(router, stack).await;
    assert_eq!(
        xml_text(&described, "StackStatus").as_deref(),
        Some(status::CREATE_COMPLETE),
        "case {case:?}: the stack must be created before the delete, got {status_code} \
         {described}"
    );
    xml_text(&created, "StackId")
        .unwrap_or_else(|| panic!("case {case:?}: expected a StackId, got {created}"))
}

#[tokio::test]
async fn a_deleted_stack_keeps_snapshots_or_deletes_each_resource_as_its_policy_states() {
    for row in POLICY_CASES {
        let router = router();
        let stack_id = created(&router, row.case, row.stack, row.template).await;

        // Every subject is really there before the deletion: a row that asserts a resource
        // was KEPT has to start from one that existed.
        let (there, body) = still_there(&router, row.subject).await;
        assert!(
            there,
            "case {:?}: expected {:?} to exist before the delete, got {body}",
            row.case, row.subject
        );

        delete_stack(&router, row.case, row.stack).await;

        // Nothing here is refused, so the stack's deletion completes whatever the policies
        // say: a retained resource is not a failed deletion.
        let (status_code, described) = describe_stack(&router, &stack_id).await;
        assert_eq!(
            xml_text(&described, "StackStatus").as_deref(),
            Some(status::DELETE_COMPLETE),
            "case {:?}: expected the deletion to complete, got {status_code} {described}",
            row.case
        );

        let (there, body) = still_there(&router, row.subject).await;
        assert_eq!(
            there,
            row.expected == Fate::Kept,
            "case {:?}: expected {:?} to be kept = {}, got {body}",
            row.case,
            row.subject,
            row.expected == Fate::Kept
        );

        // The status the stack reports for the resource says which of the two happened: a
        // resource the policy kept was SKIPPED, one it deleted reports its deletion.
        let expected_status = if row.expected == Fate::Kept {
            resource_status::DELETE_SKIPPED
        } else {
            resource_status::DELETE_COMPLETE
        };
        let unexpected_status = if row.expected == Fate::Kept {
            resource_status::DELETE_COMPLETE
        } else {
            resource_status::DELETE_SKIPPED
        };
        let (status_code, events) = call(
            &router,
            "DescribeStackEvents",
            &[member("StackName", &stack_id)],
        )
        .await;
        assert_eq!(
            status_code, 200,
            "case {:?}: DescribeStackEvents must answer 200, got {status_code} {events}",
            row.case
        );
        let statuses: Vec<String> = xml_blocks(&events, "member")
            .iter()
            .filter(|entry| {
                xml_text(entry, "LogicalResourceId").as_deref() == Some(row.logical_id)
            })
            .filter_map(|entry| xml_text(entry, "ResourceStatus"))
            .collect();
        assert!(
            statuses.iter().any(|status| status == expected_status),
            "case {:?}: expected a {expected_status} event for {:?}, got {statuses:?}",
            row.case,
            row.logical_id
        );
        assert!(
            !statuses.iter().any(|status| status == unexpected_status),
            "case {:?}: expected no {unexpected_status} event for {:?}, got {statuses:?}",
            row.case,
            row.logical_id
        );

        // A group that was kept still carries the parameter value the template set on it:
        // "keeps the resource without deleting the resource or its contents".
        if let Subject::ParameterGroup(group, (name, value)) = row.subject {
            if row.expected == Fate::Kept {
                let (status_code, body, reported) =
                    group_settings(&router, "DescribeDBParameters", "DBParameterGroupName", group)
                        .await;
                assert_eq!(
                    status_code, 200,
                    "case {:?}: DescribeDBParameters({group}) must answer 200, got \
                     {status_code} {body}",
                    row.case
                );
                assert!(
                    reported
                        .iter()
                        .any(|(reported_name, reported_value)| reported_name == name
                            && reported_value == value),
                    "case {:?}: expected the kept group to still report ({name}, {value}), \
                     got {reported:?}",
                    row.case
                );
            }
        }

        // The snapshot half of the rule, read in the service that would have taken it: a
        // `Snapshot` policy leaves one available, and no other policy leaves any.
        if let Subject::DbInstance(instance) = row.subject {
            let snapshots = manual_snapshots_of(&router, instance).await;
            match row.expected {
                Fate::Snapshotted => {
                    assert_eq!(
                        snapshots.len(),
                        1,
                        "case {:?}: expected exactly one final snapshot of {instance}, got \
                         {snapshots:?}",
                        row.case
                    );
                    assert_eq!(
                        snapshots[0].1, "available",
                        "case {:?}: expected the final snapshot to be available, got \
                         {snapshots:?}",
                        row.case
                    );
                }
                Fate::Deleted | Fate::Kept => assert!(
                    snapshots.is_empty(),
                    "case {:?}: expected no snapshot of {instance}, got {snapshots:?}",
                    row.case
                ),
            }
        }

        // Whatever the policy kept, every other resource of the stack really went.
        for subject in row.gone {
            let (there, body) = still_there(&router, *subject).await;
            assert!(
                !there,
                "case {:?}: expected {subject:?} to have been deleted, got {body}",
                row.case
            );
        }
    }
}

/// The flow a client that deploys a protected database by template drives: the deletion is
/// REFUSED while the instance protects itself, the stack says so, and once the protection is
/// lifted the same deletion completes — leaving the `Snapshot` policy's final snapshot and
/// the `Retain` policy's parameter group, with its declared value, behind.
#[tokio::test]
async fn a_protected_instance_fails_the_delete_and_the_policies_hold_once_it_is_lifted() {
    let router = router();
    let case = "a DeletionProtection=true instance under Snapshot, beside a Retain group";
    let template = concat!(
        "Resources:\n",
        "  Params:\n",
        "    Type: AWS::RDS::DBParameterGroup\n",
        "    DeletionPolicy: Retain\n",
        "    UpdateReplacePolicy: Retain\n",
        "    Properties:\n",
        "      DBParameterGroupName: protected-stack-params\n",
        "      Family: mysql8.0\n",
        "      Description: retained by its deletion policy\n",
        "      Parameters:\n",
        "        max_connections: '77'\n",
        "  Db:\n",
        "    Type: AWS::RDS::DBInstance\n",
        "    DeletionPolicy: Snapshot\n",
        "    UpdateReplacePolicy: Snapshot\n",
        "    Properties:\n",
        "      DBInstanceIdentifier: protected-stack-instance\n",
        "      Engine: mysql\n",
        "      DBInstanceClass: db.t3.micro\n",
        "      AllocatedStorage: 20\n",
        "      BackupRetentionPeriod: 0\n",
        "      DeletionProtection: true\n",
    );
    let stack_id = created(&router, case, "protected-stack", template).await;

    // The first deletion: Amazon RDS refuses the protected instance, so the stack reports
    // the failure rather than a deletion that did not happen.
    delete_stack(&router, case, "protected-stack").await;
    let (status_code, described) = describe_stack(&router, &stack_id).await;
    assert_eq!(
        xml_text(&described, "StackStatus").as_deref(),
        Some(status::DELETE_FAILED),
        "case {case:?}: expected the protected instance to fail the deletion, got \
         {status_code} {described}"
    );
    let reason =
        resource_event_reason(&router, &stack_id, "Db", resource_status::DELETE_FAILED).await;
    assert!(
        reason.contains("deletion protection"),
        "case {case:?}: expected the resource's own refusal to cite deletion protection, got \
         {reason:?}"
    );
    // Nothing was taken on the way out of a deletion that did not happen.
    let snapshots = manual_snapshots_of(&router, "protected-stack-instance").await;
    assert!(
        snapshots.is_empty(),
        "case {case:?}: expected no snapshot of a refused deletion, got {snapshots:?}"
    );
    let (there, body) = still_there(&router, Subject::DbInstance("protected-stack-instance")).await;
    assert!(
        there,
        "case {case:?}: expected the refused instance to still be there, got {body}"
    );
    assert_eq!(
        xml_text(&body, "DeletionProtection").as_deref(),
        Some("true"),
        "case {case:?}: expected the refused instance to keep its protection, got {body}"
    );

    // Protection lifted in Amazon RDS, as a client lifts it.
    let (status_code, modified) = rds(
        &router,
        "ModifyDBInstance",
        &[
            ("DBInstanceIdentifier", "protected-stack-instance"),
            ("DeletionProtection", "false"),
            ("ApplyImmediately", "true"),
        ],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: ModifyDBInstance must answer 200, got {status_code} {modified}"
    );
    wait_for_the_transition().await;

    // The same deletion now completes, and each policy is honoured.
    delete_stack(&router, case, "protected-stack").await;
    let (status_code, described) = describe_stack(&router, &stack_id).await;
    assert_eq!(
        xml_text(&described, "StackStatus").as_deref(),
        Some(status::DELETE_COMPLETE),
        "case {case:?}: expected the deletion to complete once protection was lifted, got \
         {status_code} {described}"
    );
    let (status_code, body) = rds(
        &router,
        "DescribeDBInstances",
        &[("DBInstanceIdentifier", "protected-stack-instance")],
    )
    .await;
    assert_eq!(
        status_code, 404,
        "case {case:?}: expected the deleted instance to be gone, got {status_code} {body}"
    );
    let snapshots = manual_snapshots_of(&router, "protected-stack-instance").await;
    assert_eq!(
        snapshots.len(),
        1,
        "case {case:?}: expected the Snapshot policy's final snapshot, got {snapshots:?}"
    );
    assert_eq!(
        snapshots[0].1, "available",
        "case {case:?}: expected that snapshot to be available, got {snapshots:?}"
    );
    let (status_code, body, reported) = group_settings(
        &router,
        "DescribeDBParameters",
        "DBParameterGroupName",
        "protected-stack-params",
    )
    .await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: expected the retained group to still be readable, got {status_code} \
         {body}"
    );
    assert!(
        reported
            .iter()
            .any(|(name, value)| name == "max_connections" && value == "77"),
        "case {case:?}: expected the retained group to keep its declared value, got \
         {reported:?}"
    );
}

/// The other half of the rule: a resource a DEPLOYMENT stops declaring is removed from the
/// stack, which is the same event the attribute governs — so a `Retain` resource survives the
/// update that drops it and one with no policy does not.
#[tokio::test]
async fn an_update_that_drops_a_resource_honours_the_policy_it_declared() {
    let router = router();
    let case = "an update that drops a Retain resource and one with no policy";
    let created_template = concat!(
        "Resources:\n",
        "  Kept:\n",
        "    Type: AWS::SNS::Topic\n",
        "    DeletionPolicy: Retain\n",
        "    Properties:\n",
        "      TopicName: dropped-but-retained-topic\n",
        "  Dropped:\n",
        "    Type: AWS::SNS::Topic\n",
        "    Properties:\n",
        "      TopicName: dropped-topic\n",
        "  Stays:\n",
        "    Type: AWS::SNS::Topic\n",
        "    Properties:\n",
        "      TopicName: still-declared-topic\n",
    );
    let updated_template = concat!(
        "Resources:\n",
        "  Stays:\n",
        "    Type: AWS::SNS::Topic\n",
        "    Properties:\n",
        "      TopicName: still-declared-topic\n",
    );
    created(&router, case, "dropped-resources", created_template).await;

    let (status_code, updated) = call(
        &router,
        "UpdateStack",
        &[
            member("StackName", "dropped-resources"),
            member("TemplateBody", updated_template),
        ],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: UpdateStack must answer 200, got {status_code} {updated}"
    );
    wait_for_the_transition().await;
    wait_for_the_transition().await;

    let (status_code, described) = describe_stack(&router, "dropped-resources").await;
    assert_eq!(
        xml_text(&described, "StackStatus").as_deref(),
        Some(status::UPDATE_COMPLETE),
        "case {case:?}: expected the update to complete, got {status_code} {described}"
    );

    for (subject, kept) in [
        (Subject::Topic("dropped-but-retained-topic"), true),
        (Subject::Topic("dropped-topic"), false),
        (Subject::Topic("still-declared-topic"), true),
    ] {
        let (there, body) = still_there(&router, subject).await;
        assert_eq!(
            there, kept,
            "case {case:?}: expected {subject:?} to still exist = {kept}, got {body}"
        );
    }
}
