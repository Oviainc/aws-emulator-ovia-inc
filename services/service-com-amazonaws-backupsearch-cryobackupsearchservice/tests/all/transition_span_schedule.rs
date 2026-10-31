// SPDX-License-Identifier: CC0-1.0

//! The SCHEDULE the clock owes a search job's phases on, driven over HTTP exactly as a
//! client drives it: `StartSearchJob`, `GetSearchJob` and `StopSearchJob`.
//!
//! The rule under test: a status this service's clock owes a resource is due its own span
//! after the transition BEFORE it — the first of them one span after the call that started
//! the resource — so every phase lasts a whole span and a client that acts on the resource
//! after the call that put it in a phase is answered for THAT phase, not for one the clock
//! has already carried it past. For a search job that means the set-up phase lasts the
//! set-up span and the running scan lasts the span a scan takes, so a cancel sent any time
//! before the scan has ended is accepted and the job converges on `STOPPED`.
//!
//! What this suite adds over the two beside it is the SPACING of the schedule itself:
//! `owed_status_due_instants.rs` pins that each due instant is computed fallibly, and
//! `stop_search_job_cancel_window.rs` pins which statuses a cancel is accepted from —
//! neither pins that each phase is a whole span long, which is what decides whether a
//! client's second call lands in the phase its first call created.
//!
//! Each assertion is made over a TABLE whose rows differ in what the rule generalises
//! over — the delay between the two calls, the phase that delay lands in, the resource
//! type and filters of the scope, what the scope resolved to (and therefore the terminal
//! status the scan would have reached on its own) and the signing region — never over one
//! request.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_backupsearch_cryobackupsearchservice::ops::types;
use service_com_amazonaws_backupsearch_cryobackupsearchservice::state;

use crate::owed_status_due_instants::{assert_due_after_the_call, owed_search_job_statuses};
// The slack a wall-clock sleep needs to be past a due instant rather than racing it is
// the one the harness already declares (`PAST_THE_SPAN`), so the two cannot drift apart.
use crate::search_jobs::{
    call, cancel_search_job as cancel, emulator_with_engine, held_s3_recovery_points as held,
    seed_recovery_points, PAST_THE_SPAN as MARGIN,
};

/// One row of the schedule table: the `StartSearchJob` body, the region its calls are
/// signed for, how many recovery points this account holds (which decides the terminal
/// status the scan is owed) and that status.
struct ScheduleRow {
    case: &'static str,
    region: &'static str,
    body: Value,
    recovery_points: usize,
    terminal: &'static str,
}

fn schedule_rows() -> Vec<ScheduleRow> {
    vec![
        ScheduleRow {
            case: "a bare S3 scope resolving to nothing",
            region: "us-west-2",
            body: json!({"SearchScope": {"BackupResourceTypes": ["S3"]}}),
            recovery_points: 0,
            terminal: state::FAILED,
        },
        ScheduleRow {
            case: "a named and tagged S3 scope resolving to one recovery point",
            region: "eu-central-1",
            body: json!({
                "Name": "span-schedule-s3",
                "Tags": {"env": "test"},
                "SearchScope": {"BackupResourceTypes": ["S3"]}
            }),
            recovery_points: 1,
            terminal: state::COMPLETED,
        },
        ScheduleRow {
            case: "an S3 scope with item filters resolving to three recovery points",
            region: "ap-southeast-2",
            body: json!({
                "ClientToken": "span-schedule-token",
                "SearchScope": {"BackupResourceTypes": ["S3"]},
                "ItemFilters": {
                    "S3ItemFilters": [{
                        "ObjectKeys": [{"Value": "bucket", "Operator": "BEGINS_WITH"}]
                    }]
                }
            }),
            recovery_points: 3,
            terminal: state::COMPLETED,
        },
        ScheduleRow {
            case: "an EBS scope resolving to nothing because the account holds S3 backups",
            region: "us-east-1",
            body: json!({
                "Name": "span-schedule-ebs",
                "SearchScope": {"BackupResourceTypes": ["EBS"]}
            }),
            recovery_points: 2,
            terminal: state::FAILED,
        },
    ]
}

/// `StartSearchJob` for `row`, answering the identifier it minted and the interval the
/// handler read its clock in.
async fn start(
    emulator: &MultiServiceRouter,
    row: &ScheduleRow,
) -> (String, SystemTime, SystemTime) {
    let before = SystemTime::now();
    let started = call(emulator, row.region, "PUT", "/search-jobs", Some(row.body.clone())).await;
    let after = SystemTime::now();
    assert_eq!(
        started.status, 200,
        "case {:?}: StartSearchJob must accept the request, got {} ({:?})",
        row.case, started.status, started.body
    );
    (started.string("SearchJobIdentifier"), before, after)
}

