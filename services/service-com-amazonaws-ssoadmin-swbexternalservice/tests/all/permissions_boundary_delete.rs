// SPDX-License-Identifier: CC0-1.0

//! DELETING the permissions boundary of a permission set, end to end over the `awsJson1_1`
//! wire.
//!
//! THE RULE under test: `DeletePermissionsBoundaryFromPermissionSet` removes the ONE
//! boundary the named permission set carries — whichever of the two kinds
//! `PermissionsBoundary` offers it was stored as — so a later
//! `GetPermissionsBoundaryForPermissionSet` reports none and a later put can store a new
//! one; the delete touches no other permission set; and a call that names no boundary this
//! permission set carries (never attached, or already deleted) is refused with the modelled
//! `ResourceNotFoundException` rather than answered with an empty success, as is an
//! `InstanceArn` or `PermissionSetArn` nothing is held under. The whole permission-set
//! surface, this operation included, is withdrawn on an account instance.
//!
//! Each rule is asserted over a TABLE whose rows differ along what the rule generalises
//! over: the KIND of boundary deleted, the partition and path forms the `ManagedPolicyArn`
//! pattern admits, its published upper length bound (2048), the presence of
//! `CustomerManagedPolicyReference$Path`, and the published bounds of `ManagedPolicyName`
//! (1..128) and `ManagedPolicyPath` (1..512) — the delete names no boundary itself, so what
//! it generalises over is the boundary it finds stored. Every row is a value no recording
//! carried.
//!
//! Every call goes through `MultiServiceRouter::dispatch`, the one path a socket-served
//! request takes. The harness is the one `ssoadmin_permission_sets.rs` owns, and the calls
//! run against an ORGANIZATION instance because the permission-set surface is withdrawn on
//! an account instance (asserted here too).

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ssoadmin_permission_sets::{
    assert_refused, call, organization_router, router, seeded_instance_arn, UNKNOWN_INSTANCE_ARN,
};

/// A pattern-valid `PermissionSetArn` that was never minted, for the not-found rows.
/// Shaped by the model's `PermissionSetArn` pattern
/// `^arn:aws(-[a-z]{1,5}){0,3}:sso:::permissionSet/(sso)?ins-[a-zA-Z0-9-.]{16}/ps-[a-zA-Z0-9-./]{16}$`.
const UNKNOWN_PERMISSION_SET_ARN: &str =
    "arn:aws:sso:::permissionSet/ssoins-1111111111111111/ps-1111111111111111";

/// The fixed part of an AWS managed policy ARN, as the model's `ManagedPolicyArn` pattern
/// (`^arn:aws(-[a-z]{1,5}){0,3}:iam::aws:policy((/[A-Za-z0-9\.,\+@=_-]+)*)/([A-Za-z0-9\.,\+=@_-]+)$`)
/// requires: 24 characters.
const MANAGED_POLICY_ARN_PREFIX: &str = "arn:aws:iam::aws:policy/";

/// `ManagedPolicyArn`'s published upper length bound (`length(min: 20, max: 2048)`).
const MANAGED_POLICY_ARN_MAX: usize = 2048;

/// One row: a boundary to store, and the value the get must report while it is stored.
struct BoundaryCase {
    label: String,
    /// What the put sends as `PermissionsBoundary`.
    sent: Value,
    /// What the get reports before the delete — the stored choice, with `Path`
    /// materialised to its documented default `/` when the request named none.
    expected: Value,
}

