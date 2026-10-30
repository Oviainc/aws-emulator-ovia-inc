// SPDX-License-Identifier: CC0-1.0

//! Deleting ONE cluster snapshot, end to end over the awsQuery wire.
//!
//! Every case drives a real HTTP request through the same `MultiServiceRouter`
//! the production router builds (the harness is shared with
//! [`crate::cluster_lifecycle`]), so each assertion covers the whole walk:
//! form-urlencoded deserialize -> handler -> XML serialize.
//!
//! The suite asserts the RULE rather than one request: `DeleteClusterSnapshot`
//! removes the snapshot its locators name from the store exactly when that
//! snapshot is a `manual` one in the `available` state that no other account is
//! authorized to restore, reports the removed record back in the `deleted`
//! status, and raises the snapshot-deleted event for it; a request that names
//! anything else changes nothing and is refused — with the not-found fault when
//! its locators name no snapshot, and the snapshot-state fault when they name one
//! the rule does not admit.
//!
//! It is asserted over a TABLE of requests that all name one deletable snapshot
//! in a different FORM (the identifier alone, the identifier in another letter
//! case, the identifier paired with the cluster the snapshot was taken of, a
//! snapshot held under a bounded retention window) and over a table of requests
//! that differ along WHAT each one names (nothing, an unknown snapshot, a
//! snapshot paired with a cluster it was not taken of, one Amazon Redshift took
//! on its own schedule, one still being taken, one another account may restore,
//! and a snapshot an earlier call in the same suite already deleted). The
//! not-found refusal's own wording is pinned too, over the OPERATIONS that act
//! on the snapshot they name, beside the different sentence the snapshot READ
//! words the same fault with for the same name.

use std::time::Duration;

use service_com_amazonaws_redshift_redshiftserviceversion20121201::state;

use crate::cluster_lifecycle::{Answer, Emulator};
use crate::copy_cluster_snapshot::seed_automated;

/// The cluster every case takes its snapshots of.
const CLUSTER: &str = "delete-snapshot-source";

/// A second cluster, so a request can pair a snapshot with a cluster it was not
/// taken of.
const OTHER_CLUSTER: &str = "delete-snapshot-other";

/// The account a shared snapshot is authorized for restore by, in the 12-digit
/// form `AccountWithRestoreAccess` takes.
const GRANTEE_ACCOUNT: &str = "123456789012";

/// The snapshot every case leaves alone, so each delete is seen NOT to disturb
/// the records beside the one it removed.
const BYSTANDER: &str = "delete-snapshot-bystander";

/// One row of the success table: the case name, the identifier the snapshot is
/// stored under, the manual retention period it is taken with, the identifier the
/// request spells, and the cluster the request pairs it with.
struct DeleteCase<'a> {
    case: &'a str,
    stored: &'a str,
    retention: Option<&'a str>,
    requested: &'a str,
    cluster: Option<&'a str>,
}

/// One row of a refusal table: the case name, the request parameters, and the
/// `(status, error code)` the service must answer with.
type RefusalCase<'a> = (&'a str, Vec<(&'a str, &'a str)>, u16, &'a str);

/// An emulator holding both clusters and one manual snapshot per
/// `(identifier, retention period)` pair, every one of them `available`.
///
/// The `creating` -> `available` transition is clock-owed, so the seed takes
/// every snapshot first and waits for the span once rather than per snapshot.
async fn seeded_emulator(snapshots: &[(&str, Option<&str>)]) -> Emulator {
    let emulator = Emulator::new();
    for cluster in [CLUSTER, OTHER_CLUSTER] {
        let created = emulator.create_minimal(cluster).await;
        assert_eq!(
            created.status, 200,
            "case seed cluster {cluster}: expected 200, got {} with body {}",
            created.status, created.body
        );
    }
    for (snapshot, retention) in snapshots {
        let taken = take(&emulator, snapshot, *retention).await;
        assert_eq!(
            taken.status, 200,
            "case seed snapshot {snapshot}: expected 200, got {} with body {}",
            taken.status, taken.body
        );
    }
    tokio::time::sleep(state::PROVISIONING_SPAN + Duration::from_millis(250)).await;
    emulator
}

/// `CreateClusterSnapshot` of [`CLUSTER`], with `retention` when the row names
/// one.
async fn take(emulator: &Emulator, snapshot: &str, retention: Option<&str>) -> Answer {
    let mut params = vec![
        ("SnapshotIdentifier", snapshot),
        ("ClusterIdentifier", CLUSTER),
    ];
    if let Some(retention) = retention {
        params.push(("ManualSnapshotRetentionPeriod", retention));
    }
    emulator.call("CreateClusterSnapshot", &params).await
}

