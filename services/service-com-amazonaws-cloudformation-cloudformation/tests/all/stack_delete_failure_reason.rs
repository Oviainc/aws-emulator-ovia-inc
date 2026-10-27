// SPDX-License-Identifier: CC0-1.0

//! What a stack reports about its RESOURCES once a deletion an owning service REFUSED has
//! ended, end to end over the awsQuery wire.
//!
//! RULE under test: `StackResource$ResourceStatus` is the resource's "Current status", so the
//! per-resource view and the event stream of one deletion cannot disagree about what became
//! of each resource — the one the owning service refused is reported in the failed delete
//! status carrying that refusal, one the same walk removed is reported deleted (and is not
//! asked for again by a retry), and one the walk never reached keeps the status it had. The
//! stack-level `StackStatusReason` stays the summary of WHICH resources failed
//! ([`resource_failure_summary`], the rule `stack_failure_reason.rs` pins for the create and
//! update actions), never the provider's refusal: that travels on the failing resource's own
//! `StackEvent$ResourceStatusReason`.
//!
//! The rows differ along what the rule generalises over: which service refused the deletion
//! and with which error (Amazon RDS refusing a protected DB instance, Amazon S3 refusing a
//! bucket that still holds an object), the resource type, the logical id reported, and how
//! many resources the stack declares — a stack whose other resource is deleted cleanly
//! reports that one as deleted and names only the failed one in the summary.
//!
//! Every call drives the real request path through one router carrying CloudFormation beside
//! the services that own those types, so what is asserted is what a client polling
//! `DescribeStacks`, `DescribeStackResources` and `DescribeStackEvents` after a `DeleteStack`
//! sees.

