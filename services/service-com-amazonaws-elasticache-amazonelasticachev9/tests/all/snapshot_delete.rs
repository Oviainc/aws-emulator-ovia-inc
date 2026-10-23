// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DeleteSnapshot`: "Deletes an existing snapshot. When
//! you receive a successful response from this operation, ElastiCache
//! immediately begins deleting the snapshot; you cannot cancel or revert this
//! operation."
//!
//! Every assertion goes over the wire through the shared [`crate::harness`]: an
//! `awsQuery` form-urlencoded request is dispatched through the production
//! router and only the HTTP status and the XML body are inspected. Nothing
//! reaches into the service's state struct, so a response that is not computed
//! from persisted state cannot pass.
//!
//! The rule under test: a delete names ONE stored snapshot — resolved by the
//! lowercased name every snapshot is stored under — answers with that stored
//! snapshot now in `deleting`, leaves every other snapshot where it is, and lets
//! the CLOCK carry the removal through, so a read inside the span still reports
//! the snapshot going away and a read past it does not report it at all; a name
//! no stored snapshot carries is refused `SnapshotNotFoundFault` and a snapshot
//! that is not `available` is refused `InvalidSnapshotStateFault`, neither
//! changing anything. The tests are tables whose rows differ along the source
//! cluster's configuration, how the snapshot came about (taken or copied), the
//! case the name is sent in, and which snapshot of a set is the one deleted.

use service_com_amazonaws_elasticache_amazonelasticachev9::state;

use crate::cache_cluster_lifecycle::create_available_cluster;
use crate::harness::{params, Emulator, RefusalCase, Reply, PAST_TRANSITION};
use crate::snapshots::{create_snapshot, describe_snapshots, reported_names};

/// Every `Snapshot` member that describes WHAT the snapshot captured, so the
/// delete's answer has to report exactly what the stored snapshot reported
/// before it: the delete returns the record, not a freshly built shape.
/// `SnapshotStatus` is deliberately absent — that is the one member the delete
/// moves.
const SAME_AS_STORED: [&str; 13] = [
    "Snapshot/SnapshotName",
    "Snapshot/SnapshotSource",
    "Snapshot/CacheClusterId",
    "Snapshot/Engine",
    "Snapshot/EngineVersion",
    "Snapshot/CacheNodeType",
    "Snapshot/NumCacheNodes",
    "Snapshot/Port",
    "Snapshot/CacheParameterGroupName",
    "Snapshot/PreferredMaintenanceWindow",
    "Snapshot/CacheClusterCreateTime",
    "Snapshot/SnapshotRetentionLimit",
    "Snapshot/ARN",
];

/// One snapshot to take before the delete: its `SnapshotName` and the form
/// pairs it is taken with.
type SnapshotSpec = (&'static str, &'static [(&'static str, &'static str)]);

/// Delete the snapshot `name` names.
async fn delete_snapshot(emulator: &Emulator, name: &str) -> Reply {
    emulator
        .call("DeleteSnapshot", &params(&[("SnapshotName", name)]))
        .await
}

/// Copy `source` to `target`, so a row can delete a snapshot that came about as
/// a COPY rather than as a take.
async fn copy_snapshot(emulator: &Emulator, source: &str, target: &str) -> Reply {
    emulator
        .call(
            "CopySnapshot",
            &params(&[
                ("SourceSnapshotName", source),
                ("TargetSnapshotName", target),
            ]),
        )
        .await
}

/// The events reported for one source, as `(SourceType, Message)` pairs.
async fn events_of(emulator: &Emulator, source: &str) -> Vec<(String, String)> {
    let reply = emulator
        .call("DescribeEvents", &params(&[("SourceIdentifier", source)]))
        .await;
    assert_eq!(
        reply.status, 200,
        "case {source}: DescribeEvents must answer 200, got {} {}",
        reply.status, reply.body
    );
    let types = reply.all("Event/SourceType");
    let messages = reply.all("Event/Message");
    assert_eq!(
        types.len(),
        messages.len(),
        "case {source}: every event must carry a source type and a message, got {}",
        reply.body
    );
    types
        .into_iter()
        .map(str::to_string)
        .zip(messages.into_iter().map(str::to_string))
        .collect()
}

