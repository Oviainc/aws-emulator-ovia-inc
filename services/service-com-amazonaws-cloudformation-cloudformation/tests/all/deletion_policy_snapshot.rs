// SPDX-License-Identifier: CC0-1.0

//! `DeletionPolicy` on a resource whose owning service takes its snapshot as a CALL OF ITS
//! OWN: what a stack's deletion leaves behind in that service.
//!
//! RULE: "With the `DeletionPolicy` attribute you can preserve, and in some cases, backup a
//! resource when its stack is deleted", and for each of the values the attribute takes:
//!
//! * `Snapshot` — "for resources that support snapshots, CloudFormation creates a snapshot
//!   for the resource before deleting it": the resource is gone and EXACTLY ONE snapshot of
//!   it is left behind, held by the service that owns the resource and reported by that
//!   service's own reads;
//! * `Delete` — and a declaration that states no policy, which is the documented default for
//!   a resource of this kind — "CloudFormation deletes the resource and all its content":
//!   the resource is gone and no snapshot of it exists;
//! * `Retain` — "CloudFormation keeps the resource without deleting the resource or its
//!   contents": the resource is still there afterwards, and no snapshot was taken.
//!
//! `AWS::EC2::Volume` is one of the published snapshot-supporting types whose snapshot is NOT
//! a member of its delete call: `ec2:DeleteVolume` takes only `VolumeId`, so the snapshot is
//! an `ec2:CreateSnapshot` of the volume and the volume's size and encryption are read off
//! the volume itself. Every case therefore reads the outcome back through Amazon EC2's own
//! wire — `DescribeSnapshots` filtered on the source volume and `DescribeVolumes` on the
//! volume id — so what it asserts is what a client sees, and no assertion is made against
//! this crate's own store.
//!
//! The rows span the whole catalog of the attribute's values, on the same declaration, and
//! the snapshot row is asserted on both halves of the policy: the data that survives and the
//! resource that does not.

use serde_json::json;
use service_com_amazonaws_cloudformation_cloudformation::state::status;
use smithy_server_runtime::MultiServiceRouter;

use crate::stack_lifecycle::{
    call, create_stack, ec2, member, router, wait_for_the_transition, xml_text, xml_texts,
    REGION,
};
use crate::update_rollback_recovery::status_of;

/// The size the volume every row declares asks for, in GiB — which is what its snapshot
/// reports as `volumeSize`.
const VOLUME_SIZE: &str = "1";

/// One row: the `DeletionPolicy` the declaration carries, and what the stack's deletion has
/// to leave behind in Amazon EC2.
struct DeletionPolicyCase {
    case: &'static str,
    stack: &'static str,
    /// The attribute the declaration carries, or `None` for one that states none.
    policy: Option<&'static str>,
    /// How many snapshots of the volume the deletion must leave.
    snapshots: usize,
    /// Whether the volume itself is still there afterwards.
    keeps_the_volume: bool,
}

/// The class the rule covers: the whole catalog of `DeletionPolicy` values a declaration can
/// carry, from the attribute's own documentation.
fn deletion_policy_cases() -> Vec<DeletionPolicyCase> {
    vec![
        DeletionPolicyCase {
            case: "DeletionPolicy: Snapshot",
            stack: "policy-snapshot",
            policy: Some("Snapshot"),
            snapshots: 1,
            keeps_the_volume: false,
        },
        DeletionPolicyCase {
            case: "DeletionPolicy: Delete",
            stack: "policy-delete",
            policy: Some("Delete"),
            snapshots: 0,
            keeps_the_volume: false,
        },
        DeletionPolicyCase {
            case: "no DeletionPolicy at all, which is the documented default",
            stack: "policy-absent",
            policy: None,
            snapshots: 0,
            keeps_the_volume: false,
        },
        DeletionPolicyCase {
            case: "DeletionPolicy: Retain",
            stack: "policy-retain",
            policy: Some("Retain"),
            snapshots: 0,
            keeps_the_volume: true,
        },
    ]
}

/// The template a row deploys: one EBS volume, with the row's policy where it states one.
fn template(policy: Option<&str>) -> String {
    let mut volume = json!({
        "Type": "AWS::EC2::Volume",
        "Properties": {
            "AvailabilityZone": format!("{REGION}a"),
            "Size": VOLUME_SIZE,
            "VolumeType": "gp3",
        },
    });
    if let Some(policy) = policy {
        volume["DeletionPolicy"] = json!(policy);
    }
    json!({"Resources": {"Vol": volume}}).to_string()
}

/// The `PhysicalResourceId` a stack reports for one of its resources.
async fn physical_id(router: &MultiServiceRouter, case: &str, stack: &str) -> String {
    let (status_code, body) = call(
        router,
        "DescribeStackResource",
        &[
            member("StackName", stack),
            member("LogicalResourceId", "Vol"),
        ],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: expected DescribeStackResource to succeed, got {status_code} {body}"
    );
    xml_text(&body, "PhysicalResourceId").unwrap_or_default()
}

