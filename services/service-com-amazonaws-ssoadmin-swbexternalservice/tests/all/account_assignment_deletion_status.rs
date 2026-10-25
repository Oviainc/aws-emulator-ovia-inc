// SPDX-License-Identifier: CC0-1.0

//! `DescribeAccountAssignmentDeletionStatus` — the poll
//! `DeleteAccountAssignment` documents ("After a successful response, call
//! DescribeAccountAssignmentDeletionStatus to describe the status of an assignment
//! deletion request") — driven end to end over the `awsJson1_1` wire.
//!
//! `account_assignment_delete.rs` asserts the DELETE and the status of the operation it
//! has just started; this suite is about the status READ itself, over a history that
//! holds many deletions:
//!
//! 1. the read answers from the STORED record the `RequestId` names — every detail
//!    member describes the access THAT operation withdrew, however many later deletions
//!    have been recorded since;
//! 2. only the clock moves a reported status: it is the non-terminal `IN_PROGRESS` while
//!    the operation's completion is still ahead, and the terminal `SUCCEEDED` from the
//!    instant that completion falls due onwards, with every other member of the record
//!    unmoved;
//! 3. the read is a REPORT: repeating it answers identically, and it consumes nothing —
//!    the assignments the instance still holds, the creation status of a surviving
//!    assignment and a later delete of it are all exactly what they were before the poll.
//!
//! The rows differ along what the rules generalise over: both `PrincipalType` enum
//! values, three of the forms the model's `PrincipalId` pattern admits, both ends of
//! `TargetId`'s `^\d{12}$` (the table `account_assignment_delete.rs` owns), each row
//! withdrawn under two different permission sets, and the clock read at several distances
//! on both sides of the due instant, the closest of them no more than a moment past it.
//! The due instant is moved in the store rather than slept for; the status is still
//! derived by the service, exactly as in production.
//!
//! Every call goes through `MultiServiceRouter::dispatch`, the one path a socket-served
//! request takes. The harness is the one `ssoadmin_permission_sets.rs` owns.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::account_assignment_delete::{
    assigned, assignment_body, describe, grant, hold_completion, permission_set, Case, CASES,
    DETAIL_MEMBERS,
};
use crate::ssoadmin_permission_sets::{call, organization_engine, router_over, with_store};

/// One recorded deletion: the handle it is polled by, the permission set it was withdrawn
/// under and the row it withdrew.
struct Recorded {
    request_id: String,
    permission_set_arn: String,
    case: Case,
}

/// Grant one row under `permission_set_arn` and withdraw it again, returning the
/// deletion's `RequestId`.
async fn withdraw(
    router: &MultiServiceRouter,
    instance_arn: &str,
    permission_set_arn: &str,
    case: &Case,
) -> String {
    grant(router, instance_arn, permission_set_arn, case).await;
    let (status, body) = call(
        router,
        "DeleteAccountAssignment",
        &assignment_body(instance_arn, permission_set_arn, case),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {:?}: DeleteAccountAssignment expected 200, got {status} {body}",
        case.label
    );
    body["AccountAssignmentDeletionStatus"]["RequestId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {:?}: the delete minted no RequestId: {body}", case.label))
        .to_string()
}

/// The status object `DescribeAccountAssignmentDeletionStatus` reports for `request_id`,
/// asserting the call itself succeeded.
async fn reported(
    router: &MultiServiceRouter,
    instance_arn: &str,
    request_id: &str,
    label: &str,
) -> Value {
    let (status, body) = describe(router, instance_arn, request_id).await;
    assert_eq!(
        status, 200,
        "case {label:?}: DescribeAccountAssignmentDeletionStatus expected 200, got \
         {status} {body}"
    );
    body["AccountAssignmentDeletionStatus"].clone()
}

