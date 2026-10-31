// SPDX-License-Identifier: CC0-1.0

//! How long a search job's scan RUNS, seen from the two sides a client can see it from and
//! driven over HTTP exactly as a client drives it: `StartSearchJob`, `GetSearchJob` and
//! `StopSearchJob`.
//!
//! The rule under test: a search job this service has accepted stays in a status a cancel
//! acts on for as long as its scan runs, and a scan runs for minutes and longer — so the
//! span this emulator compresses that run into has to OUTLAST the gap a client leaves
//! between two of its calls (its own intervening calls, its retries, a loaded host) while
//! still bringing the job to the status its scan ended in INSIDE the wait a client that
//! polls for that status gives it. Anywhere inside the run the cancel is the modelled empty
//! 200 and the job converges on `STOPPED`; from the instant the scan's own terminal status
//! falls due the very same cancel is the modelled `ConflictException` (409) naming the
//! status the job is in.
//!
//! What this suite adds over the three beside it: `transition_span_schedule.rs` pins the
//! SPACING of the phases, `stop_search_job_cancel_window.rs` which statuses a cancel is
//! accepted from, and `cancel_window_outlasts_the_client_gap.rs` that a cancel sent a few
//! seconds after the start is still accepted. None of them pins WHERE the window ends, nor
//! that it ends inside the wait a polling client gives the status — the two bounds that
//! together decide whether a cancel the service accepts is accepted here.
//!
//! Each assertion is made over a TABLE whose rows differ in what the rule generalises over
//! — the gap the client leaves (from none at all to past the end of the run), the phase
//! that gap lands in, what this account holds and therefore the terminal status the scan
//! would have reached on its own, and the signing region — never over one request.
//!
//! Nothing here waits out the window: the instants a transition is owed from are STORED, so
//! each row carries the clock to its own gap (`settle_a_span_ahead`) instead of sleeping
//! through it, which is what lets the table span gaps no suite could afford to sit out.

use std::time::Duration;

use serde_json::json;

use service_com_amazonaws_backupsearch_cryobackupsearchservice::state;

use crate::search_jobs::{
    call, cancel_search_job as cancel, emulator_with_engine, held_s3_recovery_points as held,
    seed_recovery_points, settle_a_span_ahead, start_s3_search_job, PAST_THE_SPAN as MARGIN,
};

/// The gap ONE client can leave between two of its calls. Not a round trip: between
/// starting a job and cancelling it a client makes its own calls against this and other
/// services, retries what the SDK retries for it, and runs on a host under load — which is
/// tens of seconds and not milliseconds. The window a cancel has must outlast this.
const A_CLIENT_GAP: Duration = Duration::from_secs(20);

/// About how long a client that polls for an asynchronous status waits before it gives up
/// and reports the wait as failed: a minute. The whole schedule a started job walks must
/// end inside this, or a client polling for the status its scan ended in never reads it.
const A_POLLING_CLIENTS_WAIT: Duration = Duration::from_secs(60);

/// The whole schedule a started job walks before its scan ends: the set-up span and then
/// the span the scan runs for, each counted from the transition before it. Derived from the
/// crate's own spans, so this suite cannot drift from them.
fn schedule() -> Duration {
    state::CREATING_DELAY.saturating_add(state::SCAN_DELAY)
}

/// One row: the gap the client leaves between starting the job and cancelling it, the
/// status the job reports by then, how many recovery points this account holds and the
/// terminal status the scan would have reached on its own.
struct GapCase {
    case: &'static str,
    region: &'static str,
    gap: Duration,
    phase: &'static str,
    recovery_points: usize,
    scan_would_reach: &'static str,
}

impl GapCase {
    /// Whether the gap lands while the scan is still running, which is what decides
    /// whether the cancel is accepted — read off the status the row expects rather than
    /// restated, so a row cannot claim one and assert the other.
    fn inside_the_run(&self) -> bool {
        !state::TERMINAL_STATES.contains(&self.phase)
    }
}

fn gap_cases() -> Vec<GapCase> {
    vec![
        GapCase {
            case: "a client that cancels back to back, over a scope resolving to nothing",
            region: "us-west-2",
            gap: Duration::ZERO,
            phase: state::CREATING,
            recovery_points: 0,
            scan_would_reach: state::FAILED,
        },
        GapCase {
            case: "a client whose cancel comes one round trip later, over one backup",
            region: "eu-west-1",
            gap: Duration::from_secs(1),
            phase: state::CREATING,
            recovery_points: 1,
            scan_would_reach: state::COMPLETED,
        },
        GapCase {
            case: "a client that cancels just after the scan started running, over two backups",
            region: "ap-southeast-2",
            gap: state::CREATING_DELAY.saturating_add(Duration::from_secs(1)),
            phase: state::RUNNING,
            recovery_points: 2,
            scan_would_reach: state::COMPLETED,
        },
        GapCase {
            case: "a client that comes back ten seconds later, over a scope resolving to nothing",
            region: "us-east-1",
            gap: Duration::from_secs(10),
            phase: state::RUNNING,
            recovery_points: 0,
            scan_would_reach: state::FAILED,
        },
        GapCase {
            case: "a client that comes back after a gap of its own calls and retries",
            region: "eu-central-1",
            gap: A_CLIENT_GAP,
            phase: state::RUNNING,
            recovery_points: 1,
            scan_would_reach: state::COMPLETED,
        },
        GapCase {
            case: "a client that cancels at the very end of the run, over three backups",
            region: "us-west-2",
            gap: schedule().saturating_sub(MARGIN),
            phase: state::RUNNING,
            recovery_points: 3,
            scan_would_reach: state::COMPLETED,
        },
        GapCase {
            case: "a client that cancels at the instant the scan's own outcome falls due",
            region: "eu-west-1",
            gap: schedule(),
            phase: state::FAILED,
            recovery_points: 0,
            scan_would_reach: state::FAILED,
        },
        GapCase {
            case: "a client that cancels well after the scan ended, over two backups",
            region: "ap-northeast-1",
            gap: schedule().saturating_add(Duration::from_secs(10)),
            phase: state::COMPLETED,
            recovery_points: 2,
            scan_would_reach: state::COMPLETED,
        },
    ]
}

