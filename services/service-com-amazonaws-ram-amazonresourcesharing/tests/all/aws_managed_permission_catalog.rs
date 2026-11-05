// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for reading an AWS managed permission through
//! `GetPermission`.
//!
//! The rule the rows below pin: RAM ships managed permissions of its own, whose ARNs
//! carry no Region and `aws` in place of an account id
//! (`arn:aws:ram::aws:permission/<PermissionName>`), and a read of one answers HTTP 200
//! with the catalog entry's name, resource type and policy document, reported as
//! `AWS_MANAGED`, `STANDARD` and `ATTACHABLE` at its default version — exactly as a
//! read of a customer managed permission reports what a create stored. A permission ARN
//! the catalog does not name, or a version it does not carry, is the
//! `UnknownResourceException` case.
//!
//! So the rows vary along what the rule generalises over: EVERY entry of the catalog is
//! driven, not only the one the issue cited, including one that is not its resource
//! type's default and resource types whose service code carries a hyphen; a read asks
//! for the default version, for that version by number, for a version no entry carries,
//! and for a name the catalog does not hold.

use serde_json::{json, Value};
use service_com_amazonaws_ram_amazonresourcesharing::state::{
    aws_managed_permission_arn, AWS_MANAGED_PERMISSIONS, FIRST_PERMISSION_VERSION,
    PERMISSION_FEATURE_SET_STANDARD, PERMISSION_STATUS_ATTACHABLE, PERMISSION_TYPE_AWS_MANAGED,
    PERMISSION_TYPE_CUSTOMER_MANAGED, RESOURCE_TYPES,
};
use smithy_server_runtime::MultiServiceRouter;

// Every suite in this crate calls the service the one way: through `tests/all/harness.rs`.
use super::harness::*;

async fn get_permission(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "POST", "/getpermission", Some(body)).await
}

async fn create_permission(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "POST", "/createpermission", Some(body)).await
}

/// The `permission` a read reported.
fn permission_of(case: &str, body: &Value) -> Value {
    body.get("permission")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: response carried no permission, got {body:?}"))
}

/// A member of a reported permission, as a string.
fn field(case: &str, permission: &Value, member: &str) -> String {
    permission
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected a {member} member, got {permission:?}"))
        .to_string()
}

/// A boolean member of a reported permission.
fn flag(case: &str, permission: &Value, member: &str) -> bool {
    permission
        .get(member)
        .and_then(Value::as_bool)
        .unwrap_or_else(|| panic!("case {case:?}: expected a {member} member, got {permission:?}"))
}

