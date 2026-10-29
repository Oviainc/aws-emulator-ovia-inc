// SPDX-License-Identifier: CC0-1.0

//! The LIFETIME of an assignment-deletion status: a status reports work on one permission
//! set, so the handle stops resolving once that permission set is gone — driven end to end
//! over the `awsJson1_1` wire.
//!
//! The rule under test, asserted over a TABLE of inputs rather than over one example:
//!
//! 1. `DescribeAccountAssignmentDeletionStatus` answers a handle whose permission set has
//!    gone exactly as it answers one nothing was ever minted under — the same
//!    `ResourceNotFoundException` and the same message — while every handle whose
//!    permission set is still there keeps reporting its own record;
//! 2. `ListAccountAssignmentDeletionStatus` enumerates exactly the handles that describe
//!    resolves, in the order the history happened, under every `Filter$Status` value as
//!    well as unfiltered, so the enumeration and the describe cannot disagree about which
//!    handles exist;
//! 3. the three status readers of this service agree: the CREATION status of an assignment
//!    of that permission set, its PROVISIONING statuses and the DELETION status of the
//!    assignment all stop resolving with it, so no reader reports a `PermissionSetArn`
//!    that `DescribePermissionSet` calls absent.
//!
//! The rows differ along what the rule generalises over: the history is the table
//! `account_assignment_delete.rs` owns (both `PrincipalType` values, three forms of
//! `PrincipalId`, both ends of `TargetId`'s `^\d{12}$`), recorded under three permission
//! sets that are deleted one after another so a status is withdrawn with ITS OWN set and
//! not with any delete, and read with the stored completion instants on either side of the
//! clock — a terminal status is withdrawn exactly as a non-terminal one is. Those instants
//! are moved in the store rather than slept for; each status is still derived by the
//! service, exactly as in production.
//!
//! Every call goes through `MultiServiceRouter::dispatch`, the one path a socket-served
//! request takes. The harness is the one `ssoadmin_permission_sets.rs` owns.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_ssoadmin_swbexternalservice::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::account_assignment_delete::{
    assignment_body, describe, grant, hold_completion, permission_set, Case, CASES,
};
use crate::ssoadmin_permission_sets::{
    assert_refused, call, organization_engine, router_over,
};

/// The refusal every reader of this service answers a handle it resolves nothing for.
const NOT_FOUND: &str = "ResourceNotFoundException";

/// The two values of the model's `StatusValues` an assignment operation of this emulator
/// walks (`smithy_query(query="shape", shape="StatusValues")`: `IN_PROGRESS | FAILED |
/// SUCCEEDED`; nothing here fails an operation).
const IN_PROGRESS: &str = "IN_PROGRESS";
const SUCCEEDED: &str = "SUCCEEDED";

/// The permission sets the history below is recorded under: the first two are deleted, one
/// after the other, and the last one survives to the end.
const SETS: [&str; 3] = ["FirstDoomed", "SecondDoomed", "Surviving"];

/// A pattern-valid `UUId` ("length 36..36",
/// `^\b[0-9a-f]{8}\b-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-\b[0-9a-f]{12}\b$`) no deletion
/// was ever minted under, which is the answer a withdrawn handle is compared against.
const UNMINTED_REQUEST_ID: &str = "00000000-0000-4000-8000-000000000000";

