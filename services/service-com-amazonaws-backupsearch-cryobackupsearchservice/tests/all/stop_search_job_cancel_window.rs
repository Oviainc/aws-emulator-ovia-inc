// SPDX-License-Identifier: CC0-1.0

//! The WINDOW `StopSearchJob` accepts a cancel in, driven over HTTP exactly as a client
//! drives it: `StartSearchJob`, `StopSearchJob` and `GetSearchJob`.
//!
//! The rule under test: a cancel is accepted for a search job the service has accepted
//! and that has not yet reached a status it never leaves — whatever phase it is in when
//! the cancel arrives — which moves it to `STOPPING` and has the clock owe it `STOPPED`,
//! replacing whatever its own scan would have reached; a cancel is refused with the
//! modelled `ConflictException` (409) only for a job already in a terminal status this
//! operation does not produce, and with `ResourceNotFoundException` (404) for an
//! identifier naming no job.
//!
//! Each assertion is made over a TABLE whose rows differ in what the rule generalises
//! over — how long after the start the cancel is sent (so which phase it lands in), what
//! the scope resolved to and therefore which terminal status the scan would have reached
//! on its own, the number of cancels in flight, and the signing region — never over one
//! request.

use std::time::Duration;

use serde_json::json;

use service_com_amazonaws_backupsearch_cryobackupsearchservice::ops::types;
use service_com_amazonaws_backupsearch_cryobackupsearchservice::state;

use crate::search_jobs::{
    call, emulator_with_engine, recovery_point, seed_recovery_points, start_s3_search_job,
    Answer, PAST_THE_CREATING_INSTANT, PAST_THE_DUE_INSTANT, WITHIN_THE_DUE_INSTANT,
};

/// A cancel sent with no wait at all, which is what a back-to-back client call does.
const AT_ONCE: Duration = Duration::ZERO;

/// One row: when the cancel is sent, the phase the job is in by then, and what this
/// account holds — which decides the terminal status the scan would have reached if
/// nothing had cancelled it.
struct CancelCase {
    case: &'static str,
    region: &'static str,
    wait_before_cancel: Duration,
    phase: &'static str,
    recovery_points: usize,
    scan_would_reach: &'static str,
}

fn cancel_cases() -> Vec<CancelCase> {
    vec![
        CancelCase {
            case: "cancelled with no wait at all, over a scope that resolves to nothing",
            region: "us-west-2",
            wait_before_cancel: AT_ONCE,
            phase: state::CREATING,
            recovery_points: 0,
            scan_would_reach: state::FAILED,
        },
        CancelCase {
            case: "cancelled inside the set-up phase, over a scope that resolves to one backup",
            region: "eu-west-1",
            wait_before_cancel: WITHIN_THE_DUE_INSTANT,
            phase: state::CREATING,
            recovery_points: 1,
            scan_would_reach: state::COMPLETED,
        },
        CancelCase {
            case: "cancelled once the scan is running, over a scope that resolves to two backups",
            region: "ap-southeast-2",
            wait_before_cancel: PAST_THE_CREATING_INSTANT,
            phase: state::RUNNING,
            recovery_points: 2,
            scan_would_reach: state::COMPLETED,
        },
        CancelCase {
            case: "cancelled once the scan is running, over a scope that resolves to nothing",
            region: "us-east-1",
            wait_before_cancel: PAST_THE_CREATING_INSTANT,
            phase: state::RUNNING,
            recovery_points: 0,
            scan_would_reach: state::FAILED,
        },
    ]
}

/// `count` S3 recovery points this account holds, which is what a scope resolves against.
fn held(count: usize) -> Vec<types::SearchJobBackupsResult> {
    (0..count)
        .map(|index| {
            recovery_point(
                // `ResourceType` accepts S3 and EBS (model enum `ResourceType`); the
                // scope these rows start with selects S3.
                "S3",
                &format!("arn:aws:backup:us-west-2:000000000000:recovery-point:rp-{index}"),
                &format!("arn:aws:s3:::bucket-{index}"),
                std::time::SystemTime::now(),
            )
        })
        .collect()
}

