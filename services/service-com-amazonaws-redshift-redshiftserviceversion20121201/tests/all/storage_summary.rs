// SPDX-License-Identifier: CC0-1.0

//! `DescribeStorage`, end to end over the awsQuery wire.
//!
//! The RULE every case here asserts: an input-less, error-less read reports the
//! account's storage summary as the SUM of what the store holds — the snapshots'
//! backup sizes and the clusters' provisioned capacities — and is never refused,
//! because the operation has no input member to get wrong and models no error at
//! all.
//!
//! The table spans what the rule generalises over: how many clusters and
//! snapshots the account holds (none, one, several), and whether the sizes they
//! carry are whole or fractional. The sizes themselves are seeded straight into
//! the shared store, because no implemented operation mints a storage size yet —
//! which is exactly why an account whose records carry none sums to zero.

use std::time::Instant;

use service_com_amazonaws_redshift_redshiftserviceversion20121201::ops::types::Snapshot;
use service_com_amazonaws_redshift_redshiftserviceversion20121201::state;

use crate::cluster_lifecycle::Emulator;

/// One row of the summary table: the case name, the `(cluster identifier,
/// provisioned capacity)` pairs, the `(snapshot identifier, backup size)` pairs,
/// and the two sums the report must carry.
struct SummaryCase {
    case: &'static str,
    clusters: Vec<(&'static str, i64)>,
    snapshots: Vec<(&'static str, f64)>,
    expected_provisioned: f64,
    expected_backup: f64,
}

/// Give a stored cluster the provisioned capacity its record carries.
///
/// `Cluster$TotalStorageCapacityInMegaBytes` is a member no implemented
/// operation mints yet, so a case that needs a non-zero provisioned total writes
/// it into the same shared store the handlers read. The request path is
/// unchanged.
fn seed_cluster_capacity(emulator: &Emulator, identifier: &str, capacity: i64) {
    let store = state::service_state(&*emulator.engine).expect("the Redshift store");
    let mut store = store.lock();
    let position = store
        .cluster_position(identifier)
        .unwrap_or_else(|| panic!("the cluster {identifier} was created"));
    store.clusters[position]
        .cluster
        .total_storage_capacity_in_mega_bytes = Some(capacity);
}

/// Seed one snapshot carrying a backup size.
fn seed_snapshot(emulator: &Emulator, identifier: &str, backup_size: f64) {
    let store = state::service_state(&*emulator.engine).expect("the Redshift store");
    let mut store = store.lock();
    store.snapshots.push(state::StoredSnapshot {
        snapshot: Snapshot {
            snapshot_identifier: Some(identifier.to_string()),
            status: Some(state::STATUS_AVAILABLE.to_string()),
            snapshot_type: Some(state::SNAPSHOT_TYPE_MANUAL.to_string()),
            total_backup_size_in_mega_bytes: Some(backup_size),
            ..Default::default()
        },
        // Already past, so the record needs no clock-owed transition.
        available_at: Instant::now(),
        source: None,
        // A seeded record was taken of no cluster this store holds, so it holds
        // no copy of any cluster's databases either.
        data: Default::default(),
    });
}

/// The value of one element of the `DescribeStorage` report, parsed as the double
/// the model declares it — after checking it is rendered the way AWS renders a
/// Smithy double, with a fractional part even when the value is whole (the
/// recorded empty account answers `0.0`, not `0`).
fn reported(body: &str, member: &str) -> f64 {
    let text = crate::cluster_lifecycle::all_text(body, member);
    assert_eq!(
        text.len(),
        1,
        "expected exactly one <{member}> element, got {text:?} in body {body}"
    );
    assert!(
        text[0].contains('.'),
        "case {member}: expected a double rendered with a fractional part, got {:?}",
        text[0]
    );
    text[0]
        .parse::<f64>()
        .unwrap_or_else(|_| panic!("expected {member} to be a double, got {:?}", text[0]))
}

/// The wire form of the summary: AWS renders a Smithy double with a fractional
/// part, so an account with nothing provisioned reads `0.0` on both members —
/// never `0`, and never the negative zero an empty floating-point sum folds to.
#[tokio::test]
async fn an_empty_account_reports_zero_in_the_decimal_form_aws_renders() {
    let emulator = Emulator::new();
    let answer = emulator.call("DescribeStorage", &[]).await;
    assert_eq!(
        answer.status, 200,
        "case an empty account: expected 200, got {} with body {}",
        answer.status, answer.body
    );
    for member in [
        "TotalBackupSizeInMegaBytes",
        "TotalProvisionedStorageInMegaBytes",
    ] {
        let element = format!("<{member}>0.0</{member}>");
        assert!(
            answer.body.contains(&element),
            "case {member}: expected {element:?} in the report, got body {}",
            answer.body
        );
    }
}

#[tokio::test]
async fn the_storage_summary_is_the_sum_of_what_the_store_holds() {
    let cases = vec![
        SummaryCase {
            // The recorded empty-account case: nothing provisioned, nothing
            // backed up, and still a served report rather than a refusal.
            case: "an empty account",
            clusters: Vec::new(),
            snapshots: Vec::new(),
            expected_provisioned: 0.0,
            expected_backup: 0.0,
        },
        SummaryCase {
            case: "one cluster, no snapshots",
            clusters: vec![("storage-one", 160_000)],
            snapshots: Vec::new(),
            expected_provisioned: 160_000.0,
            expected_backup: 0.0,
        },
        SummaryCase {
            case: "no clusters, one snapshot",
            clusters: Vec::new(),
            snapshots: vec![("storage-snap-one", 2_048.0)],
            expected_provisioned: 0.0,
            expected_backup: 2_048.0,
        },
        SummaryCase {
            case: "several of each",
            clusters: vec![
                ("storage-a", 160_000),
                ("storage-b", 2_560_000),
                ("storage-c", 32_000_000),
            ],
            snapshots: vec![
                ("storage-snap-a", 1_024.0),
                ("storage-snap-b", 4_096.0),
            ],
            expected_provisioned: 34_720_000.0,
            expected_backup: 5_120.0,
        },
        SummaryCase {
            case: "fractional backup sizes",
            clusters: vec![("storage-fraction", 1)],
            snapshots: vec![
                ("storage-snap-half", 0.5),
                ("storage-snap-quarter", 1.25),
            ],
            expected_provisioned: 1.0,
            expected_backup: 1.75,
        },
        SummaryCase {
            case: "a cluster whose record carries no capacity contributes nothing",
            clusters: Vec::new(),
            snapshots: Vec::new(),
            expected_provisioned: 0.0,
            expected_backup: 0.0,
        },
    ];

    for case in cases {
        let emulator = Emulator::new();
        // The last row creates its cluster WITHOUT seeding a capacity, so an
        // account holding a cluster that reports none still sums to zero.
        if case.case == "a cluster whose record carries no capacity contributes nothing" {
            let created = emulator.create_minimal("storage-unsized").await;
            assert_eq!(
                created.status, 200,
                "case {}: expected the cluster created, got {} with body {}",
                case.case, created.status, created.body
            );
        }
        for (identifier, capacity) in &case.clusters {
            let created = emulator.create_minimal(identifier).await;
            assert_eq!(
                created.status, 200,
                "case {}: expected the cluster {identifier} created, got {} with body {}",
                case.case, created.status, created.body
            );
            seed_cluster_capacity(&emulator, identifier, *capacity);
        }
        for (identifier, backup_size) in &case.snapshots {
            seed_snapshot(&emulator, identifier, *backup_size);
        }

        let answer = emulator.call("DescribeStorage", &[]).await;
        assert_eq!(
            answer.status, 200,
            "case {}: expected 200, got {} with body {}",
            case.case, answer.status, answer.body
        );
        assert!(
            answer.error_code().is_none(),
            "case {}: expected no error envelope, got body {}",
            case.case,
            answer.body
        );
        let provisioned = reported(&answer.body, "TotalProvisionedStorageInMegaBytes");        assert_eq!(
            provisioned, case.expected_provisioned,
            "case {}: expected TotalProvisionedStorageInMegaBytes {}, got {provisioned}",
            case.case, case.expected_provisioned
        );
        let backup = reported(&answer.body, "TotalBackupSizeInMegaBytes");
        assert_eq!(
            backup, case.expected_backup,
            "case {}: expected TotalBackupSizeInMegaBytes {}, got {backup}",
            case.case, case.expected_backup
        );
    }
}

/// The report is a pure READ: two calls either side of nothing answer the same
/// summary, and a cluster that leaves the store takes its capacity with it.
#[tokio::test]
async fn the_storage_summary_changes_only_when_the_store_does() {
    let emulator = Emulator::new();
    let created = emulator.create_minimal("storage-stable").await;
    assert_eq!(
        created.status, 200,
        "case seed storage-stable: expected 200, got {} with body {}",
        created.status, created.body
    );
    seed_cluster_capacity(&emulator, "storage-stable", 160_000);

    let first = emulator.call("DescribeStorage", &[]).await;
    let second = emulator.call("DescribeStorage", &[]).await;
    assert_eq!(
        reported(&first.body, "TotalProvisionedStorageInMegaBytes"),
        160_000.0,
        "case first read: expected the seeded capacity, got body {}",
        first.body
    );
    assert_eq!(
        reported(&second.body, "TotalProvisionedStorageInMegaBytes"),
        reported(&first.body, "TotalProvisionedStorageInMegaBytes"),
        "case repeated read: expected the same summary, got bodies {} and {}",
        first.body,
        second.body
    );

    let deleted = emulator
        .call(
            "DeleteCluster",
            &[
                ("ClusterIdentifier", "storage-stable"),
                ("SkipFinalClusterSnapshot", "true"),
            ],
        )
        .await;
    assert_eq!(
        deleted.status, 200,
        "case delete storage-stable: expected 200, got {} with body {}",
        deleted.status, deleted.body
    );
    let after = emulator.call("DescribeStorage", &[]).await;
    assert_eq!(
        reported(&after.body, "TotalProvisionedStorageInMegaBytes"),
        0.0,
        "case after the delete: expected the capacity gone, got body {}",
        after.body
    );
}
