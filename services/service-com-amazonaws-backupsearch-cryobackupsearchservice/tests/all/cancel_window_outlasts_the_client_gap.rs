// SPDX-License-Identifier: CC0-1.0

//! The GAP a client may leave between starting a search job and cancelling it, driven
//! over HTTP exactly as a client drives it: `StartSearchJob`, `GetSearchJob` and
//! `StopSearchJob`.
//!
//! The rule under test: a search job the service has accepted stays in a status a cancel
//! acts on for its whole owed schedule, and that schedule is spaced to outlast the gap
//! between two calls of one client — its own intervening work, its retries and a loaded
//! host — so a cancel sent SECONDS after the start is still the empty 200 and the job
//! converges on `STOPPED`, never the conflict the service answers a job whose scan has
//! already ended with. A scan AWS runs for minutes is compressed here, and a compression
//! a client's next call can outrun would answer that conflict for jobs AWS cancels.
//!
//! What this suite adds over the three beside it is the SIZE of that window measured in
//! wall-clock time a client takes rather than in multiples of the service's own span:
//! `transition_span_schedule.rs` pins the spacing of the schedule (each phase one span
//! after the one before it), `stop_search_job_cancel_window.rs` pins which statuses a
//! cancel is accepted from and the conflict past them, and
//! `stop_driven_search_job_lifecycle.rs` pins the caller's poll to `STOPPED`. None of
//! them pins that the window is long enough for a client whose second call comes several
//! seconds after its first, which is what decides whether a cancel AWS accepts is
//! accepted here.
//!
//! Each assertion is made over a TABLE whose rows differ in what the rule generalises
//! over — the gap the client leaves, what this account holds (and therefore the terminal
//! status the scan would have reached on its own) and the signing region — never over one
//! request.

use std::time::Duration;

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_backupsearch_cryobackupsearchservice::state;

use crate::search_jobs::{
    call, cancel_search_job as cancel, emulator_with_engine, held_s3_recovery_points as held,
    seed_recovery_points, start_s3_search_job, PAST_THE_SPAN as MARGIN,
};

/// One row: the gap the client leaves between its two calls, the region they are signed
/// for, how many recovery points this account holds and the terminal status the scan
/// would have reached on its own had nothing cancelled it.
struct GapCase {
    case: &'static str,
    region: &'static str,
    gap: Duration,
    recovery_points: usize,
    scan_would_reach: &'static str,
}

/// The gaps the rows leave between the start and the cancel, in ascending order. They are
/// WALL-CLOCK durations a client takes rather than multiples of this service's own span:
/// what a client leaves between two calls is its own work — calls against this and other
/// services, SDK retries, a host under load — and is not derived from the schedule it is
/// acting on. Every one of them is inside the schedule a started job walks, which the
/// test asserts before it uses the row, so the cancel is accepted in all of them.
fn gap_cases() -> Vec<GapCase> {
    vec![
        GapCase {
            case: "cancelled back to back, over a scope that resolves to nothing",
            region: "us-west-2",
            gap: Duration::ZERO,
            recovery_points: 0,
            scan_would_reach: state::FAILED,
        },
        GapCase {
            case: "cancelled a round trip later, over a scope that resolves to one backup",
            region: "eu-west-1",
            gap: Duration::from_secs(1),
            recovery_points: 1,
            scan_would_reach: state::COMPLETED,
        },
        GapCase {
            case: "cancelled after the client's own intervening work, over two backups",
            region: "ap-southeast-2",
            gap: Duration::from_secs(4),
            recovery_points: 2,
            scan_would_reach: state::COMPLETED,
        },
        GapCase {
            case: "cancelled seconds later from a loaded host, over a scope that resolves \
                   to nothing",
            region: "us-east-1",
            gap: Duration::from_secs(8),
            recovery_points: 0,
            scan_would_reach: state::FAILED,
        },
    ]
}

/// The whole schedule a started job walks before its scan ends: the set-up span and then
/// the scan's own span, each counted from the transition before it. This is the window
/// every row's cancel has to land inside.
fn schedule() -> Duration {
    state::CREATING_DELAY.saturating_add(state::TRANSITION_DELAY)
}

