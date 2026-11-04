// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for the stop target of `StopProcessingJob`: how the operation
//! behaves for a `ProcessingJobName` the account holds no job under.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses.
//!
//! The RULE these tests pin is that the stop resolves its target in the store before it
//! mutates anything, so a name that resolves to no job is a client error and never a
//! success: HTTP 400 with the error code `ValidationException` and the service's
//! stop-target sentence carrying the name the request presented. The tests assert it over
//! a TABLE of inputs along the axes the rule generalises over, and on the axes the sibling
//! suites do not reach: a store that holds no job at all, repeats of the same
//! unresolvable stop, a name outside `ProcessingJobName`'s published bounds (refused
//! before the target is ever looked up), and the two refusal classes — the name that does
//! not resolve versus the job that is found with no run left to stop — told apart inside
//! one store.

use std::time::Duration;

use service_com_amazonaws_sagemaker_sagemaker::state::PROCESSING_JOB_ENDS_AFTER;

// The crate's wire-test harness, shared by every suite in this one test binary, plus the
// processing-job create body and the two request helpers the sibling suites already state,
// so this suite adds axes rather than a second copy of the requests.
use crate::inference_deployment_lifecycle::{error_code, message, str_field, Emulator};
use crate::processing_job_not_found_refusals::minimal_create;
use crate::processing_job_stop::{describe, stop};

/// The crate's own span for a processing job's run, read from the constant the handler
/// enforces rather than copied, plus a margin so the terminal status is certainly due.
const PAST_JOB_END_DUE: Duration =
    PROCESSING_JOB_ENDS_AFTER.saturating_add(Duration::from_millis(300));

/// The fixed text the service words an unresolvable stop target with; the variable part is
/// the `ProcessingJobName` the request presented.
const STOP_TARGET_TEXT: &str = "Could not find job to update with name: ";

/// Names no job was ever created under, inside `ProcessingJobName`'s published constraint
/// (`len 1..63`, pattern `^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,62}$` — smithy_query
/// operation=StopProcessingJob), so every row reaches the handler's target lookup. Rows
/// differ along the name's length (both published bounds and in between), its character
/// classes (digits, letters, hyphens, mixed case) and the Region the caller signs for.
fn absent_names() -> Vec<(&'static str, &'static str, String)> {
    vec![
        // `ProcessingJobName`'s minimum length of 1.
        ("shortest-name", "us-east-1", "q".to_string()),
        ("single-digit-name", "us-west-2", "4".to_string()),
        (
            "hyphenated-name",
            "eu-west-2",
            "teardown-of-a-job-that-was-never-created".to_string(),
        ),
        (
            "mixed-case-name",
            "ap-southeast-1",
            "ProcJobNeverCreated".to_string(),
        ),
        // `ProcessingJobName`'s maximum length of 63.
        ("longest-name", "eu-central-1", format!("a{}b", "0".repeat(61))),
    ]
}

/// RULE: a stop whose `ProcessingJobName` resolves to no job is refused with HTTP 400
/// `ValidationException` carrying the service's stop-target sentence and the name the
/// request presented — even when the store holds no processing job at all — and the
/// refusal is a refusal: repeating it answers the same, nothing is created for the name,
/// and the name is still free for a create that then makes the very same stop succeed.
#[tokio::test]
async fn an_unresolvable_stop_target_is_refused_against_an_empty_store() {
    let rows = absent_names();
    // The table must really sit on `ProcessingJobName`'s published bounds (len 1..63), so
    // a typo cannot quietly move it off them.
    let lengths: Vec<usize> = rows.iter().map(|(_, _, name)| name.len()).collect();
    assert_eq!(
        (lengths.iter().min().copied(), lengths.iter().max().copied()),
        (Some(1), Some(63)),
        "expected the table to span both published bounds of ProcessingJobName, \
         got the lengths {lengths:?}"
    );

    for (case, region, name) in &rows {
        // A store that holds nothing: the stop cannot be answered from a neighbouring
        // job, only from the absence of this one.
        let emulator = Emulator::new();
        let expected = format!("{STOP_TARGET_TEXT}{name}");

        for attempt in 0..2 {
            let (status, refused) = stop(&emulator, region, name).await;
            assert_eq!(
                status, 400,
                "case {case:?} attempt {attempt}: expected HTTP 400 for a stop target that \
                 resolves to no job, got {status} with {refused:?}"
            );
            assert_eq!(
                error_code(&refused),
                "ValidationException",
                "case {case:?} attempt {attempt}: expected the error code \
                 ValidationException, got {refused:?}"
            );
            assert_eq!(
                message(&refused),
                expected,
                "case {case:?} attempt {attempt}: expected the refusal to read \
                 {expected:?}, got {refused:?}"
            );
        }

        // The refused stop created nothing: the name still resolves to no job.
        let (status, described) = describe(&emulator, region, name).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the refused name to still resolve to no job, \
             got {status} with {described:?}"
        );

        // And the gate is about resolution, not about the name: once a job exists under
        // it, the identical stop is accepted and moves that job.
        let (status, created) = emulator
            .call_in(region, "CreateProcessingJob", &minimal_create(name))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the create under the refused name to answer 200, \
             got {status} with {created:?}"
        );
        let (status, stopped) = stop(&emulator, region, name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the stop of the now-present job to answer 200, \
             got {status} with {stopped:?}"
        );
        let (status, after) = describe(&emulator, region, name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the read after the accepted stop to answer 200, \
             got {status} with {after:?}"
        );
        assert_eq!(
            str_field(&after, "ProcessingJobStatus"),
            "Stopping",
            "case {case:?}: expected the accepted stop to move the job into Stopping, \
             got {after:?}"
        );
    }
}

