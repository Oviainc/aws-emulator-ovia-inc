// SPDX-License-Identifier: CC0-1.0

//! Creating an account assignment is an ASYNCHRONOUS operation, and
//! `DescribeAccountAssignmentCreationStatus` is the poll that reports it — end to end
//! over the `awsJson1_1` wire.
//!
//! The rules under test:
//!
//! 1. the call that STARTS such an operation answers with the operation's TRACKING
//!    record — the non-terminal `IN_PROGRESS` of the model's `StatusValues`
//!    (`IN_PROGRESS | FAILED | SUCCEEDED`), the `RequestId` to poll it by and the date it
//!    was accepted, and none of the detail members, which are exactly the three the
//!    model's own `AccountAssignmentOperationStatusMetadata` carries;
//! 2. the operation reaches the terminal `SUCCEEDED` only once the clock makes its
//!    completion due, one `state::ASSIGNMENT_OPERATION_SPAN` after the call that started
//!    it, so a poller observes the transition;
//! 3. the poll AWS documents (`CreateAccountAssignment`: "After a successful response,
//!    call DescribeAccountAssignmentCreationStatus to describe the status of an assignment
//!    creation request") REPORTS the stored record of the operation its `RequestId` names,
//!    within the instance the request names — every member of it as the create that
//!    minted that id stored it — and it is a read: it moves no status, mints no record and
//!    changes nothing a later caller sees;
//! 4. a `RequestId` the instance tracks no operation under — never minted, minted under
//!    another instance, or gone with the permission set it named — is
//!    `ResourceNotFoundException`, and so is an `InstanceArn` the account holds no
//!    instance under.
//!
//! The table spans what the rules generalise over: the `PrincipalType` enum values, both
//! forms the `PrincipalId` pattern admits, the published bounds of `TargetId`
//! (`^\d{12}$`), and more than one permission set, so no row's answer can come from a
//! single record the emulator happens to hold. The status itself is read on BOTH sides of
//! the due instant, which the test moves in the store rather than sleeping for; the
//! transition is still derived by the service, exactly as in production.
//!
//! Every call goes through `MultiServiceRouter::dispatch`, the one path a socket-served
//! request takes. The harness is the one `ssoadmin_permission_sets.rs` owns.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_ssoadmin_swbexternalservice::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::ssoadmin_permission_sets::{
    call, error_code, is_uuid, organization_engine, router_over, with_store, TEST_REGION,
    UNKNOWN_INSTANCE_ARN,
};

/// One row of the table: a `CreateAccountAssignment` input, plus which of the two
/// permission sets it is made on.
struct Case {
    label: &'static str,
    /// `TargetId`: modelled `^\d{12}$`, length 12..12.
    account_id: &'static str,
    /// `PrincipalType`: the model's enum is `USER | GROUP`.
    principal_type: &'static str,
    /// `PrincipalId`: modelled
    /// `^([0-9a-f]{10}-|)[A-Fa-f0-9]{8}-[A-Fa-f0-9]{4}-[A-Fa-f0-9]{4}-[A-Fa-f0-9]{4}-[A-Fa-f0-9]{12}$`,
    /// so both the bare GUID and the identity-store-prefixed form are admitted.
    principal_id: &'static str,
    /// Which permission set of [`PERMISSION_SET_NAMES`] the assignment is made on.
    permission_set: usize,
}

/// The rows the rules are asserted over.
const CASES: &[Case] = &[
    Case {
        label: "a GROUP principal, bare GUID, the recorded shape of the request",
        account_id: "123456789012",
        principal_type: "GROUP",
        principal_id: "f81d4fae-7dec-11d0-a765-00a0c91e6bf6",
        permission_set: 0,
    },
    Case {
        label: "a USER principal with the identity-store-prefixed id, lowest account id",
        account_id: "000000000000",
        principal_type: "USER",
        principal_id: "9067f3a1c2-f81d4fae-7dec-11d0-a765-00a0c91e6bf7",
        permission_set: 0,
    },
    Case {
        label: "a USER principal with an uppercase-hex id, highest account id, second set",
        account_id: "999999999999",
        principal_type: "USER",
        principal_id: "1B2D3C4F-5A6B-7C8D-9E0F-1A2B3C4D5E6F",
        permission_set: 1,
    },
    Case {
        label: "a GROUP principal on the second permission set",
        account_id: "123456789012",
        principal_type: "GROUP",
        principal_id: "11111111-2222-3333-4444-555555555555",
        permission_set: 1,
    },
];