/// The table. Rows span both kinds of boundary, the partition suffixes and nested paths the
/// `ManagedPolicyArn` pattern admits and its upper length bound (2048), the presence and
/// forms of `CustomerManagedPolicyReference$Path`, and the published bounds of `Name`
/// (1..128) and `Path` (1..512) — the classes the rule generalises over.
fn boundary_cases() -> Vec<BoundaryCase> {
    // 2048 characters in all: the 24-character prefix plus a policy name.
    let longest_arn = format!(
        "{MANAGED_POLICY_ARN_PREFIX}{}",
        "Z".repeat(MANAGED_POLICY_ARN_MAX - MANAGED_POLICY_ARN_PREFIX.len())
    );
    // `ManagedPolicyName` at its upper bound of 128, over the full `^[\w+=,.@-]+$` class.
    let longest_name = format!("{}+=,.@-_", "m".repeat(121));
    // `ManagedPolicyPath` at its upper bound of 512: `^((/[A-Za-z0-9\.,\+@=_-]+)*)/$`.
    let longest_path = format!("/{}/", "q".repeat(510));
    vec![
        BoundaryCase {
            label: "an AWS managed policy in the aws partition".to_string(),
            sent: json!({"ManagedPolicyArn": "arn:aws:iam::aws:policy/AmazonS3FullAccess"}),
            expected: json!({"ManagedPolicyArn": "arn:aws:iam::aws:policy/AmazonS3FullAccess"}),
        },
        BoundaryCase {
            label: "an AWS managed policy under a nested path".to_string(),
            sent: json!({
                "ManagedPolicyArn": "arn:aws:iam::aws:policy/job-function/Billing",
            }),
            expected: json!({
                "ManagedPolicyArn": "arn:aws:iam::aws:policy/job-function/Billing",
            }),
        },
        BoundaryCase {
            // `^arn:aws(-[a-z]{1,5}){0,3}:iam::aws:policy…` admits partition suffixes.
            label: "an AWS managed policy in a suffixed partition".to_string(),
            sent: json!({"ManagedPolicyArn": "arn:aws-cn:iam::aws:policy/ReadOnlyAccess"}),
            expected: json!({"ManagedPolicyArn": "arn:aws-cn:iam::aws:policy/ReadOnlyAccess"}),
        },
        BoundaryCase {
            label: "an AWS managed policy ARN at its upper length bound".to_string(),
            sent: json!({"ManagedPolicyArn": longest_arn}),
            expected: json!({"ManagedPolicyArn": longest_arn}),
        },
        BoundaryCase {
            label: "a customer managed policy with no Path (the documented default)"
                .to_string(),
            sent: json!({"CustomerManagedPolicyReference": {"Name": "DeletableBoundary"}}),
            // `Path`'s own documentation: "The default is `/`".
            expected: json!({
                "CustomerManagedPolicyReference": {"Name": "DeletableBoundary", "Path": "/"},
            }),
        },
        BoundaryCase {
            label: "a customer managed policy under a nested Path".to_string(),
            sent: json!({
                "CustomerManagedPolicyReference": {"Name": "Scoped", "Path": "/finance/apac/"},
            }),
            expected: json!({
                "CustomerManagedPolicyReference": {"Name": "Scoped", "Path": "/finance/apac/"},
            }),
        },
        BoundaryCase {
            // `ManagedPolicyName` at its lower length bound (1..128).
            label: "a customer managed policy whose Name is one character".to_string(),
            sent: json!({"CustomerManagedPolicyReference": {"Name": "z"}}),
            expected: json!({"CustomerManagedPolicyReference": {"Name": "z", "Path": "/"}}),
        },
        BoundaryCase {
            label: "a customer managed policy at the upper bounds of Name and Path".to_string(),
            sent: json!({
                "CustomerManagedPolicyReference": {"Name": longest_name, "Path": longest_path},
            }),
            expected: json!({
                "CustomerManagedPolicyReference": {"Name": longest_name, "Path": longest_path},
            }),
        },
    ]
}

/// Create a permission set named `name` on `instance_arn` and return its minted ARN.
async fn create_permission_set(
    router: &MultiServiceRouter,
    instance_arn: &str,
    name: &str,
) -> String {
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
    body["PermissionSet"]["PermissionSetArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name:?}: no minted PermissionSetArn in {body}"))
        .to_string()
}

