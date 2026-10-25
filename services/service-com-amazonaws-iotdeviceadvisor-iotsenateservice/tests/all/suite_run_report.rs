// SPDX-License-Identifier: CC0-1.0

//! `GetSuiteRunReport`, over the wire.
//!
//! The rule under test: a suite run's QUALIFICATION REPORT exists exactly when the run
//! belongs to the suite definition the caller named, that definition is intended for
//! qualification, and the run SUCCEEDED (the report AWS documents for "a successful
//! Device Advisor qualifying test suite run"). The operation reports that report's
//! download URL, derived from the stored run so repeated reads announce the same one, and
//! refuses every run without one with the modelled `ResourceNotFoundException` and this
//! service's single not-found message template.
//!
//! The tables below span what the rule generalises over rather than one request: EVERY
//! `SuiteRunStatus` the model enumerates, against all three wire forms of
//! `intendedForQualification` (`true`, `false`, absent), plus the id pairs a store does not
//! hold (neither id known, the definition only, the run only, a run crossed with another
//! definition), and the run lifecycle this emulator's own clock drives (`PENDING` ->
//! `RUNNING` -> `ERROR`, and `STOPPING` -> `STOPPED`), which reaches no successful status
//! because nothing executes the suite — so a run started over the wire never has a report,
//! which the lifecycle case asserts.
//!
//! The non-terminal and successful statuses are reached by SEEDING the store, which is a
//! fixture and not a claim that this emulator produces them: what is asserted is the
//! service's rule about a stored run, over every status the model allows a run to carry.
//! Every case drives real HTTP requests through the same `MultiServiceRouter` the emulator
//! serves on a socket.

use std::time::{Duration, SystemTime};

use serde_json::json;
use service_com_amazonaws_iotdeviceadvisor_iotsenateservice as iotdeviceadvisor;

use super::suite_lifecycle::{run_configuration, thing_arn, Answer, Emulator, ROLE_ARN};

/// The region every case below signs for, and so the host the harness addresses.
const REGION: &str = "us-east-1";

/// The endpoint the harness reaches the emulator on, which is the endpoint an announced
/// report URL has to resolve back to.
const REQUEST_HOST: &str = "api.iotdeviceadvisor.us-east-1.amazonaws.com";

/// Two transition spans and a margin: long enough for `PENDING` -> `RUNNING` -> terminal,
/// the lifecycle this emulator's runs finish through, to have become due.
fn past_completion() -> Duration {
    iotdeviceadvisor::state::STATUS_TRANSITION * 5 / 2
}

/// Create one suite definition, with `intendedForQualification` in the wire form the row
/// asks for, and hand back its id.
async fn definition(emulator: &Emulator, name: &str, qualification: Option<bool>) -> String {
    let mut configuration = json!({
        "suiteDefinitionName": name,
        "rootGroup": "g",
        "devicePermissionRoleArn": ROLE_ARN,
        "devices": [{ "thingArn": thing_arn(1) }],
    });
    if let Some(qualification) = qualification {
        configuration["intendedForQualification"] = json!(qualification);
    }
    let created = emulator.create_definition(&configuration, REGION).await;
    assert_eq!(
        created.status, 200,
        "setup: creating {name:?} failed with {} {}",
        created.status, created.body
    );
    created.string("suiteDefinitionId")
}

/// Put one run of `definition_id` carrying `status` straight into the store and hand back
/// its id.
///
/// A fixture, not a behaviour: this emulator's lifecycle drives a run to `ERROR` (nothing
/// executes the suite), so the other statuses the model allows a stored run to carry —
/// including the successful ones a real qualifying run reaches — are only observable from a
/// seeded store. What the cases built on it assert is the service's RULE about a stored
/// run, never that this emulator produces that status.
fn seed_run(emulator: &Emulator, definition_id: &str, status: &str) -> String {
    let state = iotdeviceadvisor::state::service_state(&*emulator.engine)
        .expect("the service's store is reachable from the shared engine");
    let mut store = state.lock();
    let id = iotdeviceadvisor::state::new_uuid();
    let created_at = SystemTime::now() - Duration::from_secs(60);
    let terminal = iotdeviceadvisor::state::is_terminal(status);
    store.runs.insert(
        id.clone(),
        iotdeviceadvisor::state::SuiteRun {
            id: id.clone(),
            arn: iotdeviceadvisor::state::arn(
                REGION,
                &format!("suiterun/{definition_id}/{id}"),
            ),
            suite_definition_id: definition_id.to_string(),
            suite_definition_version: "v1".to_string(),
            configuration: iotdeviceadvisor::ops::types::SuiteRunConfiguration::default(),
            status: status.to_string(),
            created_at,
            started_at: (status != "PENDING").then_some(created_at),
            end_at: terminal.then_some(created_at + Duration::from_secs(1)),
            error_reason: None,
            endpoint: iotdeviceadvisor::state::endpoint_for_region(REGION),
            // No transition is left owed, so the seeded status is the one the case reads.
            next_transition_at: None,
        },
    );
    id
}