/// The snapshots Amazon EC2 holds OF one volume, as `(snapshotId, volumeId, volumeSize)` —
/// read through the filtered listing a client uses, so the answer is that service's own.
async fn snapshots_of(
    router: &MultiServiceRouter,
    case: &str,
    volume_id: &str,
) -> Vec<(String, String, String)> {
    let (status_code, body) = ec2(
        router,
        "DescribeSnapshots",
        &[
            ("OwnerId.1", "self"),
            ("Filter.1.Name", "volume-id"),
            ("Filter.1.Value.1", volume_id),
        ],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: expected DescribeSnapshots to succeed, got {status_code} {body}"
    );
    let ids = xml_texts(&body, "snapshotId");
    let volumes = xml_texts(&body, "volumeId");
    let sizes = xml_texts(&body, "volumeSize");
    ids.into_iter()
        .zip(volumes)
        .zip(sizes)
        .map(|((id, volume), size)| (id, volume, size))
        .collect()
}

#[tokio::test]
async fn a_stack_deletion_leaves_the_snapshot_its_deletion_policy_asks_for() {
    for row in deletion_policy_cases() {
        let case = row.case;
        let router = router();
        let body_of_the_template = template(row.policy);
        let (status_code, body) =
            create_stack(&router, row.stack, &body_of_the_template, &[]).await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected CreateStack to succeed, got {status_code} {body}"
        );
        wait_for_the_transition().await;
        let created = status_of(&router, row.stack).await;
        assert_eq!(
            created,
            status::CREATE_COMPLETE,
            "case {case:?}: expected the create to complete, got {created:?}"
        );
        let volume_id = physical_id(&router, case, row.stack).await;
        assert!(
            volume_id.starts_with("vol-"),
            "case {case:?}: expected the stack to report the volume EC2 minted, got \
             {volume_id:?}"
        );
        // The baseline: no snapshot of the volume exists before the deletion, whatever the
        // policy says — the snapshot is the DELETION's, not the create's.
        let before = snapshots_of(&router, case, &volume_id).await;
        assert!(
            before.is_empty(),
            "case {case:?}: expected no snapshot of {volume_id} before the deletion, got \
             {before:?}"
        );

        let (status_code, body) =
            call(&router, "DeleteStack", &[member("StackName", row.stack)]).await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected DeleteStack to succeed, got {status_code} {body}"
        );
        wait_for_the_transition().await;

        // What the deletion left in the service that OWNS the data: the snapshots the policy
        // asked for, and no more than those.
        let after = snapshots_of(&router, case, &volume_id).await;
        assert_eq!(
            after.len(),
            row.snapshots,
            "case {case:?}: expected the deletion to leave {} snapshot(s) of {volume_id}, got \
             {after:?}",
            row.snapshots
        );
        for (snapshot_id, snapshot_volume, snapshot_size) in &after {
            assert!(
                snapshot_id.starts_with("snap-"),
                "case {case:?}: expected the snapshot to carry the id EC2 minted, got \
                 {snapshot_id:?}"
            );
            assert_eq!(
                snapshot_volume, &volume_id,
                "case {case:?}: expected the snapshot to name the volume it was taken of, got \
                 {snapshot_volume:?}"
            );
            assert_eq!(
                snapshot_size, VOLUME_SIZE,
                "case {case:?}: expected the snapshot to report the volume's own size, got \
                 {snapshot_size:?}"
            );
            // ...and it is addressable by its own id, which is what a restore flow reads it
            // back with.
            let (status_code, body) =
                ec2(&router, "DescribeSnapshots", &[("SnapshotId.1", snapshot_id)]).await;
            assert_eq!(
                status_code, 200,
                "case {case:?}: expected the snapshot to be addressable by its id, got \
                 {status_code} {body}"
            );
            let reported = xml_text(&body, "volumeId").unwrap_or_default();
            assert_eq!(
                &reported, &volume_id,
                "case {case:?}: expected the snapshot read by id to name the same volume, got \
                 {reported:?}"
            );
        }

        // ...and what it left of the VOLUME: `Retain` keeps it, every other policy takes it
        // away.
        let (status_code, body) = ec2(&router, "DescribeVolumes", &[("VolumeId.1", &volume_id)])
            .await;
        if row.keeps_the_volume {
            assert_eq!(
                status_code, 200,
                "case {case:?}: expected the retained volume to still be held by EC2, got \
                 {status_code} {body}"
            );
        } else {
            assert_eq!(
                status_code, 400,
                "case {case:?}: expected EC2 to hold no volume for {volume_id} after the \
                 deletion, got {status_code} {body}"
            );
            let code = xml_text(&body, "Code").unwrap_or_default();
            assert_eq!(
                code, "InvalidVolume.NotFound",
                "case {case:?}: expected the deleted volume to be refused as not found, got \
                 {code:?} {body}"
            );
        }
    }
}