/// One row of the delete table.
#[derive(Debug, Clone)]
struct DeleteCase {
    case: &'static str,
    cluster: &'static str,
    /// The form pairs the source cluster is created with.
    cluster_extra: &'static [(&'static str, &'static str)],
    /// The snapshots taken of that cluster before the delete.
    taken: &'static [SnapshotSpec],
    /// A copy made before the delete, as `(source, target)`.
    copy: Option<(&'static str, &'static str)>,
    /// The `SnapshotName` the delete is asked for, in the case it is sent.
    requested: &'static str,
    /// The identity it resolves to: a snapshot name "is stored as a lowercase
    /// string".
    stored: &'static str,
    /// The snapshots still stored once the removal has fallen due, sorted.
    left: &'static [&'static str],
}

#[tokio::test]
async fn a_deleted_snapshot_answers_deleting_and_the_clock_removes_it() {
    // The axis: the source cluster's configuration (engine and its documented
    // defaults, port, backup settings), how the deleted snapshot came about
    // (taken or copied), the case its name is sent in, and which snapshot of a
    // stored set is the one named.
    let cases = [
        DeleteCase {
            case: "a redis snapshot of a cluster with the documented defaults",
            cluster: "del-redis",
            cluster_extra: &[("CacheNodeType", "cache.t3.micro")],
            taken: &[("redis-snap", &[]), ("redis-keep", &[])],
            copy: None,
            requested: "redis-snap",
            stored: "redis-snap",
            left: &["redis-keep"],
        },
        DeleteCase {
            case: "a valkey snapshot with a KMS key, deleted under a mixed-case name",
            cluster: "del-valkey",
            cluster_extra: &[
                ("Engine", "valkey"),
                ("Port", "7000"),
                ("SnapshotRetentionLimit", "5"),
                ("SnapshotWindow", "05:00-09:00"),
            ],
            taken: &[
                ("valkey-snap", &[("KmsKeyId", "alias/aws-elasticache")]),
                ("valkey-keep", &[]),
            ],
            copy: None,
            requested: "Valkey-SNAP",
            stored: "valkey-snap",
            left: &["valkey-keep"],
        },
        DeleteCase {
            case: "a snapshot that is itself a copy: deleting it leaves its source",
            cluster: "del-copy",
            cluster_extra: &[("EngineVersion", "6.2"), ("SnapshotRetentionLimit", "1")],
            taken: &[("origin-snap", &[])],
            copy: Some(("origin-snap", "origin-copy")),
            requested: "origin-copy",
            stored: "origin-copy",
            left: &["origin-snap"],
        },
        DeleteCase {
            case: "the source of a copy: deleting it leaves the copy",
            cluster: "del-source",
            cluster_extra: &[("Engine", "valkey")],
            taken: &[("kept-origin", &[])],
            copy: Some(("kept-origin", "kept-copy")),
            requested: "kept-origin",
            stored: "kept-origin",
            left: &["kept-copy"],
        },
    ];

    for case in cases {
        let emulator = Emulator::new();
        create_available_cluster(&emulator, case.cluster, case.cluster_extra).await;
        for (name, extra) in case.taken {
            let created = create_snapshot(&emulator, name, case.cluster, extra).await;
            assert_eq!(
                created.status, 200,
                "case {}: taking {name} must answer 200, got {} {}",
                case.case, created.status, created.body
            );
            // A cluster is `snapshotting` until the clock owes it `available`
            // again, so the next take waits for it.
            std::thread::sleep(PAST_TRANSITION);
        }
        if let Some((source, target)) = case.copy {
            let copied = copy_snapshot(&emulator, source, target).await;
            assert_eq!(
                copied.status, 200,
                "case {}: copying {source} to {target} must answer 200, got {} {}",
                case.case, copied.status, copied.body
            );
            std::thread::sleep(PAST_TRANSITION);
        }

        // The stored snapshot as it stands before the delete: every value the
        // delete's answer must report is read off THIS, never hand-written.
        let before = describe_snapshots(&emulator, &[("SnapshotName", case.stored)]).await;
        assert_eq!(
            before.text("Snapshot/SnapshotStatus"),
            Some("available"),
            "case {}: the snapshot must be available before it is deleted, got {}",
            case.case,
            before.body
        );

        let started = std::time::Instant::now();
        let deleted = delete_snapshot(&emulator, case.requested).await;
        assert_eq!(
            deleted.status, 200,
            "case {}: expected HTTP 200, got {} {}",
            case.case, deleted.status, deleted.body
        );
        // The snapshot does not vanish at the call: it reports the status
        // ElastiCache answers while it "begins deleting the snapshot".
        assert_eq!(
            deleted.text("Snapshot/SnapshotStatus"),
            Some("deleting"),
            "case {}: a just-deleted snapshot must report deleting, got {}",
            case.case,
            deleted.body
        );
        // ... and everything it captured is still what the stored record held.
        for element in SAME_AS_STORED {
            assert_eq!(
                deleted.all(element),
                before.all(element),
                "case {}: expected {element} = {:?} (the stored snapshot's), got {}",
                case.case,
                before.all(element),
                deleted.body
            );
        }

        // A read inside the span still finds it, going away. Guarded by the
        // clock so a slow machine cannot turn the span into a failure: the read
        // below happened no later than `started.elapsed()` after the delete.
        let during = describe_snapshots(&emulator, &[("SnapshotName", case.stored)]).await;
        if started.elapsed() < state::SNAPSHOT_TRANSITION {
            assert_eq!(
                during.text("Snapshot/SnapshotStatus"),
                Some("deleting"),
                "case {}: inside the removal span the snapshot must still be reported as \
                 deleting, got {}",
                case.case,
                during.body
            );
        }

        // Past the span the clock has carried the removal through.
        std::thread::sleep(PAST_TRANSITION);
        let after = describe_snapshots(&emulator, &[("SnapshotName", case.stored)]).await;
        assert_eq!(
            after.status, 200,
            "case {}: DescribeSnapshots must answer 200, got {} {}",
            case.case, after.status, after.body
        );
        assert!(
            reported_names(&after).is_empty(),
            "case {}: expected no snapshot under {}, got {}",
            case.case,
            case.stored,
            after.body
        );
        // Only the named snapshot went: every other stored snapshot is still
        // reported.
        let mut expected: Vec<&str> = case.left.to_vec();
        expected.sort_unstable();
        let all = describe_snapshots(&emulator, &[]).await;
        assert_eq!(
            reported_names(&all),
            expected,
            "case {}: expected {expected:?} to be left, got {}",
            case.case,
            all.body
        );

        // The deletion is one of the changes this service records, under the
        // cluster the snapshot was taken from.
        let events = events_of(&emulator, case.cluster).await;
        let expected_event = (
            "cache-cluster".to_string(),
            format!("Snapshot {} deleted", case.stored),
        );
        assert!(
            events.contains(&expected_event),
            "case {}: expected the event {expected_event:?} to be logged, got {events:?}",
            case.case
        );
    }
}

