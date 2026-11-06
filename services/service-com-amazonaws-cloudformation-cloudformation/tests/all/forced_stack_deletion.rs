// SPDX-License-Identifier: CC0-1.0

//! `DeleteStack` under `DeletionMode` `FORCE_DELETE_STACK`, end to end over the awsQuery
//! wire.
//!
//! RULE under test: **the deletion mode decides what a resource its owning service REFUSES
//! to delete does to the teardown — under `FORCE_DELETE_STACK` that resource is abandoned
//! (left exactly as it is, reported `DELETE_SKIPPED`) and the stack's deletion carries on to
//! `DELETE_COMPLETE`, so the stack stops being addressable by name; under the standard mode,
//! which `DeleteStackInput$DeletionMode` says is what naming no mode means, the refusal is
//! the stack's own failure and it stays `DELETE_FAILED`.**
//!
//! That is the member's own documentation: "STANDARD — use the standard behavior. Specifying
//! this value is the same as not specifying this parameter" and "FORCE_DELETE_STACK — delete
//! the stack if it's stuck in a DELETE_FAILED state due to resource deletion failure"
//! (`smithy_query(query="operation", operation="DeleteStack")`).
//!
//! The rows differ along what the rule generalises over: WHICH service refuses the deletion
//! and with which error (Amazon S3 refusing a bucket that still holds an object, Amazon RDS
//! refusing a protected DB instance) and where in the teardown order the refused resource
//! sits (resources declared both before and after it must still be deleted — a refusal no
//! longer stops the walk). Every row reaches the forced deletion from `DELETE_FAILED`, which
//! is the one status that mode is accepted from (`forced_deletion_is_gated_by_status.rs` is
//! the suite for that gate). `stack_delete_failure_reason.rs` and `refused_stack_deletion.rs`
//! are the suites for the standard mode's own outcome; this one is what the mode changes.

use smithy_server_runtime::MultiServiceRouter;

use crate::provisioned_resources::s3;
use crate::stack_lifecycle::{
    call, create_stack, describe_stack, member, rds, resource_event_reason, router, sns,
    wait_for_the_transition, xml_text,
};
use crate::stack_resources::sns_arn;
use service_com_amazonaws_cloudformation_cloudformation::state::{
    deletion_mode, resource_status, stack_does_not_exist, status,
};

/// What the force deletion ABANDONED, read back in the service that owns it: the resource is
/// still there and still serving, which is what "abandoned" means as against deleted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Abandoned {
    /// The bucket, which must still hold the object that blocked the deletion and must still
    /// accept another one.
    Bucket(&'static str),
    /// The DB instance, which must still be there with its deletion protection on.
    DbInstance(&'static str),
}

/// One stack deleted under `FORCE_DELETE_STACK`.
struct ForcedCase {
    case: &'static str,
    stack: &'static str,
    template: &'static str,
    /// A bucket to put an object into before the deletion, which is what makes Amazon S3
    /// refuse to delete it.
    fill_bucket: Option<&'static str>,
    /// The logical id whose deletion the owning service refuses, where the row has one.
    refused: Option<&'static str>,
    /// The topics of this stack Amazon SNS must no longer hold afterwards — declared on both
    /// sides of the refused resource where the row has one, so the walk is shown to carry on
    /// past the refusal.
    deleted_topics: &'static [&'static str],
    abandoned: Abandoned,
}

const FORCED_CASES: &[ForcedCase] = &[
    ForcedCase {
        case: "Amazon S3 refuses a bucket that still holds an object, declared between two \
               topics",
        stack: "forced-delete-archive",
        template: concat!(
            "Resources:\n",
            "  BeforeTopic:\n",
            "    Type: AWS::SNS::Topic\n",
            "    Properties:\n",
            "      TopicName: forced-delete-before-topic\n",
            "  Archive:\n",
            "    Type: AWS::S3::Bucket\n",
            "    Properties:\n",
            "      BucketName: cfn-forced-delete-archive\n",
            "  AfterTopic:\n",
            "    Type: AWS::SNS::Topic\n",
            "    Properties:\n",
            "      TopicName: forced-delete-after-topic\n",
        ),
        fill_bucket: Some("cfn-forced-delete-archive"),
        refused: Some("Archive"),
        deleted_topics: &["forced-delete-before-topic", "forced-delete-after-topic"],
        abandoned: Abandoned::Bucket("cfn-forced-delete-archive"),
    },
    ForcedCase {
        case: "Amazon RDS refuses a protected DB instance",
        stack: "forced-delete-protected-db",
        template: concat!(
            "Resources:\n",
            "  Db:\n",
            "    Type: AWS::RDS::DBInstance\n",
            "    Properties:\n",
            "      DBInstanceIdentifier: cfn-forced-delete-protected\n",
            "      Engine: mysql\n",
            "      DBInstanceClass: db.t3.micro\n",
            "      DeletionProtection: true\n",
            "  Topic:\n",
            "    Type: AWS::SNS::Topic\n",
            "    Properties:\n",
            "      TopicName: forced-delete-db-topic\n",
        ),
        fill_bucket: None,
        refused: Some("Db"),
        deleted_topics: &["forced-delete-db-topic"],
        abandoned: Abandoned::DbInstance("cfn-forced-delete-protected"),
    },
];