/// `GetSuiteRunReport` for one `(definition, run)` pair.
async fn report(emulator: &Emulator, definition_id: &str, run_id: &str) -> Answer {
    emulator
        .call(
            "GET",
            &format!("/suiteDefinitions/{definition_id}/suiteRuns/{run_id}/report"),
            REGION,
            None,
        )
        .await
}

/// Assert one answer is the modelled not-found refusal for `run_id`.
fn assert_no_report(case: &str, answer: &Answer, run_id: &str) {
    assert_eq!(
        (answer.status, answer.error_code.as_deref()),
        (404, Some("ResourceNotFoundException")),
        "case {case:?}: a run with no qualification report must be refused as not found, \
         got {} {}",
        answer.status,
        answer.body
    );
    let expected = iotdeviceadvisor::state::not_found_message("suiteRunId", run_id);
    assert_eq!(
        answer.member("message").as_str(),
        Some(expected.as_str()),
        "case {case:?}: expected the message {expected:?}, got {}",
        answer.body
    );
}

/// Assert one answer reports the report of `(definition_id, run_id)`.
fn assert_report_url(case: &str, answer: &Answer, definition_id: &str, run_id: &str) {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: a successful run of a qualification suite has a report, got {} {}",
        answer.status, answer.body
    );
    let expected =
        format!("https://{REQUEST_HOST}/qualificationReports/{definition_id}/{run_id}");
    assert_eq!(
        answer.member("qualificationReportDownloadUrl").as_str(),
        Some(expected.as_str()),
        "case {case:?}: the URL must name the endpoint the request reached and the run it \
         reports on ({expected:?}), got {}",
        answer.body
    );
}

/// The whole rule, over EVERY status the model lets a stored run carry and every wire form
/// of `intendedForQualification`: a report exists exactly for a successful run of a suite
/// intended for qualification.
#[tokio::test]
async fn a_report_exists_exactly_for_a_successful_run_of_a_qualification_suite() {
    // Source for the status rows: the `SuiteRunStatus` enum, re-exported from the model as
    // `SUITE_RUN_STATUS_VALUES`, so a status added to the model is covered here too.
    let statuses = iotdeviceadvisor::ops::types::SUITE_RUN_STATUS_VALUES;
    assert!(
        statuses.len() >= 9,
        "the model's SuiteRunStatus list must be the source of these rows, got {statuses:?}"
    );

    for qualification in [Some(true), Some(false), None] {
        let emulator = Emulator::new();
        let definition_id = definition(
            &emulator,
            &format!("report-rule-{qualification:?}"),
            qualification,
        )
        .await;

        for status in statuses {
            let case = format!("intendedForQualification={qualification:?}, status={status}");
            let run_id = seed_run(&emulator, &definition_id, status);
            let answer = report(&emulator, &definition_id, &run_id).await;

            let expects_report =
                qualification == Some(true) && iotdeviceadvisor::state::is_successful(status);
            if expects_report {
                assert_report_url(&case, &answer, &definition_id, &run_id);
                // A read reports the report the run HAS: reading twice announces the same
                // URL rather than minting a second one.
                let again = report(&emulator, &definition_id, &run_id).await;
                assert_eq!(
                    *again.member("qualificationReportDownloadUrl"),
                    *answer.member("qualificationReportDownloadUrl"),
                    "case {case:?}: a second read must announce the same report URL, got {}",
                    again.body
                );
            } else {
                assert_no_report(&case, &answer, &run_id);
            }
        }
    }
}

