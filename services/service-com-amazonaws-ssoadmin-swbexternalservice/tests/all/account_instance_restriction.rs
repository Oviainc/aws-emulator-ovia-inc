// SPDX-License-Identifier: CC0-1.0

//! The ACCOUNT-INSTANCE restriction, end to end over the `awsJson1_1` wire.
//!
//! The rule under test: permission sets — and the account assignments built from them —
//! are a feature of the ORGANIZATION instance of IAM Identity Center, so on an ACCOUNT
//! instance (the instance of a single standalone account, which is what `CreateInstance`
//! creates) every operation of the permission-set / account-assignment surface is refused
//! with the modelled `ValidationException` (HTTP 400), BEFORE any resource-existence
//! check — and the instance an account DISCOVERS through `ListInstances` is one of those,
//! which `discovered_instance_surface.rs` pins. On an organization instance the same
//! requests are served, and an `InstanceArn` the account holds no instance under remains a
//! missing resource rather than this refusal.
//!
//! The account instance is reached the way a client reaches one — a `CreateInstance` on an
//! account whose Identity Center is not enabled yet — not by writing a record into the
//! store.
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
    account_instance_router, call, error_code, organization_router, UNKNOWN_INSTANCE_ARN,
};

/// One call of the withdrawn surface.
pub(crate) struct SurfaceCall {
    pub(crate) label: String,
    pub(crate) operation: &'static str,
    /// What the operation answers for an `InstanceArn` the account holds no instance
    /// under: 400 `ResourceNotFoundException` for the operations that resolve the instance
    /// themselves (every refusal of a request on this awsJson1_1 surface is 400, so the
    /// error code is what tells it from the restriction). Every operation of the surface
    /// resolves the instance itself now, so no row answers 501 any more — a 501 would mean
    /// the operation models the account-instance refusal and nothing else, which is
    /// exactly why an unknown ARN would then not be refused as one.
    pub(crate) unknown_instance_status: u16,
    pub(crate) body: Value,
}

/// One row of the table: the inputs the refusal must NOT depend on.
pub(crate) struct Variant {
    pub(crate) label: &'static str,
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
    /// `AccessControlAttribute$Key` for the ABAC rows: `length(min: 1, max: 128)`,
    /// `^[\p{L}\p{Z}\p{N}_.:\/=+\-@]+$`.
    pub(crate) attribute_key: &'static str,
}

/// The rows differ along every axis of the surface's inputs: both `PrincipalType` enum
/// values, both forms the `PrincipalId` pattern admits, the published bounds of
/// `TargetId` (`^\d{12}$`) and of `MaxResults` (1..100), the key charset of an ABAC
/// attribute, and distinct permission-set ARNs and request ids — none of which can change
/// the answer on an account instance.
pub(crate) const VARIANTS: &[Variant] = &[
    Variant {
        label: "a GROUP principal, the recorded shape of the request",
        permission_set_suffix: "ps-0123456789abcdef",
        account_id: "123456789012",
        principal_type: "GROUP",
        principal_id: "f81d4fae-7dec-11d0-a765-00a0c91e6bf6",
        request_id: "11111111-2222-4333-8444-555555555555",
        max_results: None,
        attribute_key: "department",
    },
    Variant {
        label: "a USER principal with the store-prefixed id, lowest account id",
        permission_set_suffix: "ps-aaaaaaaaaaaaaaaa",
        account_id: "000000000000",
        principal_type: "USER",
        principal_id: "9067f3a1c2-f81d4fae-7dec-11d0-a765-00a0c91e6bf7",
        request_id: "66666666-7777-4888-9999-000000000000",
        max_results: Some(1),
        attribute_key: "A",
    },
    Variant {
        label: "a permission-set ARN using the full id charset, highest account id",
        permission_set_suffix: "ps-A-9.b/cDeF012345",
        account_id: "999999999999",
        principal_type: "USER",
        principal_id: "1b2d3c4f-5a6b-7c8d-9e0f-1a2b3c4d5e6f",
        request_id: "abcdefab-cdef-4bcd-abcd-efabcdefabcd",
        max_results: Some(100),
        attribute_key: "cost-center.id:eu/west=1+x@acme",
    },
];