/// Create the row's stack, wait for it to settle, and answer its stack id.
///
/// Twice: the stack's own terminal status and the status an owning service owes a resource it
/// is bringing up are two clocks, and both have to be past due before a delete is asked for.
async fn created(router: &MultiServiceRouter, case: &str, stack: &str, template: &str) -> String {
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

/// `DeleteStack`, under the deletion mode a row names (none when `mode` is `None`), waited
/// past both of the clocks a teardown runs on.
async fn delete_under(
    router: &MultiServiceRouter,
    case: &str,
    stack: &str,
    mode: Option<&str>,
) {
    let mut members = vec![member("StackName", stack)];
    if let Some(mode) = mode {
        members.push(member("DeletionMode", mode));
    }
    let (status_code, body) = call(router, "DeleteStack", &members).await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: DeleteStack({mode:?}) must answer 200, got {status_code} {body}"
    );
    wait_for_the_transition().await;
    wait_for_the_transition().await;
}

/// The `StackStatus` a stack reports, read by stack id so a completed deletion is still
/// readable.
async fn reported_status(router: &MultiServiceRouter, case: &str, stack_id: &str) -> String {
    let (status_code, described) = describe_stack(router, stack_id).await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: DescribeStacks by id must answer 200, got {status_code} {described}"
    );
    xml_text(&described, "StackStatus").unwrap_or_default()
}

/// What the owning service holds for the resource the forced deletion abandoned.
async fn assert_still_there(router: &MultiServiceRouter, case: &str, abandoned: Abandoned) {
    match abandoned {
        Abandoned::Bucket(bucket) => {
            // Still serving the object that blocked the deletion...
            let (status_code, _, body) =
                s3(router, "GET", &format!("/{bucket}/an-object"), b"").await;
            assert_eq!(
                status_code, 200,
                "case {case:?}: expected the abandoned bucket to still serve its object, got \
                 {status_code} {body}"
            );
            // ...and still taking writes, so it was left usable rather than half-deleted.
            let (status_code, _, body) =
                s3(router, "PUT", &format!("/{bucket}/another-object"), b"after").await;
            assert_eq!(
                status_code, 200,
                "case {case:?}: expected the abandoned bucket to still accept a write, got \
                 {status_code} {body}"
            );
        }
        Abandoned::DbInstance(identifier) => {
            let (status_code, body) = rds(
                router,
                "DescribeDBInstances",
                &[("DBInstanceIdentifier", identifier)],
            )
            .await;
            assert_eq!(
                status_code, 200,
                "case {case:?}: expected the abandoned DB instance to still be there, got \
                 {status_code} {body}"
            );
            assert_eq!(
                xml_text(&body, "DeletionProtection").as_deref(),
                Some("true"),
                "case {case:?}: expected the abandoned instance to keep its protection, got \
                 {body}"
            );
        }
    }
}