use service_com_amazonaws_cloudformation_cloudformation::state::{
    failed_action, resource_failure_summary, status,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::provisioned_resources::s3;
use crate::stack_lifecycle::{
    call, create_stack, describe_stack, member, router, wait_for_the_transition, xml_blocks,
    xml_text,
};

/// What the row has to do between the create and the delete so the deletion is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Setup {
    /// Nothing: the declaration itself is what makes the resource undeletable.
    Nothing,
    /// Put one object in the named bucket, so Amazon S3 refuses to delete it.
    PutAnObjectIn(&'static str),
}

/// One stack whose deletion a resource refuses.
struct DeleteFailureCase {
    case: &'static str,
    stack: &'static str,
    template: &'static str,
    setup: Setup,
    /// The logical id whose deletion the owning service refuses.
    failing: &'static str,
    /// A logical id of the same stack whose deletion IS carried through, if the row
    /// declares one. A stack's resources are deleted in the reverse of the order they were
    /// created in, and a refusal stops the pass, so such a row declares it under an id the
    /// walk reaches BEFORE the one that fails.
    deleted: Option<&'static str>,
}

const DELETE_FAILURE_CASES: &[DeleteFailureCase] = &[
    DeleteFailureCase {
        case: "Amazon RDS refuses a protected DB instance",
        stack: "delete-failed-protected-db",
        template: concat!(
            "Resources:\n",
            "  Db:\n",
            "    Type: AWS::RDS::DBInstance\n",
            "    Properties:\n",
            "      DBInstanceIdentifier: cfn-delete-failed-protected\n",
            "      Engine: mysql\n",
            "      DBInstanceClass: db.t3.micro\n",
            "      DeletionProtection: true\n",
        ),
        setup: Setup::Nothing,
        failing: "Db",
        deleted: None,
    },
    DeleteFailureCase {
        case: "a protected DB instance under another logical id, beside a topic that is \
               deleted",
        stack: "delete-failed-protected-db-and-topic",
        template: concat!(
            "Resources:\n",
            "  ProtectedDatabase:\n",
            "    Type: AWS::RDS::DBInstance\n",
            "    Properties:\n",
            "      DBInstanceIdentifier: cfn-delete-failed-protected-2\n",
            "      Engine: postgres\n",
            "      DBInstanceClass: db.t3.micro\n",
            "      DeletionProtection: true\n",
            "  Topic:\n",
            "    Type: AWS::SNS::Topic\n",
            "    Properties:\n",
            "      TopicName: cfn-delete-failed-topic\n",
        ),
        setup: Setup::Nothing,
        failing: "ProtectedDatabase",
        deleted: Some("Topic"),
    },
    DeleteFailureCase {
        case: "Amazon S3 refuses a bucket that still holds an object",
        stack: "delete-failed-bucket",
        template: concat!(
            "Resources:\n",
            "  Archive:\n",
            "    Type: AWS::S3::Bucket\n",
            "    Properties:\n",
            "      BucketName: cfn-delete-failed-archive\n",
        ),
        setup: Setup::PutAnObjectIn("cfn-delete-failed-archive"),
        failing: "Archive",
        deleted: None,
    },
];

/// Every `DescribeStackEvents` event of `stack`, as `(LogicalResourceId, ResourceStatus,
/// ResourceStatusReason)`.
async fn events(router: &MultiServiceRouter, stack: &str) -> Vec<(String, String, String)> {
    let (status_code, body) = call(
        router,
        "DescribeStackEvents",
        &[member("StackName", stack)],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "DescribeStackEvents({stack}) must answer 200, got {status_code} {body}"
    );
    xml_blocks(&body, "member")
        .iter()
        .map(|event| {
            (
                xml_text(event, "LogicalResourceId").unwrap_or_default(),
                xml_text(event, "ResourceStatus").unwrap_or_default(),
                xml_text(event, "ResourceStatusReason").unwrap_or_default(),
            )
        })
        .collect()
}

/// Every `DescribeStackResources` record of `stack`, as `(LogicalResourceId,
/// ResourceStatus, ResourceStatusReason)` — the per-resource view of what the operations
/// on the stack left behind.
async fn resources(router: &MultiServiceRouter, stack: &str) -> Vec<(String, String, String)> {
    let (status_code, body) = call(
        router,
        "DescribeStackResources",
        &[member("StackName", stack)],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "DescribeStackResources({stack}) must answer 200, got {status_code} {body}"
    );
    xml_blocks(&body, "member")
        .iter()
        .map(|resource| {
            (
                xml_text(resource, "LogicalResourceId").unwrap_or_default(),
                xml_text(resource, "ResourceStatus").unwrap_or_default(),
                xml_text(resource, "ResourceStatusReason").unwrap_or_default(),
            )
        })
        .collect()
}

#[tokio::test]
async fn a_refused_deletion_reports_each_resource_as_the_walk_left_it() {
    for row in DELETE_FAILURE_CASES {
        let router = router();
        let (status_code, created) = create_stack(&router, row.stack, row.template, &[]).await;
        assert_eq!(
            status_code, 200,
            "case {:?}: CreateStack must answer 200, got {status_code} {created}",
            row.case
        );
        // Twice: the stack's own terminal status and the status an owning service owes the
        // resource it is bringing up are two clocks, and both have to be past due before the
        // delete is asked for.
        wait_for_the_transition().await;
        wait_for_the_transition().await;
        let (status_code, described) = describe_stack(&router, row.stack).await;
        assert_eq!(
            xml_text(&described, "StackStatus").as_deref(),
            Some(status::CREATE_COMPLETE),
            "case {:?}: the stack must be created before the delete, got {status_code} \
             {described}",
            row.case
        );

        if let Setup::PutAnObjectIn(bucket) = row.setup {
            let (status_code, _, body) = s3(
                &router,
                "PUT",
                &format!("/{bucket}/an-object"),
                b"the object that keeps the bucket from being deleted",
            )
            .await;
            assert_eq!(
                status_code, 200,
                "case {:?}: the object must be stored, got {status_code} {body}",
                row.case
            );
        }

        let (status_code, deleted) =
            call(&router, "DeleteStack", &[member("StackName", row.stack)]).await;
        assert_eq!(
            status_code, 200,
            "case {:?}: DeleteStack must answer 200, got {status_code} {deleted}",
            row.case
        );
        wait_for_the_transition().await;
        wait_for_the_transition().await;

        let (status_code, described) = describe_stack(&router, row.stack).await;
        assert_eq!(
            status_code, 200,
            "case {:?}: DescribeStacks must answer 200, got {status_code} {described}",
            row.case
        );
        assert_eq!(
            xml_text(&described, "StackStatus").as_deref(),
            Some(status::DELETE_FAILED),
            "case {:?}: a deletion the owning service refused leaves the stack in \
             {}, got {described}",
            row.case,
            status::DELETE_FAILED
        );
        let reason = xml_text(&described, "StackStatusReason").unwrap_or_default();
        let expected = resource_failure_summary(failed_action::DELETE, [row.failing]);
        assert_eq!(
            reason, expected,
            "case {:?}: expected the stack-level sentence and nothing appended, got \
             {reason:?}",
            row.case
        );

        // The provider's own refusal is on the failing resource's event instead — the field
        // that carries the diagnosis, which the stack-level reason must not have swallowed.
        let reported = events(&router, row.stack).await;
        let failed: Vec<&(String, String, String)> = reported
            .iter()
            .filter(|(logical_id, event_status, _)| {
                logical_id == row.failing && event_status == status::DELETE_FAILED
            })
            .collect();
        assert!(
            !failed.is_empty(),
            "case {:?}: expected a {} event for {:?}, got {reported:?}",
            row.case,
            status::DELETE_FAILED,
            row.failing
        );
        for (_, _, event_reason) in &failed {
            assert!(
                !event_reason.is_empty(),
                "case {:?}: expected the failing resource's event to carry the refusal, got \
                 {reported:?}",
                row.case
            );
            assert!(
                !reason.contains(event_reason.as_str()),
                "case {:?}: the stack-level reason must not carry the resource's own reason \
                 {event_reason:?}, got {reason:?}",
                row.case
            );
        }

        // A resource of the same stack the deletion DID carry through is not named by the
        // sentence: only what failed is.
        if let Some(deleted_logical_id) = row.deleted {
            assert!(
                !reason.contains(deleted_logical_id),
                "case {:?}: expected the sentence to name only the failing resource, got \
                 {reason:?}",
                row.case
            );
            assert!(
                reported.iter().any(|(logical_id, event_status, _)| {
                    logical_id == deleted_logical_id && event_status == status::DELETE_COMPLETE
                }),
                "case {:?}: expected {deleted_logical_id:?} to have been deleted, got \
                 {reported:?}",
                row.case
            );
        }

        // The per-resource view agrees with the event stream about what became of each
        // resource: the one the deletion could not remove is reported in the failed delete
        // status, carrying the refusal, and one the same walk removed is reported deleted.
        let records = resources(&router, row.stack).await;
        let failing_record = records
            .iter()
            .find(|(logical_id, _, _)| logical_id == row.failing)
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: expected a record for {:?}, got {records:?}",
                    row.case, row.failing
                )
            });
        assert_eq!(
            failing_record.1,
            status::DELETE_FAILED,
            "case {:?}: expected the refused resource to report the failed delete status, \
             got {records:?}",
            row.case
        );
        assert_eq!(
            failing_record.2, failed[0].2,
            "case {:?}: expected the record to carry the same refusal its event does, got \
             {records:?}",
            row.case
        );
        if let Some(deleted_logical_id) = row.deleted {
            let deleted_record = records
                .iter()
                .find(|(logical_id, _, _)| logical_id == deleted_logical_id)
                .unwrap_or_else(|| {
                    panic!(
                        "case {:?}: expected a record for {deleted_logical_id:?}, got \
                         {records:?}",
                        row.case
                    )
                });
            assert_eq!(
                deleted_record.1,
                status::DELETE_COMPLETE,
                "case {:?}: expected the resource the walk removed to report its deletion, \
                 got {records:?}",
                row.case
            );

            // A RETRIED deletion asks nothing about the resource the first walk removed: a
            // record whose deletion was carried out no longer claims the resource exists,
            // so the retry reaches the one that failed and records no second deletion of
            // the other.
            let (status_code, retried) =
                call(&router, "DeleteStack", &[member("StackName", row.stack)]).await;
            assert_eq!(
                status_code, 200,
                "case {:?}: the retried DeleteStack must answer 200, got {status_code} \
                 {retried}",
                row.case
            );
            wait_for_the_transition().await;
            wait_for_the_transition().await;
            let after = events(&router, row.stack).await;
            let deletions = after
                .iter()
                .filter(|(logical_id, event_status, _)| {
                    logical_id == deleted_logical_id && event_status == status::DELETE_COMPLETE
                })
                .count();
            assert_eq!(
                deletions, 1,
                "case {:?}: expected {deleted_logical_id:?} to be deleted once, not asked \
                 for again by the retry, got {after:?}",
                row.case
            );
        }
    }
}