/// One recorded deletion: the handle it is polled by, the permission set it was withdrawn
/// under and that set's position in [`SETS`].
struct Withdrawn {
    label: String,
    set: usize,
    request_id: String,
    permission_set_arn: String,
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

/// Every handle `ListAccountAssignmentDeletionStatus` reports for `instance_arn`, in the
/// order it reported them, optionally under a `Filter$Status`.
///
/// One page: the histories here hold fewer rows than the model's `MaxResults` ceiling
/// (`range 1..100`), so a `NextToken` would mean the enumeration kept rows back. Paging
/// itself is `account_assignment_deletion_status_list.rs`'s business.
async fn listed_handles(
    router: &MultiServiceRouter,
    instance_arn: &str,
    status_filter: Option<&str>,
    case: &str,
) -> Vec<String> {
    let mut request = json!({"InstanceArn": instance_arn});
    if let Some(status) = status_filter {
        request["Filter"] = json!({"Status": status});
    }
    let (status, body) = call(router, "ListAccountAssignmentDeletionStatus", &request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListAccountAssignmentDeletionStatus expected 200, got {status} {body}"
    );
    assert_eq!(
        body["NextToken"],
        Value::Null,
        "case {case:?}: the whole history fits one page, so no NextToken, got {body}"
    );
    body["AccountAssignmentsDeletionStatus"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: the page carried no status list: {body}"))
        .iter()
        .map(|row| {
            row["RequestId"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a row carried no RequestId: {row}"))
                .to_string()
        })
        .collect()
}

/// Assert rules 1 and 2 over the whole history, with the first `deleted_sets` of [`SETS`]
/// already deleted: a handle of a deleted set is refused as an unminted one is, every other
/// handle reports its own record at `expected_status`, and the enumeration reports exactly
/// those surviving handles — unfiltered, under `expected_status` and under the status the
/// clock does not imply.
async fn assert_history(
    router: &MultiServiceRouter,
    instance_arn: &str,
    history: &[Withdrawn],
    deleted_sets: usize,
    expected_status: &str,
    when: &str,
) {
    let mut surviving: Vec<String> = Vec::new();
    for record in history {
        let case = format!("{} with {deleted_sets} of the sets deleted", record.label);
        let (status, body) = describe(router, instance_arn, &record.request_id).await;
        if record.set >= deleted_sets {
            assert_eq!(
                status, 200,
                "case {case:?}: a handle whose permission set is still there must resolve, \
                 got {status} {body}"
            );
            let reported = &body["AccountAssignmentDeletionStatus"];
            assert_eq!(
                reported["PermissionSetArn"].as_str(),
                Some(record.permission_set_arn.as_str()),
                "case {case:?}: expected PermissionSetArn {:?}, got {:?}",
                record.permission_set_arn,
                reported["PermissionSetArn"]
            );
            assert_eq!(
                reported["Status"].as_str(),
                Some(expected_status),
                "case {case:?}: expected status {expected_status:?}, got {:?}",
                reported["Status"]
            );
            surviving.push(record.request_id.clone());
        } else {
            assert_refused(
                &case,
                "DescribeAccountAssignmentDeletionStatus",
                NOT_FOUND,
                status,
                &body,
            );
            assert_eq!(
                body["Message"].as_str().unwrap_or_default(),
                state::assignment_operation_not_found_message(&record.request_id),
                "case {case:?}: a withdrawn handle must be refused exactly as an unminted \
                 one, expected {:?}, got {body}",
                state::assignment_operation_not_found_message(&record.request_id)
            );
        }
    }

    let case = format!("{when}, {deleted_sets} of the sets deleted");
    let listed = listed_handles(router, instance_arn, None, &case).await;
    assert_eq!(
        listed, surviving,
        "case {case:?}: the enumeration must report exactly the handles the describe \
         resolves, oldest first, expected {surviving:?}, got {listed:?}"
    );
    let filtered = listed_handles(router, instance_arn, Some(expected_status), &case).await;
    assert_eq!(
        filtered, surviving,
        "case {case:?}: filtering on {expected_status:?} must select within the surviving \
         handles, expected {surviving:?}, got {filtered:?}"
    );
    let other_status = if expected_status == IN_PROGRESS {
        SUCCEEDED
    } else {
        IN_PROGRESS
    };
    let other = listed_handles(router, instance_arn, Some(other_status), &case).await;
    assert!(
        other.is_empty(),
        "case {case:?}: no surviving handle is {other_status:?} here, got {other:?}"
    );
}

/// RULES 1 and 2 — a deletion status is withdrawn with the permission set its operation
/// named, and the describe and the enumeration are withdrawn together.
#[tokio::test]
async fn a_deletion_status_goes_with_the_permission_set_its_operation_named() {
    // The clock table: a status is derived from the stored due instant, so the history is
    // read with that instant ahead of the call and behind it — the rule holds whatever
    // status the clock implies, so a completed deletion is withdrawn as an in-flight one is.
    let clock: [(&str, i64, &str); 3] = [
        ("while the deletions are still in flight", 3_600_000, IN_PROGRESS),
        ("no more than a moment after they fell due", 0, SUCCEEDED),
        ("an hour after they fell due", -3_600_000, SUCCEEDED),
    ];
    for (when, offset_millis, expected_status) in clock {
        let (engine, instance_arn) = organization_engine();
        let router = router_over(Arc::clone(&engine));

        let mut arns: Vec<String> = Vec::new();
        for name in SETS {
            arns.push(permission_set(&router, &instance_arn, name).await);
        }
        let mut history: Vec<Withdrawn> = Vec::new();
        for (set, permission_set_arn) in arns.iter().enumerate() {
            for case in CASES {
                let request_id = withdraw(&router, &instance_arn, permission_set_arn, case).await;
                history.push(Withdrawn {
                    label: format!("{} under {} {when}", case.label, SETS[set]),
                    set,
                    request_id,
                    permission_set_arn: permission_set_arn.clone(),
                });
            }
        }

        let span = Duration::from_millis(offset_millis.unsigned_abs());
        let due_at = if offset_millis >= 0 {
            SystemTime::now() + span
        } else {
            SystemTime::now() - span
        };
        hold_completion(&engine, due_at);

        // Nothing is deleted yet: every handle of the history resolves.
        assert_history(&router, &instance_arn, &history, 0, expected_status, when).await;

        // Then the sets go, one after the other: each delete withdraws its OWN set's
        // handles and leaves every other handle resolving.
        for doomed in 0..SETS.len() - 1 {
            let (status, body) = call(
                &router,
                "DeletePermissionSet",
                &json!({"InstanceArn": instance_arn, "PermissionSetArn": arns[doomed]}),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {when:?}: DeletePermissionSet {:?} expected 200, got {status} {body}",
                SETS[doomed]
            );
            assert_history(
                &router,
                &instance_arn,
                &history,
                doomed + 1,
                expected_status,
                when,
            )
            .await;
        }
    }
}

/// RULE 3 — the service's three status readers agree: a creation status, a provisioning
/// status and a deletion status of a permission set are all withdrawn with it, each with
/// its own history's not-found message, and the permission set itself is then absent.
#[tokio::test]
async fn every_status_reader_withdraws_the_handles_of_a_deleted_permission_set() {
    let (engine, instance_arn) = organization_engine();
    let router = router_over(Arc::clone(&engine));
    let permission_set_arn = permission_set(&router, &instance_arn, "Doomed").await;

    // One assignment withdrawn (a DELETION status), one still granted (its CREATION
    // status), and the permission set provisioned into one account (a PROVISIONING status).
    let deleted = CASES[0];
    let kept = CASES[1];
    let deletion_id = withdraw(&router, &instance_arn, &permission_set_arn, &deleted).await;
    let creation_id = grant(&router, &instance_arn, &permission_set_arn, &kept).await;
    let (status, body) = call(
        &router,
        "ProvisionPermissionSet",
        &json!({
            "InstanceArn": instance_arn,
            "PermissionSetArn": permission_set_arn,
            "TargetType": "AWS_ACCOUNT",
            "TargetId": kept.account_id,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "ProvisionPermissionSet expected 200, got {status} {body}"
    );
    let provisioning_id = body["PermissionSetProvisioningStatus"]["RequestId"]
        .as_str()
        .unwrap_or_else(|| panic!("the provisioning minted no RequestId: {body}"))
        .to_string();

    // Each reader, the call it is driven by and the member carrying the handle, with the
    // not-found message of ITS history.
    let readers: [(&str, &str, &str, String); 3] = [
        (
            "the deletion status",
            "DescribeAccountAssignmentDeletionStatus",
            "AccountAssignmentDeletionRequestId",
            deletion_id.clone(),
        ),
        (
            "the creation status",
            "DescribeAccountAssignmentCreationStatus",
            "AccountAssignmentCreationRequestId",
            creation_id.clone(),
        ),
        (
            "the provisioning status",
            "DescribePermissionSetProvisioningStatus",
            "ProvisionPermissionSetRequestId",
            provisioning_id.clone(),
        ),
    ];
    for (label, target, member, handle) in &readers {
        let (status, body) = call(
            &router,
            target,
            &json!({"InstanceArn": instance_arn, (*member): handle}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: the handle must resolve while the permission set is there, got \
             {status} {body}"
        );
    }

    let (status, body) = call(
        &router,
        "DeletePermissionSet",
        &json!({"InstanceArn": instance_arn, "PermissionSetArn": permission_set_arn}),
    )
    .await;
    assert_eq!(
        status, 200,
        "DeletePermissionSet expected 200, got {status} {body}"
    );

    // The permission set every one of those handles names is now absent...
    let (status, body) = call(
        &router,
        "DescribePermissionSet",
        &json!({"InstanceArn": instance_arn, "PermissionSetArn": permission_set_arn}),
    )
    .await;
    assert_refused("the deleted set", "DescribePermissionSet", NOT_FOUND, status, &body);

    // ...so no status reader resolves a handle that reports work on it.
    for (label, target, member, handle) in &readers {
        let expected = if *target == "DescribePermissionSetProvisioningStatus" {
            state::permission_set_provisioning_not_found_message(handle)
        } else {
            state::assignment_operation_not_found_message(handle)
        };
        let (status, body) = call(
            &router,
            target,
            &json!({"InstanceArn": instance_arn, (*member): handle}),
        )
        .await;
        assert_refused(label, target, NOT_FOUND, status, &body);
        assert_eq!(
            body["Message"].as_str().unwrap_or_default(),
            expected,
            "case {label:?}: expected the message {expected:?}, got {body}"
        );
    }

    // And the withdrawn deletion handle is answered exactly as one nothing was minted
    // under: a handle whose permission set no reader can see is as absent as that one.
    let (unminted_status, unminted_body) =
        describe(&router, &instance_arn, UNMINTED_REQUEST_ID).await;
    assert_refused(
        "an unminted request id",
        "DescribeAccountAssignmentDeletionStatus",
        NOT_FOUND,
        unminted_status,
        &unminted_body,
    );
    assert_eq!(
        unminted_body["Message"].as_str().unwrap_or_default(),
        state::assignment_operation_not_found_message(UNMINTED_REQUEST_ID),
        "an unminted request id must carry the history's own message, got {unminted_body}"
    );
    let (withdrawn_status, withdrawn_body) =
        describe(&router, &instance_arn, &deletion_id).await;
    assert_eq!(
        withdrawn_status, unminted_status,
        "a withdrawn handle must earn the same HTTP status as an unminted one, expected \
         {unminted_status}, got {withdrawn_status} {withdrawn_body}"
    );
    assert_eq!(
        withdrawn_body["__type"], unminted_body["__type"],
        "a withdrawn handle must earn the same error code as an unminted one, expected {:?}, \
         got {:?}",
        unminted_body["__type"], withdrawn_body["__type"]
    );
}