/// RULES 1 and 2 — every stored deletion reports its OWN withdrawn access, and the only
/// member the clock moves is the status.
#[tokio::test]
async fn every_recorded_deletion_reports_its_own_access_at_the_status_the_clock_implies() {
    let (engine, instance_arn) = organization_engine();
    let router = router_over(Arc::clone(&engine));
    // Two permission sets carrying the same rows: a status read that picked a record by
    // anything less than the whole `RequestId` would answer with the other set's.
    let first = permission_set(&router, &instance_arn, "FirstSet").await;
    let second = permission_set(&router, &instance_arn, "SecondSet").await;

    let mut history: Vec<Recorded> = Vec::new();
    for permission_set_arn in [&first, &second] {
        for case in CASES {
            let request_id = withdraw(&router, &instance_arn, permission_set_arn, case).await;
            assert!(
                history
                    .iter()
                    .all(|recorded| recorded.request_id != request_id),
                "case {:?}: every deletion mints a distinct RequestId, got {request_id:?} again",
                case.label
            );
            history.push(Recorded {
                request_id,
                permission_set_arn: permission_set_arn.clone(),
                case: *case,
            });
        }
    }

    // The clock table: the status is derived from the stored due instant, so the record is
    // read with that instant well ahead of the call and behind it, and with the instant
    // set to the moment the row starts — the handler samples its own clock at or just
    // after that, which is the narrowest side of the boundary a wire test can ask for, and
    // a completion that is already due has happened, so the status there is terminal.
    let clock: [(&str, i64, &str); 5] = [
        ("an hour before the completion is due", 3_600_000, "IN_PROGRESS"),
        ("a minute before the completion is due", 60_000, "IN_PROGRESS"),
        ("no more than a moment after the completion fell due", 0, "SUCCEEDED"),
        ("a second after the completion fell due", -1_000, "SUCCEEDED"),
        ("an hour after the completion fell due", -3_600_000, "SUCCEEDED"),
    ];
    // What the first reading reported for each handle, so a later reading can be checked
    // to have moved NOTHING but the status.
    let mut stable: Vec<Value> = Vec::new();
    for (round, (when, offset_millis, expected_status)) in clock.iter().enumerate() {
        let span = Duration::from_millis(offset_millis.unsigned_abs());
        let due_at = if *offset_millis >= 0 {
            SystemTime::now() + span
        } else {
            SystemTime::now() - span
        };
        hold_completion(&engine, due_at);

        for (index, recorded) in history.iter().enumerate() {
            let label = format!("{} {when}", recorded.case.label);
            let status = reported(&router, &instance_arn, &recorded.request_id, &label).await;
            assert_eq!(
                status["Status"].as_str(),
                Some(*expected_status),
                "case {label:?}: expected status {expected_status:?}, got {:?}",
                status["Status"]
            );
            assert_eq!(
                status["RequestId"].as_str(),
                Some(recorded.request_id.as_str()),
                "case {label:?}: expected the polled RequestId {:?}, got {:?}",
                recorded.request_id,
                status["RequestId"]
            );
            // RULE 1 — the detail members are the access THIS handle withdrew, not the
            // one the newest deletion in the history withdrew.
            for (member, expected) in [
                ("TargetId", recorded.case.account_id),
                ("TargetType", "AWS_ACCOUNT"),
                ("PermissionSetArn", recorded.permission_set_arn.as_str()),
                ("PrincipalType", recorded.case.principal_type),
                ("PrincipalId", recorded.case.principal_id),
            ] {
                assert_eq!(
                    status[member].as_str(),
                    Some(expected),
                    "case {label:?}: expected {member} {expected:?}, got {:?}",
                    status[member]
                );
            }
            assert!(
                status["CreatedDate"].is_number(),
                "case {label:?}: expected the date the deletion was accepted, got {:?}",
                status["CreatedDate"]
            );
            assert_eq!(
                status["FailureReason"],
                Value::Null,
                "case {label:?}: nothing failed, so no FailureReason, got {:?}",
                status["FailureReason"]
            );
            if round == 0 {
                stable.push(status);
            } else {
                // RULE 2 — the clock moves the status and nothing else.
                for member in DETAIL_MEMBERS
                    .iter()
                    .chain(["RequestId", "CreatedDate", "FailureReason"].iter())
                {
                    assert_eq!(
                        status[*member], stable[index][*member],
                        "case {label:?}: {member} must not move with the status, expected \
                         {:?}, got {:?}",
                        stable[index][*member], status[*member]
                    );
                }
            }
        }
    }
}