/// The permission sets the rows above are assigned on (`Name`: `^[\w+=,.@-]+$`, 1..32).
const PERMISSION_SET_NAMES: [&str; 2] = ["StatusReadA", "StatusReadB"];

/// A `UUId` (`length 36..36`, lowercase hex groups) nothing was ever minted for, used to
/// pin the not-found side of the rule.
const UNMINTED_REQUEST_ID: &str = "abcdefab-cdef-4bcd-abcd-efabcdefabcd";

/// The detail members of `AccountAssignmentOperationStatus` that the status DESCRIBE
/// reports and the create's tracking view does not. The names are the model's
/// (`smithy_query(query="shape", shape="AccountAssignmentOperationStatus")`).
const DETAIL_MEMBERS: [&str; 5] = [
    "TargetId",
    "TargetType",
    "PermissionSetArn",
    "PrincipalType",
    "PrincipalId",
];

/// Move the stored completion instant of every assignment operation to `due_at`, so the
/// status the clock owes can be observed on both sides of it without the test sleeping.
/// The status itself is still derived by the service, exactly as in production.
fn hold_completion(engine: &Arc<engine::InMemoryEngine>, due_at: SystemTime) {
    with_store(engine, |store| {
        for record in store.assignment_operations.values_mut() {
            record.succeeded_due = due_at;
        }
    });
}

/// The stored instant at which `request_id`'s operation completes.
fn completion_due(engine: &Arc<engine::InMemoryEngine>, request_id: &str) -> SystemTime {
    with_store(engine, |store| {
        store
            .assignment_operations
            .get(request_id)
            .unwrap_or_else(|| panic!("no stored status record under {request_id:?}"))
            .succeeded_due
    })
}

/// Create the two permission sets of [`PERMISSION_SET_NAMES`] and answer their ARNs.
async fn permission_sets(router: &MultiServiceRouter, instance_arn: &str) -> Vec<String> {
    let mut arns = Vec::new();
    for name in PERMISSION_SET_NAMES {
        let (status, body) = call(
            router,
            "CreatePermissionSet",
            &json!({"InstanceArn": instance_arn, "Name": name}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?}: CreatePermissionSet expected 200, got {status} {body}"
        );
        arns.push(
            body["PermissionSet"]["PermissionSetArn"]
                .as_str()
                .unwrap_or_else(|| panic!("case {name:?}: no minted PermissionSetArn in {body}"))
                .to_string(),
        );
    }
    arns
}

/// The `CreateAccountAssignment` request one row of the table sends.
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

/// `CreateAccountAssignment` for `case`, answering the `AccountAssignmentCreationStatus`
/// it reported.
async fn create_assignment(
    router: &MultiServiceRouter,
    instance_arn: &str,
    permission_set_arn: &str,
    case: &Case,
) -> Value {
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
    body["AccountAssignmentCreationStatus"].clone()
}

/// `DescribeAccountAssignmentCreationStatus` for one request id.
async fn describe(
    router: &MultiServiceRouter,
    instance_arn: &str,
    request_id: &str,
) -> (u16, Value) {
    call(
        router,
        "DescribeAccountAssignmentCreationStatus",
        &json!({
            "InstanceArn": instance_arn,
            "AccountAssignmentCreationRequestId": request_id,
        }),
    )
    .await
}

