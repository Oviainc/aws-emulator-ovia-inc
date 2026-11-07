// SPDX-License-Identifier: CC0-1.0

//! `DeleteStack` with `RetainResources`, end to end over the awsQuery wire.
//!
//! RULE under test: **for a stack whose deletion FAILED, `RetainResources` names the
//! resources this deletion must not remove: nothing at all is asked of the services that own
//! them, so each stays exactly as it is and the stack records its deletion as
//! `DELETE_SKIPPED`, while every resource the request does NOT name is deleted as it would
//! be without the member and the stack's own deletion reaches `DELETE_COMPLETE`.** The
//! member overrides whatever `DeletionPolicy` the retained resource's declaration carries,
//! and it is refused for a stack in any status other than `DELETE_FAILED`.
//!
//! Why it matters: it is the documented way out of a stack stuck in `DELETE_FAILED` for a
//! resource that cannot be deleted — a non-empty bucket — without destroying that resource
//! or leaving the stack behind forever.
//!
//! The rows differ along what the rule generalises over: how many of the stack's resources
//! are retained (the one that blocked the deletion, and that one together with a resource
//! that would have deleted cleanly), and the status the call arrives in (the `DELETE_FAILED`
//! the member is for, and a healthy stack, which is refused).

use service_com_amazonaws_cloudformation_cloudformation::state::status;
use smithy_server_runtime::MultiServiceRouter;

use crate::stack_lifecycle::{
    call, create_stack, describe_stack, member, resource_event_reason, router,
    wait_for_the_transition, xml_text,
};
use crate::stack_resource_provisioning::s3;

/// A stack over two buckets: the one the case fills with an object, which Amazon S3 then
/// refuses to delete, and one that stays empty. The held bucket DEPENDS ON the empty one, so
/// a deletion that cannot remove the held bucket leaves the empty one alone too — a resource
/// the refused one needs cannot be deleted while that one is still there — which is what
/// makes the retry's own work observable.
fn two_buckets(held: &str, empty: &str) -> String {
    format!(
        r#"{{"Resources":{{
            "Empty":{{"Type":"AWS::S3::Bucket","Properties":{{"BucketName":"{empty}"}}}},
            "Held":{{"Type":"AWS::S3::Bucket","DependsOn":"Empty",
                "Properties":{{"BucketName":"{held}"}}}}}}}}"#
    )
}

/// `DeleteStack`, with whatever `RetainResources` the case names.
async fn delete_stack(
    router: &MultiServiceRouter,
    stack: &str,
    retained: &[&str],
) -> (u16, String) {
    let mut members = vec![member("StackName", stack)];
    for (position, logical_id) in retained.iter().enumerate() {
        members.push(member(
            &format!("RetainResources.member.{}", position + 1),
            logical_id,
        ));
    }
    call(router, "DeleteStack", &members).await
}

/// Deploy the two-bucket stack with an object in the `held` bucket, drive its deletion into
/// `DELETE_FAILED`, and answer the stack's id (a failed deletion leaves the stack live, but
/// the id is what a completed one is read by).
async fn a_stack_whose_deletion_failed(
    router: &MultiServiceRouter,
    case: &str,
    stack: &str,
    held: &str,
    empty: &str,
) -> String {
    let (code, body) = create_stack(router, stack, &two_buckets(held, empty), &[]).await;
    assert_eq!(
        code, 200,
        "case {case:?}: CreateStack must answer 200, got {code} {body}"
    );
    wait_for_the_transition().await;
    let (code, body) = describe_stack(router, stack).await;
    assert_eq!(
        code, 200,
        "case {case:?}: DescribeStacks must answer 200, got {code} {body}"
    );
    let stack_id = xml_text(&body, "StackId").unwrap_or_default();

    // The object is written AFTER the create, so the deletion is the thing Amazon S3
    // refuses: a bucket that still holds objects cannot be deleted.
    let written = s3(
        router,
        "PUT",
        &format!("/{held}/held/object.txt"),
        b"an object the deletion cannot remove",
    )
    .await;
    assert!(
        (200..300).contains(&written.status),
        "case {case:?}: the object must be written, got {} {}",
        written.status,
        written.body
    );

    let (code, body) = delete_stack(router, stack, &[]).await;
    assert_eq!(
        code, 200,
        "case {case:?}: DeleteStack must be accepted, got {code} {body}"
    );
    wait_for_the_transition().await;
    wait_for_the_transition().await;
    let (settled, _) = reported(router, case, &stack_id).await;
    assert_eq!(
        settled,
        status::DELETE_FAILED,
        "case {case:?}: expected the deletion to fail on the non-empty bucket, got {settled:?}"
    );
    // Both buckets are still there: the one S3 refused, and the one it needs.
    for bucket in [held, empty] {
        let answered = s3(router, "HEAD", &format!("/{bucket}"), b"").await;
        assert_eq!(
            answered.status, 200,
            "case {case:?}: expected {bucket} to survive the failed deletion, got {} {}",
            answered.status, answered.body
        );
    }
    stack_id
}