/// A cancel of `identifier`.
async fn cancel(
    emulator: &smithy_server_runtime::MultiServiceRouter,
    region: &str,
    identifier: &str,
) -> Answer {
    call(
        emulator,
        region,
        "PUT",
        &format!("/search-jobs/{identifier}/actions/cancel"),
        None,
    )
    .await
}

#[tokio::test]
async fn a_cancel_of_a_job_that_has_not_ended_is_accepted_and_converges_on_stopped() {
    for CancelCase {
        case,
        region,
        wait_before_cancel,
        phase,
        recovery_points,
        scan_would_reach,
    } in cancel_cases()
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

        let stopped = cancel(&emulator, region, &identifier).await;
        assert_eq!(
            stopped.status, 200,
            "case {case:?}: a cancel of a job that has not ended must succeed, got {} ({:?})",
            stopped.status, stopped.body
        );
        assert_eq!(
            stopped.body,
            json!({}),
            "case {case:?}: the modelled output is empty, got {:?}",
            stopped.body
        );

        // The cancel is in flight, so the transient status is observable and a repeat
        // against it is the `@idempotent` no-op success.
        let stopping = call(&emulator, region, "GET", &job_uri, None).await;
        assert_eq!(
            stopping.string("Status"),
            state::STOPPING,
            "case {case:?}: a job whose cancel is in flight is stopping, got {:?}",
            stopping.get("Status")
        );
        let repeat = cancel(&emulator, region, &identifier).await;
        assert_eq!(
            repeat.status, 200,
            "case {case:?}: a repeat while the cancel is in flight must be a no-op success, got {} ({:?})",
            repeat.status, repeat.body
        );

        // And the job converges on the status the cancel asked for, never on the one its
        // own scan would have reached.
        tokio::time::sleep(PAST_THE_DUE_INSTANT).await;
        let settled = call(&emulator, region, "GET", &job_uri, None).await;
        assert_eq!(
            settled.string("Status"),
            state::STOPPED,
            "case {case:?}: a cancelled job settles to stopped and not to the {scan_would_reach} its scan would have reached, got {:?}",
            settled.get("Status")
        );
        assert!(
            settled.get("CompletionTime").as_f64().is_some(),
            "case {case:?}: a stopped job carries a CompletionTime, got {:?}",
            settled.get("CompletionTime")
        );
        let again = cancel(&emulator, region, &identifier).await;
        assert_eq!(
            again.status, 200,
            "case {case:?}: a repeat against the end state must be a no-op success, got {} ({:?})",
            again.status, again.body
        );
    }
}

#[tokio::test]
async fn concurrent_cancels_of_one_just_started_job_all_succeed_and_it_converges_on_stopped() {
    // Rows differ in how many cancels are in flight at once, and in what the scope
    // resolved to: the operation is `@idempotent`, so every one of them answers the same
    // empty 200 and the job converges on the single end state they all ask for.
    for (case, region, three_at_once, recovery_points) in [
        ("two concurrent cancels of a scope with one backup", "us-west-2", false, 1usize),
        ("three concurrent cancels of an empty scope", "eu-central-1", true, 0),
    ] {
        let (emulator, engine) = emulator_with_engine();
        seed_recovery_points(&engine, held(recovery_points));
        let identifier = start_s3_search_job(&emulator, region, case).await;

        let statuses: Vec<u16> = if three_at_once {
            let (first, second, third) = tokio::join!(
                cancel(&emulator, region, &identifier),
                cancel(&emulator, region, &identifier),
                cancel(&emulator, region, &identifier),
            );
            vec![first.status, second.status, third.status]
        } else {
            let (first, second) = tokio::join!(
                cancel(&emulator, region, &identifier),
                cancel(&emulator, region, &identifier),
            );
            vec![first.status, second.status]
        };
        assert_eq!(
            statuses,
            vec![200; statuses.len()],
            "case {case:?}: every concurrent cancel must succeed, got {statuses:?}"
        );

        tokio::time::sleep(PAST_THE_DUE_INSTANT).await;
        let settled = call(
            &emulator,
            region,
            "GET",
            &format!("/search-jobs/{identifier}"),
            None,
        )
        .await;
        assert_eq!(
            settled.string("Status"),
            state::STOPPED,
            "case {case:?}: the job must converge on stopped, got {:?}",
            settled.get("Status")
        );
    }
}