#[tokio::test]
async fn a_delete_the_rule_forbids_is_refused_and_persists_nothing() {
    let emulator = Emulator::new();
    create_available_cluster(&emulator, "refused-del", &[]).await;
    for name in ["held-snap", "second-snap"] {
        let created = create_snapshot(&emulator, name, "refused-del", &[]).await;
        assert_eq!(
            created.status, 200,
            "case {name}: the snapshot must be created, got {} {}",
            created.status, created.body
        );
        std::thread::sleep(PAST_TRANSITION);
    }

    // Each row: the form pairs that make the request, and the modelled refusal.
    // The axis is the reason the delete cannot be made — every row names a
    // snapshot the store does not hold, in a different form.
    let cases: [RefusalCase; 4] = [
        (
            "a snapshot the store does not hold",
            &[("SnapshotName", "no-such-snap")],
            404,
            "SnapshotNotFoundFault",
        ),
        (
            "a name whose lowercased form the store does not hold either",
            &[("SnapshotName", "NO-SUCH-SNAP")],
            404,
            "SnapshotNotFoundFault",
        ),
        (
            "a name no snapshot of this service could carry",
            &[("SnapshotName", "not.a.snapshot.name")],
            404,
            "SnapshotNotFoundFault",
        ),
        (
            "a name that differs from a stored one only by a suffix",
            &[("SnapshotName", "held-snap-2")],
            404,
            "SnapshotNotFoundFault",
        ),
    ];

    for (case, pairs, status, code) in cases {
        let reply = emulator.call("DeleteSnapshot", &params(pairs)).await;
        assert_eq!(
            reply.status, status,
            "case {case}: expected HTTP {status}, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            code,
            "case {case}: expected error code {code}, got {}",
            reply.body
        );
        // The fault names the snapshot the request asked for, as it was sent.
        let requested = pairs
            .iter()
            .find(|(key, _)| *key == "SnapshotName")
            .map(|(_, value)| *value)
            .unwrap_or_default();
        assert!(
            reply.error_message().contains(requested),
            "case {case}: expected the message to name {requested:?}, got {:?}",
            reply.error_message()
        );
    }

    // Not one refusal changed the store.
    let all = describe_snapshots(&emulator, &[]).await;
    assert_eq!(
        reported_names(&all),
        vec!["held-snap", "second-snap"],
        "a refused delete must persist nothing, got {}",
        all.body
    );

    // A snapshot that is not settled yet is not deletable: this row is timed
    // rather than tabled, because it has to run inside the span the create
    // opened.
    let creating = create_snapshot(&emulator, "creating-snap", "refused-del", &[]).await;
    assert_eq!(
        creating.status, 200,
        "case a snapshot still creating: the create must succeed, got {}",
        creating.body
    );
    let too_early = delete_snapshot(&emulator, "creating-snap").await;
    assert_eq!(
        too_early.status, 400,
        "case a snapshot still creating: expected HTTP 400, got {} {}",
        too_early.status, too_early.body
    );
    assert_eq!(
        too_early.error_code(),
        "InvalidSnapshotState",
        "case a snapshot still creating: expected InvalidSnapshotState, got {}",
        too_early.body
    );

    // ... and once the clock owes it `available`, the same delete is accepted.
    std::thread::sleep(PAST_TRANSITION);
    let started = std::time::Instant::now();
    let accepted = delete_snapshot(&emulator, "creating-snap").await;
    assert_eq!(
        accepted.status, 200,
        "case a settled snapshot: expected HTTP 200, got {} {}",
        accepted.status, accepted.body
    );

    // A second delete inside the span is refused: a snapshot already going away
    // is not deleted twice. Guarded by the clock, so a slow machine reads the
    // removed-record row below instead of failing here.
    let second = delete_snapshot(&emulator, "creating-snap").await;
    if started.elapsed() < state::SNAPSHOT_TRANSITION {
        assert_eq!(
            second.status, 400,
            "case a second delete inside the span: expected HTTP 400, got {} {}",
            second.status, second.body
        );
        assert_eq!(
            second.error_code(),
            "InvalidSnapshotState",
            "case a second delete inside the span: expected InvalidSnapshotState, got {}",
            second.body
        );
    }

    // Past the span the record is gone, so the same call is a not-found.
    std::thread::sleep(PAST_TRANSITION);
    let gone = delete_snapshot(&emulator, "creating-snap").await;
    assert_eq!(
        gone.status, 404,
        "case a snapshot the clock has removed: expected HTTP 404, got {} {}",
        gone.status, gone.body
    );
    assert_eq!(
        gone.error_code(),
        "SnapshotNotFoundFault",
        "case a snapshot the clock has removed: expected SnapshotNotFoundFault, got {}",
        gone.body
    );

    // The two snapshots no call deleted are still stored.
    let all = describe_snapshots(&emulator, &[]).await;
    assert_eq!(
        reported_names(&all),
        vec!["held-snap", "second-snap"],
        "only the deleted snapshot may be gone, got {}",
        all.body
    );
}

