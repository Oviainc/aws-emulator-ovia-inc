// SPDX-License-Identifier: CC0-1.0

//! Modifying the settings of a SET of cluster snapshots in one call, end to end
//! over the awsQuery wire.
//!
//! Every case drives a real HTTP request through the same `MultiServiceRouter`
//! the production router builds (the harness is shared with
//! [`crate::cluster_lifecycle`]), so each assertion covers the whole walk:
//! form-urlencoded deserialize -> handler -> XML serialize.
//!
//! The suite asserts the RULE rather than one request:
//! `BatchModifyClusterSnapshots` applies one manual retention period to every
//! snapshot its list names, reports the ones it acted on in `Resources` in
//! request order and the ones it could not act on in `Errors` — each as a
//! `SnapshotErrorMessage` whose `FailureCode` is the wire code of the fault
//! `ModifyClusterSnapshot` raises for that same condition — and refuses the WHOLE
//! call only for what the request itself gets wrong: a list longer than the
//! published batch limit, or a period outside the published range.
//!
//! It is asserted over a TABLE of list LENGTHS and periods (the sentinel, both
//! published bounds, values inside the range), over the entry FORM (a stored
//! identifier in another case), over a table of per-entry failures (an unknown
//! snapshot, a snapshot still being created, one Redshift took on its own
//! schedule, a window already in the past), over the batch limit and one past it,
//! and over each retention bound one past it.

use std::time::Duration;

use service_com_amazonaws_redshift_redshiftserviceversion20121201::state;

use crate::cluster_lifecycle::{all_text, Answer, Emulator};
use crate::modify_cluster_snapshot::{assert_retention, seed_snapshot};

/// The cluster every case takes its snapshots of.
const CLUSTER: &str = "batch-retention-source";

/// One row of the per-entry failure table: the case name, the identifier the
/// request lists, and the `SnapshotErrorMessage$FailureCode` it must come back
/// with.
type FailureCase<'a> = (&'a str, String, &'a str);

/// An emulator holding [`CLUSTER`] and one `available` manual snapshot per name.
///
/// The `creating` -> `available` transition is clock-owed, so the seed waits for
/// it once rather than per snapshot.
async fn seeded_emulator(snapshots: &[&str]) -> Emulator {
    let emulator = Emulator::new();
    let created = emulator.create_minimal(CLUSTER).await;
    assert_eq!(
        created.status, 200,
        "case seed cluster {CLUSTER}: expected 200, got {} with body {}",
        created.status, created.body
    );
    for snapshot in snapshots {
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
    }
    tokio::time::sleep(state::PROVISIONING_SPAN + Duration::from_millis(250)).await;
    emulator
}

