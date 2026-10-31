// SPDX-License-Identifier: CC0-1.0

//! A suite run's start stamp belongs to having LEFT the queue, not to one way of leaving it.
//!
//! `GetSuiteRun$startTime` is "Date (in Unix epoch time) when the test suite run started"
//! and `SuiteRunInformation$startedAt` the same for the listing, so a run that is no longer
//! queued reports one and a run that reports an end instant reports a start instant too.
//! The table below spans the statuses a run can be in when a stop reaches it — still
//! queued, already executing, already finished — because that is what the rule generalises
//! over; both readers that project the pair are asserted for every row.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_iotdeviceadvisor_iotsenateservice as iotdeviceadvisor;

use crate::suite_lifecycle::{run_configuration, thing_arn, Answer, Emulator, ROLE_ARN};

/// The status-transition span this service promises callers, as the state module fixes it.
fn span() -> Duration {
    iotdeviceadvisor::state::STATUS_TRANSITION
}

const REGION: &str = "eu-west-1";

/// One timestamp member, as a client reading epoch time off the wire sees it.
fn epoch(answer: &Answer, member: &str) -> Option<f64> {
    answer.member(member).as_f64()
}

/// The `suiteRunsList` entry for `run_id`, from a listing of its definition's runs.
async fn listed_entry(emulator: &Emulator, definition_id: &str, run_id: &str) -> Value {
    let listing = emulator
        .call(
            "GET",
            &format!("/suiteRuns?suiteDefinitionId={definition_id}"),
            REGION,
            None,
        )
        .await;
    listing
        .member("suiteRunsList")
        .as_array()
        .unwrap_or_else(|| panic!("ListSuiteRuns answers a suiteRunsList, got {}", listing.body))
        .iter()
        .find(|entry| entry["suiteRunId"].as_str() == Some(run_id))
        .unwrap_or_else(|| panic!("the listing carries run {run_id}, got {}", listing.body))
        .clone()
}

/// Start one run on its own definition, so no row contends with another for the
/// definition's single in-progress slot.
async fn start_run(emulator: &Emulator, case: &str, index: usize) -> (String, String) {
    let definition = emulator
        .create_definition(
            &json!({
                "suiteDefinitionName": format!("stop-stamp-suite-{index}"),
                "rootGroup": "g",
                "devicePermissionRoleArn": ROLE_ARN,
            }),
            REGION,
        )
        .await;
    assert_eq!(
        definition.status, 200,
        "case {case:?}: setting up a definition must succeed, got {} {}",
        definition.status, definition.body
    );
    let definition_id = definition.string("suiteDefinitionId");

    let started = emulator
        .call(
            "POST",
            &format!("/suiteDefinitions/{definition_id}/suiteRuns"),
            REGION,
            Some(json!({
                "suiteRunConfiguration": run_configuration(&thing_arn(index), None, None),
            })),
        )
        .await;
    assert_eq!(
        started.status, 200,
        "case {case:?}: starting a run must succeed, got {} {}",
        started.status, started.body
    );
    let run_id = started.string("suiteRunId");
    (definition_id, run_id)
}

/// Whatever status a run is in when a stop reaches it, the run that comes out of the stop
/// reports a `startTime`, never later than its `endTime`, and the stamp it already carried
/// is not moved.
#[tokio::test]
async fn a_stopped_run_reports_the_start_stamp_it_left_the_queue_with() {
    let emulator = Emulator::new();

    // (case, how long to wait before stopping, the status the run is in then, the status
    // the stop leaves it in once the clock has run out).
    let rows: Vec<(&str, Duration, &str, &str)> = vec![
        (
            "stopped while still queued",
            Duration::ZERO,
            "PENDING",
            "STOPPED",
        ),
        (
            "stopped while executing",
            span() * 3 / 2,
            "RUNNING",
            "STOPPED",
        ),
        (
            // Already terminal: the stop moves nothing, so the stamps it reached stand.
            "stopped after it already finished",
            span() * 5 / 2,
            "ERROR",
            "ERROR",
        ),
    ];

    for (index, (case, wait, status_before_stop, status_after_stop)) in
        rows.into_iter().enumerate()
    {
        let (definition_id, run_id) = start_run(&emulator, case, index).await;
        let run_path = format!("/suiteDefinitions/{definition_id}/suiteRuns/{run_id}");

        if !wait.is_zero() {
            tokio::time::sleep(wait).await;
        }
        let before = emulator.call("GET", &run_path, REGION, None).await;
        assert_eq!(
            before.string("status"),
            status_before_stop,
            "case {case:?}: the stop must reach a run in this status, got {}",
            before.body
        );
        let start_before_stop = epoch(&before, "startTime");

        let stop = emulator
            .call("POST", &format!("{run_path}/stop"), REGION, None)
            .await;
        assert_eq!(
            stop.status, 200,
            "case {case:?}: StopSuiteRun answers 200, got {} {}",
            stop.status, stop.body
        );

        // One span is what the stop transition is owed; waiting it out means every row
        // is read in the status it ends in.
        tokio::time::sleep(span() * 3 / 2).await;
        let after = emulator.call("GET", &run_path, REGION, None).await;
        assert_eq!(
            after.string("status"),
            status_after_stop,
            "case {case:?}: expected the stop to leave the run in this status, got {}",
            after.body
        );

        let start_time = epoch(&after, "startTime");
        let end_time = epoch(&after, "endTime");
        assert!(
            start_time.is_some(),
            "case {case:?}: a run that left PENDING carries a startTime, got {}",
            after.body
        );
        assert!(
            end_time.is_some(),
            "case {case:?}: a run in a terminal status carries an endTime, got {}",
            after.body
        );
        assert!(
            start_time <= end_time,
            "case {case:?}: expected startTime no later than endTime, got {start_time:?} \
             and {end_time:?}"
        );
        if let Some(already_stamped) = start_before_stop {
            assert_eq!(
                start_time,
                Some(already_stamped),
                "case {case:?}: a run that already carried a startTime must keep it, got \
                 {start_time:?}"
            );
        }

        // The listing projects the SAME pair under its own member names.
        let entry = listed_entry(&emulator, &definition_id, &run_id).await;
        assert_eq!(
            entry["startedAt"].as_f64(),
            start_time,
            "case {case:?}: ListSuiteRuns must report the startedAt GetSuiteRun reports, \
             got {entry}"
        );
        assert_eq!(
            entry["endAt"].as_f64(),
            end_time,
            "case {case:?}: ListSuiteRuns must report the endAt GetSuiteRun reports, got \
             {entry}"
        );
        let created_at = entry["createdAt"].as_f64();
        assert!(
            created_at.is_some() && created_at <= start_time,
            "case {case:?}: expected createdAt no later than startedAt, got {created_at:?} \
             and {start_time:?}"
        );

        // Stopping again moves nothing, so neither stamp moves either.
        let restop = emulator
            .call("POST", &format!("{run_path}/stop"), REGION, None)
            .await;
        assert_eq!(
            restop.status, 200,
            "case {case:?}: a second StopSuiteRun answers 200, got {} {}",
            restop.status, restop.body
        );
        let again = emulator.call("GET", &run_path, REGION, None).await;
        assert_eq!(
            (
                again.string("status"),
                epoch(&again, "startTime"),
                epoch(&again, "endTime"),
            ),
            (status_after_stop.to_string(), start_time, end_time),
            "case {case:?}: a second stop must leave the finished run exactly as it was, \
             got {}",
            again.body
        );
    }
}