#[tokio::test]
async fn a_removed_snapshots_name_is_free_again() {
    // The rule the removal has to be REAL for: the record is gone, so neither
    // the name it held nor the quota slot it took is held any more, while a
    // snapshot that is only `deleting` still holds both.
    let emulator = Emulator::new();
    create_available_cluster(&emulator, "reuse-cluster", &[("Engine", "valkey")]).await;

    // The axis: the name's case, so the freed identity is the stored lowercase
    // one however it is sent.
    for (case, first, second) in [
        ("the same spelling", "reuse-snap", "reuse-snap"),
        ("another case of the same name", "Reuse-Snap-2", "reuse-SNAP-2"),
    ] {
        let stored = first.to_ascii_lowercase();
        let created = create_snapshot(&emulator, first, "reuse-cluster", &[]).await;
        assert_eq!(
            created.status, 200,
            "case {case}: the first snapshot must be created, got {} {}",
            created.status, created.body
        );
        std::thread::sleep(PAST_TRANSITION);
        let before = describe_snapshots(&emulator, &[("SnapshotName", &stored)]).await;
        let first_taken_at = before
            .text("NodeSnapshot/SnapshotCreateTime")
            .unwrap_or_default()
            .to_string();

        let started = std::time::Instant::now();
        let deleted = delete_snapshot(&emulator, first).await;
        assert_eq!(
            deleted.status, 200,
            "case {case}: the delete must answer 200, got {} {}",
            deleted.status, deleted.body
        );
        // While it is still going away the name is taken: the record is there.
        let too_early = create_snapshot(&emulator, second, "reuse-cluster", &[]).await;
        if started.elapsed() < state::SNAPSHOT_TRANSITION {
            assert_eq!(
                too_early.error_code(),
                "SnapshotAlreadyExistsFault",
                "case {case}: a snapshot that is still deleting must keep its name, got {} {}",
                too_early.status,
                too_early.body
            );
        }
        if too_early.status == 200 {
            // Only reachable on a machine slower than the removal span: the
            // clock had already carried the removal through, so this create took
            // the freed name itself — which is what the rest of the row
            // asserts, leaving nothing to repeat.
            std::thread::sleep(PAST_TRANSITION);
            continue;
        }

        // Past the span the record is gone and the name is free.
        std::thread::sleep(PAST_TRANSITION);
        let again = create_snapshot(&emulator, second, "reuse-cluster", &[]).await;
        assert_eq!(
            again.status, 200,
            "case {case}: the freed name must be usable again, got {} {}",
            again.status, again.body
        );
        std::thread::sleep(PAST_TRANSITION);
        let after = describe_snapshots(&emulator, &[("SnapshotName", &stored)]).await;
        assert_eq!(
            after.text("Snapshot/SnapshotStatus"),
            Some("available"),
            "case {case}: the new snapshot under the freed name must be available, got {}",
            after.body
        );
        // It is the NEW snapshot, not the deleted record come back: it was taken
        // at a later instant.
        let second_taken_at = after
            .text("NodeSnapshot/SnapshotCreateTime")
            .unwrap_or_default();
        assert_ne!(
            second_taken_at, first_taken_at,
            "case {case}: expected a snapshot taken after {first_taken_at:?}, got {}",
            after.body
        );
    }
}