/// The `AccountAssignmentCreationStatus` a describe reports, as a 200 body.
async fn describe_ok(
    router: &MultiServiceRouter,
    instance_arn: &str,
    request_id: &str,
    label: &str,
) -> Value {
    let (status, body) = describe(router, instance_arn, request_id).await;
    assert_eq!(
        status, 200,
        "case {label:?}: the status describe expected 200, got {status} {body}"
    );
    body["AccountAssignmentCreationStatus"].clone()
}

/// The assignments `permission_set_arn` holds for the account every row of the table
/// that uses it names, read the way a client reads them.
async fn list_assignments(
    router: &MultiServiceRouter,
    instance_arn: &str,
    permission_set_arn: &str,
) -> Value {
    let (status, body) = call(
        router,
        "ListAccountAssignments",
        &json!({
            "InstanceArn": instance_arn,
            "AccountId": "123456789012",
            "PermissionSetArn": permission_set_arn,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "ListAccountAssignments expected 200, got {status} {body}"
    );
    body["AccountAssignments"].clone()
}

/// RULES 1-3, over the whole table: the create hands back a tracking handle in the
/// non-terminal status, and the documented poll reports the stored record member for
/// member, walking it to the terminal status as the clock makes the completion due.
#[tokio::test]
async fn an_assignment_creation_is_tracked_from_in_progress_to_succeeded() {
    let (engine, instance_arn) = organization_engine();
    let router = router_over(Arc::clone(&engine));
    let sets = permission_sets(&router, &instance_arn).await;

    for case in CASES {
        let label = case.label;
        let permission_set_arn = &sets[case.permission_set];
        let request = assignment_body(&instance_arn, permission_set_arn, case);

        let opened_at = SystemTime::now();
        let accepted = create_assignment(&router, &instance_arn, permission_set_arn, case).await;
        let closed_at = SystemTime::now();

        // RULE 1 — the tracking view: the in-flight status, a UUID handle, the date it was
        // accepted, and nothing else.
        assert_eq!(
            accepted["Status"].as_str(),
            Some("IN_PROGRESS"),
            "case {label:?}: expected the accepted operation's non-terminal status, got {:?}",
            accepted["Status"]
        );
        let request_id = accepted["RequestId"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            is_uuid(&request_id),
            "case {label:?}: expected a UUID RequestId to poll by, got {request_id:?}"
        );
        assert!(
            accepted["CreatedDate"].is_number(),
            "case {label:?}: expected a server-set CreatedDate, got {:?}",
            accepted["CreatedDate"]
        );
        for member in DETAIL_MEMBERS {
            assert!(
                accepted[member].is_null(),
                "case {label:?}: the tracking view carries no {member}, got {:?}",
                accepted[member]
            );
        }
        assert!(
            accepted["FailureReason"].is_null(),
            "case {label:?}: nothing failed, so there is no FailureReason, got {:?}",
            accepted["FailureReason"]
        );

        // RULE 2 — the completion falls due one span after the call that started it.
        let due_at = completion_due(&engine, &request_id);
        assert!(
            due_at >= opened_at + state::ASSIGNMENT_OPERATION_SPAN
                && due_at <= closed_at + state::ASSIGNMENT_OPERATION_SPAN,
            "case {label:?}: the completion must fall due one span after the call, got \
             {due_at:?}"
        );

        // RULE 3 — the poll, with the completion held open: the whole stored record, still
        // in flight, member for member as the create's request named them.
        hold_completion(&engine, SystemTime::now() + Duration::from_secs(3_600));
        let in_flight = describe_ok(&router, &instance_arn, &request_id, label).await;
        assert_eq!(
            in_flight["Status"].as_str(),
            Some("IN_PROGRESS"),
            "case {label:?}: an operation still in flight reports IN_PROGRESS, got {:?}",
            in_flight["Status"]
        );
        assert_eq!(
            in_flight["RequestId"].as_str(),
            Some(request_id.as_str()),
            "case {label:?}: expected the polled RequestId {request_id:?}, got {:?}",
            in_flight["RequestId"]
        );
        assert_eq!(
            in_flight["CreatedDate"], accepted["CreatedDate"],
            "case {label:?}: expected the accepted CreatedDate {:?}, got {:?}",
            accepted["CreatedDate"], in_flight["CreatedDate"]
        );
        for member in DETAIL_MEMBERS {
            assert_eq!(
                in_flight[member], request[member],
                "case {label:?}: expected {member} {:?} from the stored record, got {:?}",
                request[member], in_flight[member]
            );
        }
        assert!(
            in_flight["FailureReason"].is_null(),
            "case {label:?}: nothing failed, so there is no FailureReason, got {:?}",
            in_flight["FailureReason"]
        );

        // RULE 2 again — past the due instant the clock owes it the terminal status, and
        // the detail does not move with it.
        hold_completion(&engine, SystemTime::now() - Duration::from_secs(1));
        let completed = describe_ok(&router, &instance_arn, &request_id, label).await;
        assert_eq!(
            completed["Status"].as_str(),
            Some("SUCCEEDED"),
            "case {label:?}: once the completion is due the status is SUCCEEDED, got {:?}",
            completed["Status"]
        );
        for member in DETAIL_MEMBERS {
            assert_eq!(
                completed[member], in_flight[member],
                "case {label:?}: {member} does not change when the status does, got {:?}",
                completed[member]
            );
        }

        // A repeat of the create grants the assignment once, so it answers with the SAME
        // operation's tracking view — reporting the status that operation is in now.
        let repeated = create_assignment(&router, &instance_arn, permission_set_arn, case).await;
        assert_eq!(
            repeated["RequestId"].as_str(),
            Some(request_id.as_str()),
            "case {label:?}: expected the stored RequestId {request_id:?}, got {:?}",
            repeated["RequestId"]
        );
        assert_eq!(
            repeated["Status"].as_str(),
            Some("SUCCEEDED"),
            "case {label:?}: the repeat reports the status that operation reached, got {:?}",
            repeated["Status"]
        );
        for member in DETAIL_MEMBERS {
            assert!(
                repeated[member].is_null(),
                "case {label:?}: the tracking view carries no {member}, got {:?}",
                repeated[member]
            );
        }
    }
}

/// RULE 3 — the read is REPEATABLE and changes nothing a later caller sees: with the
/// clock held still the same request id answers the same object however often it is
/// asked, the record is not consumed, and the assignments the instance holds are
/// untouched.
#[tokio::test]
async fn describing_a_status_changes_nothing() {
    let (engine, instance_arn) = organization_engine();
    let router = router_over(Arc::clone(&engine));
    let sets = permission_sets(&router, &instance_arn).await;

    let mut request_ids = Vec::new();
    for case in CASES {
        let created =
            create_assignment(&router, &instance_arn, &sets[case.permission_set], case).await;
        request_ids.push((
            case.label,
            created["RequestId"].as_str().unwrap_or_default().to_string(),
            created,
        ));
    }

    // The clock is the only thing that may move a status, so it is held still here: any
    // difference between the reads below would then be the READ's doing.
    hold_completion(&engine, SystemTime::now() + Duration::from_secs(3_600));
    let before = list_assignments(&router, &instance_arn, &sets[0]).await;

    // Three reads of every id: a read that moved a status or consumed a record would
    // show up as a difference between them.
    for (label, request_id, created) in &request_ids {
        let first = describe_ok(&router, &instance_arn, request_id, label).await;
        // The create's tracking view and the describe's full record agree on every member
        // the tracking view carries.
        for member in ["Status", "RequestId", "CreatedDate"] {
            assert_eq!(
                first[member], created[member],
                "case {label:?}: expected {member} {:?} from the create, got {:?}",
                created[member], first[member]
            );
        }
        for attempt in 2..=3 {
            let again = describe_ok(&router, &instance_arn, request_id, label).await;
            assert_eq!(
                again, first,
                "case {label:?} attempt {attempt}: expected {first}, got {again}"
            );
        }
    }

    let after = list_assignments(&router, &instance_arn, &sets[0]).await;
    assert_eq!(
        after, before,
        "describing a status must not change the assignments the instance holds: \
         expected {before}, got {after}"
    );
}

/// RULE 4 — the not-found side of the rule, over the ways a request id fails to resolve.
#[tokio::test]
async fn an_unresolvable_request_id_is_a_missing_resource() {
    let (engine, instance_arn) = organization_engine();
    let router = router_over(Arc::clone(&engine));
    let sets = permission_sets(&router, &instance_arn).await;
    let case = &CASES[0];
    let created = create_assignment(&router, &instance_arn, &sets[0], case).await;
    let request_id = created["RequestId"].as_str().unwrap_or_default().to_string();

    // A SECOND organization instance, seeded through the same constructor the handlers
    // use (an organization instance is enabled through AWS Organizations, not through
    // this API), so the scoping of a request id to its instance can be asserted.
    let other_instance_arn = with_store(&engine, |store| {
        let instance = state::Instance::active(
            "ssoins-9f8e7d6c5b4a3210",
            "d-9f8e7d6c5b",
            state::InstanceType::Organization,
            Some("Other".to_string()),
            aws_smithy_types::DateTime::from(SystemTime::now()),
            TEST_REGION,
        );
        let arn = instance.instance_arn.clone();
        store.instances.insert(arn.clone(), instance);
        arn
    });

    // (case, instance arn, request id)
    let rows: [(&str, &str, &str); 3] = [
        (
            "a request id nothing was ever minted for",
            instance_arn.as_str(),
            UNMINTED_REQUEST_ID,
        ),
        (
            "a minted request id under an instance that does not exist",
            UNKNOWN_INSTANCE_ARN,
            request_id.as_str(),
        ),
        (
            "a request id minted under another instance",
            other_instance_arn.as_str(),
            request_id.as_str(),
        ),
    ];
    for (label, instance, id) in rows {
        let (status, body) = describe(&router, instance, id).await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected a refusal, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {label:?}: expected ResourceNotFoundException, got {body}"
        );
    }

    // And the id that DOES resolve still resolves: the rows above refuse nothing else.
    let (status, body) = describe(&router, &instance_arn, &request_id).await;
    assert_eq!(
        status, 200,
        "the minted request id must still resolve, got {status} {body}"
    );
}

/// RULE 4 — a status record belongs to the permission set its operation named: deleting
/// that permission set takes its assignments AND their status records with it, so the
/// request id stops resolving — the read reports state rather than a history of its own.
///
/// This pins the STORE's rule (`ComAmazonawsSsoadminSwbexternalserviceState::
/// assignment_operations`, `DeletePermissionSet`): the records live exactly as long as the
/// permission set they name. Should the emulator later keep operation statuses for a
/// window after their permission set is gone, this expectation moves with that change
/// rather than being a regression of the read.
#[tokio::test]
async fn a_status_goes_with_the_permission_set_its_operation_named() {
    let (engine, instance_arn) = organization_engine();
    let router = router_over(engine);
    let sets = permission_sets(&router, &instance_arn).await;

    let mut ids = Vec::new();
    for case in CASES {
        let created =
            create_assignment(&router, &instance_arn, &sets[case.permission_set], case).await;
        ids.push((
            case.label,
            case.permission_set,
            created["RequestId"].as_str().unwrap_or_default().to_string(),
        ));
    }

    let (status, body) = call(
        &router,
        "DeletePermissionSet",
        &json!({"InstanceArn": instance_arn, "PermissionSetArn": sets[0]}),
    )
    .await;
    assert_eq!(
        status, 200,
        "DeletePermissionSet expected 200, got {status} {body}"
    );

    for (label, permission_set, request_id) in &ids {
        let (status, body) = describe(&router, &instance_arn, request_id).await;
        let expected = if *permission_set == 0 { 400 } else { 200 };
        assert_eq!(
            status, expected,
            "case {label:?}: expected HTTP {expected} after the delete, got {status} {body}"
        );
        if expected == 400 {
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {label:?}: expected ResourceNotFoundException, got {body}"
            );
        }
    }
}
