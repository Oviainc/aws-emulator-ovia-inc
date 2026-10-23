// SPDX-License-Identifier: CC0-1.0

//! The ACCOUNT-INSTANCE restriction, end to end over the `awsJson1_1` wire.
//!
//! The rule under test: permission sets — and the account assignments built from them —
//! are a feature of the ORGANIZATION instance of IAM Identity Center, so on an ACCOUNT
//! instance (the instance of a single standalone account, which is what `CreateInstance`
//! creates) every operation of the permission-set / account-assignment surface is refused
//! with the modelled `ValidationException` (HTTP 400), BEFORE any resource-existence
//! check; on an organization instance the same requests are served, and an `InstanceArn`
//! the account holds no instance under remains a missing resource rather than this
//! refusal.
//!
//! The table spans what the rule generalises over: every operation of the surface, and —
//! since the refusal depends on the INSTANCE and on nothing else — rows that vary the
//! permission set named, the account, the principal type and id, the page size and the
//! request id. The recorded case (one `CreateAccountAssignment` with a `GROUP` principal
//! on a fabricated permission-set ARN) is one row of it.
//!
//! Every call goes through `MultiServiceRouter::dispatch`, the one path a socket-served
//! request takes. The harness is the one `ssoadmin_permission_sets.rs` owns.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ssoadmin_permission_sets::{
    call, error_code, organization_router, router, seeded_instance_arn, UNKNOWN_INSTANCE_ARN,
};

/// One call of the withdrawn surface.
struct SurfaceCall {
    label: String,
    operation: &'static str,
    /// What the operation answers for an `InstanceArn` the account holds no instance
    /// under: 404 for the operations that resolve the instance themselves, and 501 for
    /// the three that are still scaffolds — those model the account-instance refusal and
    /// nothing else, which is exactly why an unknown ARN is not refused as one.
    unknown_instance_status: u16,
    body: Value,
}

/// One row of the table: the inputs the refusal must NOT depend on.
struct Variant {
    label: &'static str,
    /// A pattern-valid `PermissionSetArn` under the instance
    /// (`ps-` + 16 of `[a-zA-Z0-9-./]`).
    permission_set_suffix: &'static str,
    account_id: &'static str,
    principal_type: &'static str,
    principal_id: &'static str,
    /// A `UUId` (`length 36..36`, lowercase hex groups), as the status operations take.
    request_id: &'static str,
    /// `MaxResults` for the paginated reads: modelled `range(min: 1, max: 100)`.
    max_results: Option<i64>,
}

/// The rows differ along every axis of the surface's inputs: both `PrincipalType` enum
/// values, both forms the `PrincipalId` pattern admits, the published bounds of
/// `TargetId` (`^\d{12}$`) and of `MaxResults` (1..100), and distinct permission-set
/// ARNs and request ids — none of which can change the answer on an account instance.
const VARIANTS: &[Variant] = &[
    Variant {
        label: "a GROUP principal, the recorded shape of the request",
        permission_set_suffix: "ps-0123456789abcdef",
        account_id: "123456789012",
        principal_type: "GROUP",
        principal_id: "f81d4fae-7dec-11d0-a765-00a0c91e6bf6",
        request_id: "11111111-2222-4333-8444-555555555555",
        max_results: None,
    },
    Variant {
        label: "a USER principal with the store-prefixed id, lowest account id",
        permission_set_suffix: "ps-aaaaaaaaaaaaaaaa",
        account_id: "000000000000",
        principal_type: "USER",
        principal_id: "9067f3a1c2-f81d4fae-7dec-11d0-a765-00a0c91e6bf7",
        request_id: "66666666-7777-4888-9999-000000000000",
        max_results: Some(1),
    },
    Variant {
        label: "a permission-set ARN using the full id charset, highest account id",
        permission_set_suffix: "ps-A-9.b/cDeF012345",
        account_id: "999999999999",
        principal_type: "USER",
        principal_id: "1b2d3c4f-5a6b-7c8d-9e0f-1a2b3c4d5e6f",
        request_id: "abcdefab-cdef-4bcd-abcd-efabcdefabcd",
        max_results: Some(100),
    },
];