#[tokio::test]
async fn a_cancel_is_accepted_for_the_whole_run_of_the_scan_and_refused_once_it_has_ended() {
    for row in gap_cases() {
        let case = row.case;
        let gap = row.gap;
        let (emulator, engine) = emulator_with_engine();
        seed_recovery_points(&engine, held(row.recovery_points));
        let identifier = start_s3_search_job(&emulator, row.region, case).await;
        let job_uri = format!("/search-jobs/{identifier}");

        // The client's gap, carried on the clock rather than waited out.
        settle_a_span_ahead(&engine, gap);
        let polled = call(&emulator, row.region, "GET", &job_uri, None).await;
        assert_eq!(
            polled.string("Status"),
            row.phase,
            "case {case:?}: {gap:?} after the start the job must report {}, got {:?}",
            row.phase,
            polled.get("Status")
        );

        let answered = cancel(&emulator, row.region, &identifier).await;
        if !row.inside_the_run() {
            // The scan has ended, so there is nothing left to cancel: the service refuses
            // with the conflict, naming the status the job is in.
            assert_eq!(
                answered.status, 409,
                "case {case:?}: a cancel {gap:?} after the start, with the scan already \
                 ended, must be the conflict, got {} ({:?})",
                answered.status, answered.body
            );
            assert_eq!(
                answered.error_code.as_deref(),
                Some("ConflictException"),
                "case {case:?}: the refusal is the modelled conflict, got {:?}",
                answered.error_code
            );
            assert_eq!(
                answered.body.get("message").and_then(|text| text.as_str()),
                Some(state::stop_conflict_message(row.phase).as_str()),
                "case {case:?}: the refusal names the status the job is in, got {:?}",
                answered.body.get("message")
            );
            continue;
        }

        assert_eq!(
            answered.status, 200,
            "case {case:?}: a cancel sent {gap:?} after the start, while the scan is still \
             running, must be accepted, got {} ({:?})",
            answered.status, answered.body
        );
        assert_eq!(
            answered.body,
            json!({}),
            "case {case:?}: the modelled output has no members, got {:?}",
            answered.body
        );

        let stopping = call(&emulator, row.region, "GET", &job_uri, None).await;
        assert_eq!(
            stopping.string("Status"),
            state::STOPPING,
            "case {case:?}: a job whose cancel is in flight is stopping, got {:?}",
            stopping.get("Status")
        );

        // And it converges on the status the cancel asked for, never on the one its own
        // scan would have reached.
        settle_a_span_ahead(&engine, state::TRANSITION_DELAY.saturating_add(MARGIN));
        let settled = call(&emulator, row.region, "GET", &job_uri, None).await;
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

#[test]
fn the_run_of_a_scan_outlasts_a_client_gap_and_still_ends_inside_a_polling_wait() {
    let schedule = schedule();

    // One side of the rule: the window a cancel has is the run of the scan, so it has to
    // be longer than the gap a client leaves between the two calls — otherwise the cancel
    // the service accepts is answered with the conflict, for no reason the client can see.
    assert!(
        A_CLIENT_GAP.saturating_add(MARGIN) < schedule,
        "a client gap of {A_CLIENT_GAP:?} must be inside the {schedule:?} a started job \
         walks before its scan ends"
    );
    // The other side: a client that polls for the status its scan ended in has to reach
    // it, so the whole schedule ends well inside the wait such a client gives it.
    assert!(
        schedule.saturating_add(MARGIN) < A_POLLING_CLIENTS_WAIT,
        "the {schedule:?} a started job walks must end inside the {A_POLLING_CLIENTS_WAIT:?} \
         a polling client waits for an asynchronous status"
    );
    // And the run of a scan is WORK, not a record moving from one phase to the next, so it
    // is the longer of the two spans this service keeps.
    assert!(
        state::SCAN_DELAY > state::TRANSITION_DELAY,
        "the span a scan runs for ({:?}) must be longer than the span a phase change takes \
         ({:?}), because the run is the window a cancel has",
        state::SCAN_DELAY,
        state::TRANSITION_DELAY
    );
}