/// `DeleteClusterSnapshot` with these parameters.
async fn delete(emulator: &Emulator, params: &[(&str, &str)]) -> Answer {
    emulator.call("DeleteClusterSnapshot", params).await
}

/// The snapshot `identifier` as `DescribeClusterSnapshots` reports it.
async fn describe(emulator: &Emulator, identifier: &str) -> Answer {
    emulator
        .call(
            "DescribeClusterSnapshots",
            &[("SnapshotIdentifier", identifier)],
        )
        .await
}

/// Assert one refusal row: the answer's status and `<Code>`.
fn assert_refused(case: &str, answer: &Answer, status: u16, code: &str) {
    assert_eq!(
        answer.status, status,
        "case {case}: expected status {status}, got {} with body {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code().as_deref(),
        Some(code),
        "case {case}: expected error code {code:?}, got {:?} with body {}",
        answer.error_code(),
        answer.body
    );
}

/// Assert the snapshot `identifier` is still readable, so a refused request is
/// seen to have written nothing.
async fn assert_still_held(case: &str, emulator: &Emulator, identifier: &str) {
    let read = describe(emulator, identifier).await;
    assert_eq!(
        read.status, 200,
        "case {case}: expected {identifier} to be kept, got {} with body {}",
        read.status, read.body
    );
    assert_eq!(
        read.text("SnapshotIdentifier").as_deref(),
        Some(identifier),
        "case {case}: expected {identifier} to be reported back, got {:?}",
        read.text("SnapshotIdentifier")
    );
}

/// Every request that names a deletable snapshot removes exactly that snapshot,
/// reports it in the `deleted` status and raises the snapshot-deleted event.
#[tokio::test]
async fn every_request_naming_a_deletable_snapshot_removes_exactly_that_snapshot() {
    // The rows differ along the FORM the one deletable snapshot is named in, and
    // along the retention window the record is held under.
    let rows: Vec<DeleteCase> = vec![
        DeleteCase {
            case: "the identifier alone",
            stored: "delete-plain",
            retention: None,
            requested: "delete-plain",
            cluster: None,
        },
        DeleteCase {
            case: "the identifier in another letter case",
            stored: "delete-mixed-case",
            retention: None,
            requested: "DELETE-MIXED-CASE",
            cluster: None,
        },
        DeleteCase {
            case: "the identifier paired with the cluster it was taken of",
            stored: "delete-with-cluster",
            retention: None,
            requested: "delete-with-cluster",
            cluster: Some(CLUSTER),
        },
        DeleteCase {
            case: "a snapshot held under a bounded retention window",
            stored: "delete-retained",
            retention: Some("30"),
            requested: "delete-retained",
            cluster: None,
        },
        DeleteCase {
            case: "a snapshot kept indefinitely, paired with its cluster",
            stored: "delete-indefinite",
            retention: Some("-1"),
            requested: "delete-indefinite",
            cluster: Some(CLUSTER),
        },
    ];

    let mut seeds: Vec<(&str, Option<&str>)> = vec![(BYSTANDER, None)];
    for row in &rows {
        seeds.push((row.stored, row.retention));
    }
    let emulator = seeded_emulator(&seeds).await;

    for row in &rows {
        let case = row.case;
        let mut params = vec![("SnapshotIdentifier", row.requested)];
        if let Some(cluster) = row.cluster {
            params.push(("SnapshotClusterIdentifier", cluster));
        }
        let answer = delete(&emulator, &params).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected 200, got {} with body {}",
            answer.status, answer.body
        );
        // The record the store held is what comes back, under the identifier it
        // was stored with rather than the spelling the request used.
        assert_eq!(
            answer.text("SnapshotIdentifier").as_deref(),
            Some(row.stored),
            "case {case}: expected the removed snapshot {:?} back, got {:?} with body {}",
            row.stored,
            answer.text("SnapshotIdentifier"),
            answer.body
        );
        assert_eq!(
            answer.text("ClusterIdentifier").as_deref(),
            Some(CLUSTER),
            "case {case}: expected the snapshot's own cluster {CLUSTER:?}, got {:?}",
            answer.text("ClusterIdentifier")
        );
        assert_eq!(
            answer.text("Status").as_deref(),
            Some(state::STATUS_DELETED),
            "case {case}: expected the snapshot reported as {}, got {:?} with body {}",
            state::STATUS_DELETED,
            answer.text("Status"),
            answer.body
        );

        // ROUND TRIP: the store no longer holds it, and a second delete finds
        // nothing to remove.
        let gone = describe(&emulator, row.stored).await;
        assert_refused(
            &format!("{case} read back"),
            &gone,
            404,
            "ClusterSnapshotNotFound",
        );
        let again = delete(&emulator, &params).await;
        assert_refused(
            &format!("{case} deleted a second time"),
            &again,
            404,
            "ClusterSnapshotNotFound",
        );

        // The removal raised the snapshot-deleted event for the snapshot it
        // removed, which an event read reports.
        let events = emulator
            .call(
                "DescribeEvents",
                &[
                    ("SourceType", state::EVENT_SOURCE_TYPE_SNAPSHOT),
                    ("SourceIdentifier", row.stored),
                ],
            )
            .await;
        assert_eq!(
            events.status, 200,
            "case {case} events: expected 200, got {} with body {}",
            events.status, events.body
        );
        assert!(
            events.body.contains(state::EVENT_ID_SNAPSHOT_DELETED),
            "case {case} events: expected the event {} to be reported, got body {}",
            state::EVENT_ID_SNAPSHOT_DELETED,
            events.body
        );

        // Nothing beside the named snapshot was removed.
        assert_still_held(case, &emulator, BYSTANDER).await;
    }
}