/// The instance id an `InstanceArn` carries.
fn instance_id_of(instance_arn: &str) -> &str {
    instance_arn.rsplit('/').next().unwrap_or_default()
}

/// Every operation of the permission-set / account-assignment surface, as the one
/// request each takes for `variant` under `instance_arn`.
fn surface_calls(instance_arn: &str, variant: &Variant) -> Vec<SurfaceCall> {
    let permission_set_arn = format!(
        "arn:aws:sso:::permissionSet/{}/{}",
        instance_id_of(instance_arn),
        variant.permission_set_suffix
    );
    let mut list_permission_sets = json!({"InstanceArn": instance_arn});
    let mut list_account_assignments = json!({
        "InstanceArn": instance_arn,
        "AccountId": variant.account_id,
        "PermissionSetArn": permission_set_arn,
    });
    if let Some(max_results) = variant.max_results {
        list_permission_sets["MaxResults"] = json!(max_results);
        list_account_assignments["MaxResults"] = json!(max_results);
    }
    let assignment = json!({
        "InstanceArn": instance_arn,
        "TargetId": variant.account_id,
        "TargetType": "AWS_ACCOUNT",
        "PermissionSetArn": permission_set_arn,
        "PrincipalType": variant.principal_type,
        "PrincipalId": variant.principal_id,
    });
    vec![
        SurfaceCall {
            label: format!("{}: CreatePermissionSet", variant.label),
            operation: "CreatePermissionSet",
            unknown_instance_status: 404,
            body: json!({"InstanceArn": instance_arn, "Name": "Withdrawn"}),
        },
        SurfaceCall {
            label: format!("{}: DescribePermissionSet", variant.label),
            operation: "DescribePermissionSet",
            unknown_instance_status: 404,
            body: json!({
                "InstanceArn": instance_arn,
                "PermissionSetArn": permission_set_arn,
            }),
        },
        SurfaceCall {
            label: format!("{}: ListPermissionSets", variant.label),
            operation: "ListPermissionSets",
            unknown_instance_status: 404,
            body: list_permission_sets,
        },
        SurfaceCall {
            label: format!("{}: DeletePermissionSet", variant.label),
            operation: "DeletePermissionSet",
            unknown_instance_status: 404,
            body: json!({
                "InstanceArn": instance_arn,
                "PermissionSetArn": permission_set_arn,
            }),
        },
        SurfaceCall {
            label: format!("{}: CreateAccountAssignment", variant.label),
            operation: "CreateAccountAssignment",
            unknown_instance_status: 404,
            body: assignment.clone(),
        },
        SurfaceCall {
            label: format!("{}: DeleteAccountAssignment", variant.label),
            operation: "DeleteAccountAssignment",
            // Still a scaffold beyond this refusal.
            unknown_instance_status: 501,
            body: assignment,
        },
        SurfaceCall {
            label: format!("{}: ListAccountAssignments", variant.label),
            operation: "ListAccountAssignments",
            unknown_instance_status: 404,
            body: list_account_assignments,
        },
        SurfaceCall {
            label: format!("{}: DescribeAccountAssignmentCreationStatus", variant.label),
            operation: "DescribeAccountAssignmentCreationStatus",
            unknown_instance_status: 501,
            body: json!({
                "InstanceArn": instance_arn,
                "AccountAssignmentCreationRequestId": variant.request_id,
            }),
        },
        SurfaceCall {
            label: format!("{}: DescribeAccountAssignmentDeletionStatus", variant.label),
            operation: "DescribeAccountAssignmentDeletionStatus",
            unknown_instance_status: 501,
            body: json!({
                "InstanceArn": instance_arn,
                "AccountAssignmentDeletionRequestId": variant.request_id,
            }),
        },
    ]
}