/// Assert one reported detail against the catalog entry it must have come from.
fn assert_detail(case: &str, permission: &Value, index: usize, version_asked_for: Option<i32>) {
    let entry = &AWS_MANAGED_PERMISSIONS[index];
    assert_eq!(
        field(case, permission, "arn"),
        aws_managed_permission_arn(entry.name),
        "case {case:?}: expected the catalog entry's ARN, got {permission:?}"
    );
    assert_eq!(
        field(case, permission, "name"),
        entry.name,
        "case {case:?}: expected the catalog entry's name, got {permission:?}"
    );
    assert_eq!(
        field(case, permission, "resourceType"),
        entry.resource_type,
        "case {case:?}: expected the catalog entry's resourceType, got {permission:?}"
    );
    assert_eq!(
        field(case, permission, "permissionType"),
        PERMISSION_TYPE_AWS_MANAGED,
        "case {case:?}: a permission Amazon Web Services manages reports AWS_MANAGED, \
         got {permission:?}"
    );
    assert_eq!(
        field(case, permission, "featureSet"),
        PERMISSION_FEATURE_SET_STANDARD,
        "case {case:?}: expected the STANDARD feature set, got {permission:?}"
    );
    assert_eq!(
        field(case, permission, "status"),
        PERMISSION_STATUS_ATTACHABLE,
        "case {case:?}: a catalog entry can be attached to a share, got {permission:?}"
    );
    assert_eq!(
        field(case, permission, "version"),
        FIRST_PERMISSION_VERSION.to_string(),
        "case {case:?}: expected the version the catalog ships, got {permission:?}"
    );
    assert!(
        flag(case, permission, "defaultVersion"),
        "case {case:?}: the version the catalog ships is the default one, got {permission:?}"
    );
    assert_eq!(
        flag(case, permission, "isResourceTypeDefault"),
        entry.is_resource_type_default,
        "case {case:?}: expected the catalog's resource-type default flag, got {permission:?}"
    );
    // The document reports the effect and the actions the catalog entry allows. It
    // goes on the wire as a JSON string, so it is compared parsed: what matters is the
    // document, not one serializer's member order.
    let document: Value = serde_json::from_str(&field(case, permission, "permission"))
        .unwrap_or_else(|error| {
            panic!("case {case:?}: the permission document does not parse as JSON: {error}")
        });
    assert_eq!(
        document,
        json!({"Effect": "Allow", "Action": entry.actions}),
        "case {case:?}: expected the catalog entry's policy document, got {document:?}"
    );
    for member in ["creationTime", "lastUpdatedTime"] {
        // The harness reads a timestamp the one way every suite here does.
        let seconds = timestamp(case, permission, member);
        assert!(
            seconds > 0.0,
            "case {case:?}: expected a {member} in the past, got {seconds}"
        );
    }
    // A request that named a version got that version back, and one that named none
    // got the default version.
    if let Some(asked) = version_asked_for {
        assert_eq!(
            field(case, permission, "version"),
            asked.to_string(),
            "case {case:?}: expected the version the request asked for, got {permission:?}"
        );
    }
}

#[tokio::test]
async fn every_aws_managed_permission_in_the_catalog_reads_back() {
    let router = emulator();
    assert!(
        AWS_MANAGED_PERMISSIONS.len() > 1,
        "the catalog must hold more than the one entry this issue cited, got {}",
        AWS_MANAGED_PERMISSIONS.len()
    );
    for (index, entry) in AWS_MANAGED_PERMISSIONS.iter().enumerate() {
        let arn = aws_managed_permission_arn(entry.name);
        // The ARN of a permission Amazon Web Services owns names no Region and no
        // account id.
        assert_eq!(
            arn,
            format!("arn:aws:ram::aws:permission/{}", entry.name),
            "case {:?}: expected an account-less catalog ARN, got {arn:?}",
            entry.name
        );

        // Asking for no version reports the default version.
        let (status, read) = get_permission(&router, &json!({"permissionArn": arn})).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200, got {status} with {read:?}",
            entry.name
        );
        assert_detail(entry.name, &permission_of(entry.name, &read), index, None);

        // Asking for the version the catalog ships reports the same detail.
        let (status, by_version) = get_permission(
            &router,
            &json!({"permissionArn": arn, "permissionVersion": FIRST_PERMISSION_VERSION}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200 for the version by number, got {status} with \
             {by_version:?}",
            entry.name
        );
        assert_detail(
            entry.name,
            &permission_of(entry.name, &by_version),
            index,
            Some(FIRST_PERMISSION_VERSION),
        );
        assert_eq!(
            read, by_version,
            "case {:?}: naming the default version by number reports the same detail",
            entry.name
        );
    }
}

