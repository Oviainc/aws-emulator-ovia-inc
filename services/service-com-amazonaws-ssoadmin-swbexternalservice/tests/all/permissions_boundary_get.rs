// SPDX-License-Identifier: CC0-1.0

//! `GetPermissionsBoundaryForPermissionSet` as a REPORT of the permission set's boundary
//! SLOT, end to end over the `awsJson1_1` wire.
//!
//! THE RULE under test: the permissions boundary is a slot of its own on the permission set
//! the request names, so the get reports exactly what is stored in that slot — unchanged by
//! a write to any OTHER facet of the same permission set (its description, session duration
//! and relay state; its attached Amazon Web Services managed policies; its attached customer
//! managed policy references; its inline policy; its tags; a provisioning of it) — and the
//! slot lives and dies with the permission-set resource: once that resource is deleted the
//! get answers `ResourceNotFoundException` naming the ARN it was asked for, and a permission
//! set created afterwards is a new resource whose slot is empty.
//!
//! What a put stores and gets back, what a later put replaces and how the refusals read is
//! `permissions_boundary.rs`; what a delete removes is `permissions_boundary_delete.rs`.
//! This suite asserts the INDEPENDENCE of the slot from the permission set's other facets
//! and its lifetime, which neither of those covers.
//!
//! Each rule is asserted over a TABLE whose rows differ along what the rule generalises
//! over: the KIND of boundary stored (an Amazon Web Services managed policy ARN, a customer
//! managed policy reference with a `Path` and one without, whose absent `Path` the service
//! materialises as its documented default `/`), and the facet written in between — one row
//! per operation that writes another facet of the same permission set.
//!
//! Every call goes through `MultiServiceRouter::dispatch`, the one path a socket-served
//! request takes. The harness is the one `ssoadmin_permission_sets.rs` owns, and the calls
//! run against an ORGANIZATION instance because the permission-set surface is withdrawn on
//! an account instance.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ssoadmin_permission_sets::{call, error_code, organization_router};

/// One row of the boundary table: what the put sends, and what every later get must report.
struct BoundaryCase {
    label: &'static str,
    sent: Value,
    expected: Value,
}

/// The boundary kinds `PermissionsBoundary` offers ("Specify either
/// `CustomerManagedPolicyReference` to use the name and path of a customer managed policy,
/// or `ManagedPolicyArn` to use the ARN of an Amazon Web Services managed policy"), with the
/// documented default of `CustomerManagedPolicyReference$Path` ("The default is `/`").
fn boundary_cases() -> Vec<BoundaryCase> {
    vec![
        BoundaryCase {
            label: "an AWS managed policy ARN",
            sent: json!({"ManagedPolicyArn": "arn:aws:iam::aws:policy/SecurityAudit"}),
            expected: json!({"ManagedPolicyArn": "arn:aws:iam::aws:policy/SecurityAudit"}),
        },
        BoundaryCase {
            label: "a customer managed policy reference under a path",
            sent: json!({
                "CustomerManagedPolicyReference": {
                    "Name": "BoundaryPolicy",
                    "Path": "/boundaries/team-a/",
                },
            }),
            expected: json!({
                "CustomerManagedPolicyReference": {
                    "Name": "BoundaryPolicy",
                    "Path": "/boundaries/team-a/",
                },
            }),
        },
        BoundaryCase {
            label: "a customer managed policy reference with no path",
            sent: json!({"CustomerManagedPolicyReference": {"Name": "Boundary.policy+2"}}),
            expected: json!({
                "CustomerManagedPolicyReference": {"Name": "Boundary.policy+2", "Path": "/"},
            }),
        },
    ]
}

/// One row of the OTHER-FACET table: a call that writes some facet of the permission set
/// other than its boundary slot.
struct FacetWrite {
    label: &'static str,
    operation: &'static str,
    /// The request body, built from the instance and permission set it is aimed at.
    body: fn(&str, &str) -> Value,
}