/// A request that names no `available`, `manual` snapshot this account holds
/// alone is refused, writes nothing — and the same request succeeds once the
/// reason it was refused is gone.
#[tokio::test]
async fn only_an_available_manual_snapshot_nobody_else_may_restore_is_deleted() {
    let shared = "delete-shared";
    let pending = "delete-pending";
    let automated = "delete-automated";
    let emulator = seeded_emulator(&[(BYSTANDER, None), (shared, None)]).await;
    seed_automated(&emulator, CLUSTER, automated, state::STATUS_AVAILABLE);
    // A snapshot still being taken: its clock-owed transition is not due yet.
    let taken = take(&emulator, pending, None).await;
    assert_eq!(
        taken.status, 200,
        "case seed a creating snapshot: expected 200, got {} with body {}",
        taken.status, taken.body
    );
    let granted = emulator
        .call(
            "AuthorizeSnapshotAccess",
            &[
                ("SnapshotIdentifier", shared),
                ("AccountWithRestoreAccess", GRANTEE_ACCOUNT),
            ],
        )
        .await;
    assert_eq!(
        granted.status, 200,
        "case seed a shared snapshot: expected 200, got {} with body {}",
        granted.status, granted.body
    );

    // The rows differ along WHAT each request names: nothing, a snapshot no
    // store holds, a snapshot paired with a cluster it was not taken of, one
    // Amazon Redshift took on its own schedule, one still being taken, and one
    // another account may restore.
    let rows: Vec<RefusalCase> = vec![
        (
            "an empty identifier",
            vec![("SnapshotIdentifier", "")],
            400,
            "ValidationException",
        ),
        (
            "an unknown snapshot",
            vec![("SnapshotIdentifier", "no-such-snapshot")],
            404,
            "ClusterSnapshotNotFound",
        ),
        (
            "a snapshot paired with a cluster it was not taken of",
            vec![
                ("SnapshotIdentifier", BYSTANDER),
                ("SnapshotClusterIdentifier", OTHER_CLUSTER),
            ],
            404,
            "ClusterSnapshotNotFound",
        ),
        (
            "a snapshot Amazon Redshift took on its own schedule",
            vec![("SnapshotIdentifier", automated)],
            400,
            "InvalidClusterSnapshotState",
        ),
        (
            "a snapshot still being taken",
            vec![("SnapshotIdentifier", pending)],
            400,
            "InvalidClusterSnapshotState",
        ),
        (
            "a snapshot another account may restore",
            vec![("SnapshotIdentifier", shared)],
            400,
            "InvalidClusterSnapshotState",
        ),
    ];

    for (case, params, status, code) in rows {
        let answer = delete(&emulator, &params).await;
        assert_refused(case, &answer, status, code);
    }

    // Every snapshot a refusal named is still held, so none of those requests
    // wrote anything.
    for kept in [BYSTANDER, shared, pending, automated] {
        assert_still_held("a refused request keeps the snapshot", &emulator, kept).await;
    }

    // Releasing the one authorization makes the shared snapshot deletable, so the
    // refusal was about the grant and not about the snapshot.
    let revoked = emulator
        .call(
            "RevokeSnapshotAccess",
            &[
                ("SnapshotIdentifier", shared),
                ("AccountWithRestoreAccess", GRANTEE_ACCOUNT),
            ],
        )
        .await;
    assert_eq!(
        revoked.status, 200,
        "case revoke the one authorization: expected 200, got {} with body {}",
        revoked.status, revoked.body
    );
    let after_revoke = delete(&emulator, &[("SnapshotIdentifier", shared)]).await;
    assert_eq!(
        after_revoke.status, 200,
        "case a snapshot whose last authorization was revoked: expected 200, got {} with body {}",
        after_revoke.status, after_revoke.body
    );
    assert_eq!(
        after_revoke.text("Status").as_deref(),
        Some(state::STATUS_DELETED),
        "case a snapshot whose last authorization was revoked: expected {}, got {:?}",
        state::STATUS_DELETED,
        after_revoke.text("Status")
    );

    // The snapshot the clock has since made available takes the same request, so
    // that refusal was about the state and not about the snapshot.
    tokio::time::sleep(state::PROVISIONING_SPAN + Duration::from_millis(250)).await;
    let after_clock = delete(&emulator, &[("SnapshotIdentifier", pending)]).await;
    assert_eq!(
        after_clock.status, 200,
        "case a snapshot the clock made available: expected 200, got {} with body {}",
        after_clock.status, after_clock.body
    );
    assert_eq!(
        after_clock.text("Status").as_deref(),
        Some(state::STATUS_DELETED),
        "case a snapshot the clock made available: expected {}, got {:?}",
        state::STATUS_DELETED,
        after_clock.text("Status")
    );

    // What the refusals named and nothing deleted is still there.
    assert_still_held("the snapshot no case deleted", &emulator, BYSTANDER).await;
    assert_still_held(
        "a snapshot Amazon Redshift took on its own schedule",
        &emulator,
        automated,
    )
    .await;
}