#[tokio::test]
async fn each_owed_phase_is_due_one_span_after_the_phase_before_it() {
    for row in schedule_rows() {
        let (emulator, engine) = emulator_with_engine();
        seed_recovery_points(&engine, held(row.recovery_points));
        let (identifier, before, after) = start(&emulator, &row).await;

        let owed = owed_search_job_statuses(&engine, &identifier);
        assert_eq!(
            owed.iter().map(|(_, status)| status.as_str()).collect::<Vec<_>>(),
            vec![state::RUNNING, row.terminal],
            "case {:?}: a new search job owes RUNNING and then the terminal status its \
             scope gives it, got {owed:?}",
            row.case
        );

        // The first phase is owed a span after the call itself...
        assert_due_after_the_call(
            row.case,
            "the RUNNING phase",
            owed[0].0,
            before,
            after,
            state::CREATING_DELAY,
        );
        // ...and every later one exactly a span after the phase before it, which is
        // measurable without clock slack because both instants are derived from the same
        // reading.
        let gap = owed[1]
            .0
            .duration_since(owed[0].0)
            .unwrap_or_else(|_| panic!("case {:?}: the schedule must run forwards", row.case));
        assert_eq!(
            gap,
            state::SCAN_DELAY,
            "case {:?}: the terminal status must be owed one scan span after the RUNNING \
             phase, got {gap:?}",
            row.case
        );
        // A phase that lasts no time at all would be skipped by the very next call, so
        // the span the schedule is spaced on is non-zero.
        assert!(
            gap > Duration::ZERO && state::CREATING_DELAY > Duration::ZERO,
            "case {:?}: neither span may be zero, got {:?} then {gap:?}",
            row.case,
            state::CREATING_DELAY
        );

        // What the client sees of that schedule: the job is in the phase the call created
        // it in, and `CREATING` is the unmodelled status the service reports for it.
        let fetched = call(
            &emulator,
            row.region,
            "GET",
            &format!("/search-jobs/{identifier}"),
            None,
        )
        .await;
        assert_eq!(
            fetched.string("Status"),
            state::CREATING,
            "case {:?}: a job read straight after the start call is still being set up, \
             got {:?}",
            row.case,
            fetched.get("Status")
        );
        assert!(
            !types::SEARCH_JOB_STATE_VALUES.contains(&state::CREATING),
            "case {:?}: the set-up status is the unmodelled one the service reports, got {:?}",
            row.case,
            types::SEARCH_JOB_STATE_VALUES
        );
    }
}

/// One row of the cancel table: how long after the start the cancel is sent, and the
/// phase the schedule above puts the job in by then.
struct CancelRow {
    case: &'static str,
    wait_before_cancel: Duration,
    phase: &'static str,
}

/// The delays a cancel is sent after, in ascending order: a back-to-back call, a delay of
/// the order a client's intervening round trips take, the start of the running scan, and
/// halfway through it. Every one of them is before the terminal status falls due, so the
/// cancel is accepted in all of them.
fn cancel_rows() -> Vec<CancelRow> {
    vec![
        CancelRow {
            case: "cancelled back to back",
            wait_before_cancel: Duration::ZERO,
            phase: state::CREATING,
        },
        CancelRow {
            case: "cancelled after a client's intervening round trips",
            wait_before_cancel: Duration::from_millis(900),
            phase: state::CREATING,
        },
        CancelRow {
            case: "cancelled just after the scan started running",
            wait_before_cancel: state::CREATING_DELAY.saturating_add(MARGIN),
            phase: state::RUNNING,
        },
        CancelRow {
            case: "cancelled halfway through the running scan",
            wait_before_cancel: state::CREATING_DELAY
                .saturating_add(state::SCAN_DELAY / 2),
            phase: state::RUNNING,
        },
    ]
}

#[tokio::test]
async fn a_cancel_sent_at_any_delay_before_the_scan_ends_is_accepted_and_converges_on_stopped() {
    // Every row is started BEFORE any of the waits, so the waits overlap instead of
    // adding up, and each row's cancel still lands at its own delay after its own start.
    let mut started: Vec<(ScheduleRow, MultiServiceRouter, String)> = Vec::new();
    for row in schedule_rows() {
        let (emulator, engine) = emulator_with_engine();
        seed_recovery_points(&engine, held(row.recovery_points));
        let (identifier, _, _) = start(&emulator, &row).await;
        started.push((row, emulator, identifier));
    }
    let started_at = tokio::time::Instant::now();

    // The rows above are crossed with the delays: each scope is cancelled at one delay,
    // and the delays are walked in ascending order so one timeline carries them all.
    for (index, cancel_row) in cancel_rows().into_iter().enumerate() {
        let (row, emulator, identifier) = &started[index % started.len()];
        let case = format!("{:?} {}", row.case, cancel_row.case);
        tokio::time::sleep_until(started_at + cancel_row.wait_before_cancel).await;

        let job_uri = format!("/search-jobs/{identifier}");
        let polled = call(emulator, row.region, "GET", &job_uri, None).await;
        assert_eq!(
            polled.string("Status"),
            cancel_row.phase,
            "case {case}: the cancel must land while the job is {}, got {:?}",
            cancel_row.phase,
            polled.get("Status")
        );

        let accepted = cancel(emulator, row.region, identifier).await;
        assert_eq!(
            accepted.status, 200,
            "case {case}: a cancel sent before the scan ended must be accepted, got {} ({:?})",
            accepted.status, accepted.body
        );
        assert_eq!(
            accepted.body,
            json!({}),
            "case {case}: the modelled output has no members, got {:?}",
            accepted.body
        );
        let stopping = call(emulator, row.region, "GET", &job_uri, None).await;
        assert_eq!(
            stopping.string("Status"),
            state::STOPPING,
            "case {case}: a job whose cancel is in flight is stopping, got {:?}",
            stopping.get("Status")
        );
    }

    // One span after the last cancel, every job has converged on the status the cancel
    // asked for — never on the terminal status its own scan would have reached.
    tokio::time::sleep(state::TRANSITION_DELAY.saturating_add(MARGIN)).await;
    for (row, emulator, identifier) in &started {
        let settled = call(
            emulator,
            row.region,
            "GET",
            &format!("/search-jobs/{identifier}"),
            None,
        )
        .await;
        assert_eq!(
            settled.string("Status"),
            state::STOPPED,
            "case {:?}: a cancelled job settles to stopped and not to the {} its scan \
             would have reached, got {:?}",
            row.case,
            row.terminal,
            settled.get("Status")
        );
        assert!(
            settled.get("CompletionTime").as_f64().is_some(),
            "case {:?}: a stopped job carries a CompletionTime, got {:?}",
            row.case,
            settled.get("CompletionTime")
        );
    }
}
