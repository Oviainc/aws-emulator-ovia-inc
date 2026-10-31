// SPDX-License-Identifier: CC0-1.0

//! The one bound on `stepTimeoutInMinutes`, driven through BOTH operations that
//! accept the member.
//!
//! The rule under test: a `stepTimeoutInMinutes` a request carries is honoured only
//! inside the range its documentation publishes
//! (`MIN_STEP_TIMEOUT_MINUTES`..=`MAX_STEP_TIMEOUT_MINUTES`, from
//! `smithy_query(query="doc", shape="UpdateJobExecutionInput",
//! member="stepTimeoutInMinutes")`: "Valid values for this parameter range from 1 to
//! 10080 (1 minute to 7 days)"), and a value outside it is refused 400
//! `InvalidRequestException` with the execution untouched. The one exception is the
//! cancelling value `CANCEL_STEP_TIMER_MINUTES`, which only
//! `UpdateJobExecution$stepTimeoutInMinutes` documents ("A value of -1 is also valid
//! and will cancel the current step timer") and which is therefore outside the range
//! for `StartNextPendingJobExecution`, whose member documents the range alone.
//!
//! `StepTimeoutInMinutes` is a bare `long` with no declared `range`
//! (`smithy_query(query="shape", shape="StepTimeoutInMinutes")`), so the framework's
//! constraint walk reports nothing about the value and the bound is applied by the
//! handlers — through one shared check, so the two operations cannot drift apart on
//! the range while each refusal keeps looking right in isolation. Each row therefore
//! drives the SAME value through both operations and asserts both answers, including
//! the message each reports.
//!
//! The table spans the dimension the rule generalises over — the magnitude of the
//! value — at each published bound, one step inside and one step past each, the
//! cancelling value, and the extremes a `long` admits. Every request goes over the
//! wire (restJson1 through the multi-service router), so what is asserted is the HTTP
//! behaviour a device agent sees.

use serde_json::{json, Value};
use service_com_amazonaws_iotjobsdataplane_iotlaserthingjobmanagerexternalservice::state::{
    CANCEL_STEP_TIMER_MINUTES, MAX_STEP_TIMEOUT_MINUTES, MIN_STEP_TIMEOUT_MINUTES,
};

use crate::support::{error_code, error_message, Emulator};

/// The modelled client error both operations refuse a value outside the range with.
const MODELED_REFUSAL: &str = "InvalidRequestException";

/// Seconds in one minute: the member is documented in MINUTES and the timer the
/// response reports is in seconds.
const SECONDS_PER_MINUTE: i64 = 60;

/// The values one row drives through both operations, and the case name it is
/// asserted under. The expectation is DERIVED from the rule rather than listed per
/// operation: inside the range both operations honour the value, and the cancelling
/// value is admitted by the update alone.
fn rows() -> Vec<(&'static str, i64)> {
    vec![
        ("the documented minimum", MIN_STEP_TIMEOUT_MINUTES),
        ("one minute inside the minimum", MIN_STEP_TIMEOUT_MINUTES + 1),
        ("a value well inside the range", 10),
        ("one minute inside the maximum", MAX_STEP_TIMEOUT_MINUTES - 1),
        ("the documented maximum", MAX_STEP_TIMEOUT_MINUTES),
        ("one minute below the minimum", MIN_STEP_TIMEOUT_MINUTES - 1),
        ("one minute past the maximum", MAX_STEP_TIMEOUT_MINUTES + 1),
        ("the cancelling value", CANCEL_STEP_TIMER_MINUTES),
        ("far below the range", -100),
        ("far past the maximum", MAX_STEP_TIMEOUT_MINUTES * 100),
        ("the largest long", i64::MAX),
        ("the smallest long", i64::MIN),
    ]
}

/// True while `minutes` is inside the range both members document.
fn inside_the_range(minutes: i64) -> bool {
    (MIN_STEP_TIMEOUT_MINUTES..=MAX_STEP_TIMEOUT_MINUTES).contains(&minutes)
}

