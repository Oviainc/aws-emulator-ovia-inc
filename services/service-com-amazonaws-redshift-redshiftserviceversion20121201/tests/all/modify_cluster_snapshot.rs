// SPDX-License-Identifier: CC0-1.0

//! Modifying the settings of one cluster snapshot, end to end over the awsQuery
//! wire.
//!
//! Every case drives a real HTTP request through the same `MultiServiceRouter`
//! the production router builds (the harness is shared with
//! [`crate::cluster_lifecycle`]), so each assertion covers the whole walk:
//! form-urlencoded deserialize -> handler -> XML serialize.
//!
//! The suite asserts the RULE rather than one request: `ModifyClusterSnapshot`
//! sets the manual retention period of the named, `available`, `manual` snapshot
//! — admitting only the "indefinitely" sentinel or a day count inside the
//! published range — persists it for a later read, and reports the snapshot with
//! the two members that period DERIVES (`SnapshotRetentionStartTime`, the
//! snapshot's own creation instant, and `ManualSnapshotRemainingDays`, the days
//! left of the window counted from it). A period whose window has already ended
//! for that snapshot is refused, unless the request forces it, which deletes the
//! snapshot.
//!
//! It is asserted over a TABLE of periods (the sentinel, both published bounds, a
//! value inside the range), over each bound one past it, over a table of snapshot
//! AGES (so the derived day count is read at more than one point of the window)
//! and over the refusals (an unknown snapshot, an empty identifier, a snapshot
//! still being created, a snapshot Redshift took on its own schedule, a window
//! already in the past).

use std::time::{Duration, Instant, SystemTime};

use service_com_amazonaws_redshift_redshiftserviceversion20121201::ops::types::Snapshot;
use service_com_amazonaws_redshift_redshiftserviceversion20121201::state;

use crate::cluster_lifecycle::{Answer, Emulator};

/// The cluster every case takes its snapshot of.
const CLUSTER: &str = "retention-source";

/// Seconds in one day, so a seeded snapshot can be given an age in days.
const SECONDS_PER_DAY: u64 = 24 * 60 * 60;

/// One row of a refusal table: the case name, the request parameters, and the
/// `(status, error code)` the service must answer with.
type RefusalCase<'a> = (&'a str, Vec<(&'a str, &'a str)>, u16, &'a str);

/// An emulator holding [`CLUSTER`] and a manual snapshot of it that is still
/// `creating` — the clock-owed `creating` -> `available` transition is not due
/// yet.
async fn creating_emulator(snapshot: &str) -> Emulator {
    let emulator = Emulator::new();
    let created = emulator.create_minimal(CLUSTER).await;
    assert_eq!(
        created.status, 200,
        "case seed cluster {CLUSTER}: expected 200, got {} with body {}",
        created.status, created.body
    );
    let taken = emulator
        .call(
            "CreateClusterSnapshot",
            &[
                ("SnapshotIdentifier", snapshot),
                ("ClusterIdentifier", CLUSTER),
            ],
        )
        .await;
    assert_eq!(
        taken.status, 200,
        "case seed snapshot {snapshot}: expected 200, got {} with body {}",
        taken.status, taken.body
    );
    emulator
}

/// An emulator holding [`CLUSTER`] and an `available` manual snapshot of it.
async fn seeded_emulator(snapshot: &str) -> Emulator {
    let emulator = creating_emulator(snapshot).await;
    // Only an `available` snapshot's settings can be changed, and the transition
    // is due a span after the call that minted it.
    tokio::time::sleep(state::PROVISIONING_SPAN + Duration::from_millis(250)).await;
    emulator
}