/// `GetPermissionsBoundaryForPermissionSet`, as `(status, PermissionsBoundary or Null)`.
async fn get_boundary(
    router: &MultiServiceRouter,
    instance_arn: &str,
    permission_set_arn: &str,
) -> (u16, Value) {
    let (status, body) = call(
        router,
        "GetPermissionsBoundaryForPermissionSet",
        &json!({"InstanceArn": instance_arn, "PermissionSetArn": permission_set_arn}),
    )
    .await;
    if status != 200 {
        return (status, body);
    }
    (status, body["PermissionsBoundary"].clone())
}

/// A put of `boundary`, as `(status, body)`.
async fn put_boundary(
    router: &MultiServiceRouter,
    instance_arn: &str,
    permission_set_arn: &str,
    boundary: &Value,
) -> (u16, Value) {
    call(
        router,
        "PutPermissionsBoundaryToPermissionSet",
        &json!({
            "InstanceArn": instance_arn,
            "PermissionSetArn": permission_set_arn,
            "PermissionsBoundary": boundary,
        }),
    )
    .await
}

/// A delete, as `(status, body)`.
async fn delete_boundary(
    router: &MultiServiceRouter,
    instance_arn: &str,
    permission_set_arn: &str,
) -> (u16, Value) {
    call(
        router,
        "DeletePermissionsBoundaryFromPermissionSet",
        &json!({"InstanceArn": instance_arn, "PermissionSetArn": permission_set_arn}),
    )
    .await
}

/// THE RULE: the delete removes whichever boundary is stored, so the get that reported it
/// reports none afterwards — over every row of the table, each on its own permission set.
#[tokio::test]
async fn the_delete_removes_the_stored_boundary_whatever_kind_it_is() {
    let (router, instance_arn) = organization_router();

    for (index, case) in boundary_cases().into_iter().enumerate() {
        let permission_set_arn =
            create_permission_set(&router, &instance_arn, &format!("Deleted{index}")).await;

        let (status, body) =
            put_boundary(&router, &instance_arn, &permission_set_arn, &case.sent).await;
        assert_eq!(
            status, 200,
            "case {:?}: the put expected 200, got {status} {body}",
            case.label
        );
        let (status, stored) = get_boundary(&router, &instance_arn, &permission_set_arn).await;
        assert_eq!(
            status, 200,
            "case {:?}: the get before the delete expected 200, got {status} {stored}",
            case.label
        );
        assert_eq!(
            stored, case.expected,
            "case {:?}: expected the stored boundary {}, got {stored}",
            case.label, case.expected
        );

        let (status, body) = delete_boundary(&router, &instance_arn, &permission_set_arn).await;
        assert_eq!(
            status, 200,
            "case {:?}: the delete expected 200, got {status} {body}",
            case.label
        );

        let (status, after) = get_boundary(&router, &instance_arn, &permission_set_arn).await;
        assert_eq!(
            status, 200,
            "case {:?}: the get after the delete expected 200, got {status} {after}",
            case.label
        );
        assert_eq!(
            after,
            Value::Null,
            "case {:?}: the delete must leave no boundary, got {after}",
            case.label
        );
    }
}