/// Every operation of this crate that writes another facet of a permission set: its
/// description, session duration and relay state (`UpdatePermissionSet`), the AWS managed
/// policies and customer managed policy references attached to it, its inline policy, its
/// tags, and a provisioning of it. None of them names a permissions boundary, so none may
/// change what the get reports.
fn other_facet_writes() -> Vec<FacetWrite> {
    vec![
        FacetWrite {
            label: "UpdatePermissionSet rewrites the description, duration and relay state",
            operation: "UpdatePermissionSet",
            body: |instance_arn, permission_set_arn| {
                json!({
                    "InstanceArn": instance_arn,
                    "PermissionSetArn": permission_set_arn,
                    "Description": "A facet that is not the boundary",
                    // `Duration`'s pattern is an ISO-8601 duration.
                    "SessionDuration": "PT4H",
                    "RelayState": "https://example.aws/start",
                })
            },
        },
        FacetWrite {
            label: "AttachManagedPolicyToPermissionSet attaches an AWS managed policy",
            operation: "AttachManagedPolicyToPermissionSet",
            body: |instance_arn, permission_set_arn| {
                json!({
                    "InstanceArn": instance_arn,
                    "PermissionSetArn": permission_set_arn,
                    "ManagedPolicyArn": "arn:aws:iam::aws:policy/AWSSupportAccess",
                })
            },
        },
        FacetWrite {
            label: "AttachCustomerManagedPolicyReferenceToPermissionSet attaches a reference",
            operation: "AttachCustomerManagedPolicyReferenceToPermissionSet",
            body: |instance_arn, permission_set_arn| {
                json!({
                    "InstanceArn": instance_arn,
                    "PermissionSetArn": permission_set_arn,
                    "CustomerManagedPolicyReference": {
                        "Name": "AttachedPolicy",
                        "Path": "/attachments/",
                    },
                })
            },
        },
        FacetWrite {
            label: "PutInlinePolicyToPermissionSet stores the inline policy",
            operation: "PutInlinePolicyToPermissionSet",
            body: |instance_arn, permission_set_arn| {
                json!({
                    "InstanceArn": instance_arn,
                    "PermissionSetArn": permission_set_arn,
                    "InlinePolicy": "{\"Version\":\"2012-10-17\",\"Statement\":[]}",
                })
            },
        },
        FacetWrite {
            label: "TagResource tags the permission set",
            operation: "TagResource",
            body: |instance_arn, permission_set_arn| {
                json!({
                    "InstanceArn": instance_arn,
                    "ResourceArn": permission_set_arn,
                    "Tags": [{"Key": "facet", "Value": "tags"}],
                })
            },
        },
        FacetWrite {
            label: "ProvisionPermissionSet provisions it to every account holding a copy",
            operation: "ProvisionPermissionSet",
            body: |instance_arn, permission_set_arn| {
                json!({
                    "InstanceArn": instance_arn,
                    "PermissionSetArn": permission_set_arn,
                    // One of the two values `ProvisionTargetType` declares; this one names
                    // no `TargetId`.
                    "TargetType": "ALL_PROVISIONED_ACCOUNTS",
                })
            },
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

/// `GetPermissionsBoundaryForPermissionSet`, as `(status, PermissionsBoundary or Null)` on
/// success and `(status, error body)` otherwise.
async fn get_boundary(
    router: &MultiServiceRouter,
    instance_arn: &str,
    permission_set_arn: &str,
) -> (u16, Value) {
    let (status, body) = call(
        router,
        "GetPermissionsBoundaryForPermissionSet",
        &json!({
            "InstanceArn": instance_arn,
            "PermissionSetArn": permission_set_arn,
        }),
    )
    .await;
    if status != 200 {
        return (status, body);
    }
    (status, body["PermissionsBoundary"].clone())
}

/// Store `boundary` as the permission set's boundary, asserting the put was accepted.
async fn put_boundary(
    router: &MultiServiceRouter,
    case: &str,
    instance_arn: &str,
    permission_set_arn: &str,
    boundary: &Value,
) {
    let (status, body) = call(
        router,
        "PutPermissionsBoundaryToPermissionSet",
        &json!({
            "InstanceArn": instance_arn,
            "PermissionSetArn": permission_set_arn,
            "PermissionsBoundary": boundary,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: PutPermissionsBoundaryToPermissionSet expected 200, got {status} {body}"
    );
}

/// THE RULE: the boundary slot is independent of every other facet of the permission set —
/// each write of another facet is accepted and the get still reports the stored boundary,
/// for every kind of boundary and every facet.
#[tokio::test]
async fn the_reported_boundary_survives_a_write_to_every_other_facet() {
    let (router, instance_arn) = organization_router();

    for (index, boundary) in boundary_cases().into_iter().enumerate() {
        let permission_set_arn =
            create_permission_set(&router, &instance_arn, &format!("Facets{index}")).await;
        put_boundary(
            &router,
            boundary.label,
            &instance_arn,
            &permission_set_arn,
            &boundary.sent,
        )
        .await;

        for facet in other_facet_writes() {
            let case = format!("{} then {}", boundary.label, facet.label);
            let (status, body) = call(
                &router,
                facet.operation,
                &(facet.body)(&instance_arn, &permission_set_arn),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: {} expected 200, got {status} {body}",
                facet.operation
            );

            let (status, reported) = get_boundary(&router, &instance_arn, &permission_set_arn).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the get expected 200, got {status} {reported}"
            );
            assert_eq!(
                reported, boundary.expected,
                "case {case:?}: expected the stored boundary {}, got {reported}",
                boundary.expected
            );
        }
    }
}

/// THE RULE, other half: the slot lives and dies with the permission-set resource. Once the
/// permission set is deleted the get is a `ResourceNotFoundException` naming the ARN it was
/// asked for, and a permission set created afterwards — same name, new ARN — is a new
/// resource whose slot is empty.
#[tokio::test]
async fn a_deleted_permission_set_reports_no_boundary_and_its_successor_carries_none() {
    let (router, instance_arn) = organization_router();

    for (index, boundary) in boundary_cases().into_iter().enumerate() {
        let name = format!("Lifetime{index}");
        let permission_set_arn = create_permission_set(&router, &instance_arn, &name).await;
        put_boundary(
            &router,
            boundary.label,
            &instance_arn,
            &permission_set_arn,
            &boundary.sent,
        )
        .await;

        let (status, body) = call(
            &router,
            "DeletePermissionSet",
            &json!({"InstanceArn": instance_arn, "PermissionSetArn": permission_set_arn}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: DeletePermissionSet expected 200, got {status} {body}",
            boundary.label
        );

        // This service answers a refusal of the request with HTTP 400 whatever 4xx the
        // error shape declares, as its own error channel documents, so the status asserted
        // here is the one every other refusal of this crate carries.
        let (status, body) = get_boundary(&router, &instance_arn, &permission_set_arn).await;
        assert_eq!(
            status, 400,
            "case {:?}: a get of a deleted permission set expected 400, got {status} {body}",
            boundary.label
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {:?}: expected ResourceNotFoundException, got {body}",
            boundary.label
        );
        let message = body["Message"].as_str().unwrap_or_default();
        assert!(
            message.contains(&permission_set_arn),
            "case {:?}: the refusal must name the ARN asked for, got {body}",
            boundary.label
        );

        // The successor is a NEW resource: the same name mints another ARN, and nothing of
        // the deleted permission set's boundary slot carries over to it.
        let successor_arn = create_permission_set(&router, &instance_arn, &name).await;
        assert_ne!(
            successor_arn, permission_set_arn,
            "case {:?}: a re-created permission set must mint its own ARN, got {successor_arn}",
            boundary.label
        );
        let (status, reported) = get_boundary(&router, &instance_arn, &successor_arn).await;
        assert_eq!(
            status, 200,
            "case {:?}: the get of the successor expected 200, got {status} {reported}",
            boundary.label
        );
        assert_eq!(
            reported,
            Value::Null,
            "case {:?}: the successor must carry no boundary, got {reported}",
            boundary.label
        );
    }
}