/// The refusal `StartNextPendingJobExecution` reports, whose member documents the
/// range alone.
fn range_refusal(minutes: i64) -> String {
    format!(
        "stepTimeoutInMinutes must be between {MIN_STEP_TIMEOUT_MINUTES} and {MAX_STEP_TIMEOUT_MINUTES}, got {minutes}"
    )
}

/// The refusal `UpdateJobExecution` reports, whose member documents the range plus
/// the cancelling value.
fn range_or_cancel_refusal(minutes: i64) -> String {
    format!(
        "stepTimeoutInMinutes must be between {MIN_STEP_TIMEOUT_MINUTES} and {MAX_STEP_TIMEOUT_MINUTES}, or {CANCEL_STEP_TIMER_MINUTES} to cancel the step timer, got {minutes}"
    )
}

/// The seconds a response says are left before the execution times out, or `None`
/// while no timer is armed.
fn seconds_before_timed_out(execution: &Value) -> Option<i64> {
    execution["approximateSecondsBeforeTimedOut"].as_i64()
}

/// RULE: `stepTimeoutInMinutes` is honoured only inside its published range
/// (`MIN_STEP_TIMEOUT_MINUTES`..=`MAX_STEP_TIMEOUT_MINUTES`), where it arms the step
/// timer for that many minutes, and a value outside it is refused 400
/// `InvalidRequestException` with the execution left as it was —
/// `CANCEL_STEP_TIMER_MINUTES` excepted, which `UpdateJobExecution` alone documents
/// and admits as "cancel the current step timer".
///
/// Every row drives one value through `StartNextPendingJobExecution` AND
/// `UpdateJobExecution`, so the two operations are pinned to the same range and to
/// the message each of them states it with.
#[tokio::test]
async fn the_step_timeout_range_is_the_same_through_both_operations_that_accept_it() {
    let emulator = Emulator::new();

    for (index, (case, minutes)) in rows().into_iter().enumerate() {
        // One thing per row, so a row's own queue holds exactly its own execution.
        let thing = format!("laser-step-shared-{index}");
        let id = emulator.start_command_on(&thing, None).await;

        // `StartNextPendingJobExecution`: the step timeout is applied as the queued
        // execution is started. Its member documents the range alone, so the
        // cancelling value is outside it here.
        let (status, started) = emulator
            .start_next(&thing, json!({"stepTimeoutInMinutes": minutes}))
            .await;
        if inside_the_range(minutes) {
            assert_eq!(
                status, 200,
                "case {case:?} (stepTimeoutInMinutes={minutes}): expected 200 from $next, got {status} {started}"
            );
            let remaining = seconds_before_timed_out(&started["execution"]);
            let armed = minutes * SECONDS_PER_MINUTE;
            assert!(
                remaining.is_some_and(|left| (armed - SECONDS_PER_MINUTE..=armed).contains(&left)),
                "case {case:?} (stepTimeoutInMinutes={minutes}): expected about {armed} seconds left from $next, got {remaining:?}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?} (stepTimeoutInMinutes={minutes}): expected 400 from $next, got {status} {started}"
            );
            assert_eq!(
                error_code(&started), MODELED_REFUSAL,
                "case {case:?} (stepTimeoutInMinutes={minutes}): expected {MODELED_REFUSAL} from $next, got {started}"
            );
            assert_eq!(
                error_message(&started),
                range_refusal(minutes),
                "case {case:?} (stepTimeoutInMinutes={minutes}): unexpected refusal message from $next, got {started}"
            );
            // The refused start wrote nothing: the execution is still queued at the
            // version its creation gave it, with no timer armed.
            let stored = emulator.stored_execution(&thing, &id);
            assert_eq!(
                stored.status, "QUEUED",
                "case {case:?} (stepTimeoutInMinutes={minutes}): expected the refused start to change nothing, got {stored:?}"
            );
            assert_eq!(
                stored.step_timeout_expires_at, None,
                "case {case:?} (stepTimeoutInMinutes={minutes}): expected the refused start to arm no timer, got {stored:?}"
            );
        }

        // `UpdateJobExecution`: the same value through the device's own update, whose
        // member documents the range plus the cancelling value.
        let (status, updated) = emulator
            .update(
                &thing,
                &id,
                json!({"status": "IN_PROGRESS", "stepTimeoutInMinutes": minutes}),
            )
            .await;
        let admitted_by_the_update =
            inside_the_range(minutes) || minutes == CANCEL_STEP_TIMER_MINUTES;
        if !admitted_by_the_update {
            assert_eq!(
                status, 400,
                "case {case:?} (stepTimeoutInMinutes={minutes}): expected 400 from the update, got {status} {updated}"
            );
            assert_eq!(
                error_code(&updated), MODELED_REFUSAL,
                "case {case:?} (stepTimeoutInMinutes={minutes}): expected {MODELED_REFUSAL} from the update, got {updated}"
            );
            assert_eq!(
                error_message(&updated),
                range_or_cancel_refusal(minutes),
                "case {case:?} (stepTimeoutInMinutes={minutes}): unexpected refusal message from the update, got {updated}"
            );
            continue;
        }

        assert_eq!(
            status, 200,
            "case {case:?} (stepTimeoutInMinutes={minutes}): expected 200 from the update, got {status} {updated}"
        );

        // Read the execution back: an admitted value armed the step timer for that
        // many minutes, and the cancelling value left no timer armed.
        let (status, described) = emulator.describe(&thing, &id, "").await;
        assert_eq!(
            status, 200,
            "case {case:?} (stepTimeoutInMinutes={minutes}): expected to read the execution back, got {status} {described}"
        );
        let remaining = seconds_before_timed_out(&described["execution"]);
        if minutes == CANCEL_STEP_TIMER_MINUTES {
            assert_eq!(
                remaining, None,
                "case {case:?} (stepTimeoutInMinutes={minutes}): expected the step timer cancelled, got {described}"
            );
        } else {
            let armed = minutes * SECONDS_PER_MINUTE;
            assert!(
                remaining.is_some_and(|left| (armed - SECONDS_PER_MINUTE..=armed).contains(&left)),
                "case {case:?} (stepTimeoutInMinutes={minutes}): expected about {armed} seconds left after the update, got {remaining:?}"
            );
        }
    }
}