/// The snapshot-not-found refusal is worded as the OPERATION words it: every
/// operation that acts on the snapshot it names answers the one sentence, and
/// the snapshot read answers its own for the very same name.
#[tokio::test]
async fn the_not_found_refusal_is_worded_per_operation() {
    let emulator = seeded_emulator(&[(BYSTANDER, None)]).await;
    let missing = "delete-snapshot-absent";

    // The rows are the operations that ACT on the snapshot their request names,
    // each with the other members its model requires, so the one sentence is
    // read off more than the operation this suite is about.
    let rows: Vec<(&str, Vec<(&str, &str)>)> = vec![
        (
            "DeleteClusterSnapshot",
            vec![("SnapshotIdentifier", missing)],
        ),
        (
            "ModifyClusterSnapshot",
            vec![
                ("SnapshotIdentifier", missing),
                ("ManualSnapshotRetentionPeriod", "7"),
            ],
        ),
        (
            "AuthorizeSnapshotAccess",
            vec![
                ("SnapshotIdentifier", missing),
                ("AccountWithRestoreAccess", GRANTEE_ACCOUNT),
            ],
        ),
        (
            "RevokeSnapshotAccess",
            vec![
                ("SnapshotIdentifier", missing),
                ("AccountWithRestoreAccess", GRANTEE_ACCOUNT),
            ],
        ),
        (
            "CopyClusterSnapshot",
            vec![
                ("SourceSnapshotIdentifier", missing),
                ("TargetSnapshotIdentifier", "delete-snapshot-copy-target"),
            ],
        ),
        (
            "RestoreFromClusterSnapshot",
            vec![
                ("SnapshotIdentifier", missing),
                ("ClusterIdentifier", "delete-snapshot-restored"),
            ],
        ),
    ];

    let expected = format!("ClusterSnapshot not found: {missing}");
    for (case, params) in rows {
        let answer = emulator.call(case, &params).await;
        assert_refused(case, &answer, 404, "ClusterSnapshotNotFound");
        assert_eq!(
            answer.text("Message").as_deref(),
            Some(expected.as_str()),
            "case {case}: expected the message {expected:?}, got {:?} with body {}",
            answer.text("Message"),
            answer.body
        );
    }

    // The READ words the same fault its own way, so the sentence is an attribute
    // of the operation and not of the service.
    let read = describe(&emulator, missing).await;
    assert_refused("DescribeClusterSnapshots", &read, 404, "ClusterSnapshotNotFound");
    let expected_read = format!("Snapshot {missing} not found.");
    assert_eq!(
        read.text("Message").as_deref(),
        Some(expected_read.as_str()),
        "case DescribeClusterSnapshots: expected the message {expected_read:?}, got {:?} with body {}",
        read.text("Message"),
        read.body
    );
}