/// Write a snapshot record straight into the shared store, taken `age_days` ago
/// with `snapshot_type` and the manual retention period `period`.
///
/// Nothing in this slice can mint a snapshot that is OLD (every create stamps
/// the current instant) or one Redshift took on its own schedule, so seeding the
/// same store the handlers read is the only way to observe a retention window
/// that has already ended.
///
/// `pub(crate)` so the sibling batch suite seeds such a record through this one
/// helper rather than writing a second copy of it.
pub(crate) fn seed_snapshot(emulator: &Emulator, identifier: &str, snapshot_type: &str, age_days: u64, period: i32) {
    let store = state::service_state(&*emulator.engine).expect("the Redshift store");
    let mut store = store.lock();
    let created_at = aws_smithy_types::DateTime::from(
        SystemTime::now() - Duration::from_secs(age_days * SECONDS_PER_DAY),
    );
    let mut snapshot = Snapshot {
        snapshot_identifier: Some(identifier.to_string()),
        cluster_identifier: Some(CLUSTER.to_string()),
        snapshot_create_time: Some(created_at),
        status: Some(state::STATUS_AVAILABLE.to_string()),
        snapshot_type: Some(snapshot_type.to_string()),
        ..Default::default()
    };
    state::apply_manual_retention(&mut snapshot, period, created_at);
    store.snapshots.push(state::StoredSnapshot {
        snapshot,
        // Already past, so the record needs no clock-owed transition.
        available_at: Instant::now(),
        source: None,
        // A seeded record holds no copy of any cluster's databases.
        data: Default::default(),
    });
}