/// The `(StackStatus, StackStatusReason)` one stack reports.
async fn reported(router: &MultiServiceRouter, case: &str, id: &str) -> (String, String) {
    let (code, body) = describe_stack(router, id).await;
    assert_eq!(
        code, 200,
        "case {case:?}: DescribeStacks({id}) must answer 200, got {code} {body}"
    );
    (
        xml_text(&body, "StackStatus").unwrap_or_default(),
        xml_text(&body, "StackStatusReason").unwrap_or_default(),
    )
}

#[tokio::test]
async fn a_retained_resource_is_left_intact_and_the_stack_is_deleted() {
    // The rows differ along WHICH resources the retry retains.
    let rows: &[(&str, &[&str])] = &[
        (
            "only the resource the deletion failed on is retained",
            &["Held"],
        ),
        (
            "the failed resource AND one that would have deleted cleanly are retained",
            &["Held", "Empty"],
        ),
    ];

    for (case, retained) in rows {
        let router = router();
        let stack = "retained-delete";
        let held = format!("retained-held-{}", retained.len());
        let empty = format!("retained-empty-{}", retained.len());
        let stack_id =
            a_stack_whose_deletion_failed(&router, case, stack, &held, &empty).await;

        let (code, body) = delete_stack(&router, stack, retained).await;
        assert_eq!(
            code, 200,
            "case {case:?}: the retrying DeleteStack must be accepted, got {code} {body}"
        );
        wait_for_the_transition().await;
        wait_for_the_transition().await;

        // The stack is GONE: addressed by its id, since a completed deletion leaves the
        // live set.
        let (settled, reason) = reported(&router, case, &stack_id).await;
        assert_eq!(
            settled,
            status::DELETE_COMPLETE,
            "case {case:?}: expected the deletion to complete around the retained \
             resources, got {settled:?} ({reason:?})"
        );

        // Each retained resource reports a SKIPPED deletion, and is still there in Amazon
        // S3 with its contents.
        for logical_id in *retained {
            let skipped =
                resource_event_reason(&router, &stack_id, logical_id, "DELETE_SKIPPED").await;
            assert!(
                skipped.is_empty(),
                "case {case:?}: a retained resource is skipped, not refused, got {skipped:?}"
            );
        }
        let bucket = s3(&router, "HEAD", &format!("/{held}"), b"").await;
        assert_eq!(
            bucket.status, 200,
            "case {case:?}: the retained bucket must survive, got {} {}",
            bucket.status, bucket.body
        );
        let object = s3(&router, "GET", &format!("/{held}/held/object.txt"), b"").await;
        assert_eq!(
            object.status, 200,
            "case {case:?}: the retained bucket's object must survive, got {} {}",
            object.status, object.body
        );

        // ...and a resource the request did NOT name is deleted: the member retains what it
        // names, not whatever the failed deletion had left standing.
        let empty_bucket = s3(&router, "HEAD", &format!("/{empty}"), b"").await;
        let expected = if retained.contains(&"Empty") { 200 } else { 404 };
        assert_eq!(
            empty_bucket.status, expected,
            "case {case:?}: expected the unnamed bucket to be {}, got {} {}",
            if expected == 200 { "retained" } else { "deleted" },
            empty_bucket.status,
            empty_bucket.body
        );
    }
}

/// The member belongs to the one status it is documented for: a healthy stack is refused,
/// and keeps everything it has.
#[tokio::test]
async fn retaining_resources_is_refused_for_a_stack_that_is_not_delete_failed() {
    let case = "a stack whose create completed";
    let router = router();
    let (code, body) = create_stack(
        &router,
        "retained-healthy",
        &two_buckets("retained-healthy-held", "retained-healthy-empty"),
        &[],
    )
    .await;
    assert_eq!(
        code, 200,
        "case {case:?}: CreateStack must answer 200, got {code} {body}"
    );
    wait_for_the_transition().await;

    let (code, body) = delete_stack(&router, "retained-healthy", &["Held"]).await;
    assert_eq!(
        code, 400,
        "case {case:?}: expected the refusal, got {code} {body}"
    );
    assert_eq!(
        xml_text(&body, "Code").as_deref(),
        Some("ValidationError"),
        "case {case:?}: expected a ValidationError, got {body}"
    );
    let message = xml_text(&body, "Message").unwrap_or_default();
    assert!(
        message.contains("DELETE_FAILED") && message.contains("CREATE_COMPLETE"),
        "case {case:?}: expected the refusal to name the status required and the one in \
         force, got {message:?}"
    );
    // Nothing happened: the stack is still there, in the status it was in.
    let (settled, _) = reported(&router, case, "retained-healthy").await;
    assert_eq!(
        settled,
        status::CREATE_COMPLETE,
        "case {case:?}: expected the refused call to leave the stack alone, got {settled:?}"
    );
}
