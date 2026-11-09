// SPDX-License-Identifier: CC0-1.0

//! What a DELETE of a snapshot leaves of the fast snapshot restores enabled on it, driven
//! over both wires the one snapshot resource is reached through on one engine: `ec2Query` for
//! EC2 ([`super::ec2_query`]) and restJson1 for the EBS direct APIs
//! ([`super::ebs_direct_api_snapshots`]).
//!
//! The rule under test: a fast snapshot restore is a property of a (snapshot, Availability
//! Zone) PAIR — "enables fast snapshot restores for the specified snapshots in the specified
//! Availability Zones" (`EnableFastSnapshotRestores`) — so a snapshot the account no longer
//! holds has none, and `DescribeFastSnapshotRestores` reports no pair naming a deleted
//! snapshot. That holds whichever API minted the snapshot: the pairs are kept in THIS
//! service's store even for a snapshot the EBS direct APIs hold the record of, which is the
//! case a delete has to apply the rule to itself.
//!
//! A DISABLE is the contrast that makes the rule a rule rather than a side effect of any
//! call: `FastSnapshotRestoreStateCode` has a word for a pair whose restore was turned off
//! (`disabled`), so a disabled pair is still reported — only a deleted snapshot's pairs are
//! gone.
//!
//! Each row is a distinct input along what the rule generalises over — which API minted the
//! snapshot, how many zones the restore was enabled in, and whether the snapshot was deleted
//! or merely disabled — so no row is the specification.

use smithy_server_runtime::MultiServiceRouter;

use super::ebs_direct_api_snapshots::{complete_snapshot, emulator, put_block, start_snapshot};
use super::ec2_query::{call, element, elements, error_code};
use super::volume_lifecycle::{REGION, p, zone_name};

/// Which API minted the snapshot a row enables a fast snapshot restore on.
#[derive(Debug, Clone, Copy)]
enum Minted {
    /// `ec2:CreateSnapshot`, of a volume of the account.
    ByEc2,
    /// `ebs:StartSnapshot`, sealed by `ebs:CompleteSnapshot` once its blocks are written.
    ByDirectApi { blocks: i32 },
}

/// One `action` call signed for the Region the snapshot suites work in.
async fn op(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(String, String)],
) -> (u16, String) {
    call(router, REGION, action, params).await
}

/// Mint the snapshot a row works on, through the API the row names, and answer its id.
async fn mint_snapshot(router: &MultiServiceRouter, case: &str, minted: Minted) -> String {
    match minted {
        Minted::ByEc2 => {
            let (status, body) = op(
                router,
                "CreateVolume",
                &[p("AvailabilityZone", zone_name(0)), p("Size", "1")],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 from CreateVolume, got {status} with body \
                 {body}"
            );
            let volume_id = element(&body, "volumeId")
                .unwrap_or_else(|| panic!("case {case:?}: no volumeId in {body}"));
            let (status, body) = op(router, "CreateSnapshot", &[p("VolumeId", &volume_id)]).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 from CreateSnapshot, got {status} with body \
                 {body}"
            );
            element(&body, "snapshotId")
                .unwrap_or_else(|| panic!("case {case:?}: no snapshotId in {body}"))
        }
        Minted::ByDirectApi { blocks } => {
            let snapshot_id =
                start_snapshot(router, case, serde_json::json!({ "VolumeSize": 1 })).await;
            for index in 0..blocks {
                put_block(router, case, &snapshot_id, index).await;
            }
            complete_snapshot(router, case, &snapshot_id, blocks).await;
            snapshot_id
        }
    }
}

/// `EnableFastSnapshotRestores` for `snapshot_id` in the Region's first `zones` zones.
async fn enable(router: &MultiServiceRouter, case: &str, snapshot_id: &str, zones: usize) {
    let mut params = vec![p("SourceSnapshotId.1", snapshot_id)];
    for zone in 0..zones {
        params.push(p(&format!("AvailabilityZone.{}", zone + 1), zone_name(zone)));
    }
    let (status, body) = op(router, "EnableFastSnapshotRestores", &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from EnableFastSnapshotRestores, got {status} with \
         body {body}"
    );
    let enabled = elements(&body, "snapshotId");
    assert_eq!(
        enabled,
        vec![snapshot_id.to_string(); zones],
        "case {case:?}: expected one successful pair per zone for {snapshot_id}, got {enabled:?}"
    );
}

/// `DisableFastSnapshotRestores` for `snapshot_id` in the Region's first `zones` zones.
async fn disable(router: &MultiServiceRouter, case: &str, snapshot_id: &str, zones: usize) {
    let mut params = vec![p("SourceSnapshotId.1", snapshot_id)];
    for zone in 0..zones {
        params.push(p(&format!("AvailabilityZone.{}", zone + 1), zone_name(zone)));
    }
    let (status, body) = op(router, "DisableFastSnapshotRestores", &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DisableFastSnapshotRestores, got {status} with \
         body {body}"
    );
}