/// The delete removes the boundary and nothing else of the permission set: the set itself,
/// and the boundary a put stores after the delete, are both still observable.
#[tokio::test]
async fn a_put_after_the_delete_stores_a_new_boundary() {
    let (router, instance_arn) = organization_router();
    let permission_set_arn = create_permission_set(&router, &instance_arn, "Recycled").await;

    for case in boundary_cases() {
        let (status, body) =
            put_boundary(&router, &instance_arn, &permission_set_arn, &case.sent).await;
        assert_eq!(
            status, 200,
            "case {:?}: the put expected 200, got {status} {body}",
            case.label
        );
        let (status, body) = delete_boundary(&router, &instance_arn, &permission_set_arn).await;
        assert_eq!(
            status, 200,
            "case {:?}: the delete expected 200, got {status} {body}",
            case.label
        );
        let (_, after) = get_boundary(&router, &instance_arn, &permission_set_arn).await;
        assert_eq!(
            after,
            Value::Null,
            "case {:?}: the delete must leave no boundary, got {after}",
            case.label
        );
    }

    // The permission set survives every delete of its boundary: a describe still answers.
    let (status, body) = call(
        &router,
        "DescribePermissionSet",
        &json!({"InstanceArn": instance_arn, "PermissionSetArn": permission_set_arn}),
    )
    .await;
    assert_eq!(
        status, 200,
        "DescribePermissionSet after the deletes expected 200, got {status} {body}"
    );
    assert_eq!(
        body["PermissionSet"]["PermissionSetArn"].as_str(),
        Some(permission_set_arn.as_str()),
        "the permission set must survive the delete of its boundary, got {body}"
    );
}

/// A call that names no boundary this permission set carries is REFUSED: a permission set
/// that never had one, and one whose boundary a previous delete already removed, both earn
/// the modelled `ResourceNotFoundException` rather than an empty success.
#[tokio::test]
async fn a_delete_with_no_boundary_attached_is_not_found() {
    let (router, instance_arn) = organization_router();

    for (index, case) in boundary_cases().into_iter().enumerate() {
        let permission_set_arn =
            create_permission_set(&router, &instance_arn, &format!("NoBoundary{index}")).await;

        // Never attached.
        let (status, body) = delete_boundary(&router, &instance_arn, &permission_set_arn).await;
        assert_refused(
            &case.label,
            "a delete with no boundary attached",
            "ResourceNotFoundException",
            status,
            &body,
        );

        // Attached, deleted once — the second delete has nothing left to name.
        let (status, body) =
            put_boundary(&router, &instance_arn, &permission_set_arn, &case.sent).await;
        assert_eq!(
            status, 200,
            "case {:?}: the put expected 200, got {status} {body}",
            case.label
        );
        let (status, body) = delete_boundary(&router, &instance_arn, &permission_set_arn).await;
        assert_eq!(
            status, 200,
            "case {:?}: the first delete expected 200, got {status} {body}",
            case.label
        );
        let (status, body) = delete_boundary(&router, &instance_arn, &permission_set_arn).await;
        assert_refused(
            &case.label,
            "a repeated delete",
            "ResourceNotFoundException",
            status,
            &body,
        );
    }
}

/// The delete removes the boundary of the permission set it NAMED and of no other: every
/// other permission set still reports its own.
#[tokio::test]
async fn the_delete_touches_only_the_permission_set_it_names() {
    let (router, instance_arn) = organization_router();
    let cases = boundary_cases();
    let mut attached: Vec<(String, Value)> = Vec::new();

    for (index, case) in cases.iter().enumerate() {
        let permission_set_arn =
            create_permission_set(&router, &instance_arn, &format!("Neighbour{index}")).await;
        let (status, body) =
            put_boundary(&router, &instance_arn, &permission_set_arn, &case.sent).await;
        assert_eq!(
            status, 200,
            "case {:?}: the put expected 200, got {status} {body}",
            case.label
        );
        attached.push((permission_set_arn, case.expected.clone()));
    }

    // Delete them one at a time; after each, every remaining permission set still carries
    // the boundary it was given and every deleted one carries none.
    for deleted in 0..attached.len() {
        let (status, body) = delete_boundary(&router, &instance_arn, &attached[deleted].0).await;
        assert_eq!(
            status, 200,
            "case {:?}: the delete expected 200, got {status} {body}",
            cases[deleted].label
        );

        for (index, (permission_set_arn, expected)) in attached.iter().enumerate() {
            let (status, stored) = get_boundary(&router, &instance_arn, permission_set_arn).await;
            assert_eq!(
                status, 200,
                "case {:?}: the get expected 200, got {status} {stored}",
                cases[index].label
            );
            let expected = if index <= deleted { &Value::Null } else { expected };
            assert_eq!(
                &stored, expected,
                "case {:?}: after deleting {} the boundary of {permission_set_arn} must be \
                 {expected}, got {stored}",
                cases[index].label, attached[deleted].0
            );
        }
    }
}

