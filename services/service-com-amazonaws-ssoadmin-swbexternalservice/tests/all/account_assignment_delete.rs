// SPDX-License-Identifier: CC0-1.0

//! `DeleteAccountAssignment` and the deletion status it tracks, end to end over the
//! `awsJson1_1` wire.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than over one
//! example:
//!
//! 1. a delete withdraws exactly the access its (instance, account, permission set,
//!    principal type, principal id) tuple names — `ListAccountAssignments` stops
//!    reporting that principal and reports every other row unchanged — and answers with
//!    the operation's TRACKING record: the non-terminal `IN_PROGRESS` of the model's
//!    `StatusValues`, the `RequestId` to poll it by and the date it was accepted, and
//!    none of the detail members, exactly as `CreateAccountAssignment` answers the
//!    operation it starts;
//! 2. the deletion reaches the terminal `SUCCEEDED` only once the clock makes its
//!    completion due, one `state::ASSIGNMENT_OPERATION_SPAN` after the call that started
//!    it, so a poller observes the transition;
//! 3. the poll AWS documents (`DeleteAccountAssignment`: "After a successful response,
//!    call DescribeAccountAssignmentDeletionStatus to describe the status of an assignment
//!    deletion request") REPORTS the stored record of the operation its `RequestId` names,
//!    within the instance the request names, with every detail member of the access that
//!    was withdrawn — and it is a read: repeating it answers identically and changes
//!    nothing;
//! 4. a delete is not idempotent on an access that is not granted: a tuple that was
//!    never granted, one already deleted, an unknown `InstanceArn` and an unknown
//!    `PermissionSetArn` are each refused `ResourceNotFoundException`, never answered
//!    200, and change nothing a later caller sees;
//! 5. a `RequestId` no deletion of this instance ran under names no deletion status —
//!    including the `RequestId` of a CREATE, which is another operation's history;
//! 6. a delete really removes the assignment, so the same tuple can be granted again
//!    afterwards and is then a new operation under a new `RequestId`.
//!
//! The rows differ along what the rules generalise over: both `PrincipalType` enum
//! values, both forms the model's `PrincipalId` pattern admits (a bare GUID, an
//! upper-case-hex GUID and the `[0-9a-f]{10}-` prefixed form at its 47-character upper
//! length bound), several accounts including both ends of `TargetId`'s `^\d{12}$`, and
//! two permission sets so a delete under one cannot be mistaken for a delete under the
//! other. The status is read on BOTH sides of the due instant, which the test moves in
//! the store rather than sleeping for; the transition is still derived by the service,
//! exactly as in production.
//!
//! Every call goes through `MultiServiceRouter::dispatch`, the one path a socket-served
//! request takes. The harness is the one `ssoadmin_permission_sets.rs` owns.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ssoadmin_permission_sets::{
    call, error_code, is_uuid, organization_engine, organization_router, router_over,
    with_store, UNKNOWN_INSTANCE_ARN,
};

/// One assignment to grant and then delete.
#[derive(Debug, Clone, Copy)]
struct Case {
    label: &'static str,
    /// `TargetId`: modelled `^\d{12}$`, length 12..12.
    account_id: &'static str,
    /// `PrincipalType`: the model's enum is `USER | GROUP`.
    principal_type: &'static str,
    /// `PrincipalId`: modelled
    /// `^([0-9a-f]{10}-|)[A-Fa-f0-9]{8}-[A-Fa-f0-9]{4}-[A-Fa-f0-9]{4}-[A-Fa-f0-9]{4}-[A-Fa-f0-9]{12}$`.
    principal_id: &'static str,
}