/// The instance id an `InstanceArn` carries.
pub(crate) fn instance_id_of(instance_arn: &str) -> &str {
    instance_arn.rsplit('/').next().unwrap_or_default()
}

/// Every operation of the permission-set / account-assignment / ABAC surface, as the one
/// request each takes for `variant` under `instance_arn`.
///
/// `DeleteInlinePolicyFromPermissionSet` is the one operation of the surface still
/// scaffolded, so it is not a row here: it answers 501 for every instance.
pub(crate) fn surface_calls(instance_arn: &str, variant: &Variant) -> Vec<SurfaceCall> {
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
    let mut list_managed_policies = json!({
        "InstanceArn": instance_arn,
        "PermissionSetArn": permission_set_arn,
    });
    let mut list_customer_references = list_managed_policies.clone();
    let mut list_provisioned_accounts = list_managed_policies.clone();
    if let Some(max_results) = variant.max_results {
        list_permission_sets["MaxResults"] = json!(max_results);
        list_account_assignments["MaxResults"] = json!(max_results);
        list_managed_policies["MaxResults"] = json!(max_results);
        list_customer_references["MaxResults"] = json!(max_results);
        list_provisioned_accounts["MaxResults"] = json!(max_results);
    }
    let assignment = json!({
        "InstanceArn": instance_arn,
        "TargetId": variant.account_id,
        "TargetType": "AWS_ACCOUNT",
        "PermissionSetArn": permission_set_arn,
        "PrincipalType": variant.principal_type,
        "PrincipalId": variant.principal_id,
    });
    let on_set = json!({
        "InstanceArn": instance_arn,
        "PermissionSetArn": permission_set_arn,
    });
    // `ManagedPolicyArn` (`^arn:aws(-[a-z]{1,5}){0,3}:iam::aws:policy/[\p{L}\p{M}\p{Z}\p{S}\p{N}\p{P}]+$`)
    // and `CustomerManagedPolicyReference` (`Name` `^[\w+=,.@-]+$`, `Path`
    // `^((/[A-Za-z0-9\.,\+@=_-]+)*)/$`) as the model constrains them.
    let managed_policy = json!({
        "InstanceArn": instance_arn,
        "PermissionSetArn": permission_set_arn,
        "ManagedPolicyArn": "arn:aws:iam::aws:policy/ReadOnlyAccess",
    });
    let customer_reference = json!({
        "InstanceArn": instance_arn,
        "PermissionSetArn": permission_set_arn,
        "CustomerManagedPolicyReference": {"Name": "ReadOnlyPolicy", "Path": "/"},
    });
    // One attribute mapping, the shape `AccessControlAttribute` takes: a key and the
    // identity-source list its `Value` carries.
    let abac_configuration = json!({
        "InstanceArn": instance_arn,
        "InstanceAccessControlAttributeConfiguration": {
            "AccessControlAttributes": [{
                "Key": variant.attribute_key,
                "Value": {"Source": ["${path:enterprise.department}"]},
            }],
        },
    });
    vec![
        SurfaceCall {
            label: format!("{}: CreatePermissionSet", variant.label),
            operation: "CreatePermissionSet",
            unknown_instance_status: 400,
            body: json!({"InstanceArn": instance_arn, "Name": "Withdrawn"}),
        },
        SurfaceCall {
            label: format!("{}: DescribePermissionSet", variant.label),
            operation: "DescribePermissionSet",
            unknown_instance_status: 400,
            body: on_set.clone(),
        },
        SurfaceCall {
            label: format!("{}: ListPermissionSets", variant.label),
            operation: "ListPermissionSets",
            unknown_instance_status: 400,
            body: list_permission_sets,
        },
        SurfaceCall {
            label: format!("{}: UpdatePermissionSet", variant.label),
            operation: "UpdatePermissionSet",
            unknown_instance_status: 400,
            body: json!({
                "InstanceArn": instance_arn,
                "PermissionSetArn": permission_set_arn,
                "Description": "a set no account instance can hold",
                "SessionDuration": "PT1H",
            }),
        },
        SurfaceCall {
            label: format!("{}: DeletePermissionSet", variant.label),
            operation: "DeletePermissionSet",
            unknown_instance_status: 400,
            body: on_set.clone(),
        },
        SurfaceCall {
            label: format!("{}: CreateAccountAssignment", variant.label),
            operation: "CreateAccountAssignment",
            unknown_instance_status: 400,
            body: assignment.clone(),
        },
        SurfaceCall {
            label: format!("{}: DeleteAccountAssignment", variant.label),
            operation: "DeleteAccountAssignment",
            unknown_instance_status: 400,
            body: assignment,
        },
        SurfaceCall {
            label: format!("{}: ListAccountAssignments", variant.label),
            operation: "ListAccountAssignments",
            unknown_instance_status: 400,
            body: list_account_assignments,
        },
        SurfaceCall {
            label: format!("{}: DescribeAccountAssignmentCreationStatus", variant.label),
            operation: "DescribeAccountAssignmentCreationStatus",
            unknown_instance_status: 400,
            body: json!({
                "InstanceArn": instance_arn,
                "AccountAssignmentCreationRequestId": variant.request_id,
            }),
        },
        SurfaceCall {
            label: format!("{}: DescribeAccountAssignmentDeletionStatus", variant.label),
            operation: "DescribeAccountAssignmentDeletionStatus",
            unknown_instance_status: 400,
            body: json!({
                "InstanceArn": instance_arn,
                "AccountAssignmentDeletionRequestId": variant.request_id,
            }),
        },
        SurfaceCall {
            label: format!(
                "{}: ListAccountsForProvisionedPermissionSet",
                variant.label
            ),
            operation: "ListAccountsForProvisionedPermissionSet",
            unknown_instance_status: 400,
            body: list_provisioned_accounts,
        },
        SurfaceCall {
            label: format!("{}: PutInlinePolicyToPermissionSet", variant.label),
            operation: "PutInlinePolicyToPermissionSet",
            unknown_instance_status: 400,
            body: json!({
                "InstanceArn": instance_arn,
                "PermissionSetArn": permission_set_arn,
                "InlinePolicy": "{\"Version\":\"2012-10-17\",\"Statement\":[]}",
            }),
        },
        SurfaceCall {
            label: format!("{}: GetInlinePolicyForPermissionSet", variant.label),
            operation: "GetInlinePolicyForPermissionSet",
            unknown_instance_status: 400,
            body: on_set.clone(),
        },
        SurfaceCall {
            label: format!("{}: AttachManagedPolicyToPermissionSet", variant.label),
            operation: "AttachManagedPolicyToPermissionSet",
            unknown_instance_status: 400,
            body: managed_policy.clone(),
        },
        SurfaceCall {
            label: format!("{}: DetachManagedPolicyFromPermissionSet", variant.label),
            operation: "DetachManagedPolicyFromPermissionSet",
            unknown_instance_status: 400,
            body: managed_policy,
        },
        SurfaceCall {
            label: format!("{}: ListManagedPoliciesInPermissionSet", variant.label),
            operation: "ListManagedPoliciesInPermissionSet",
            unknown_instance_status: 400,
            body: list_managed_policies,
        },
        SurfaceCall {
            label: format!(
                "{}: AttachCustomerManagedPolicyReferenceToPermissionSet",
                variant.label
            ),
            operation: "AttachCustomerManagedPolicyReferenceToPermissionSet",
            unknown_instance_status: 400,
            body: customer_reference.clone(),
        },
        SurfaceCall {
            label: format!(
                "{}: DetachCustomerManagedPolicyReferenceFromPermissionSet",
                variant.label
            ),
            operation: "DetachCustomerManagedPolicyReferenceFromPermissionSet",
            unknown_instance_status: 400,
            body: customer_reference,
        },
        SurfaceCall {
            label: format!(
                "{}: ListCustomerManagedPolicyReferencesInPermissionSet",
                variant.label
            ),
            operation: "ListCustomerManagedPolicyReferencesInPermissionSet",
            unknown_instance_status: 400,
            body: list_customer_references,
        },
        SurfaceCall {
            label: format!("{}: PutPermissionsBoundaryToPermissionSet", variant.label),
            operation: "PutPermissionsBoundaryToPermissionSet",
            unknown_instance_status: 400,
            body: json!({
                "InstanceArn": instance_arn,
                "PermissionSetArn": permission_set_arn,
                "PermissionsBoundary": {
                    "ManagedPolicyArn": "arn:aws:iam::aws:policy/PowerUserAccess",
                },
            }),
        },
        SurfaceCall {
            label: format!("{}: GetPermissionsBoundaryForPermissionSet", variant.label),
            operation: "GetPermissionsBoundaryForPermissionSet",
            unknown_instance_status: 400,
            body: on_set.clone(),
        },
        SurfaceCall {
            label: format!(
                "{}: DeletePermissionsBoundaryFromPermissionSet",
                variant.label
            ),
            operation: "DeletePermissionsBoundaryFromPermissionSet",
            unknown_instance_status: 400,
            body: on_set,
        },
        SurfaceCall {
            label: format!(
                "{}: CreateInstanceAccessControlAttributeConfiguration",
                variant.label
            ),
            operation: "CreateInstanceAccessControlAttributeConfiguration",
            unknown_instance_status: 400,
            body: abac_configuration.clone(),
        },
        SurfaceCall {
            label: format!(
                "{}: DescribeInstanceAccessControlAttributeConfiguration",
                variant.label
            ),
            operation: "DescribeInstanceAccessControlAttributeConfiguration",
            unknown_instance_status: 400,
            body: json!({"InstanceArn": instance_arn}),
        },
        SurfaceCall {
            label: format!(
                "{}: UpdateInstanceAccessControlAttributeConfiguration",
                variant.label
            ),
            operation: "UpdateInstanceAccessControlAttributeConfiguration",
            unknown_instance_status: 400,
            body: abac_configuration,
        },
        SurfaceCall {
            label: format!(
                "{}: DeleteInstanceAccessControlAttributeConfiguration",
                variant.label
            ),
            operation: "DeleteInstanceAccessControlAttributeConfiguration",
            unknown_instance_status: 400,
            body: json!({"InstanceArn": instance_arn}),
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
    let (router, instance_arn) = account_instance_router().await;
    assert_eq!(
        permission_sets_enabled(&router, &instance_arn).await,
        Value::Bool(false),
        "an instance created through CreateInstance is an account instance, so it reports \
         permission sets disabled"
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
    let (router, instance_arn) = account_instance_router().await;
    let variant = &VARIANTS[0];

    // Nothing was ever created on this instance, so every resource these calls name is
    // absent — and the answer is the restriction's `ValidationException` rather than a
    // not-found that would tell a client "create the permission set and retry".
    for case in surface_calls(&instance_arn, variant) {
        let (status, body) = call(&router, case.operation, &case.body).await;
        assert_eq!(
            status, 400,
            "case {:?}: a missing resource on an account instance is still the \
             restriction, got {status} {body}",
            case.label
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {:?}: a missing resource on an account instance is still the \
             restriction, got {body}",
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
        if case.unknown_instance_status == 400 {
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
/// refusal above a property of the instance type rather than of the operations — and an
/// operation whose target resource is absent answers a not-found here, not the restriction.
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

    // The served operations of the surface whose target resource is ABSENT on this
    // instance: a missing resource, which is a `ResourceNotFoundException` — never the
    // account-instance refusal, which would claim this instance cannot serve them, and
    // never a 501.
    let not_found: Vec<(&str, Value)> = vec![
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
            "DescribeAccountAssignmentDeletionStatus",
            json!({
                "InstanceArn": instance_arn,
                "AccountAssignmentDeletionRequestId": "11111111-2222-4333-8444-555555555555",
            }),
        ),
        (
            "DescribeAccountAssignmentCreationStatus",
            json!({
                "InstanceArn": instance_arn,
                "AccountAssignmentCreationRequestId": "11111111-2222-4333-8444-555555555555",
            }),
        ),
    ];
    for (operation, body) in not_found {
        let (status, body) = call(&router, operation, &body).await;
        assert_eq!(
            status, 400,
            "case {operation}: an organization instance serves this operation, so an \
             absent resource is a not-found, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {operation}: expected ResourceNotFoundException, got {body}"
        );
    }
}

/// The refusal is a READ: it writes nothing, so a refused call leaves the account's
/// instance exactly as it was and a later caller sees no trace of it.
#[tokio::test]
async fn a_refused_call_changes_nothing() {
    let (router, instance_arn) = account_instance_router().await;

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