#[tokio::test]
async fn a_cancel_sent_a_client_sized_gap_after_the_start_is_accepted_and_converges_on_stopped() {
    // Every row's job is started BEFORE any of the waits, so the gaps overlap on one
    // timeline instead of adding up, and each row's cancel still lands at its own gap
    // after its own start.
    let mut started: Vec<(GapCase, MultiServiceRouter, String)> = Vec::new();
    for row in gap_cases() {
        let (emulator, engine) = emulator_with_engine();
        seed_recovery_points(&engine, held(row.recovery_points));
        let identifier = start_s3_search_job(&emulator, row.region, row.case).await;
        started.push((row, emulator, identifier));
    }
    let started_at = tokio::time::Instant::now();

    for (row, emulator, identifier) in &started {
        let case = row.case;
        let gap = row.gap;
        // The gap is the client's, so the schedule is what has to outlast it: a schedule
        // shorter than the gap carries the job past its scan and turns the cancel below
        // into the conflict the service answers an ended job with.
        let schedule = schedule();
        assert!(
            gap.saturating_add(MARGIN) < schedule,
            "case {case:?}: a gap of {gap:?} between a client's two calls must be inside \
             the {schedule:?} a started job walks before its scan ends"
        );
        tokio::time::sleep_until(started_at + gap).await;

        let job_uri = format!("/search-jobs/{identifier}");
        let polled = call(emulator, row.region, "GET", &job_uri, None).await;
        let phase = polled.string("Status");
        assert!(
            !state::TERMINAL_STATES.contains(&phase.as_str()),
            "case {case:?}: {gap:?} after the start the job must still be in a phase a \
             cancel acts on, got {phase:?}"
        );

        let accepted = cancel(emulator, row.region, identifier).await;
        assert_eq!(
            accepted.status, 200,
            "case {case:?}: a cancel sent {gap:?} after the start must be accepted, got {} ({:?})",
            accepted.status, accepted.body
        );
        assert_eq!(
            accepted.body,
            json!({}),
            "case {case:?}: the modelled output has no members, got {:?}",
            accepted.body
        );

        let stopping = call(emulator, row.region, "GET", &job_uri, None).await;
        assert_eq!(
            stopping.string("Status"),
            state::STOPPING,
            "case {case:?}: a job whose cancel is in flight is stopping, got {:?}",
            stopping.get("Status")
        );
    }

    // One span after the last cancel, every job has converged on the status its cancel
    // asked for — never on the terminal status its own scan would have reached.
    tokio::time::sleep(state::TRANSITION_DELAY.saturating_add(MARGIN)).await;
    for (row, emulator, identifier) in &started {
        let case = row.case;
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
            "case {case:?}: a cancelled job settles to stopped and not to the {} its scan \
             would have reached, got {:?}",
            row.scan_would_reach,
            settled.get("Status")
        );
        assert!(
            settled.get("CompletionTime").as_f64().is_some(),
            "case {case:?}: a stopped job carries a CompletionTime, got {:?}",
            settled.get("CompletionTime")
        );
    }
}

/// One row of the per-phase table: the phase a client's next call lands in, how long
/// after the start that call comes, how far into the phase that is, and the span the
/// phase lasts.
struct PhaseCase {
    case: &'static str,
    region: &'static str,
    gap: Duration,
    into_the_phase: Duration,
    span: Duration,
    phase: &'static str,
}

/// One row per transient phase a scan passes through, each with a client-sized gap that
/// lands inside it.
fn phase_cases() -> Vec<PhaseCase> {
    let a_second = Duration::from_secs(1);
    vec![
        PhaseCase {
            case: "a client whose next call lands a second into the set-up phase",
            region: "us-west-2",
            gap: a_second,
            into_the_phase: a_second,
            span: state::CREATING_DELAY,
            phase: state::CREATING,
        },
        PhaseCase {
            case: "a client whose next call lands two seconds into the running scan",
            region: "eu-central-1",
            gap: state::CREATING_DELAY.saturating_add(2 * a_second),
            into_the_phase: 2 * a_second,
            span: state::TRANSITION_DELAY,
            phase: state::RUNNING,
        },
    ]
}

#[tokio::test]
async fn every_phase_of_the_schedule_outlasts_a_client_gap_of_its_own() {
    // The rule holds per PHASE and not only for the schedule as a whole: a client whose
    // next call lands in the set-up phase and one whose next call lands in the running
    // scan must both find the job in the phase the call before it created, so each span
    // has to outlast a client's gap on its own.
    for PhaseCase {
        case,
        region,
        gap,
        into_the_phase,
        span,
        phase,
    } in phase_cases()
    {
        assert!(
            into_the_phase.saturating_add(MARGIN) < span,
            "case {case:?}: {into_the_phase:?} into the phase must still be inside the \
             {span:?} that phase lasts"
        );

        let (emulator, engine) = emulator_with_engine();
        seed_recovery_points(&engine, held(1));
        let identifier = start_s3_search_job(&emulator, region, case).await;
        tokio::time::sleep(gap).await;

        let job_uri = format!("/search-jobs/{identifier}");
        let polled = call(&emulator, region, "GET", &job_uri, None).await;
        assert_eq!(
            polled.string("Status"),
            phase,
            "case {case:?}: {gap:?} after the start the job must report {phase}, got {:?}",
            polled.get("Status")
        );

        let accepted = cancel(&emulator, region, &identifier).await;
        assert_eq!(
            accepted.status, 200,
            "case {case:?}: a cancel landing in {phase} must be accepted, got {} ({:?})",
            accepted.status, accepted.body
        );
    }
}
