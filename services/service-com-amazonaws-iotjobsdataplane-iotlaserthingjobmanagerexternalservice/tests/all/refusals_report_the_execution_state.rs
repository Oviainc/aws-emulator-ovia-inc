// SPDX-License-Identifier: CC0-1.0

//! What `UpdateJobExecution` tells a device it REFUSED about the execution it refused
//! it against.
//!
//! THE RULE under test: a refusal this operation decides by comparing the request with
//! the job execution the service HOLDS — a status the execution's current state forbids,
//! or an `expectedVersion` that is not the stored version — reports that execution's
//! current state in the error body, as the modelled `executionState` object (`status`,
//! `statusDetails`, `versionNumber`); a refusal decided from the REQUEST alone reports
//! none, because there is no stored state it was refused against.
//!
//! The service documents it for both paths: `InvalidStateTransitionException` is "an
//! update attempted to change the job execution to a state that is invalid because of
//! the job execution's current state … In this case, the body of the error message also
//! contains the executionState field", and a mismatched `expectedVersion` is "rejected
//! with a VersionMismatch error, and an ErrorResponse that contains the current job
//! execution status data is returned. (This makes it unnecessary to perform a separate
//! DescribeJobExecution request in order to obtain the job execution status data.)"
//! The second sentence is the point of the rule, and for a FINISHED execution it is the
//! only way the state is reachable at all: that execution's own read is refused 410.
//!
//! The tables span the rule rather than one case: every stage an execution can be
//! refused in (both pending statuses and every terminal status of the model's
//! `JobExecutionStatus` catalog), every status the refusal can be requested with, a
//! stored map of details and none at all, and versions named on either side of the
//! stored one. The state the body reports is checked against the state the service
//! really holds — `DescribeJobExecution` for an execution still live, and the refusal's
//! own repetition for a finished one.
//!
//! Every request goes over the wire (restJson1 through the multi-service router), so
//! what is asserted is the HTTP behaviour a device agent sees.

use serde_json::{json, Value};
use service_com_amazonaws_iotjobsdataplane_iotlaserthingjobmanagerexternalservice::ops::types::JOB_EXECUTION_STATUS_VALUES;
use service_com_amazonaws_iotjobsdataplane_iotlaserthingjobmanagerexternalservice::state::{
    is_pending_status, MAX_STEP_TIMEOUT_MINUTES, MIN_STEP_TIMEOUT_MINUTES, STATUS_IN_PROGRESS,
    STATUS_QUEUED,
};

use crate::support::{error_code, error_message, Emulator};

/// The two statuses an execution is still pending in, which are the stages a device
/// update can be refused in while the execution is still live.
const PENDING_STAGES: [&str; 2] = [STATUS_QUEUED, STATUS_IN_PROGRESS];

/// The wire error code of a refused transition (HTTP 409).
const TRANSITION_REFUSAL: &str = "InvalidStateTransitionException";
/// The wire error code of a refused request (HTTP 400).
const REQUEST_REFUSAL: &str = "InvalidRequestException";

/// A `statusDetails` map the rows store on the execution before refusing it, so the
/// state the refusal reports has details to carry.
fn details() -> Value {
    json!({"stage": "download", "attempt": "1"})
}

/// How far through its lifecycle a status leaves an execution: enqueued, taken up by its
/// device, finished. Stated here independently of the handler, so the rows this suite
/// drives are not chosen by the code under test.
fn lifecycle_stage(status: &str) -> u8 {
    match status {
        STATUS_QUEUED => 0,
        STATUS_IN_PROGRESS => 1,
        _ => 2,
    }
}

/// Whether an update of an execution in `current` to `requested` is one the execution's
/// own state admits: an update may repeat its stage or carry it forward, and `REJECTED`
/// is reachable only from `QUEUED`.
fn transition_is_legal(current: &str, requested: &str) -> bool {
    if requested == "REJECTED" {
        return current == STATUS_QUEUED;
    }
    lifecycle_stage(requested) >= lifecycle_stage(current)
}