/// The snapshot ids `DescribeFastSnapshotRestores` reports a pair for, in report order.
async fn pair_snapshot_ids(router: &MultiServiceRouter, case: &str) -> Vec<String> {
    let (status, body) = op(router, "DescribeFastSnapshotRestores", &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeFastSnapshotRestores, got {status} with \
         body {body}"
    );
    elements(&body, "snapshotId")
}

/// `DeleteSnapshot` of `snapshot_id`, requiring the success the envelope reports for a
/// mutation with no result of its own.
async fn delete(router: &MultiServiceRouter, case: &str, snapshot_id: &str) {
    let (status, body) = op(router, "DeleteSnapshot", &[p("SnapshotId", snapshot_id)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 deleting {snapshot_id}, got {status} with body {body}"
    );
    assert_eq!(
        element(&body, "return").as_deref(),
        Some("true"),
        "case {case:?}: a delete that reports no result of its own carries the envelope's \
         success flag, got {body}"
    );
}

/// THE RULE: every pair of a deleted snapshot is gone, whichever API minted the snapshot and
/// however many zones its restore covered — while the pairs of the snapshot beside it stay.
#[tokio::test]
async fn deleting_a_snapshot_takes_its_fast_snapshot_restores_with_it() {
    // (case, how the snapshot is minted, how many zones the restore covers).
    let rows: [(&str, Minted, usize); 4] = [
        ("an EC2 snapshot enabled in one zone", Minted::ByEc2, 1),
        ("an EC2 snapshot enabled in two zones", Minted::ByEc2, 2),
        (
            "a sealed direct-API snapshot enabled in one zone",
            Minted::ByDirectApi { blocks: 1 },
            1,
        ),
        (
            "a sealed direct-API snapshot of no block enabled in three zones",
            Minted::ByDirectApi { blocks: 0 },
            3,
        ),
    ];
    for (case, minted, zones) in rows {
        let (router, _engine) = emulator();
        // The snapshot no row deletes, with a restore of its own: its pair must survive, so a
        // delete that dropped every pair cannot pass.
        let bystander = mint_snapshot(&router, case, Minted::ByEc2).await;
        enable(&router, case, &bystander, 1).await;

        let snapshot_id = mint_snapshot(&router, case, minted).await;
        enable(&router, case, &snapshot_id, zones).await;
        let reported = pair_snapshot_ids(&router, case).await;
        assert_eq!(
            reported
                .iter()
                .filter(|id| *id == &snapshot_id)
                .count(),
            zones,
            "case {case:?}: expected {zones} pair(s) of {snapshot_id} before the delete, got \
             {reported:?}"
        );

        delete(&router, case, &snapshot_id).await;

        let reported = pair_snapshot_ids(&router, case).await;
        assert!(
            !reported.contains(&snapshot_id),
            "case {case:?}: expected no fast snapshot restore pair for the deleted \
             {snapshot_id}, got {reported:?}"
        );
        assert!(
            reported.contains(&bystander),
            "case {case:?}: expected the pair of {bystander} to survive the delete of \
             {snapshot_id}, got {reported:?}"
        );
    }
}

/// THE CONTRAST: a DISABLE keeps the pair, because `disabled` is a state the pair can be in —
/// so the rule above is about the snapshot being gone and not about any call that touches it.
#[tokio::test]
async fn disabling_a_fast_snapshot_restore_keeps_the_pair_a_delete_removes() {
    for (case, minted) in [
        ("an EC2 snapshot", Minted::ByEc2),
        (
            "a sealed direct-API snapshot",
            Minted::ByDirectApi { blocks: 1 },
        ),
    ] {
        let (router, _engine) = emulator();
        let snapshot_id = mint_snapshot(&router, case, minted).await;
        enable(&router, case, &snapshot_id, 1).await;
        disable(&router, case, &snapshot_id, 1).await;

        let reported = pair_snapshot_ids(&router, case).await;
        assert!(
            reported.contains(&snapshot_id),
            "case {case:?}: a disabled pair of {snapshot_id} is still reported, got {reported:?}"
        );

        // And the snapshot is still the account's, so the pair above outlived the disable
        // rather than the snapshot.
        let (status, body) = op(
            &router,
            "DescribeSnapshots",
            &[p("SnapshotId.1", &snapshot_id)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected {snapshot_id} to still be readable, got {status} with body \
             {body}"
        );

        delete(&router, case, &snapshot_id).await;
        let reported = pair_snapshot_ids(&router, case).await;
        assert!(
            !reported.contains(&snapshot_id),
            "case {case:?}: expected the disabled pair of the deleted {snapshot_id} to be gone \
             too, got {reported:?}"
        );
        // The snapshot is gone with it: the delete did happen.
        let (status, body) = op(
            &router,
            "DescribeSnapshots",
            &[p("SnapshotId.1", &snapshot_id)],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 reading the deleted {snapshot_id}, got {status} \
             with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidSnapshot.NotFound",
            "case {case:?}: expected InvalidSnapshot.NotFound for the deleted {snapshot_id}, \
             got {body}"
        );
    }
}