/// `BatchModifyClusterSnapshots` naming `identifiers`, with the extra parameters
/// `extra`.
async fn batch_modify(
    emulator: &Emulator,
    identifiers: &[&str],
    extra: &[(&str, &str)],
) -> Answer {
    // An awsQuery list member is sent as `<List>.<member xmlName>.<index>`, and
    // `SnapshotIdentifierList`'s member carries the xmlName `String`
    // (`smithy_query(query="shape", shape="SnapshotIdentifierList")`).
    let mut params: Vec<(String, String)> = identifiers
        .iter()
        .enumerate()
        .map(|(position, identifier)| {
            (
                format!("SnapshotIdentifierList.String.{}", position + 1),
                (*identifier).to_string(),
            )
        })
        .collect();
    params.extend(
        extra
            .iter()
            .map(|(name, value)| ((*name).to_string(), (*value).to_string())),
    );
    let borrowed: Vec<(&str, &str)> = params
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    emulator
        .call("BatchModifyClusterSnapshots", &borrowed)
        .await
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

/// The `Resources` entries of a batch answer, in the order the body lists them.
///
/// `SnapshotIdentifierList`'s member is serialized as `<String>`, and the only
/// `SnapshotIdentifierList` in this output is `Resources`.
fn resources(answer: &Answer) -> Vec<String> {
    all_text(&answer.body, "String")
}

/// The `Errors` entries of a batch answer as `(SnapshotIdentifier, FailureCode,
/// FailureReason)`, in the order the body lists them.
///
/// The open tag is matched WITHOUT its closing `>`: the serializer puts the
/// service's XML namespace on the first member element of the list, so that one
/// element carries an attribute.
fn errors(answer: &Answer) -> Vec<(String, String, String)> {
    answer
        .body
        .split("<SnapshotErrorMessage")
        .skip(1)
        .filter_map(|block| {
            let block = block.split_once('>')?.1;
            let block = block.split("</SnapshotErrorMessage>").next().unwrap_or("");
            let first = |tag: &str| {
                all_text(block, tag)
                    .into_iter()
                    .next()
                    .unwrap_or_else(|| String::from("<absent>"))
            };
            Some((
                first("SnapshotIdentifier"),
                first("FailureCode"),
                first("FailureReason"),
            ))
        })
        .collect()
}

/// Assert one whole-call refusal: the answer's status and `<Code>`.
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

/// One admitted period is applied to EVERY snapshot the list names, whatever the
/// length of the list and the form of each entry, and PERSISTS for a later read.
#[tokio::test]
async fn one_period_is_applied_to_every_named_snapshot_and_persists() {
    let stored = ["batch-a", "batch-b", "batch-c"];
    let emulator = seeded_emulator(&stored).await;
    let (lower, upper) = state::MANUAL_RETENTION_BOUNDS;

    // The rows differ along the COUNT of snapshots named, the ORDER they are
    // named in, the FORM of each entry (an identifier is matched without regard
    // to case) and the VALUE of the period: both published bounds, the
    // documented sentinel, and values inside the range.
    let rows: Vec<(&str, Vec<&str>, Vec<&str>, i32)> = vec![
        (
            "one snapshot at the documented lower bound",
            vec!["batch-a"],
            vec!["batch-a"],
            lower,
        ),
        (
            "two snapshots at a period inside the range",
            vec!["batch-a", "batch-b"],
            vec!["batch-a", "batch-b"],
            30,
        ),
        (
            "three snapshots at the documented upper bound",
            vec!["batch-a", "batch-b", "batch-c"],
            vec!["batch-a", "batch-b", "batch-c"],
            upper,
        ),
        (
            "three snapshots at the indefinite sentinel",
            vec!["batch-c", "batch-a", "batch-b"],
            vec!["batch-c", "batch-a", "batch-b"],
            state::MANUAL_RETENTION_INDEFINITE,
        ),
        (
            "two snapshots named in the reverse order",
            vec!["batch-c", "batch-b"],
            vec!["batch-c", "batch-b"],
            365,
        ),
        (
            "an entry naming a stored snapshot in another case",
            vec!["BATCH-A", "Batch-B"],
            vec!["batch-a", "batch-b"],
            7,
        ),
    ];

    for (case, named, expected, period) in rows {
        let answer = batch_modify(
            &emulator,
            &named,
            &[("ManualSnapshotRetentionPeriod", &period.to_string())],
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected 200, got {} with body {}",
            answer.status, answer.body
        );
        // `Resources` is "a list of the snapshots that were modified", in the
        // order the request named them, spelled as the store holds them.
        assert_eq!(
            resources(&answer),
            expected,
            "case {case}: expected the named snapshots reported as modified, got {:?} with body {}",
            resources(&answer),
            answer.body
        );
        assert_eq!(
            errors(&answer),
            Vec::new(),
            "case {case}: expected no per-entry errors, got {:?} with body {}",
            errors(&answer),
            answer.body
        );

        // ROUND TRIP: what the batch persisted is what a later read reports, for
        // every snapshot it named.
        for identifier in &expected {
            let described = describe(&emulator, identifier).await;
            assert_eq!(
                described.status, 200,
                "case {case} read back {identifier}: expected 200, got {} with body {}",
                described.status, described.body
            );
            assert_retention(&format!("{case} read back {identifier}"), &described, period);
        }
    }
}

/// A batch touches ONLY the snapshots its list names, and a request naming no
/// setting changes none of them.
#[tokio::test]
async fn only_the_named_snapshots_are_modified() {
    let emulator = seeded_emulator(&["batch-named", "batch-untouched"]).await;

    let applied = batch_modify(
        &emulator,
        &["batch-named"],
        &[("ManualSnapshotRetentionPeriod", "12")],
    )
    .await;
    assert_eq!(
        applied.status, 200,
        "case a batch naming one of two snapshots: expected 200, got {} with body {}",
        applied.status, applied.body
    );

    let named = describe(&emulator, "batch-named").await;
    assert_retention("a batch naming one of two snapshots", &named, 12);

    // The snapshot the list did not name kept the period `CreateClusterSnapshot`
    // gave it.
    let untouched = describe(&emulator, "batch-untouched").await;
    assert_eq!(
        untouched.text("ManualSnapshotRetentionPeriod").as_deref(),
        Some(state::MANUAL_RETENTION_INDEFINITE.to_string().as_str()),
        "case a snapshot the list did not name: expected its own period untouched, got {:?}",
        untouched.text("ManualSnapshotRetentionPeriod")
    );

    // A request that names no period changes none, and still reports both
    // snapshots as the ones it acted on.
    let bare = batch_modify(&emulator, &["batch-named", "batch-untouched"], &[]).await;
    assert_eq!(
        bare.status, 200,
        "case a request naming no setting: expected 200, got {} with body {}",
        bare.status, bare.body
    );
    assert_eq!(
        resources(&bare),
        vec!["batch-named", "batch-untouched"],
        "case a request naming no setting: expected both snapshots reported, got {:?}",
        resources(&bare)
    );
    assert_retention(
        "a request naming no setting read back",
        &describe(&emulator, "batch-named").await,
        12,
    );
    assert_eq!(
        describe(&emulator, "batch-untouched")
            .await
            .text("ManualSnapshotRetentionPeriod")
            .as_deref(),
        Some(state::MANUAL_RETENTION_INDEFINITE.to_string().as_str()),
        "case a request naming no setting: expected the second snapshot unchanged too"
    );
}

/// An entry the call cannot act on is reported in `Errors` with the wire code of
/// the fault the one-snapshot operation raises for that condition — and never
/// fails the batch or the entries beside it.
#[tokio::test]
async fn an_entry_that_cannot_be_acted_on_is_reported_per_entry() {
    let emulator = seeded_emulator(&["batch-good"]).await;
    // A snapshot still being taken: minted after the seed's wait, so its
    // clock-owed transition is not due yet.
    let pending = emulator
        .call(
            "CreateClusterSnapshot",
            &[
                ("SnapshotIdentifier", "batch-pending"),
                ("ClusterIdentifier", CLUSTER),
            ],
        )
        .await;
    assert_eq!(
        pending.status, 200,
        "case seed snapshot batch-pending: expected 200, got {} with body {}",
        pending.status, pending.body
    );
    // Nothing in this slice mints a snapshot Redshift took on its own schedule,
    // or one old enough to have left its window, so those two are written
    // straight into the shared store the handlers read.
    seed_snapshot(
        &emulator,
        "batch-automated",
        state::SNAPSHOT_TYPE_AUTOMATED,
        1,
        state::MANUAL_RETENTION_INDEFINITE,
    );
    seed_snapshot(
        &emulator,
        "batch-expired",
        state::SNAPSHOT_TYPE_MANUAL,
        40,
        state::MANUAL_RETENTION_INDEFINITE,
    );

    // The rows differ along WHAT the entry names: a snapshot no store holds, an
    // entry naming nothing at all, a snapshot still being taken, one Redshift
    // took on its own schedule, and one whose window the requested period has
    // already ended.
    // The `creating` row runs FIRST: that state is transient by design, so it is
    // asserted before the clock-owed transition can fall due.
    let rows: Vec<FailureCase> = vec![
        (
            "a snapshot still being created",
            "batch-pending".to_string(),
            state::SNAPSHOT_FAILURE_INVALID_STATE,
        ),
        (
            "an unknown snapshot",
            "no-such-snapshot".to_string(),
            state::SNAPSHOT_FAILURE_NOT_FOUND,
        ),
        (
            "an entry naming nothing at all",
            String::new(),
            state::SNAPSHOT_FAILURE_NOT_FOUND,
        ),
        (
            "a snapshot Redshift took on its own schedule",
            "batch-automated".to_string(),
            state::SNAPSHOT_FAILURE_INVALID_STATE,
        ),
        (
            "a snapshot whose window the period has already ended",
            "batch-expired".to_string(),
            state::SNAPSHOT_FAILURE_INVALID_RETENTION_PERIOD,
        ),
    ];

    for (case, identifier, failure_code) in &rows {
        // The bad entry is sent BESIDE a good one, so the assertion also covers
        // that one failing entry does not fail the batch.
        let answer = batch_modify(
            &emulator,
            &["batch-good", identifier.as_str()],
            &[("ManualSnapshotRetentionPeriod", "9")],
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected the batch itself to succeed with 200, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            resources(&answer),
            vec!["batch-good"],
            "case {case}: expected only the entry that could be acted on, got {:?} with body {}",
            resources(&answer),
            answer.body
        );
        let reported = errors(&answer);
        assert_eq!(
            reported.len(),
            1,
            "case {case}: expected exactly one per-entry error, got {reported:?} with body {}",
            answer.body
        );
        assert_eq!(
            reported[0].1,
            *failure_code,
            "case {case}: expected the failure code {failure_code}, got {:?} with body {}",
            reported[0],
            answer.body
        );
        assert_eq!(
            reported[0].0, *identifier,
            "case {case}: expected the error to name {identifier:?}, got {:?}",
            reported[0].0
        );
        assert!(
            !reported[0].2.is_empty(),
            "case {case}: expected a failure reason, got {:?}",
            reported[0]
        );
        // The good entry really was modified, so the batch did its work.
        assert_retention(case, &describe(&emulator, "batch-good").await, 9);
    }

    // Every refused entry kept its own record exactly as it stood.
    for identifier in ["batch-pending", "batch-automated", "batch-expired"] {
        let described = describe(&emulator, identifier).await;
        assert_eq!(
            described.status, 200,
            "case {identifier} after refusal: expected the snapshot kept, got {} with body {}",
            described.status, described.body
        );
        assert_eq!(
            described.text("ManualSnapshotRetentionPeriod").as_deref(),
            Some(state::MANUAL_RETENTION_INDEFINITE.to_string().as_str()),
            "case {identifier} after refusal: expected its period unchanged, got {:?}",
            described.text("ManualSnapshotRetentionPeriod")
        );
    }

    // Every bad entry in ONE list is reported, each in the order it was named.
    let together = batch_modify(
        &emulator,
        &[
            "no-such-snapshot",
            "batch-good",
            "batch-automated",
            "batch-expired",
        ],
        &[("ManualSnapshotRetentionPeriod", "9")],
    )
    .await;
    assert_eq!(
        together.status, 200,
        "case every bad entry in one list: expected 200, got {} with body {}",
        together.status, together.body
    );
    let codes: Vec<String> = errors(&together)
        .into_iter()
        .map(|(_, code, _)| code)
        .collect();
    assert_eq!(
        codes,
        vec![
            state::SNAPSHOT_FAILURE_NOT_FOUND,
            state::SNAPSHOT_FAILURE_INVALID_STATE,
            state::SNAPSHOT_FAILURE_INVALID_RETENTION_PERIOD,
        ],
        "case every bad entry in one list: expected one error per bad entry in request order, got {codes:?} with body {}",
        together.body
    );
}

/// A snapshot whose retention window has already ended under the requested
/// period is reported per entry — and the forced request deletes it instead.
#[tokio::test]
async fn a_window_already_in_the_past_is_forced_away_entry_by_entry() {
    let emulator = seeded_emulator(&["batch-inside"]).await;

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
        let identifier = format!("batch-expired-{age_days}-{period}");
        seed_snapshot(
            &emulator,
            &identifier,
            state::SNAPSHOT_TYPE_MANUAL,
            age_days,
            state::MANUAL_RETENTION_INDEFINITE,
        );

        let unforced = batch_modify(
            &emulator,
            &[identifier.as_str()],
            &[("ManualSnapshotRetentionPeriod", &period.to_string())],
        )
        .await;
        assert_eq!(
            unforced.status, 200,
            "case {case}: expected 200, got {} with body {}",
            unforced.status, unforced.body
        );
        assert_eq!(
            resources(&unforced),
            Vec::<String>::new(),
            "case {case}: expected no snapshot reported as modified, got {:?}",
            resources(&unforced)
        );
        let reported = errors(&unforced);
        assert_eq!(
            reported.len(),
            1,
            "case {case}: expected one per-entry error, got {reported:?} with body {}",
            unforced.body
        );
        assert_eq!(
            reported[0].1,
            state::SNAPSHOT_FAILURE_INVALID_RETENTION_PERIOD,
            "case {case}: expected the retention failure code, got {:?}",
            reported[0]
        );
        // The reported entry kept its snapshot and its period.
        let kept = describe(&emulator, &identifier).await;
        assert_eq!(
            kept.status, 200,
            "case {case}: expected the snapshot kept, got {} with body {}",
            kept.status, kept.body
        );
        assert_eq!(
            kept.text("ManualSnapshotRetentionPeriod").as_deref(),
            Some(state::MANUAL_RETENTION_INDEFINITE.to_string().as_str()),
            "case {case}: expected the stored period unchanged, got {:?}",
            kept.text("ManualSnapshotRetentionPeriod")
        );

        let forced = batch_modify(
            &emulator,
            &[identifier.as_str()],
            &[
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
            resources(&forced),
            vec![identifier.clone()],
            "case {case} forced: expected the snapshot reported as acted on, got {:?} with body {}",
            resources(&forced),
            forced.body
        );
        assert_eq!(
            errors(&forced),
            Vec::new(),
            "case {case} forced: expected no per-entry error, got {:?}",
            errors(&forced)
        );

        // ROUND TRIP: the forced entry removed the snapshot from the store.
        let gone = describe(&emulator, &identifier).await;
        assert_refused(
            &format!("{case} forced read back"),
            &gone,
            404,
            "ClusterSnapshotNotFound",
        );
    }

    // A window that has NOT ended is applied without `Force` being needed, so the
    // per-entry failure above is about the window and not about the flag.
    let inside = batch_modify(
        &emulator,
        &["batch-inside"],
        &[("ManualSnapshotRetentionPeriod", "3")],
    )
    .await;
    assert_eq!(
        inside.status, 200,
        "case a snapshot taken today kept for three days: expected 200, got {} with body {}",
        inside.status, inside.body
    );
    assert_eq!(
        resources(&inside),
        vec!["batch-inside"],
        "case a snapshot taken today kept for three days: expected it reported as modified, got {:?}",
        resources(&inside)
    );
    assert_retention(
        "a snapshot taken today kept for three days",
        &describe(&emulator, "batch-inside").await,
        3,
    );
}

/// What the REQUEST gets wrong refuses the whole call: a list longer than the
/// published batch limit, or a period outside the published range. Both leave
/// every stored snapshot exactly as it was.
#[tokio::test]
async fn a_request_level_fault_refuses_the_whole_call() {
    let emulator = seeded_emulator(&["batch-limit"]).await;
    let seeded = batch_modify(
        &emulator,
        &["batch-limit"],
        &[("ManualSnapshotRetentionPeriod", "45")],
    )
    .await;
    assert_eq!(
        seeded.status, 200,
        "case seed retention: expected 200, got {} with body {}",
        seeded.status, seeded.body
    );

    // The published batch limit, and one entry past it: at the limit the call is
    // served (every entry names no snapshot, so each is reported per entry),
    // one past it the whole call is refused.
    let limit = state::BATCH_SNAPSHOT_IDENTIFIER_LIMIT;
    let names: Vec<String> = (0..=limit).map(|n| format!("absent-{n}")).collect();
    let at_limit: Vec<&str> = names[..limit].iter().map(String::as_str).collect();
    let answer = batch_modify(
        &emulator,
        &at_limit,
        &[("ManualSnapshotRetentionPeriod", "9")],
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case a list at the published batch limit: expected 200, got {} with body {}",
        answer.status, answer.body
    );
    assert_eq!(
        errors(&answer).len(),
        limit,
        "case a list at the published batch limit: expected one error per entry, got {} with body {}",
        errors(&answer).len(),
        answer.body
    );

    let past_limit: Vec<&str> = names.iter().map(String::as_str).collect();
    let refused = batch_modify(
        &emulator,
        &past_limit,
        &[("ManualSnapshotRetentionPeriod", "9")],
    )
    .await;
    assert_refused(
        "a list one entry past the published batch limit",
        &refused,
        400,
        "BatchModifyClusterSnapshotsLimitExceededFault",
    );

    // One row per published retention bound, one step outside it, plus the
    // values below the sentinel and the extremes of the modelled integer.
    let (lower, upper) = state::MANUAL_RETENTION_BOUNDS;
    let below = (lower - 1).to_string();
    let above = (upper + 1).to_string();
    let under_sentinel = (state::MANUAL_RETENTION_INDEFINITE - 1).to_string();
    let periods: Vec<(&str, &str)> = vec![
        ("one below the documented lower bound", &below),
        ("one above the documented upper bound", &above),
        ("one below the indefinite sentinel", &under_sentinel),
        ("far below every admitted value", "-2147483648"),
        ("far above every admitted value", "2147483647"),
    ];
    for (case, period) in periods {
        let answer = batch_modify(
            &emulator,
            &["batch-limit"],
            &[("ManualSnapshotRetentionPeriod", period)],
        )
        .await;
        assert_refused(case, &answer, 400, "InvalidRetentionPeriodFault");

        let described = describe(&emulator, "batch-limit").await;
        assert_eq!(
            described.text("ManualSnapshotRetentionPeriod").as_deref(),
            Some("45"),
            "case {case}: expected the refused call to change nothing, got {:?}",
            described.text("ManualSnapshotRetentionPeriod")
        );
    }

    // An admitted period inside the same request shape is still served, so the
    // refusals above are about the value and not about the call.
    let served = batch_modify(
        &emulator,
        &["batch-limit"],
        &[("ManualSnapshotRetentionPeriod", &upper.to_string())],
    )
    .await;
    assert_eq!(
        served.status, 200,
        "case the documented upper bound: expected 200, got {} with body {}",
        served.status, served.body
    );
    assert_retention(
        "the documented upper bound",
        &describe(&emulator, "batch-limit").await,
        upper,
    );
}