/// The `executionState` an error body carries, as the triple this suite asserts on, or
/// `None` when the body carries no such member at all.
fn reported_state(body: &Value) -> Option<(String, Value, i64)> {
    let state = body.get("executionState")?;
    Some((
        state["status"].as_str().unwrap_or_default().to_string(),
        state["statusDetails"].clone(),
        state["versionNumber"].as_i64().unwrap_or_default(),
    ))
}

/// RULE, on the 409: a status the stored execution's current state refuses is answered
/// with that state — the status the execution is really in, the details it really
/// holds and the version it is really at.
///
/// The table is every (stage, requested status) pair the state machine refuses, over
/// both pending stages and the whole modelled `JobExecutionStatus` catalog, and each
/// row's reported state is checked against what a read of the same execution reports.
#[tokio::test]
async fn a_refused_transition_reports_the_state_it_was_refused_against() {
    let emulator = Emulator::new();

    for current in PENDING_STAGES {
        for requested in JOB_EXECUTION_STATUS_VALUES {
            if transition_is_legal(current, requested) {
                continue;
            }
            let case = format!("{current} -> {requested}");
            let thing = format!(
                "laser-refused-{}-{}",
                current.to_lowercase().replace('_', "-"),
                requested.to_lowercase().replace('_', "-")
            );
            let id = emulator.start_command_on(&thing, None).await;

            // Arrange the stage the row is refused in, and a stored map of details the
            // refusal must carry back.
            let (status, arranged) = emulator
                .update(
                    &thing,
                    &id,
                    json!({
                        "status": current,
                        "statusDetails": details(),
                        "includeJobExecutionState": true,
                    }),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200 arranging the stage, got {status} {arranged}"
            );
            let stored_version = arranged["executionState"]["versionNumber"]
                .as_i64()
                .unwrap_or_default();

            let (status, refused) = emulator
                .update(&thing, &id, json!({"status": requested}))
                .await;
            assert_eq!(
                status, 409,
                "case {case:?}: expected 409, got {status} {refused}"
            );
            assert_eq!(
                error_code(&refused),
                TRANSITION_REFUSAL,
                "case {case:?}: expected {TRANSITION_REFUSAL}, got {refused}"
            );
            assert!(
                !error_message(&refused).is_empty(),
                "case {case:?}: a refusal states its reason, got {refused}"
            );
            assert_eq!(
                reported_state(&refused),
                Some((current.to_string(), details(), stored_version)),
                "case {case:?}: expected the refusal to report {current} at version {stored_version} with the stored details, got {refused}"
            );

            // The state reported is the state the service holds: the same execution's
            // own read answers exactly that, so the device needed no second call.
            let (status, described) = emulator.describe(&thing, &id, "").await;
            assert_eq!(
                status, 200,
                "case {case:?}: the refused update leaves a live execution, got {status} {described}"
            );
            assert_eq!(
                reported_state(&refused),
                Some((
                    described["execution"]["status"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                    described["execution"]["statusDetails"].clone(),
                    described["execution"]["versionNumber"]
                        .as_i64()
                        .unwrap_or_default(),
                )),
                "case {case:?}: the state the refusal reported must be the state a read reports, got {refused} and {described}"
            );
        }
    }
}

/// RULE, on the 409 for an execution that has FINISHED: the body reports the status it
/// finished in — the case the error's own documentation gives ("an attempt to change a
/// request in state SUCCESS to state IN_PROGRESS"). This is the path where the rule
/// earns its keep: a finished execution's read is refused 410, so the refusal body is
/// the only place its state is reachable.
///
/// The rows are every TERMINAL value of the modelled catalog, each driven with every
/// status an update can name — and the repetition also pins that a refusal writes
/// nothing: the version the body reports never moves.
#[tokio::test]
async fn an_update_of_a_finished_execution_reports_the_status_it_finished_in() {
    let emulator = Emulator::new();

    for terminal in JOB_EXECUTION_STATUS_VALUES
        .iter()
        .filter(|status| !is_pending_status(status))
    {
        let thing = format!("laser-finished-{}", terminal.to_lowercase().replace('_', "-"));
        let id = emulator.start_command_on(&thing, None).await;

        // Every terminal status is reachable from QUEUED, `REJECTED` included ("Only
        // queued jobs can be rejected").
        let (status, finished) = emulator
            .update(
                &thing,
                &id,
                json!({
                    "status": terminal,
                    "statusDetails": details(),
                    "includeJobExecutionState": true,
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {terminal:?}: expected 200 finishing the execution, got {status} {finished}"
        );
        let finished_version = finished["executionState"]["versionNumber"]
            .as_i64()
            .unwrap_or_default();

        // The execution's own read is refused, which is why the refusal below has to
        // carry the state.
        let (status, described) = emulator.describe(&thing, &id, "").await;
        assert_eq!(
            status, 410,
            "case {terminal:?}: a finished execution's read is refused, got {status} {described}"
        );

        for requested in JOB_EXECUTION_STATUS_VALUES {
            let case = format!("{terminal} -> {requested}");
            let (status, refused) = emulator
                .update(&thing, &id, json!({"status": requested}))
                .await;
            assert_eq!(
                status, 409,
                "case {case:?}: expected 409, got {status} {refused}"
            );
            assert_eq!(
                error_code(&refused),
                TRANSITION_REFUSAL,
                "case {case:?}: expected {TRANSITION_REFUSAL}, got {refused}"
            );
            assert_eq!(
                reported_state(&refused),
                Some((terminal.to_string(), details(), finished_version)),
                "case {case:?}: expected the refusal to report {terminal} at version {finished_version} with the stored details, got {refused}"
            );
        }
    }
}

/// RULE, on the 400 for a mismatched `expectedVersion`: the body carries the "current
/// job execution status data", so the device can re-read its own version from the
/// refusal and retry — the separate `DescribeJobExecution` the member's documentation
/// calls unnecessary.
///
/// The rows span the stage the execution is in when the guard fails (freshly queued with
/// no details at all, queued with details, in progress, and FINISHED — the version guard
/// is decided before the execution's own state is), and for each, versions named on
/// either side of the stored one and at a distance from it.
#[tokio::test]
async fn a_mismatched_expected_version_reports_the_current_job_execution_state() {
    let emulator = Emulator::new();

    // (case, the update that arranges the stage, the status and details the state must
    // report)
    let rows: Vec<(&str, Option<Value>, &str, Value)> = vec![
        (
            "a freshly queued execution holding no details",
            None,
            STATUS_QUEUED,
            Value::Null,
        ),
        (
            "a queued execution holding details",
            Some(json!({"status": STATUS_QUEUED, "statusDetails": details()})),
            STATUS_QUEUED,
            details(),
        ),
        (
            "an execution taken up by its device",
            Some(json!({"status": STATUS_IN_PROGRESS, "statusDetails": details()})),
            STATUS_IN_PROGRESS,
            details(),
        ),
        (
            "an execution that has already finished",
            Some(json!({"status": "SUCCEEDED", "statusDetails": details()})),
            "SUCCEEDED",
            details(),
        ),
    ];

    for (case, arrangement, expected_status, expected_details) in rows {
        let thing = format!(
            "laser-guard-{}",
            case.replace(' ', "-").replace(['\'', ','], "")
        );
        let id = emulator.start_command_on(&thing, None).await;

        let mut stored_version = 1_i64;
        if let Some(mut arrangement) = arrangement {
            arrangement["includeJobExecutionState"] = json!(true);
            let (status, arranged) = emulator.update(&thing, &id, arrangement).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200 arranging the stage, got {status} {arranged}"
            );
            stored_version = arranged["executionState"]["versionNumber"]
                .as_i64()
                .unwrap_or_default();
        }

        // Versions on either side of the stored one, near and far, plus zero — which no
        // execution ever carries.
        let named: Vec<i64> = [-1_i64, 1, -5, 5, 97]
            .iter()
            .map(|offset| stored_version + offset)
            .chain(std::iter::once(0))
            .filter(|version| *version != stored_version)
            .collect();

        for expected_version in named {
            let row = format!("{case} expectedVersion={expected_version}");
            let (status, refused) = emulator
                .update(
                    &thing,
                    &id,
                    json!({"status": STATUS_IN_PROGRESS, "expectedVersion": expected_version}),
                )
                .await;
            assert_eq!(
                status, 400,
                "case {row:?}: expected 400, got {status} {refused}"
            );
            assert_eq!(
                error_code(&refused),
                REQUEST_REFUSAL,
                "case {row:?}: expected {REQUEST_REFUSAL}, got {refused}"
            );
            assert_eq!(
                error_message(&refused),
                format!("Expected version {expected_version} but found version {stored_version}"),
                "case {row:?}: the message names both versions, got {refused}"
            );
            assert_eq!(
                reported_state(&refused),
                Some((
                    expected_status.to_string(),
                    expected_details.clone(),
                    stored_version
                )),
                "case {row:?}: expected the refusal to report {expected_status} at version {stored_version}, got {refused}"
            );
        }

        // The version the body reported is the one the guard then accepts, which is what
        // makes the reported state usable for the retry: no separate read in between.
        if is_pending_status(expected_status) {
            let (status, accepted) = emulator
                .update(
                    &thing,
                    &id,
                    json!({
                        "status": STATUS_IN_PROGRESS,
                        "expectedVersion": stored_version,
                        "includeJobExecutionState": true,
                    }),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: the version the refusal reported is accepted, got {status} {accepted}"
            );
            assert_eq!(
                accepted["executionState"]["versionNumber"].as_i64(),
                Some(stored_version + 1),
                "case {case:?}: the accepted retry advances the version once, got {accepted}"
            );
        }
    }
}

/// RULE, the other side: a refusal decided from the REQUEST alone reports no execution
/// state — there is no stored state it was refused against, so the body is the one the
/// model declares.
///
/// The rows span the request-only refusals of this operation: a `stepTimeoutInMinutes`
/// outside its documented range (on both sides, and the value one past each published
/// bound), a `statusDetails` name outside the map's key constraints, and a `jobId` that
/// names no execution at all.
#[tokio::test]
async fn a_refusal_decided_from_the_request_alone_reports_no_execution_state() {
    let emulator = Emulator::new();
    let thing = "laser-request-only";
    let id = emulator.start_command_on(thing, None).await;

    // (case, the jobId named, the request body, the status and code expected)
    let rows: Vec<(&str, String, Value, u16, &str)> = vec![
        (
            "a step timeout one minute under the published minimum",
            id.clone(),
            json!({
                "status": STATUS_IN_PROGRESS,
                "stepTimeoutInMinutes": MIN_STEP_TIMEOUT_MINUTES - 1,
            }),
            400,
            REQUEST_REFUSAL,
        ),
        (
            "a step timeout one minute over the published maximum",
            id.clone(),
            json!({
                "status": STATUS_IN_PROGRESS,
                "stepTimeoutInMinutes": MAX_STEP_TIMEOUT_MINUTES + 1,
            }),
            400,
            REQUEST_REFUSAL,
        ),
        (
            "a step timeout below the one value that cancels the timer",
            id.clone(),
            json!({"status": STATUS_IN_PROGRESS, "stepTimeoutInMinutes": -2}),
            400,
            REQUEST_REFUSAL,
        ),
        (
            "a statusDetails name outside the map key's own pattern",
            id.clone(),
            json!({"status": STATUS_IN_PROGRESS, "statusDetails": {"step name": "v"}}),
            400,
            REQUEST_REFUSAL,
        ),
        (
            "a jobId naming no execution of this thing",
            "no-such-execution".to_string(),
            json!({"status": STATUS_IN_PROGRESS}),
            404,
            "ResourceNotFoundException",
        ),
    ];

    for (case, job_id, body, expected_status, expected_code) in rows {
        let (status, refused) = emulator.update(thing, &job_id, body).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected {expected_status}, got {status} {refused}"
        );
        assert_eq!(
            error_code(&refused),
            expected_code,
            "case {case:?}: expected {expected_code}, got {refused}"
        );
        assert_eq!(
            reported_state(&refused),
            None,
            "case {case:?}: a request-only refusal reports no execution state, got {refused}"
        );
    }
}