/// The lifecycle this emulator's own clock drives reaches no successful status, so a run
/// started over the wire has no report at any point of it.
#[tokio::test]
async fn a_run_this_emulator_drives_never_has_a_report() {
    let emulator = Emulator::new();
    let definition_id = definition(&emulator, "driven-qualification-suite", Some(true)).await;

    // A run that runs to its end: PENDING, then RUNNING, then the terminal status this
    // emulator reaches because nothing executes the suite.
    let started = emulator
        .call(
            "POST",
            &format!("/suiteDefinitions/{definition_id}/suiteRuns"),
            REGION,
            Some(json!({
                "suiteRunConfiguration": run_configuration(&thing_arn(2), None, None)
            })),
        )
        .await;
    assert_eq!(
        started.status, 200,
        "setup: starting a run failed with {} {}",
        started.status, started.body
    );
    let run_id = started.string("suiteRunId");
    let run_path = format!("/suiteDefinitions/{definition_id}/suiteRuns/{run_id}");

    let in_progress = report(&emulator, &definition_id, &run_id).await;
    assert_no_report("the run is still in progress", &in_progress, &run_id);

    tokio::time::sleep(past_completion()).await;
    let finished = emulator.call("GET", &run_path, REGION, None).await;
    assert_eq!(
        finished.string("status"),
        "ERROR",
        "setup: a run nothing could execute must end in ERROR, got {}",
        finished.body
    );
    let after = report(&emulator, &definition_id, &run_id).await;
    assert_no_report("the run finished unsuccessfully", &after, &run_id);

    // And a run a caller stops: STOPPING, then STOPPED, which is not a success either.
    let second = emulator
        .call(
            "POST",
            &format!("/suiteDefinitions/{definition_id}/suiteRuns"),
            REGION,
            Some(json!({
                "suiteRunConfiguration": run_configuration(&thing_arn(3), None, None)
            })),
        )
        .await;
    assert_eq!(
        second.status, 200,
        "setup: starting the second run failed with {} {}",
        second.status, second.body
    );
    let stopped_id = second.string("suiteRunId");
    let stopped_path = format!("/suiteDefinitions/{definition_id}/suiteRuns/{stopped_id}");
    let stop = emulator
        .call("POST", &format!("{stopped_path}/stop"), REGION, None)
        .await;
    assert_eq!(
        stop.status, 200,
        "setup: stopping the run failed with {} {}",
        stop.status, stop.body
    );
    tokio::time::sleep(past_completion()).await;
    let read_back = emulator.call("GET", &stopped_path, REGION, None).await;
    assert_eq!(
        read_back.string("status"),
        "STOPPED",
        "setup: the stopped run must reach STOPPED, got {}",
        read_back.body
    );
    let stopped_report = report(&emulator, &definition_id, &stopped_id).await;
    assert_no_report("the run was stopped", &stopped_report, &stopped_id);
}

/// An id pair the store does not hold names no report, whichever half of it is unknown —
/// including a run of ANOTHER definition, which the pair is what rules out.
#[tokio::test]
async fn an_id_pair_the_store_does_not_hold_names_no_report() {
    let emulator = Emulator::new();
    let known = definition(&emulator, "known-qualification-suite", Some(true)).await;
    let other = definition(&emulator, "other-qualification-suite", Some(true)).await;
    // A run that HAS a report, so every refusal below is about the pair of ids rather than
    // about the run having none.
    let run_of_known = seed_run(&emulator, &known, "PASS");
    let served = report(&emulator, &known, &run_of_known).await;
    assert_report_url("setup", &served, &known, &run_of_known);

    let unknown_definition = iotdeviceadvisor::state::new_uuid();
    let unknown_run = iotdeviceadvisor::state::new_uuid();
    let rows: [(&str, &str, &str); 4] = [
        (
            "neither id known",
            unknown_definition.as_str(),
            unknown_run.as_str(),
        ),
        (
            "definition known, run unknown",
            known.as_str(),
            unknown_run.as_str(),
        ),
        (
            "run known, definition unknown",
            unknown_definition.as_str(),
            run_of_known.as_str(),
        ),
        (
            "run crossed with another definition",
            other.as_str(),
            run_of_known.as_str(),
        ),
    ];

    for (case, definition_id, run_id) in rows {
        let answer = report(&emulator, definition_id, run_id).await;
        assert_no_report(case, &answer, run_id);
    }
}

/// Reporting a report changes nothing a later caller sees: the run reads back exactly as
/// it did before, and a second read answers the same URL.
#[tokio::test]
async fn reading_a_report_changes_nothing() {
    let emulator = Emulator::new();
    let definition_id = definition(&emulator, "stable-qualification-suite", Some(true)).await;
    let run_id = seed_run(&emulator, &definition_id, "PASS_WITH_WARNINGS");
    let run_path = format!("/suiteDefinitions/{definition_id}/suiteRuns/{run_id}");

    let before = emulator.call("GET", &run_path, REGION, None).await;
    let first = report(&emulator, &definition_id, &run_id).await;
    assert_report_url("a successful run", &first, &definition_id, &run_id);
    let second = report(&emulator, &definition_id, &run_id).await;
    let after = emulator.call("GET", &run_path, REGION, None).await;

    assert_eq!(
        *second.member("qualificationReportDownloadUrl"),
        *first.member("qualificationReportDownloadUrl"),
        "two reads of one report must announce the same URL, got {} and {}",
        first.body,
        second.body
    );
    assert_eq!(
        after.body, before.body,
        "reading the report must not change what a later read of the run answers"
    );
}