#[tokio::test]
async fn a_cancel_of_a_job_whose_scan_already_ended_conflicts_and_writes_nothing() {
    // One row per terminal status a scan reaches on its own: a scope that resolved to
    // backups completes, one that resolved to nothing fails. Both are statuses the job
    // never leaves and that a cancel does not produce, so the cancel has nothing left to
    // act on.
    for (case, region, recovery_points, terminal) in [
        ("a scan that completed over one backup", "us-west-2", 1usize, state::COMPLETED),
        ("a scan that completed over three backups", "eu-west-1", 3, state::COMPLETED),
        ("a scan that failed over an empty scope", "ap-southeast-2", 0, state::FAILED),
    ] {
        let (emulator, engine) = emulator_with_engine();
        seed_recovery_points(&engine, held(recovery_points));
        let identifier = start_s3_search_job(&emulator, region, case).await;
        let job_uri = format!("/search-jobs/{identifier}");

        tokio::time::sleep(PAST_THE_DUE_INSTANT).await;
        let ended = call(&emulator, region, "GET", &job_uri, None).await;
        assert_eq!(
            ended.string("Status"),
            terminal,
            "case {case:?}: the scan must have ended {terminal} before the cancel, got {:?}",
            ended.get("Status")
        );

        let refused = cancel(&emulator, region, &identifier).await;
        assert_eq!(
            refused.status, 409,
            "case {case:?}: a cancel of a job whose scan already ended must be refused 409, got {} ({:?})",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code.as_deref(),
            Some("ConflictException"),
            "case {case:?}: the refusal must carry ConflictException, got {:?}",
            refused.error_code
        );
        // The service's own wording, whose only variable part is the state the job is in
        // (recorded behaviour; the model states no text for this error).
        assert_eq!(
            refused.string("message"),
            format!(
                "SearchJob can not be stopped since it is in {terminal} state. Please try again later"
            ),
            "case {case:?}: the refusal must carry the service's message, got {:?}",
            refused.get("message")
        );

        // And the refusal wrote nothing.
        let after = call(&emulator, region, "GET", &job_uri, None).await;
        assert_eq!(
            after.string("Status"),
            terminal,
            "case {case:?}: a refused cancel must leave the status where it was, got {:?}",
            after.get("Status")
        );
        assert_eq!(
            after.get("CompletionTime"),
            ended.get("CompletionTime"),
            "case {case:?}: a refused cancel must leave the completion time where it was, got {:?}",
            after.get("CompletionTime")
        );
        let retried = cancel(&emulator, region, &identifier).await;
        assert_eq!(
            retried.status, refused.status,
            "case {case:?}: a retry must answer as the first refusal did, got {} ({:?})",
            retried.status, retried.body
        );
    }
}

#[tokio::test]
async fn a_cancel_of_an_identifier_naming_no_job_is_not_found() {
    // Well-formed identifiers of the shape this service mints (`state::check_job_identifier`)
    // that this account holds no job for: the lookup's answer, not an input refusal.
    for (case, region, unknown) in [
        (
            "an identifier no call minted",
            "us-west-2",
            "48434c42-40a0-4057-b04d-3b484c610e2b",
        ),
        (
            "another identifier no call minted",
            "eu-west-1",
            "0ed71c69-53a8-4a2a-acfd-8e2e28d0ec67",
        ),
    ] {
        let (emulator, _engine) = emulator_with_engine();
        let refused = cancel(&emulator, region, unknown).await;
        assert_eq!(
            refused.status, 404,
            "case {case:?}: a cancel of an unknown identifier must be refused 404, got {} ({:?})",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case:?}: the refusal must carry ResourceNotFoundException, got {:?}",
            refused.error_code
        );
    }
}
