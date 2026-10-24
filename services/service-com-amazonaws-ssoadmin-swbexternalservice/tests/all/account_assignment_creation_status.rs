// SPDX-License-Identifier: CC0-1.0

//! `DescribeAccountAssignmentCreationStatus`, end to end over the `awsJson1_1` wire.
//!
//! THE RULE: the operation REPORTS the stored status record of the assignment-creation
//! operation its `RequestId` names, within the instance the request names — every member
//! of it as the create that minted that id stored it — and answers
//! `ResourceNotFoundException` for an id that instance tracks no operation under. It is a
//! read: it moves no status, mints no record and changes nothing a later caller sees.
//!
//! The table spans what the rule generalises over: the `PrincipalType` enum values, both
//! forms the `PrincipalId` pattern admits, the published bounds of `TargetId`
//! (`^\d{12}$`), and more than one permission set, so no row's answer can come from a
//! single record the emulator happens to hold. The not-found side spans the ways an id
//! fails to resolve: never minted, minted under another instance, and gone with the
//! permission set it named.
//!
//! Every call goes through `MultiServiceRouter::dispatch`, the one path a socket-served
//! request takes. The harness is the one `ssoadmin_permission_sets.rs` owns.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ssoadmin_permission_sets::{
    call, error_code, organization_engine, router_over, with_store, TEST_REGION,
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

/// The rows the rule is asserted over.
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
        &json!({
            "InstanceArn": instance_arn,
            "TargetId": case.account_id,
            "TargetType": "AWS_ACCOUNT",
            "PermissionSetArn": permission_set_arn,
            "PrincipalType": case.principal_type,
            "PrincipalId": case.principal_id,
        }),
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

/// THE RULE, over the whole table: every created assignment's status is readable back by
/// the `RequestId` its create minted, member for member.
#[tokio::test]
async fn every_creation_status_is_read_back_from_the_stored_record() {
    let (engine, instance_arn) = organization_engine();
    let router = router_over(engine);
    let sets = permission_sets(&router, &instance_arn).await;

    for case in CASES {
        let permission_set_arn = &sets[case.permission_set];
        let created = create_assignment(&router, &instance_arn, permission_set_arn, case).await;
        let request_id = created["RequestId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {:?}: no RequestId in {created}", case.label))
            .to_string();

        let (status, body) = describe(&router, &instance_arn, &request_id).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200 for the minted request id, got {status} {body}",
            case.label
        );
        let reported = &body["AccountAssignmentCreationStatus"];

        // Every member, against the request it was created from and the create's answer.
        assert_eq!(
            reported["RequestId"].as_str(),
            Some(request_id.as_str()),
            "case {:?}: expected RequestId {request_id:?}, got {:?}",
            case.label,
            reported["RequestId"]
        );
        assert_eq!(
            reported["Status"].as_str(),
            Some("SUCCEEDED"),
            "case {:?}: expected the applied assignment's terminal status, got {:?}",
            case.label,
            reported["Status"]
        );
        assert_eq!(
            reported["TargetId"].as_str(),
            Some(case.account_id),
            "case {:?}: expected TargetId {:?}, got {:?}",
            case.label,
            case.account_id,
            reported["TargetId"]
        );
        assert_eq!(
            reported["TargetType"].as_str(),
            Some("AWS_ACCOUNT"),
            "case {:?}: expected TargetType AWS_ACCOUNT, got {:?}",
            case.label,
            reported["TargetType"]
        );
        assert_eq!(
            reported["PermissionSetArn"].as_str(),
            Some(permission_set_arn.as_str()),
            "case {:?}: expected PermissionSetArn {permission_set_arn:?}, got {:?}",
            case.label,
            reported["PermissionSetArn"]
        );
        assert_eq!(
            reported["PrincipalType"].as_str(),
            Some(case.principal_type),
            "case {:?}: expected PrincipalType {:?}, got {:?}",
            case.label,
            case.principal_type,
            reported["PrincipalType"]
        );
        assert_eq!(
            reported["PrincipalId"].as_str(),
            Some(case.principal_id),
            "case {:?}: expected PrincipalId {:?}, got {:?}",
            case.label,
            case.principal_id,
            reported["PrincipalId"]
        );
        assert_eq!(
            reported["CreatedDate"], created["CreatedDate"],
            "case {:?}: expected the stored CreatedDate {:?}, got {:?}",
            case.label,
            created["CreatedDate"],
            reported["CreatedDate"]
        );
        // Nothing failed, so the record carries no failure text.
        assert_eq!(
            reported["FailureReason"],
            Value::Null,
            "case {:?}: a succeeded operation carries no FailureReason, got {:?}",
            case.label,
            reported["FailureReason"]
        );
        // The whole object the create reported, not just the members above.
        assert_eq!(
            reported, &created,
            "case {:?}: expected the create's status object {created}, got {reported}",
            case.label
        );
    }
}

/// The read is REPEATABLE and changes nothing a later caller sees: the same request id
/// answers the same object however often it is asked, the record is not consumed, and the
/// assignments the instance holds are untouched.
#[tokio::test]
async fn describing_a_status_changes_nothing() {
    let (engine, instance_arn) = organization_engine();
    let router = router_over(engine);
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

    let before = list_assignments(&router, &instance_arn, &sets[0]).await;

    // Three reads of every id: a read that moved a status or consumed a record would
    // show up as a difference between them.
    for (label, request_id, created) in &request_ids {
        for attempt in 1..=3 {
            let (status, body) = describe(&router, &instance_arn, request_id).await;
            assert_eq!(
                status, 200,
                "case {label:?} attempt {attempt}: expected 200, got {status} {body}"
            );
            assert_eq!(
                &body["AccountAssignmentCreationStatus"], created,
                "case {label:?} attempt {attempt}: expected {created}, got {body}"
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

/// The not-found side of the rule, over the ways a request id fails to resolve.
#[tokio::test]
async fn an_unresolvable_request_id_is_a_missing_resource() {
    use service_com_amazonaws_ssoadmin_swbexternalservice::state;

    let (engine, instance_arn) = organization_engine();
    let router = router_over(engine.clone());
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
            aws_smithy_types::DateTime::from(std::time::SystemTime::now()),
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

/// A status record belongs to the permission set its operation named: deleting that
/// permission set takes its assignments AND their status records with it, so the request
/// id stops resolving — the read reports state rather than a history of its own.
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