/// RULE: an ABSENT `stepTimeoutInMinutes` is always acceptable and arms nothing —
/// the member bounds only a value a request carries. Asserted through both
/// operations that accept it, since one shared check decides it for both.
#[tokio::test]
async fn an_absent_step_timeout_is_accepted_and_arms_no_timer_on_either_operation() {
    let emulator = Emulator::new();

    for (case, body) in [
        ("no member at all", json!({})),
        ("a body carrying only other members", json!({"statusDetails": {"stage": "start"}})),
    ] {
        let thing = format!("laser-step-absent-{}", body.to_string().len());
        let id = emulator.start_command_on(&thing, None).await;

        let (status, started) = emulator.start_next(&thing, body.clone()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from $next, got {status} {started}"
        );
        assert_eq!(
            seconds_before_timed_out(&started["execution"]),
            None,
            "case {case:?}: expected $next to arm no timer, got {started}"
        );

        let mut update = body.clone();
        update["status"] = json!("IN_PROGRESS");
        let (status, updated) = emulator.update(&thing, &id, update).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from the update, got {status} {updated}"
        );
        let stored = emulator.stored_execution(&thing, &id);
        assert_eq!(
            stored.step_timeout_expires_at, None,
            "case {case:?}: expected the update to arm no timer, got {stored:?}"
        );
    }
}