/// What `DescribeInstance` reports as `PermissionSetsEnabled` for `instance_arn`.
async fn permission_sets_enabled(router: &MultiServiceRouter, instance_arn: &str) -> Value {
    let (status, body) =
        call(router, "DescribeInstance", &json!({"InstanceArn": instance_arn})).await;
    assert_eq!(
        status, 200,
        "DescribeInstance expected 200 for {instance_arn}, got {status} {body}"
    );
    body["PermissionSetsEnabled"].clone()
}

/// THE RULE, over the whole table: on an account instance every operation of the
/// permission-set / account-assignment surface answers 400 `ValidationException`.
#[tokio::test]
async fn the_withdrawn_surface_is_refused_on_an_account_instance() {
    let router = router();
    let instance_arn = seeded_instance_arn(&router).await;
    assert_eq!(
        permission_sets_enabled(&router, &instance_arn).await,
        Value::Bool(false),
        "the account's instance is an account instance, so it reports permission sets \
         disabled"
    );

    for variant in VARIANTS {
        for case in surface_calls(&instance_arn, variant) {
            let (status, body) = call(&router, case.operation, &case.body).await;
            assert_eq!(
                status, 400,
                "case {:?}: expected HTTP 400 on an account instance, got {status} {body}",
                case.label
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {:?}: expected ValidationException, got {body}",
                case.label
            );
            let message = body["message"]
                .as_str()
                .or_else(|| body["Message"].as_str())
                .unwrap_or_default();
            assert!(
                message.contains("account instances"),
                "case {:?}: the refusal must say the operation is unsupported for \
                 account instances, got {message:?}",
                case.label
            );
        }
    }
}

/// The refusal PRECEDES every existence check: on an account instance it is the answer
/// whether or not the permission set, the assignment or the status record named exists —
/// none of them ever can — while an `InstanceArn` the account holds no instance under is
/// still a missing resource, because the restriction belongs to a RESOLVED instance.
#[tokio::test]
async fn the_restriction_precedes_the_existence_checks() {
    let router = router();
    let instance_arn = seeded_instance_arn(&router).await;
    let variant = &VARIANTS[0];

    // Nothing was ever created on this instance, so every resource these calls name is
    // absent — and the answer is the 400 above rather than a 404 that would tell a
    // client "create the permission set and retry".
    for case in surface_calls(&instance_arn, variant) {
        let (status, body) = call(&router, case.operation, &case.body).await;
        assert_eq!(
            status, 400,
            "case {:?}: a missing resource on an account instance is still the \
             restriction, got {status} {body}",
            case.label
        );
    }

    // The same requests against an ARN no instance is held under: the restriction does
    // not apply, so each operation answers as it does for an unknown instance.
    for case in surface_calls(UNKNOWN_INSTANCE_ARN, variant) {
        let (status, body) = call(&router, case.operation, &case.body).await;
        assert_eq!(
            status, case.unknown_instance_status,
            "case {:?}: an unknown InstanceArn expected HTTP {}, got {status} {body}",
            case.label, case.unknown_instance_status
        );
        if case.unknown_instance_status == 404 {
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {:?}: expected ResourceNotFoundException, got {body}",
                case.label
            );
        }
    }
}

