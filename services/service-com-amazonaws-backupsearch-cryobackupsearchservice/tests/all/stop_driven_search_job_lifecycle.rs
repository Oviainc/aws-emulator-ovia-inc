// SPDX-License-Identifier: CC0-1.0

//! The stop-driven search-job LIFECYCLE a client drives end to end over HTTP, with
//! nothing seeded into the job store: `StartSearchJob`, `StopSearchJob`, a POLL of
//! `GetSearchJob` to the terminal status, `ListSearchJobs` and the operation gated on
//! that terminal status, `StartSearchResultExportJob`.
//!
//! The rule under test: a stop the service accepts is the end state the job converges
//! on, so a caller that cancels a job and then polls it reaches `STOPPED` — the status
//! the stop asked for and never the one the job's own scan would otherwise have settled
//! to — and every operation keyed on that status (the listing's status filter, the export
//! start's precondition) observes it, while the `@idempotent` repeat of the cancel stays
//! an empty 200.
//!
//! What this suite adds over the two beside it (`stop_search_job_cancel_window.rs` and
//! `stop_search_job_status_gate.rs`, which pin the window a cancel is accepted in and the
//! refusals outside it) is the CALLER'S loop: the status is polled until it is one the
//! job never leaves, rather than read once after a fixed wait, and what is reachable
//! afterwards is asserted through the API instead of from a seeded record.
//!
//! Each assertion is made over a TABLE whose rows differ in what the rule generalises
//! over — how long after the start the cancel is sent (so which phase it lands in), what
//! this account holds and therefore which terminal status the scan would have reached on
//! its own, how many times the cancel is repeated, and the signing region — never over
//! one request.

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_backupsearch_cryobackupsearchservice::ops::types;
use service_com_amazonaws_backupsearch_cryobackupsearchservice::state;

use crate::export_listing::passable_role;
use crate::search_jobs::{
    call, cancel_search_job as cancel, emulator_with_engine,
    held_s3_recovery_points as held, seed_recovery_points, start_s3_search_job,
    PAST_THE_CREATING_INSTANT, WITHIN_THE_DUE_INSTANT,
};

/// A cancel sent with no wait at all, which is what a back-to-back client call does.
const BACK_TO_BACK: Duration = Duration::ZERO;

/// How long a poller waits between two `GetSearchJob` calls, and how many times it asks.
/// The product is DERIVED from the walk a cancelled job makes — the set-up span, the
/// scan's span and the stop's own span (`state::CREATING_DELAY` and two
/// `state::TRANSITION_DELAY`s), each counted from the transition before it — at twice
/// that walk, so the budget cannot drift from the spans it has to outlast and a loop that
/// gives up has found a job that never converges rather than one that was merely slow.
/// The loop returns as soon as the status is terminal, so the budget costs nothing when
/// the job does converge.
const POLL_INTERVAL: Duration = Duration::from_millis(100);
/// See [`POLL_INTERVAL`].
const POLL_BUDGET: Duration = state::CREATING_DELAY
    .saturating_add(state::TRANSITION_DELAY)
    .saturating_add(state::TRANSITION_DELAY)
    .saturating_mul(2);
/// See [`POLL_INTERVAL`].
const POLL_ATTEMPTS: usize = (POLL_BUDGET.as_millis() / POLL_INTERVAL.as_millis()) as usize;

/// One row: when the cancel is sent, the phase the job is in by then, what this account
/// holds, the terminal status the scan would have settled to had nothing cancelled it,
/// how many repeats of the cancel the row sends, and the region its calls are signed for.
struct LifecycleCase {
    case: &'static str,
    region: &'static str,
    wait_before_cancel: Duration,
    phase: &'static str,
    recovery_points: usize,
    scan_would_reach: &'static str,
    repeats: usize,
}

fn lifecycle_cases() -> Vec<LifecycleCase> {
    vec![
        LifecycleCase {
            case: "cancelled back to back, over a scope that resolves to nothing",
            region: "us-west-2",
            wait_before_cancel: BACK_TO_BACK,
            phase: state::CREATING,
            recovery_points: 0,
            scan_would_reach: state::FAILED,
            repeats: 1,
        },
        LifecycleCase {
            case: "cancelled inside the set-up phase, over a scope that resolves to one backup",
            region: "eu-west-1",
            wait_before_cancel: WITHIN_THE_DUE_INSTANT,
            phase: state::CREATING,
            recovery_points: 1,
            scan_would_reach: state::COMPLETED,
            repeats: 2,
        },
        LifecycleCase {
            case: "cancelled once the scan is running, over a scope that resolves to nothing",
            region: "ap-southeast-2",
            wait_before_cancel: PAST_THE_CREATING_INSTANT,
            phase: state::RUNNING,
            recovery_points: 0,
            scan_would_reach: state::FAILED,
            repeats: 3,
        },
        LifecycleCase {
            case: "cancelled once the scan is running, over a scope that resolves to three backups",
            region: "us-east-1",
            wait_before_cancel: PAST_THE_CREATING_INSTANT,
            phase: state::RUNNING,
            recovery_points: 3,
            scan_would_reach: state::COMPLETED,
            repeats: 1,
        },
    ]
}