/// RULE 3 — the poll is a REPORT: it answers the same object however often it is asked
/// and changes nothing a later caller sees.
#[tokio::test]
async fn polling_a_deletion_status_changes_nothing_a_later_caller_sees() {
    let (engine, instance_arn) = organization_engine();
    let router = router_over(Arc::clone(&engine));
    let permission_set_arn = permission_set(&router, &instance_arn, "Polled").await;
    let withdrawn = CASES[0];
    let kept = CASES[1];
    assert_eq!(
        withdrawn.account_id, kept.account_id,
        "the two rows must share an account so one ListAccountAssignments call sees both"
    );

    let kept_creation_id = grant(&router, &instance_arn, &permission_set_arn, &kept).await;
    let request_id = withdraw(&router, &instance_arn, &permission_set_arn, &withdrawn).await;
    // The clock is the only thing that may move a status, so it is held still: any
    // difference between the reads below would then be the READ's doing.
    hold_completion(&engine, SystemTime::now() - Duration::from_secs(1));

    let rows_before = assigned(
        &router,
        &instance_arn,
        withdrawn.account_id,
        &permission_set_arn,
    )
    .await;
    let history_before = with_store(&engine, |store| store.assignment_deletions.len());

    let first = reported(&router, &instance_arn, &request_id, withdrawn.label).await;
    for attempt in 2..=4 {
        let again = reported(&router, &instance_arn, &request_id, withdrawn.label).await;
        assert_eq!(
            again, first,
            "attempt {attempt}: a poll must report the same record, expected {first}, got \
             {again}"
        );
    }

    let rows_after = assigned(
        &router,
        &instance_arn,
        withdrawn.account_id,
        &permission_set_arn,
    )
    .await;
    assert_eq!(
        rows_after, rows_before,
        "polling a deletion status must leave the assignments untouched, expected \
         {rows_before:?}, got {rows_after:?}"
    );
    let history_after = with_store(&engine, |store| store.assignment_deletions.len());
    assert_eq!(
        history_after, history_before,
        "polling must consume no deletion record, expected {history_before} stored, got \
         {history_after}"
    );

    // The CREATION history of the assignment that is still granted is another operation's
    // record, and the deletion poll leaves it readable.
    let (status, creation) = call(
        &router,
        "DescribeAccountAssignmentCreationStatus",
        &json!({
            "InstanceArn": instance_arn,
            "AccountAssignmentCreationRequestId": kept_creation_id,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the surviving assignment's creation status must still read, got {status} {creation}"
    );
    assert_eq!(
        creation["AccountAssignmentCreationStatus"]["PrincipalId"].as_str(),
        Some(kept.principal_id),
        "expected the granted access's PrincipalId {:?}, got {:?}",
        kept.principal_id,
        creation["AccountAssignmentCreationStatus"]["PrincipalId"]
    );

    // And the access the poll reported on is still gone while the other is still there:
    // deleting the surviving row afterwards is a NEW operation under a new handle, and
    // the polled handle keeps reporting its own record.
    let (status, deleted) = call(
        &router,
        "DeleteAccountAssignment",
        &assignment_body(&instance_arn, &permission_set_arn, &kept),
    )
    .await;
    assert_eq!(
        status, 200,
        "deleting the surviving assignment expected 200, got {status} {deleted}"
    );
    let second_id = deleted["AccountAssignmentDeletionStatus"]["RequestId"]
        .as_str()
        .unwrap_or_else(|| panic!("the second delete minted no RequestId: {deleted}"))
        .to_string();
    assert_ne!(
        second_id, request_id,
        "the second deletion must mint its own RequestId, got the polled {request_id:?}"
    );
    let still = reported(&router, &instance_arn, &request_id, withdrawn.label).await;
    assert_eq!(
        still, first,
        "a later deletion must not rewrite an earlier record, expected {first}, got {still}"
    );
}