/// An `InstanceArn` or `PermissionSetArn` nothing is held under is a missing resource for
/// the delete too — never a success — and a deleted permission set is as absent as one that
/// never existed.
#[tokio::test]
async fn an_unknown_instance_or_permission_set_is_not_found() {
    let (router, instance_arn) = organization_router();
    let present = create_permission_set(&router, &instance_arn, "Present").await;
    let (status, body) = put_boundary(
        &router,
        &instance_arn,
        &present,
        &boundary_cases()[0].sent.clone(),
    )
    .await;
    assert_eq!(status, 200, "the put expected 200, got {status} {body}");

    let mut rows: Vec<(String, String, String)> = vec![
        (
            "an InstanceArn no instance is held under".to_string(),
            UNKNOWN_INSTANCE_ARN.to_string(),
            present.clone(),
        ),
        (
            "a PermissionSetArn never minted".to_string(),
            instance_arn.clone(),
            UNKNOWN_PERMISSION_SET_ARN.to_string(),
        ),
    ];

    // A permission set that carried a boundary, then was deleted outright.
    let gone = create_permission_set(&router, &instance_arn, "Gone").await;
    let (status, body) =
        put_boundary(&router, &instance_arn, &gone, &boundary_cases()[0].sent.clone()).await;
    assert_eq!(status, 200, "the put on the doomed set expected 200, got {status} {body}");
    let (status, body) = call(
        &router,
        "DeletePermissionSet",
        &json!({"InstanceArn": instance_arn, "PermissionSetArn": gone}),
    )
    .await;
    assert_eq!(status, 200, "DeletePermissionSet expected 200, got {status} {body}");
    rows.push((
        "a deleted PermissionSetArn".to_string(),
        instance_arn.clone(),
        gone,
    ));

    for (label, instance, permission_set) in rows {
        let (status, body) = delete_boundary(&router, &instance, &permission_set).await;
        assert_refused(
            &label,
            "the delete",
            "ResourceNotFoundException",
            status,
            &body,
        );
    }

    // And the refusals changed nothing: the present permission set still carries its
    // boundary.
    let (status, stored) = get_boundary(&router, &instance_arn, &present).await;
    assert_eq!(status, 200, "the get expected 200, got {status} {stored}");
    assert_eq!(
        stored,
        boundary_cases()[0].expected,
        "a refused delete must leave the stored boundary, expected {}, got {stored}",
        boundary_cases()[0].expected
    );
}

/// The delete belongs to the permission-set surface, which is WITHDRAWN on an account
/// instance: there it answers the modelled `ValidationException` naming account instances,
/// before any resource lookup.
#[tokio::test]
async fn the_delete_is_refused_on_an_account_instance() {
    let router = router();
    let account_instance_arn = seeded_instance_arn(&router).await;
    // Pattern-valid under the account instance's own id, so the refusal cannot be the
    // modelled constraint check.
    let permission_set_arn = format!(
        "arn:aws:sso:::permissionSet/{}/ps-0123456789abcdef",
        account_instance_arn.rsplit('/').next().unwrap_or_default()
    );

    let (status, body) =
        delete_boundary(&router, &account_instance_arn, &permission_set_arn).await;
    assert_refused(
        "an account instance",
        "the delete",
        "ValidationException",
        status,
        &body,
    );
    let message = body["message"]
        .as_str()
        .or_else(|| body["Message"].as_str())
        .unwrap_or_default();
    assert!(
        message.contains("account instances"),
        "the refusal must say the operation is unsupported for account instances, got \
         {message:?}"
    );
}