/// Poll `GetSearchJob` the way the caller of a cancel does — until the status is one the
/// job never leaves — and answer that status together with every status seen on the way.
async fn poll_to_terminal(
    emulator: &MultiServiceRouter,
    region: &str,
    identifier: &str,
    case: &str,
) -> (String, Vec<String>) {
    let job_uri = format!("/search-jobs/{identifier}");
    let mut seen: Vec<String> = Vec::new();
    for _ in 0..POLL_ATTEMPTS {
        let polled = call(emulator, region, "GET", &job_uri, None).await;
        assert_eq!(
            polled.status, 200,
            "case {case:?}: a poll of a job this account holds must answer 200, got {} ({:?})",
            polled.status, polled.body
        );
        let status = polled.string("Status");
        if seen.last().map(String::as_str) != Some(status.as_str()) {
            seen.push(status.clone());
        }
        if state::TERMINAL_STATES.contains(&status.as_str()) {
            return (status, seen);
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    panic!("case {case:?}: the polled job never reached a terminal status, saw {seen:?}");
}

/// The identifiers `ListSearchJobs` reports through the `Status` filter `status`.
async fn listed_under(emulator: &MultiServiceRouter, region: &str, status: &str) -> Vec<String> {
    let listed = call(
        emulator,
        region,
        "GET",
        &format!("/search-jobs?Status={status}"),
        None,
    )
    .await;
    listed
        .get("SearchJobs")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|summary| {
            summary["SearchJobIdentifier"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

#[tokio::test]
async fn a_cancelled_job_polled_to_its_terminal_status_reaches_stopped() {
    for LifecycleCase {
        case,
        region,
        wait_before_cancel,
        phase,
        recovery_points,
        scan_would_reach,
        repeats,
    } in lifecycle_cases()
    {
        let (emulator, engine) = emulator_with_engine();
        seed_recovery_points(&engine, held(recovery_points));
        let identifier = start_s3_search_job(&emulator, region, case).await;
        let job_uri = format!("/search-jobs/{identifier}");

        tokio::time::sleep(wait_before_cancel).await;
        let before = call(&emulator, region, "GET", &job_uri, None).await;
        assert_eq!(
            before.string("Status"),
            phase,
            "case {case:?}: the cancel must land while the job is {phase}, got {:?}",
            before.get("Status")
        );

        let accepted = cancel(&emulator, region, &identifier).await;
        assert_eq!(
            accepted.status, 200,
            "case {case:?}: the cancel of a job that has not ended must be accepted, got {} ({:?})",
            accepted.status, accepted.body
        );
        assert_eq!(
            accepted.body,
            json!({}),
            "case {case:?}: the modelled output has no members, got {:?}",
            accepted.body
        );

        // The caller's own loop: poll until the status is one the job never leaves. It is
        // the status the cancel asked for, never the one the scan would have settled to.
        let (terminal, seen) = poll_to_terminal(&emulator, region, &identifier, case).await;
        assert_eq!(
            terminal, state::STOPPED,
            "case {case:?}: a polled cancelled job must reach stopped and not the {scan_would_reach} its scan would have reached, saw {seen:?}"
        );
        assert!(
            !seen.iter().any(|status| status == scan_would_reach),
            "case {case:?}: the scan's own {scan_would_reach} must never be observed after the cancel, saw {seen:?}"
        );

        let settled = call(&emulator, region, "GET", &job_uri, None).await;
        // The job was created before it was stopped, so both instants are present and
        // ordered: an absent one is a failure of the assertion and not a reason to skip it.
        let created = before
            .get("CreationTime")
            .as_f64()
            .unwrap_or_else(|| panic!(
                "case {case:?}: a job this account holds reports a CreationTime, got {:?}",
                before.get("CreationTime")
            ));
        let completed = settled
            .get("CompletionTime")
            .as_f64()
            .unwrap_or_else(|| panic!(
                "case {case:?}: a stopped job reports a CompletionTime, got {:?}",
                settled.get("CompletionTime")
            ));
        assert!(
            completed >= created,
            "case {case:?}: a stopped job completed at or after it was created, got {completed} against {created}"
        );

        // The `@idempotent` repeats a retrying client sends against the end state are the
        // same empty 200 and leave that end state where it is.
        for repeat in 1..=repeats {
            let again = cancel(&emulator, region, &identifier).await;
            assert_eq!(
                again.status, 200,
                "case {case:?}: repeat {repeat} of the cancel must be a no-op success, got {} ({:?})",
                again.status, again.body
            );
            assert_eq!(
                again.body,
                json!({}),
                "case {case:?}: repeat {repeat} must answer the modelled empty output, got {:?}",
                again.body
            );
        }
        assert_eq!(
            call(&emulator, region, "GET", &job_uri, None)
                .await
                .string("Status"),
            state::STOPPED,
            "case {case:?}: {repeats} repeat(s) of the cancel must leave the job stopped"
        );
    }
}

#[tokio::test]
async fn the_status_a_cancelled_job_converged_on_is_what_every_reader_of_it_observes() {
    for LifecycleCase {
        case,
        region,
        wait_before_cancel,
        recovery_points,
        scan_would_reach,
        ..
    } in lifecycle_cases()
    {
        let (emulator, engine) = emulator_with_engine();
        seed_recovery_points(&engine, held(recovery_points));
        let identifier = start_s3_search_job(&emulator, region, case).await;

        tokio::time::sleep(wait_before_cancel).await;
        let accepted = cancel(&emulator, region, &identifier).await;
        assert_eq!(
            accepted.status, 200,
            "case {case:?}: the cancel must be accepted, got {} ({:?})",
            accepted.status, accepted.body
        );
        let (terminal, seen) = poll_to_terminal(&emulator, region, &identifier, case).await;
        assert_eq!(
            terminal, state::STOPPED,
            "case {case:?}: the cancelled job must reach stopped, saw {seen:?}"
        );

        // The listing's `Status` filter reads the same record the poll did, so the job is
        // reported under the status it converged on and under no other — including the one
        // its own scan would have settled to.
        // `SearchJobState` accepts COMPLETED | FAILED | RUNNING | STOPPED | STOPPING
        // (model enum `SearchJobState`, rendered as `types::SEARCH_JOB_STATE_VALUES`).
        for candidate in types::SEARCH_JOB_STATE_VALUES {
            let listed = listed_under(&emulator, region, candidate).await;
            let expected: Vec<String> = if *candidate == state::STOPPED {
                vec![identifier.clone()]
            } else {
                Vec::new()
            };
            assert_eq!(
                listed, expected,
                "case {case:?}: the stopped job must be listed under {candidate} exactly when that is the status it converged on, got {listed:?}"
            );
        }

        // And what the converged status is the precondition of is reachable through the
        // API: an export of the results of a job whose results are final
        // (`state::EXPORTABLE_SEARCH_JOB_STATES`, which a cancelled job is in) is accepted,
        // which is the end of the lifecycle a client that cancels and waits drives.
        let exported = call(
            &emulator,
            region,
            "PUT",
            "/export-search-jobs",
            Some(json!({
                "SearchJobIdentifier": identifier,
                "RoleArn": passable_role("export"),
                "ExportSpecification": {
                    "s3ExportSpecification": {"DestinationBucket": "exports"}
                }
            })),
        )
        .await;
        assert_eq!(
            exported.status, 200,
            "case {case:?}: an export of a job that reached stopped (and not {scan_would_reach}) must be accepted, got {} ({:?})",
            exported.status, exported.body
        );
        let export_identifier = exported.string("ExportJobIdentifier");
        assert!(
            !export_identifier.is_empty(),
            "case {case:?}: the accepted export must name the record it minted, got {:?}",
            exported.body
        );

        // Read that export back, so the acceptance is a record and not an answer: it
        // names the search job the cancel drove to its terminal status.
        let export = call(
            &emulator,
            region,
            "GET",
            &format!("/export-search-jobs/{export_identifier}"),
            None,
        )
        .await;
        assert_eq!(
            export.status, 200,
            "case {case:?}: the minted export job must be readable, got {} ({:?})",
            export.status, export.body
        );
        assert_eq!(
            export.get("ExportJobIdentifier"),
            &Value::String(export_identifier.clone()),
            "case {case:?}: the export read back must be the one the start minted, got {:?}",
            export.body
        );
    }
}