/// RULE: a `ProcessingJobName` outside its modelled constraint never reaches the stop's
/// target lookup — it is refused at the boundary with HTTP 400 `ValidationException`, so
/// the refusal is the constraint's and not the stop-target sentence, and nothing is
/// stored for it.
#[tokio::test]
async fn a_name_outside_the_modelled_constraint_never_reaches_the_target_gate() {
    let emulator = Emulator::new();
    // Rows sit one past each published bound of `ProcessingJobName` (len 1..63, pattern
    // `^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,62}$`): below the minimum length, one past the
    // maximum, and outside the pattern at either end.
    let too_long = "a".repeat(64);
    let rows: [(&str, &str); 4] = [
        ("below-the-minimum-length", ""),
        ("one-past-the-maximum-length", too_long.as_str()),
        ("leading-hyphen-outside-the-pattern", "-proc-job"),
        ("trailing-hyphen-outside-the-pattern", "proc-job-"),
    ];

    for (case, name) in rows {
        let (status, refused) = stop(&emulator, "us-east-1", name).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a name outside the modelled constraint to be refused \
             with HTTP 400, got {status} with {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {case:?}: expected the error code ValidationException, got {refused:?}"
        );
        let refusal = message(&refused);
        assert!(
            !refusal.contains(STOP_TARGET_TEXT),
            "case {case:?}: expected the constraint's own refusal rather than the \
             stop-target sentence, got {refusal:?}"
        );

        let (status, described) = describe(&emulator, "us-east-1", name).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the refused name to hold no job, got {status} \
             with {described:?}"
        );
    }
}

/// RULE: the stop tells its two refusal classes apart by whether the name resolves — a
/// name that resolves to no job carries the stop-target sentence, while a job that IS
/// found with no run left to stop is refused on its status instead — and neither refusal
/// touches the store.
#[tokio::test]
async fn an_absent_target_and_a_job_with_no_run_left_are_refused_differently() {
    let emulator = Emulator::new();
    let present = "proc-job-run-is-over";
    let absent = "proc-job-never-created";

    let (status, created) = emulator
        .call_in("us-east-1", "CreateProcessingJob", &minimal_create(present))
        .await;
    assert_eq!(
        status, 200,
        "expected the seeding create to answer 200, got {status} with {created:?}"
    );
    let created_arn = str_field(&created, "ProcessingJobArn");

    // Let the clock run the job's status out, so the stop below finds a job whose run is
    // over rather than one still in flight.
    tokio::time::sleep(PAST_JOB_END_DUE).await;
    let (status, before) = describe(&emulator, "us-east-1", present).await;
    assert_eq!(
        status, 200,
        "expected the stored job to read back, got {status} with {before:?}"
    );
    let reached = str_field(&before, "ProcessingJobStatus");

    let (status, on_status) = stop(&emulator, "us-east-1", present).await;
    assert_eq!(
        status, 400,
        "case present-but-not-running: expected HTTP 400, got {status} with {on_status:?}"
    );
    assert_eq!(
        error_code(&on_status),
        "ValidationException",
        "case present-but-not-running: expected the error code ValidationException, \
         got {on_status:?}"
    );
    let on_status_message = message(&on_status);
    assert!(
        on_status_message.contains(present)
            && on_status_message.contains(&reached)
            && !on_status_message.contains(STOP_TARGET_TEXT),
        "case present-but-not-running: expected a refusal naming the job {present:?} and \
         the status {reached:?} it found rather than the stop-target sentence, \
         got {on_status_message:?}"
    );

    let (status, on_absence) = stop(&emulator, "us-east-1", absent).await;
    assert_eq!(
        status, 400,
        "case absent: expected HTTP 400, got {status} with {on_absence:?}"
    );
    let expected = format!("{STOP_TARGET_TEXT}{absent}");
    assert_eq!(
        message(&on_absence),
        expected,
        "case absent: expected the refusal to read {expected:?}, got {on_absence:?}"
    );
    assert_ne!(
        message(&on_absence),
        on_status_message,
        "expected the two refusal classes to be worded differently, both read {expected:?}"
    );

    // Neither refusal touched the store: the job that was found still reports exactly
    // what it reported, and the name that was not found still resolves to nothing.
    let (status, after) = describe(&emulator, "us-east-1", present).await;
    assert_eq!(
        status, 200,
        "expected the read after both refusals to answer 200, got {status} with {after:?}"
    );
    assert_eq!(
        after, before,
        "expected the refused stops to leave the stored job untouched, got {after:?}"
    );
    assert_eq!(
        str_field(&after, "ProcessingJobArn"),
        created_arn,
        "expected the stored job to keep the ARN its create minted ({created_arn}), \
         got {after:?}"
    );
    let (status, missing) = describe(&emulator, "us-east-1", absent).await;
    assert_eq!(
        status, 400,
        "expected the absent name to still resolve to no job, got {status} with {missing:?}"
    );
}