/// The same surface on an ORGANIZATION instance: it is SERVED. This is what makes the
/// refusal above a property of the instance type rather than of the operations — and the
/// three operations that are still scaffolds answer their 501 here, not the restriction.
#[tokio::test]
async fn the_same_surface_is_served_on_an_organization_instance() {
    let (router, instance_arn) = organization_router();
    assert_eq!(
        permission_sets_enabled(&router, &instance_arn).await,
        Value::Bool(true),
        "an organization instance serves permission sets, so it reports them enabled"
    );

    let (status, created) = call(
        &router,
        "CreatePermissionSet",
        &json!({"InstanceArn": instance_arn, "Name": "Served"}),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreatePermissionSet on an organization instance expected 200, got {status} {created}"
    );
    let permission_set_arn = created["PermissionSet"]["PermissionSetArn"]
        .as_str()
        .unwrap_or_else(|| panic!("no minted PermissionSetArn in {created}"))
        .to_string();

    // Each served operation of the surface, with the resources it names present.
    let served: Vec<(&str, Value)> = vec![
        (
            "DescribePermissionSet",
            json!({"InstanceArn": instance_arn, "PermissionSetArn": permission_set_arn}),
        ),
        ("ListPermissionSets", json!({"InstanceArn": instance_arn})),
        (
            "CreateAccountAssignment",
            json!({
                "InstanceArn": instance_arn,
                "TargetId": "123456789012",
                "TargetType": "AWS_ACCOUNT",
                "PermissionSetArn": permission_set_arn,
                "PrincipalType": "GROUP",
                "PrincipalId": "f81d4fae-7dec-11d0-a765-00a0c91e6bf6",
            }),
        ),
        (
            "ListAccountAssignments",
            json!({
                "InstanceArn": instance_arn,
                "AccountId": "123456789012",
                "PermissionSetArn": permission_set_arn,
            }),
        ),
        (
            "DeletePermissionSet",
            json!({"InstanceArn": instance_arn, "PermissionSetArn": permission_set_arn}),
        ),
    ];
    for (operation, body) in served {
        let (status, body) = call(&router, operation, &body).await;
        assert_eq!(
            status, 200,
            "case {operation}: an organization instance serves this operation, got \
             {status} {body}"
        );
    }

    // The three that are still scaffolds: unimplemented, which is a 501 — never the
    // account-instance refusal, which would claim this instance cannot serve them.
    let scaffolded: Vec<(&str, Value)> = vec![
        (
            "DeleteAccountAssignment",
            json!({
                "InstanceArn": instance_arn,
                "TargetId": "123456789012",
                "TargetType": "AWS_ACCOUNT",
                "PermissionSetArn": format!(
                    "arn:aws:sso:::permissionSet/{}/ps-0123456789abcdef",
                    instance_id_of(&instance_arn)
                ),
                "PrincipalType": "GROUP",
                "PrincipalId": "f81d4fae-7dec-11d0-a765-00a0c91e6bf6",
            }),
        ),
        (
            "DescribeAccountAssignmentCreationStatus",
            json!({
                "InstanceArn": instance_arn,
                "AccountAssignmentCreationRequestId": "11111111-2222-4333-8444-555555555555",
            }),
        ),
        (
            "DescribeAccountAssignmentDeletionStatus",
            json!({
                "InstanceArn": instance_arn,
                "AccountAssignmentDeletionRequestId": "11111111-2222-4333-8444-555555555555",
            }),
        ),
    ];
    for (operation, body) in scaffolded {
        let (status, body) = call(&router, operation, &body).await;
        assert_eq!(
            status, 501,
            "case {operation}: an unimplemented operation answers 501 on an organization \
             instance, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "NotImplemented",
            "case {operation}: expected NotImplemented, got {body}"
        );
    }
}

/// The refusal is a READ: it writes nothing, so a refused call leaves the account's
/// instance exactly as it was and a later caller sees no trace of it.
#[tokio::test]
async fn a_refused_call_changes_nothing() {
    let router = router();
    let instance_arn = seeded_instance_arn(&router).await;

    let (status, before) = call(&router, "ListInstances", &json!({})).await;
    assert_eq!(status, 200, "ListInstances expected 200, got {status} {before}");

    for variant in VARIANTS {
        for case in surface_calls(&instance_arn, variant) {
            let (status, _) = call(&router, case.operation, &case.body).await;
            assert_eq!(status, 400, "case {:?}: expected the refusal", case.label);
        }
    }

    let (status, after) = call(&router, "ListInstances", &json!({})).await;
    assert_eq!(status, 200, "ListInstances expected 200, got {status} {after}");
    assert_eq!(
        before, after,
        "a refused call must change nothing a later caller sees, got {before} then {after}"
    );
}