#[tokio::test]
async fn a_forced_deletion_abandons_what_it_cannot_delete_and_still_completes() {
    for row in FORCED_CASES {
        let router = router();
        let stack_id = created(&router, row.case, row.stack, row.template).await;

        if let Some(bucket) = row.fill_bucket {
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

        // The standard deletion first, which is where a row with a refusal gets stuck: that
        // is the state `FORCE_DELETE_STACK` is documented for.
        if row.refused.is_some() {
            delete_under(&router, row.case, row.stack, None).await;
            assert_eq!(
                reported_status(&router, row.case, &stack_id).await,
                status::DELETE_FAILED,
                "case {:?}: expected the standard deletion to leave the stack stuck",
                row.case
            );
        }

        delete_under(
            &router,
            row.case,
            row.stack,
            Some(deletion_mode::FORCE_DELETE_STACK),
        )
        .await;
        assert_eq!(
            reported_status(&router, row.case, &stack_id).await,
            status::DELETE_COMPLETE,
            "case {:?}: expected the forced deletion to complete",
            row.case
        );

        // A completed deletion takes the stack out of the live set, so the NAME resolves
        // nothing: the stack is retired and its name is free again.
        let (status_code, body) = describe_stack(&router, row.stack).await;
        assert_eq!(
            status_code, 400,
            "case {:?}: expected the deleted stack's name to resolve nothing, got \
             {status_code} {body}",
            row.case
        );
        assert_eq!(
            (
                xml_text(&body, "Code").unwrap_or_default(),
                xml_text(&body, "Message").unwrap_or_default()
            ),
            (
                "ValidationError".to_string(),
                stack_does_not_exist(row.stack)
            ),
            "case {:?}: expected the read's own not-found refusal, got {body}",
            row.case
        );

        // The resource the deletion could not delete is reported SKIPPED, carrying the
        // owning service's own refusal, rather than as a deletion that did not happen.
        if let Some(refused) = row.refused {
            let reason = resource_event_reason(
                &router,
                &stack_id,
                refused,
                resource_status::DELETE_SKIPPED,
            )
            .await;
            assert!(
                !reason.is_empty(),
                "case {:?}: expected a {} event for {refused:?} carrying why it was skipped",
                row.case,
                resource_status::DELETE_SKIPPED
            );
        }

        // Every other resource of the stack really went, wherever it sat in the walk: a
        // refusal no longer stops the teardown.
        for topic in row.deleted_topics {
            let (status_code, body) = sns(
                &router,
                "GetTopicAttributes",
                &[("TopicArn", &sns_arn(topic))],
            )
            .await;
            assert_ne!(
                status_code, 200,
                "case {:?}: expected {topic:?} to have been deleted, got {status_code} {body}",
                row.case
            );
        }

        assert_still_there(&router, row.case, row.abandoned).await;
    }
}

/// The modes that are NOT the forced one leave the refusal as the stack's own failure, and
/// the forced one still gets the same stack out afterwards: naming `STANDARD` is "the same as
/// not specifying this parameter".
#[tokio::test]
async fn the_standard_mode_named_or_not_leaves_the_refused_deletion_failed() {
    for (case, mode) in [
        ("no DeletionMode at all", None),
        ("DeletionMode STANDARD", Some(deletion_mode::STANDARD)),
    ] {
        let router = router();
        let stack = "standard-delete-archive";
        let bucket = "cfn-standard-delete-archive";
        let template = concat!(
            "Resources:\n",
            "  Archive:\n",
            "    Type: AWS::S3::Bucket\n",
            "    Properties:\n",
            "      BucketName: cfn-standard-delete-archive\n",
        );
        let stack_id = created(&router, case, stack, template).await;
        let (status_code, _, body) =
            s3(&router, "PUT", &format!("/{bucket}/an-object"), b"held").await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: the object must be stored, got {status_code} {body}"
        );

        delete_under(&router, case, stack, mode).await;
        assert_eq!(
            reported_status(&router, case, &stack_id).await,
            status::DELETE_FAILED,
            "case {case:?}: expected the refusal to fail the stack under this mode"
        );
        // The resource reports the FAILED delete under this mode, not the skipped one.
        let failed =
            resource_event_reason(&router, &stack_id, "Archive", resource_status::DELETE_FAILED)
                .await;
        assert!(
            !failed.is_empty(),
            "case {case:?}: expected the refused resource to report a failed delete"
        );
        assert!(
            resource_event_reason(
                &router,
                &stack_id,
                "Archive",
                resource_status::DELETE_SKIPPED
            )
            .await
            .is_empty(),
            "case {case:?}: expected nothing skipped under this mode"
        );

        // And the escape hatch still works from there.
        delete_under(
            &router,
            case,
            stack,
            Some(deletion_mode::FORCE_DELETE_STACK),
        )
        .await;
        assert_eq!(
            reported_status(&router, case, &stack_id).await,
            status::DELETE_COMPLETE,
            "case {case:?}: expected the forced deletion to retire the stuck stack"
        );
    }
}