/// The table the rules are asserted over.
const CASES: &[Case] = &[
    Case {
        label: "a user, bare GUID",
        account_id: "123456789012",
        principal_type: "USER",
        principal_id: "f81d4fae-7dec-11d0-a765-00a0c91e6bf6",
    },
    Case {
        label: "a group on the same account",
        account_id: "123456789012",
        principal_type: "GROUP",
        principal_id: "1b2d3c4f-5a6b-7c8d-9e0f-1a2b3c4d5e6f",
    },
    Case {
        label: "a user with the store-prefixed principal id",
        account_id: "123456789012",
        principal_type: "USER",
        principal_id: "9067f3a1c2-f81d4fae-7dec-11d0-a765-00a0c91e6bf7",
    },
    Case {
        label: "the lowest 12-digit account, upper-case hex id",
        account_id: "000000000000",
        principal_type: "USER",
        principal_id: "F81D4FAE-7DEC-11D0-A765-00A0C91E6BF6",
    },
    Case {
        label: "the highest 12-digit account",
        account_id: "999999999999",
        principal_type: "GROUP",
        principal_id: "1b2d3c4f-5a6b-7c8d-9e0f-1a2b3c4d5e6f",
    },
];

/// The detail members of `AccountAssignmentOperationStatus` the status DESCRIBE reports
/// and the tracking view of the call that starts the operation does not. The names are
/// the model's
/// (`smithy_query(query="shape", shape="AccountAssignmentOperationStatus")`).
const DETAIL_MEMBERS: [&str; 5] = [
    "TargetId",
    "TargetType",
    "PermissionSetArn",
    "PrincipalType",
    "PrincipalId",
];

/// Move the stored completion instant of every assignment DELETION to `due_at`, so the
/// status the clock owes can be observed on both sides of it without the test sleeping.
/// The status itself is still derived by the service, exactly as in production.
fn hold_completion(engine: &Arc<engine::InMemoryEngine>, due_at: SystemTime) {
    with_store(engine, |store| {
        for record in store.assignment_deletions.iter_mut() {
            record.succeeded_due = due_at;
        }
    });
}

/// A pattern-valid `PermissionSetArn` under `instance_arn` that no create ever minted.
///
/// Not the harness's `UNKNOWN_PERMISSION_SET_ARN`, which is pinned to the unknown
/// instance: this one must sit under a LIVE instance so the permission-set lookup, and
/// not the instance lookup, is the one that fails.
fn unknown_permission_set_arn(instance_arn: &str) -> String {
    let instance_id = instance_arn.rsplit('/').next().unwrap_or_default();
    format!("arn:aws:sso:::permissionSet/{instance_id}/ps-0000000000000000")
}

/// Create a permission set on `instance_arn` and return its minted ARN.
async fn permission_set(router: &MultiServiceRouter, instance_arn: &str, name: &str) -> String {
    let (status, body) = call(
        router,
        "CreatePermissionSet",
        &json!({"InstanceArn": instance_arn, "Name": name}),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreatePermissionSet {name}: expected 200, got {status} {body}"
    );
    body["PermissionSet"]["PermissionSetArn"]
        .as_str()
        .unwrap_or_else(|| panic!("CreatePermissionSet {name} minted no ARN: {body}"))
        .to_string()
}

/// The request body both `CreateAccountAssignment` and `DeleteAccountAssignment` take for
/// one row.
fn assignment_body(instance_arn: &str, permission_set_arn: &str, case: &Case) -> Value {
    json!({
        "InstanceArn": instance_arn,
        "TargetId": case.account_id,
        "TargetType": "AWS_ACCOUNT",
        "PermissionSetArn": permission_set_arn,
        "PrincipalType": case.principal_type,
        "PrincipalId": case.principal_id,
    })
}

/// Grant one row's assignment and return the `RequestId` of the create.
async fn grant(
    router: &MultiServiceRouter,
    instance_arn: &str,
    permission_set_arn: &str,
    case: &Case,
) -> String {
    let (status, body) = call(
        router,
        "CreateAccountAssignment",
        &assignment_body(instance_arn, permission_set_arn, case),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {:?}: CreateAccountAssignment expected 200, got {status} {body}",
        case.label
    );
    body["AccountAssignmentCreationStatus"]["RequestId"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("case {:?}: the create minted no RequestId: {body}", case.label)
        })
        .to_string()
}