/// `ModifyClusterSnapshot` with these parameters.
async fn modify(emulator: &Emulator, params: &[(&str, &str)]) -> Answer {
    emulator.call("ModifyClusterSnapshot", params).await
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

/// Assert the retention bookkeeping one answer reports for `period`: the period
/// itself, and the two members it derives.
///
/// `pub(crate)` so the sibling batch suite asserts the derived members through
/// this one helper, and the two suites cannot drift on what they must be.
pub(crate) fn assert_retention(case: &str, answer: &Answer, period: i32) {
    assert_eq!(
        answer.text("ManualSnapshotRetentionPeriod").as_deref(),
        Some(period.to_string().as_str()),
        "case {case}: expected the retention period {period}, got {:?} with body {}",
        answer.text("ManualSnapshotRetentionPeriod"),
        answer.body
    );
    if period == state::MANUAL_RETENTION_INDEFINITE {
        // A snapshot retained indefinitely is under no retention period, so it
        // passes none and reports neither derived member.
        assert_eq!(
            answer.text("ManualSnapshotRemainingDays"),
            None,
            "case {case}: expected no remaining-day count for an indefinite period, got {:?}",
            answer.text("ManualSnapshotRemainingDays")
        );
        assert_eq!(
            answer.text("SnapshotRetentionStartTime"),
            None,
            "case {case}: expected no retention start for an indefinite period, got {:?}",
            answer.text("SnapshotRetentionStartTime")
        );
    } else {
        // The window is counted from the snapshot's own creation instant, so a
        // snapshot just taken has its whole period left.
        assert_eq!(
            answer.text("SnapshotRetentionStartTime"),
            answer.text("SnapshotCreateTime"),
            "case {case}: expected the retention start at the snapshot's creation time {:?}, got {:?}",
            answer.text("SnapshotCreateTime"),
            answer.text("SnapshotRetentionStartTime")
        );
        assert_eq!(
            answer.text("ManualSnapshotRemainingDays").as_deref(),
            Some(period.to_string().as_str()),
            "case {case}: expected {period} days remaining on a snapshot just taken, got {:?}",
            answer.text("ManualSnapshotRemainingDays")
        );
    }
}

/// Every retention period the service admits is applied, reported with the
/// members it derives, and PERSISTED for a later read.
#[tokio::test]
async fn every_admitted_retention_period_is_applied_and_persists() {
    let snapshot = "retention-applied";
    let emulator = seeded_emulator(snapshot).await;
    let (lower, upper) = state::MANUAL_RETENTION_BOUNDS;

    // The rows differ along the VALUE the rule generalises over: the documented
    // sentinel, both published bounds, and a value inside the range.
    let rows: Vec<(&str, i32)> = vec![
        ("the documented lower bound", lower),
        ("a period inside the published range", 30),
        ("the documented upper bound", upper),
        ("the indefinite sentinel", state::MANUAL_RETENTION_INDEFINITE),
        ("a second period inside the range", 365),
    ];

    for (case, period) in rows {
        let answer = modify(
            &emulator,
            &[
                ("SnapshotIdentifier", snapshot),
                ("ManualSnapshotRetentionPeriod", &period.to_string()),
            ],
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected 200, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.text("SnapshotIdentifier").as_deref(),
            Some(snapshot),
            "case {case}: expected the modified snapshot back, got {:?}",
            answer.text("SnapshotIdentifier")
        );
        // Changing a setting does not move the snapshot out of `available`.
        assert_eq!(
            answer.text("Status").as_deref(),
            Some(state::STATUS_AVAILABLE),
            "case {case}: expected the snapshot to stay {}, got {:?}",
            state::STATUS_AVAILABLE,
            answer.text("Status")
        );
        assert_retention(case, &answer, period);

        // ROUND TRIP: what the modify persisted is what a later read reports.
        let described = describe(&emulator, snapshot).await;
        assert_eq!(
            described.status, 200,
            "case {case} read back: expected 200, got {} with body {}",
            described.status, described.body
        );
        assert_retention(&format!("{case} read back"), &described, period);
    }
}

/// A request that names no retention period changes nothing, and still reports
/// the snapshot it named.
#[tokio::test]
async fn a_request_naming_no_setting_changes_nothing() {
    let snapshot = "retention-untouched";
    let emulator = seeded_emulator(snapshot).await;

    let applied = modify(
        &emulator,
        &[
            ("SnapshotIdentifier", snapshot),
            ("ManualSnapshotRetentionPeriod", "12"),
        ],
    )
    .await;
    assert_eq!(
        applied.status, 200,
        "case seed retention: expected 200, got {} with body {}",
        applied.status, applied.body
    );

    let bare = modify(&emulator, &[("SnapshotIdentifier", snapshot)]).await;
    assert_eq!(
        bare.status, 200,
        "case a request naming no setting: expected 200, got {} with body {}",
        bare.status, bare.body
    );
    assert_retention("a request naming no setting", &bare, 12);

    let described = describe(&emulator, snapshot).await;
    assert_retention("a request naming no setting read back", &described, 12);
}

/// A retention period outside the published range is refused with the modelled
/// retention fault, and leaves the stored period exactly as it was.
#[tokio::test]
async fn a_retention_period_outside_the_published_range_is_refused() {
    let snapshot = "retention-rejected";
    let emulator = seeded_emulator(snapshot).await;
    let (lower, upper) = state::MANUAL_RETENTION_BOUNDS;

    let seeded = modify(
        &emulator,
        &[
            ("SnapshotIdentifier", snapshot),
            ("ManualSnapshotRetentionPeriod", "45"),
        ],
    )
    .await;
    assert_eq!(
        seeded.status, 200,
        "case seed retention: expected 200, got {} with body {}",
        seeded.status, seeded.body
    );

    // One row per published bound, one step outside it, plus the values between
    // the sentinel and the lower bound.
    let below = (lower - 1).to_string();
    let above = (upper + 1).to_string();
    let under_sentinel = (state::MANUAL_RETENTION_INDEFINITE - 1).to_string();
    let rows: Vec<(&str, &str)> = vec![
        ("one below the documented lower bound", &below),
        ("one above the documented upper bound", &above),
        ("one below the indefinite sentinel", &under_sentinel),
        ("far below every admitted value", "-2147483648"),
        ("far above every admitted value", "2147483647"),
    ];

    for (case, period) in rows {
        let answer = modify(
            &emulator,
            &[
                ("SnapshotIdentifier", snapshot),
                ("ManualSnapshotRetentionPeriod", period),
            ],
        )
        .await;
        assert_refused(case, &answer, 400, "InvalidRetentionPeriodFault");

        let described = describe(&emulator, snapshot).await;
        assert_eq!(
            described.text("ManualSnapshotRetentionPeriod").as_deref(),
            Some("45"),
            "case {case}: expected the refused request to change nothing, got {:?}",
            described.text("ManualSnapshotRetentionPeriod")
        );
    }
}

/// A request that names no snapshot this store holds as an `available`, `manual`
/// snapshot is refused — and the same request succeeds once the clock-owed
/// `creating` -> `available` transition has fallen due.
#[tokio::test]
async fn only_an_available_manual_snapshot_can_be_modified() {
    let snapshot = "retention-pending";
    let emulator = creating_emulator(snapshot).await;
    seed_snapshot(
        &emulator,
        "retention-automated",
        state::SNAPSHOT_TYPE_AUTOMATED,
        1,
        state::MANUAL_RETENTION_INDEFINITE,
    );

    // The rows differ along WHAT each one names: nothing, an unknown snapshot, a
    // snapshot still being taken, and one Redshift took on its own schedule.
    let rows: Vec<RefusalCase> = vec![
        (
            "an empty identifier",
            vec![("SnapshotIdentifier", "")],
            400,
            "ValidationException",
        ),
        (
            "an unknown snapshot",
            vec![
                ("SnapshotIdentifier", "no-such-snapshot"),
                ("ManualSnapshotRetentionPeriod", "7"),
            ],
            404,
            "ClusterSnapshotNotFound",
        ),
        (
            "a snapshot still being created",
            vec![
                ("SnapshotIdentifier", snapshot),
                ("ManualSnapshotRetentionPeriod", "7"),
            ],
            400,
            "InvalidClusterSnapshotState",
        ),
        (
            "a snapshot Redshift took on its own schedule",
            vec![
                ("SnapshotIdentifier", "retention-automated"),
                ("ManualSnapshotRetentionPeriod", "7"),
            ],
            400,
            "InvalidClusterSnapshotState",
        ),
    ];

    for (case, params, status, code) in rows {
        let answer = modify(&emulator, &params).await;
        assert_refused(case, &answer, status, code);
    }

    // The snapshot the clock has since made available takes the same request.
    tokio::time::sleep(state::PROVISIONING_SPAN + Duration::from_millis(250)).await;
    let later = modify(
        &emulator,
        &[
            ("SnapshotIdentifier", snapshot),
            ("ManualSnapshotRetentionPeriod", "7"),
        ],
    )
    .await;
    assert_eq!(
        later.status, 200,
        "case a snapshot the clock made available: expected 200, got {} with body {}",
        later.status, later.body
    );
    assert_retention("a snapshot the clock made available", &later, 7);

    // The automated snapshot the refusal named kept its own record.
    let automated = describe(&emulator, "retention-automated").await;
    assert_eq!(
        automated.text("ManualSnapshotRetentionPeriod").as_deref(),
        Some(state::MANUAL_RETENTION_INDEFINITE.to_string().as_str()),
        "case a snapshot Redshift took on its own schedule: expected the refused request to change nothing, got {:?}",
        automated.text("ManualSnapshotRetentionPeriod")
    );
}

/// The days left of a snapshot's retention period are counted from the
/// snapshot's own creation instant, so the same period reports fewer days left
/// on an older snapshot.
#[tokio::test]
async fn the_remaining_days_are_counted_from_the_snapshots_creation_time() {
    let emulator = Emulator::new();
    let period = 10;

    // The rows differ along the AGE of the snapshot the one period is applied
    // to: the derived count is the days left of the window, not the period.
    let rows: Vec<(&str, u64)> = vec![
        ("a snapshot taken today", 0),
        ("a snapshot taken yesterday", 1),
        ("a snapshot a third of the way through its window", 3),
        ("a snapshot one day short of its window", 9),
    ];

    for (case, age_days) in rows {
        let identifier = format!("remaining-{age_days}");
        seed_snapshot(
            &emulator,
            &identifier,
            state::SNAPSHOT_TYPE_MANUAL,
            age_days,
            state::MANUAL_RETENTION_INDEFINITE,
        );
        let answer = modify(
            &emulator,
            &[
                ("SnapshotIdentifier", &identifier),
                ("ManualSnapshotRetentionPeriod", &period.to_string()),
            ],
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected 200, got {} with body {}",
            answer.status, answer.body
        );
        let expected = (period - age_days as i32).to_string();
        assert_eq!(
            answer.text("ManualSnapshotRemainingDays").as_deref(),
            Some(expected.as_str()),
            "case {case}: expected {expected} days remaining, got {:?} with body {}",
            answer.text("ManualSnapshotRemainingDays"),
            answer.body
        );
        assert_eq!(
            answer.text("SnapshotRetentionStartTime"),
            answer.text("SnapshotCreateTime"),
            "case {case}: expected the retention start at the snapshot's creation time {:?}, got {:?}",
            answer.text("SnapshotCreateTime"),
            answer.text("SnapshotRetentionStartTime")
        );

        // ROUND TRIP: a later read derives the same count from the same record.
        let described = describe(&emulator, &identifier).await;
        assert_eq!(
            described.text("ManualSnapshotRemainingDays").as_deref(),
            Some(expected.as_str()),
            "case {case} read back: expected {expected} days remaining, got {:?}",
            described.text("ManualSnapshotRemainingDays")
        );
    }
}

/// A retention period whose window has ALREADY ended for the named snapshot is
/// refused, and the forced request deletes that snapshot instead.
#[tokio::test]
async fn a_window_already_in_the_past_is_refused_unless_it_is_forced() {
    let emulator = Emulator::new();

    // The rows differ along the age of the snapshot and the length of the window
    // it is asked to fit into; each row's window ended before the call.
    let rows: Vec<(&str, u64, i32)> = vec![
        ("a two-day-old snapshot kept for one day", 2, 1),
        ("a year-old snapshot kept for thirty days", 400, 30),
        (
            "a snapshot one day past the documented lower bound",
            2,
            state::MANUAL_RETENTION_BOUNDS.0,
        ),
    ];

    for (case, age_days, period) in rows {
        let identifier = format!("expired-{age_days}-{period}");
        seed_snapshot(
            &emulator,
            &identifier,
            state::SNAPSHOT_TYPE_MANUAL,
            age_days,
            state::MANUAL_RETENTION_INDEFINITE,
        );

        let refused = modify(
            &emulator,
            &[
                ("SnapshotIdentifier", &identifier),
                ("ManualSnapshotRetentionPeriod", &period.to_string()),
            ],
        )
        .await;
        assert_refused(case, &refused, 400, "InvalidRetentionPeriodFault");

        // The refused request left the snapshot, and its period, alone.
        let described = describe(&emulator, &identifier).await;
        assert_eq!(
            described.status, 200,
            "case {case}: expected the refused request to keep the snapshot, got {} with body {}",
            described.status, described.body
        );
        assert_eq!(
            described.text("ManualSnapshotRetentionPeriod").as_deref(),
            Some(state::MANUAL_RETENTION_INDEFINITE.to_string().as_str()),
            "case {case}: expected the stored period unchanged, got {:?}",
            described.text("ManualSnapshotRetentionPeriod")
        );

        let forced = modify(
            &emulator,
            &[
                ("SnapshotIdentifier", &identifier),
                ("ManualSnapshotRetentionPeriod", &period.to_string()),
                ("Force", "true"),
            ],
        )
        .await;
        assert_eq!(
            forced.status, 200,
            "case {case} forced: expected 200, got {} with body {}",
            forced.status, forced.body
        );
        assert_eq!(
            forced.text("Status").as_deref(),
            Some(state::STATUS_DELETED),
            "case {case} forced: expected the snapshot reported as {}, got {:?}",
            state::STATUS_DELETED,
            forced.text("Status")
        );

        // ROUND TRIP: the forced request removed the snapshot from the store.
        let gone = describe(&emulator, &identifier).await;
        assert_refused(
            &format!("{case} forced read back"),
            &gone,
            404,
            "ClusterSnapshotNotFound",
        );
    }

    // A window that has NOT ended is applied without `Force` being needed, so the
    // refusal above is about the window and not about the flag.
    seed_snapshot(
        &emulator,
        "expired-none",
        state::SNAPSHOT_TYPE_MANUAL,
        2,
        state::MANUAL_RETENTION_INDEFINITE,
    );
    let inside = modify(
        &emulator,
        &[
            ("SnapshotIdentifier", "expired-none"),
            ("ManualSnapshotRetentionPeriod", "3"),
        ],
    )
    .await;
    assert_eq!(
        inside.status, 200,
        "case a two-day-old snapshot kept for three days: expected 200, got {} with body {}",
        inside.status, inside.body
    );
    assert_eq!(
        inside.text("ManualSnapshotRemainingDays").as_deref(),
        Some("1"),
        "case a two-day-old snapshot kept for three days: expected 1 day remaining, got {:?}",
        inside.text("ManualSnapshotRemainingDays")
    );
}