#[tokio::test]
async fn the_catalog_entry_the_recording_names_is_served() {
    let router = emulator();
    // The row the issue cited, asserted by hand rather than off the table: the
    // permission the recording read, at the ARN the recording named.
    let case = "APIGatewayPrivateDomainNameManagementPolicyDefaultPermission";
    let (status, read) = get_permission(
        &router,
        &json!({"permissionArn": format!("arn:aws:ram::aws:permission/{case}")}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with {read:?}"
    );
    let permission = permission_of(case, &read);
    assert_eq!(
        field(case, &permission, "name"),
        case,
        "case {case:?}: expected that permission's name, got {permission:?}"
    );
    assert_eq!(
        field(case, &permission, "resourceType"),
        "apigateway:Domainnames",
        "case {case:?}: expected the resource type the catalog records, got {permission:?}"
    );
    assert_eq!(
        field(case, &permission, "permissionType"),
        PERMISSION_TYPE_AWS_MANAGED,
        "case {case:?}: expected AWS_MANAGED, got {permission:?}"
    );
    assert!(
        flag(case, &permission, "isResourceTypeDefault"),
        "case {case:?}: that permission is its resource type's default, got {permission:?}"
    );
}

#[tokio::test]
async fn the_catalog_holds_an_entry_that_is_not_its_resource_types_default() {
    // The flag is per entry, not per read: a resource type RAM publishes more than one
    // permission for has exactly one default, and the others report false.
    let non_default: Vec<&str> = AWS_MANAGED_PERMISSIONS
        .iter()
        .filter(|entry| !entry.is_resource_type_default)
        .map(|entry| entry.name)
        .collect();
    assert!(
        !non_default.is_empty(),
        "the catalog must carry an entry that is not its resource type's default so the \
         flag is exercised in both states"
    );

    let router = emulator();
    for name in non_default {
        let (status, read) = get_permission(
            &router,
            &json!({"permissionArn": aws_managed_permission_arn(name)}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected HTTP 200, got {status} with {read:?}"
        );
        assert!(
            !flag(name, &permission_of(name, &read), "isResourceTypeDefault"),
            "case {name:?}: this entry is not its resource type's default, got {read:?}"
        );
    }
}

#[tokio::test]
async fn one_default_permission_per_resource_type_and_every_type_is_shareable() {
    // Two invariants over the whole catalog: a resource type an AWS managed permission
    // applies to is a resource type this service shares (`RESOURCE_TYPES`, what
    // `ListResourceTypes` reports), and at most one permission is the default for it.
    for entry in AWS_MANAGED_PERMISSIONS {
        assert!(
            RESOURCE_TYPES
                .iter()
                .any(|known| known.resource_type == entry.resource_type),
            "case {:?}: resource type {:?} is not one this service shares",
            entry.name,
            entry.resource_type
        );
        let defaults = AWS_MANAGED_PERMISSIONS
            .iter()
            .filter(|other| {
                other.resource_type == entry.resource_type && other.is_resource_type_default
            })
            .count();
        assert_eq!(
            defaults, 1,
            "case {:?}: resource type {:?} must have exactly one default permission, got \
             {defaults}",
            entry.name, entry.resource_type
        );
    }
    let mut names: Vec<&str> = AWS_MANAGED_PERMISSIONS
        .iter()
        .map(|entry| entry.name)
        .collect();
    let total = names.len();
    names.sort_unstable();
    names.dedup();
    assert_eq!(
        names.len(),
        total,
        "every catalog entry carries its own name, got {names:?}"
    );
}

#[tokio::test]
async fn a_catalog_read_reports_the_same_detail_to_a_later_caller() {
    let router = emulator();
    // A read writes nothing: what the catalog reports does not drift between calls,
    // timestamps included, and it is not consumed by being read.
    let arn = aws_managed_permission_arn(AWS_MANAGED_PERMISSIONS[0].name);
    let (first_status, first) = get_permission(&router, &json!({"permissionArn": arn})).await;
    let (second_status, second) = get_permission(&router, &json!({"permissionArn": arn})).await;
    assert_eq!(
        (first_status, second_status),
        (200, 200),
        "case \"repeat read\": both reads expected 200, got {first_status} then {second_status}"
    );
    assert_eq!(
        first, second,
        "case \"repeat read\": a read reported something else the second time, got {second:?}"
    );
}

#[tokio::test]
async fn a_permission_the_catalog_does_not_hold_is_an_unknown_resource() {
    let router = emulator();
    // The refusals the rule leaves in place: an account-less permission ARN whose name
    // no catalog entry carries, and a version no entry ships, are unknown resources —
    // not malformed ARNs and not server errors.
    for name in [
        "NoSuchAwsManagedPermission",
        "AWSRAMDefaultPermissionAppMesh2",
        "awsramdefaultpermissionsubnet",
    ] {
        let arn = aws_managed_permission_arn(name);
        let (status, refused) = get_permission(&router, &json!({"permissionArn": arn.clone()})).await;
        assert_eq!(
            status, 400,
            "case {name:?}: expected HTTP 400, got {status} with {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            "UnknownResourceException",
            "case {name:?}: expected UnknownResourceException, got {refused:?}"
        );
        assert_eq!(
            error_message(&refused),
            format!("Permission {arn} could not be found."),
            "case {name:?}: the refusal names the permission it was asked for, got {refused:?}"
        );
    }

    let arn = aws_managed_permission_arn(AWS_MANAGED_PERMISSIONS[0].name);
    for version in [FIRST_PERMISSION_VERSION + 1, 7] {
        let (status, refused) = get_permission(
            &router,
            &json!({"permissionArn": arn.clone(), "permissionVersion": version}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case \"version {version}\": expected HTTP 400, got {status} with {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            "UnknownResourceException",
            "case \"version {version}\": expected UnknownResourceException, got {refused:?}"
        );
        assert_eq!(
            error_message(&refused),
            format!("Permission {arn} has no version {version}."),
            "case \"version {version}\": the refusal names the version it was asked for, got \
             {refused:?}"
        );
    }
}

#[tokio::test]
async fn a_created_permission_is_this_accounts_and_the_catalog_is_not() {
    let router = emulator();
    // The two populations live in one store and are told apart by their ARN and their
    // type: a create mints a CUSTOMER_MANAGED permission in this account and Region,
    // and leaves every catalog entry exactly as it was.
    let (status, created) = create_permission(
        &router,
        &json!({
            "name": "CatalogNeighbour",
            "resourceType": "codebuild:Project",
            "policyTemplate": r#"{"Effect":"ALLOW","Action":["codebuild:BatchGetProjects"]}"#,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"create beside the catalog\": expected 200, got {status} with {created:?}"
    );
    let minted = permission_of("create beside the catalog", &created);
    assert_eq!(
        field("create beside the catalog", &minted, "arn"),
        format!("arn:aws:ram:{REGION}:{ACCOUNT}:permission/CatalogNeighbour"),
        "case \"create beside the catalog\": a created permission is this account's, got {minted:?}"
    );
    assert_eq!(
        field("create beside the catalog", &minted, "permissionType"),
        PERMISSION_TYPE_CUSTOMER_MANAGED,
        "case \"create beside the catalog\": expected CUSTOMER_MANAGED, got {minted:?}"
    );

    // The default permission of that resource type is still the catalog's, and still
    // reads back as AWS_MANAGED.
    let catalog_entry = AWS_MANAGED_PERMISSIONS
        .iter()
        .find(|entry| entry.resource_type == "codebuild:Project")
        .expect("the catalog publishes a permission for that resource type");
    let (status, read) = get_permission(
        &router,
        &json!({"permissionArn": aws_managed_permission_arn(catalog_entry.name)}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {:?}: expected HTTP 200, got {status} with {read:?}",
        catalog_entry.name
    );
    let permission = permission_of(catalog_entry.name, &read);
    assert_eq!(
        field(catalog_entry.name, &permission, "permissionType"),
        PERMISSION_TYPE_AWS_MANAGED,
        "case {:?}: the catalog entry is still AWS_MANAGED, got {permission:?}",
        catalog_entry.name
    );
    assert!(
        flag(catalog_entry.name, &permission, "isResourceTypeDefault"),
        "case {:?}: a created permission does not take over the resource type's default, got \
         {permission:?}",
        catalog_entry.name
    );
}