/// `DescribeAccountAssignmentDeletionStatus` for one request id.
async fn describe(
    router: &MultiServiceRouter,
    instance_arn: &str,
    request_id: &str,
) -> (u16, Value) {
    call(
        router,
        "DescribeAccountAssignmentDeletionStatus",
        &json!({
            "InstanceArn": instance_arn,
            "AccountAssignmentDeletionRequestId": request_id,
        }),
    )
    .await
}

/// The (principal type, principal id) pairs `ListAccountAssignments` reports under one
/// (account, permission set) filter, sorted so two readings compare.
async fn assigned(
    router: &MultiServiceRouter,
    instance_arn: &str,
    account_id: &str,
    permission_set_arn: &str,
) -> Vec<(String, String)> {
    let (status, body) = call(
        router,
        "ListAccountAssignments",
        &json!({
            "InstanceArn": instance_arn,
            "AccountId": account_id,
            "PermissionSetArn": permission_set_arn,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "ListAccountAssignments {account_id}: expected 200, got {status} {body}"
    );
    let mut rows: Vec<(String, String)> = body["AccountAssignments"]
        .as_array()
        .unwrap_or_else(|| panic!("ListAccountAssignments {account_id} returned no list: {body}"))
        .iter()
        .map(|row| {
            (
                row["PrincipalType"].as_str().unwrap_or_default().to_string(),
                row["PrincipalId"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    rows.sort();
    rows
}

/// RULES 1, 2, 3 and 6 — a delete withdraws the named access, tracks the deletion as an
/// asynchronous operation whose status the describe reports on both sides of its due
/// instant, and leaves the tuple grantable again.
#[tokio::test]
async fn a_delete_withdraws_the_named_access_and_tracks_its_status() {
    let (engine, instance_arn) = organization_engine();
    let router = router_over(Arc::clone(&engine));
    let target = permission_set(&router, &instance_arn, "Deleted").await;
    // A second permission set carrying the SAME assignments: a delete under one must not
    // touch the other, which is what makes the permission set part of the identity.
    let bystander = permission_set(&router, &instance_arn, "Kept").await;

    let mut creation_request_ids = Vec::new();
    for case in CASES {
        creation_request_ids.push(grant(&router, &instance_arn, &target, case).await);
        grant(&router, &instance_arn, &bystander, case).await;
    }

    let mut deletion_request_ids: Vec<String> = Vec::new();
    for (index, case) in CASES.iter().enumerate() {
        let before = assigned(&router, &instance_arn, case.account_id, &target).await;
        let bystander_before = assigned(&router, &instance_arn, case.account_id, &bystander).await;

        let (status, body) = call(
            &router,
            "DeleteAccountAssignment",
            &assignment_body(&instance_arn, &target, case),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: DeleteAccountAssignment expected 200, got {status} {body}",
            case.label
        );
        let tracking = body["AccountAssignmentDeletionStatus"].clone();
        let request_id = tracking["RequestId"].as_str().unwrap_or_default().to_string();
        assert!(
            is_uuid(&request_id),
            "case {:?}: expected a UUID RequestId, got {request_id:?}",
            case.label
        );
        assert!(
            !creation_request_ids.contains(&request_id),
            "case {:?}: the deletion must mint its own RequestId, got the create's \
             {request_id:?}",
            case.label
        );
        assert!(
            !deletion_request_ids.contains(&request_id),
            "case {:?}: every deletion mints a distinct RequestId, got {request_id:?} again",
            case.label
        );
        // RULE 1 — the TRACKING record: the in-flight status, the handle and the date,
        // and none of the detail members.
        assert_eq!(
            tracking["Status"].as_str(),
            Some("IN_PROGRESS"),
            "case {:?}: expected the accepted deletion's in-flight status, got {:?}",
            case.label,
            tracking["Status"]
        );
        assert!(
            tracking["CreatedDate"].is_number(),
            "case {:?}: expected a server-set CreatedDate, got {:?}",
            case.label,
            tracking["CreatedDate"]
        );
        for member in DETAIL_MEMBERS {
            assert_eq!(
                tracking[member],
                Value::Null,
                "case {:?}: the tracking view carries no {member}, got {:?}",
                case.label,
                tracking[member]
            );
        }
        assert_eq!(
            tracking["FailureReason"],
            Value::Null,
            "case {:?}: nothing failed, so no FailureReason, got {:?}",
            case.label,
            tracking["FailureReason"]
        );

        // RULE 1 — exactly the named row is gone, under exactly the named permission set,
        // as soon as the deletion is accepted.
        let expected: Vec<(String, String)> = before
            .iter()
            .filter(|(principal_type, principal_id)| {
                !(principal_type == case.principal_type && principal_id == case.principal_id)
            })
            .cloned()
            .collect();
        let after = assigned(&router, &instance_arn, case.account_id, &target).await;
        assert_eq!(
            after, expected,
            "case {:?}: expected the remaining assignments {expected:?}, got {after:?}",
            case.label
        );
        let bystander_after = assigned(&router, &instance_arn, case.account_id, &bystander).await;
        assert_eq!(
            bystander_after, bystander_before,
            "case {:?}: the other permission set's assignments must be untouched, \
             expected {bystander_before:?}, got {bystander_after:?}",
            case.label
        );

        // RULE 3 — the describe reports the stored record, detail members and all, and
        // RULE 2 — it is still `IN_PROGRESS` before the completion the clock owes.
        let (status, in_flight) = describe(&router, &instance_arn, &request_id).await;
        assert_eq!(
            status, 200,
            "case {:?}: DescribeAccountAssignmentDeletionStatus expected 200, got \
             {status} {in_flight}",
            case.label
        );
        let reported = in_flight["AccountAssignmentDeletionStatus"].clone();
        assert_eq!(
            reported["Status"].as_str(),
            Some("IN_PROGRESS"),
            "case {:?}: the deletion is in flight until its completion is due, got {:?}",
            case.label,
            reported["Status"]
        );
        assert_eq!(
            reported["RequestId"].as_str(),
            Some(request_id.as_str()),
            "case {:?}: expected the polled RequestId {request_id:?}, got {:?}",
            case.label,
            reported["RequestId"]
        );
        assert_eq!(
            reported["CreatedDate"], tracking["CreatedDate"],
            "case {:?}: expected the accepted date {:?}, got {:?}",
            case.label,
            tracking["CreatedDate"],
            reported["CreatedDate"]
        );
        // Every detail member describes the access that was WITHDRAWN.
        for (member, expected) in [
            ("TargetId", case.account_id),
            ("TargetType", "AWS_ACCOUNT"),
            ("PermissionSetArn", target.as_str()),
            ("PrincipalType", case.principal_type),
            ("PrincipalId", case.principal_id),
        ] {
            assert_eq!(
                reported[member].as_str(),
                Some(expected),
                "case {:?}: expected {member} {expected:?}, got {:?}",
                case.label,
                reported[member]
            );
        }
        // A repeated poll is a REPORT: it answers identically and moves nothing.
        let (status, again) = describe(&router, &instance_arn, &request_id).await;
        assert_eq!(
            status, 200,
            "case {:?}: the second poll expected 200, got {status} {again}",
            case.label
        );
        assert_eq!(
            again["AccountAssignmentDeletionStatus"], reported,
            "case {:?}: a poll must report the same record, got {:?}",
            case.label, again["AccountAssignmentDeletionStatus"]
        );

        // RULE 2 — once the clock makes the completion due, and only then, the status is
        // the terminal `SUCCEEDED`; every other member is the one that was stored.
        hold_completion(&engine, SystemTime::now() - Duration::from_millis(1));
        let (status, settled) = describe(&router, &instance_arn, &request_id).await;
        assert_eq!(
            status, 200,
            "case {:?}: the poll after the due instant expected 200, got {status} {settled}",
            case.label
        );
        let settled = settled["AccountAssignmentDeletionStatus"].clone();
        assert_eq!(
            settled["Status"].as_str(),
            Some("SUCCEEDED"),
            "case {:?}: the deletion must reach its terminal status, got {:?}",
            case.label,
            settled["Status"]
        );
        for member in DETAIL_MEMBERS.iter().chain(["RequestId", "CreatedDate"].iter()) {
            assert_eq!(
                settled[*member], reported[*member],
                "case {:?}: {member} must not move with the status, expected {:?}, got {:?}",
                case.label, reported[*member], settled[*member]
            );
        }

        // RULE 4 — the same delete again has nothing to withdraw.
        let (status, repeat) = call(
            &router,
            "DeleteAccountAssignment",
            &assignment_body(&instance_arn, &target, case),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {:?}: a second delete of the same tuple must be refused, got {status} \
             {repeat}",
            case.label
        );
        assert_eq!(
            error_code(&repeat),
            "ResourceNotFoundException",
            "case {:?}: expected ResourceNotFoundException, got {repeat}",
            case.label
        );

        // RULE 6 — the access really is gone, so granting it again is a NEW operation
        // under a new creation `RequestId`, not the record the first create wrote.
        let regranted = grant(&router, &instance_arn, &target, case).await;
        assert_ne!(
            regranted, creation_request_ids[index],
            "case {:?}: re-granting after a delete must mint a new RequestId, got the \
             removed assignment's {regranted:?}",
            case.label
        );
        let after_regrant = assigned(&router, &instance_arn, case.account_id, &target).await;
        assert_eq!(
            after_regrant, before,
            "case {:?}: re-granting must restore the assignment set {before:?}, got \
             {after_regrant:?}",
            case.label
        );
        // Leave the row deleted again, so the next iteration starts from one known state.
        let (status, body) = call(
            &router,
            "DeleteAccountAssignment",
            &assignment_body(&instance_arn, &target, case),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: deleting the re-granted assignment expected 200, got {status} {body}",
            case.label
        );
        deletion_request_ids.push(request_id);
        deletion_request_ids.push(
            body["AccountAssignmentDeletionStatus"]["RequestId"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        );
    }
}

/// RULE 4 — every delete whose target access is absent is refused
/// `ResourceNotFoundException` and writes nothing.
#[tokio::test]
async fn a_delete_of_an_access_that_is_not_granted_is_refused() {
    let (router, instance_arn) = organization_router();
    let granted_set = permission_set(&router, &instance_arn, "Granted").await;
    let empty_set = permission_set(&router, &instance_arn, "Empty").await;
    let case = CASES[0];
    grant(&router, &instance_arn, &granted_set, &case).await;
    let before = assigned(&router, &instance_arn, case.account_id, &granted_set).await;

    // Each row changes ONE part of the identity the granted assignment carries, plus the
    // two resources the call resolves before it.
    let refused: Vec<(&str, Value)> = vec![
        (
            "an InstanceArn no instance is held under",
            assignment_body(UNKNOWN_INSTANCE_ARN, &granted_set, &case),
        ),
        (
            "a PermissionSetArn no create minted",
            assignment_body(
                &instance_arn,
                &unknown_permission_set_arn(&instance_arn),
                &case,
            ),
        ),
        (
            "a permission set that holds no assignment",
            assignment_body(&instance_arn, &empty_set, &case),
        ),
        (
            "another account",
            assignment_body(
                &instance_arn,
                &granted_set,
                &Case {
                    account_id: "210987654321",
                    ..case
                },
            ),
        ),
        (
            "the other principal type",
            assignment_body(
                &instance_arn,
                &granted_set,
                &Case {
                    principal_type: "GROUP",
                    ..case
                },
            ),
        ),
        (
            "another principal id",
            assignment_body(
                &instance_arn,
                &granted_set,
                &Case {
                    principal_id: "1b2d3c4f-5a6b-7c8d-9e0f-1a2b3c4d5e6f",
                    ..case
                },
            ),
        ),
    ];
    for (label, body) in refused {
        let (status, answer) = call(&router, "DeleteAccountAssignment", &body).await;
        assert_eq!(
            status, 400,
            "case {label:?}: an absent access must be refused, got {status} {answer}"
        );
        assert_eq!(
            error_code(&answer),
            "ResourceNotFoundException",
            "case {label:?}: expected ResourceNotFoundException, got {answer}"
        );
    }

    let after = assigned(&router, &instance_arn, case.account_id, &granted_set).await;
    assert_eq!(
        after, before,
        "a refused delete must change nothing a later caller sees, expected {before:?}, \
         got {after:?}"
    );
}

/// RULE 5 — a `RequestId` no deletion of this instance ran under names no deletion
/// status, and neither does a CREATE's `RequestId`: the two operations keep their own
/// history.
#[tokio::test]
async fn an_unknown_deletion_request_id_is_refused() {
    let (router, instance_arn) = organization_router();
    let permission_set_arn = permission_set(&router, &instance_arn, "Statuses").await;
    let case = CASES[1];
    let creation_request_id = grant(&router, &instance_arn, &permission_set_arn, &case).await;

    let unknown: Vec<(&str, String)> = vec![
        (
            "the RequestId of the CREATE, which is not a deletion",
            creation_request_id,
        ),
        (
            "an all-zero UUId",
            "00000000-0000-4000-8000-000000000000".to_string(),
        ),
        (
            "an all-f UUId",
            "ffffffff-ffff-4fff-bfff-ffffffffffff".to_string(),
        ),
    ];
    for (label, request_id) in unknown {
        let (status, body) = describe(&router, &instance_arn, &request_id).await;
        assert_eq!(
            status, 400,
            "case {label:?}: an unknown deletion RequestId must be refused, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {label:?}: expected ResourceNotFoundException, got {body}"
        );
    }

    // And the one id that DOES name a deletion resolves, so the refusals above are about
    // the id rather than about the operation.
    let (status, deleted) = call(
        &router,
        "DeleteAccountAssignment",
        &assignment_body(&instance_arn, &permission_set_arn, &case),
    )
    .await;
    assert_eq!(
        status, 200,
        "DeleteAccountAssignment expected 200, got {status} {deleted}"
    );
    let request_id = deleted["AccountAssignmentDeletionStatus"]["RequestId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let (status, read) = describe(&router, &instance_arn, &request_id).await;
    assert_eq!(
        status, 200,
        "the deletion's own RequestId must resolve, got {status} {read}"
    );
    let reported = &read["AccountAssignmentDeletionStatus"];
    assert_eq!(
        reported["RequestId"].as_str(),
        Some(request_id.as_str()),
        "expected the polled RequestId {request_id:?}, got {:?}",
        reported["RequestId"]
    );
    assert_eq!(
        reported["PrincipalId"].as_str(),
        Some(case.principal_id),
        "expected the withdrawn access's PrincipalId {:?}, got {:?}",
        case.principal_id,
        reported["PrincipalId"]
    );

    // An unknown `InstanceArn` is a missing resource for the poll too, not a status.
    let (status, body) = describe(&router, UNKNOWN_INSTANCE_ARN, &request_id).await;
    assert_eq!(
        status, 400,
        "an InstanceArn the account holds no instance under must be refused, got \
         {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "expected ResourceNotFoundException, got {body}"
    );
}
