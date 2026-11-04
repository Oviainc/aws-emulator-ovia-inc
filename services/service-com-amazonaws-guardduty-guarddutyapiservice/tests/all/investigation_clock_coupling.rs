// SPDX-License-Identifier: CC0-1.0

//! What a client reads about one investigation AS ITS OWN CALL SEQUENCE RUNS ON, end to
//! end over the wire (the harness lives in the `detector_lifecycle` suite).
//!
//! The rules under test, each asserted over a table rather than over one recorded call:
//!
//! 1. The members that report whether the analysis has ended move TOGETHER. While the
//!    investigation is `RUNNING` it has not ended, so it reports NEITHER an `EndTime` —
//!    "the timestamp at which the investigation completed"
//!    (`smithy_query(query="doc",shape="Investigation",member="EndTime")`) — NOR an
//!    `Error`, which exists only "if the investigation status is FAILED"
//!    (`…member="Error"`). Once the clock has ended it, the status is terminal AND the
//!    end instant is in (at or after `StartTime`) AND the error says why no summary was
//!    produced. No reader may report one of the three settled and another still running:
//!    that mix is a body no run of the real service ever produced.
//! 2. Only the CLOCK ends it. Every call a client makes before it looks — reads of the
//!    investigation, of the detector that holds it, of that detector's investigation
//!    list — leaves the investigation exactly where the clock had it, so the rows differ
//!    along HOW MANY calls the client makes first, and the phase a read reports never
//!    goes back to running once it has ended.
//! 3. Every reader agrees: for the same investigation, `ListInvestigations` reports the
//!    status and end instant `GetInvestigation` does, so a client cannot learn two
//!    different facts about one analysis.
//!
//! Nothing here reaches into the store or nudges a stored due instant: these rules are
//! about the real clock and the wire, so every claim is read back through the operations
//! a client has.

use std::time::Duration;

use serde_json::{json, Value};

use crate::detector_lifecycle::{call_in, new_router, Router};
use service_com_amazonaws_guardduty_guarddutyapiservice::state::{
    INVESTIGATION_FAILED, INVESTIGATION_RUNNING, INVESTIGATION_TRANSITION_MILLIS,
};

/// The longest an `InvestigationErrorDetails` may be: `len 0..2048`
/// (`smithy_query(shape="InvestigationErrorDetails")`).
const MAX_ERROR_LENGTH: usize = 2048;

/// Long enough for the clock to owe the end of the analysis, with margin for a loaded
/// test machine. Derived from the span the service itself holds, so the two cannot drift.
fn past_the_analysis() -> Duration {
    Duration::from_millis(INVESTIGATION_TRANSITION_MILLIS.unsigned_abs() + 400)
}

/// Which side of the clock-owed end an investigation is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// The analysis is still under way.
    Running,
    /// The clock has ended it.
    Ended,
}

/// One row: what it exercises, the Region the client talks to, the prompt it investigates
/// and how many rounds of its OWN calls it makes before it reads the result.
struct Row {
    case: &'static str,
    region: &'static str,
    prompt: &'static str,
    rounds: usize,
}

/// Rows differ along what the rules generalise over: the number of calls the client makes
/// before it looks (none, one, a handful, a long run) and the endpoint it makes them
/// against (Regions from the published catalogue, not one recorded endpoint).
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a client that reads straight back",
            region: "us-east-1",
            prompt: "summarise the finding on the bastion host",
            rounds: 0,
        },
        Row {
            case: "a client that makes one round of calls first",
            region: "eu-west-2",
            prompt: "why was this role assumed from a new ASN",
            rounds: 1,
        },
        Row {
            case: "a client that makes three rounds of calls first",
            region: "ap-northeast-1",
            prompt: "investigate the credential exfiltration finding",
            rounds: 3,
        },
        Row {
            case: "a client that makes eight rounds of calls first",
            region: "ca-central-1",
            prompt: "I",
            rounds: 8,
        },
    ]
}

/// Enable GuardDuty through `region`'s endpoint and return the detector's id.
async fn detector_in(router: &Router, region: &str, case: &str) -> String {
    let (status, body) = call_in(
        router,
        region,
        "POST",
        "/detector",
        Some(json!({ "enable": true })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected CreateDetector in {region} to answer 200, got {status} {body}"
    );
    body["detectorId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a detectorId, got {body}"))
        .to_string()
}

/// Initiate one investigation and return its id.
async fn initiate(
    router: &Router,
    region: &str,
    detector_id: &str,
    prompt: &str,
    case: &str,
) -> String {
    let (status, body) = call_in(
        router,
        region,
        "POST",
        &format!("/detector/{detector_id}/investigation"),
        Some(json!({ "triggerPrompt": prompt })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected CreateInvestigation to answer 200, got {status} {body}"
    );
    body["investigationId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected an investigationId, got {body}"))
        .to_string()
}

/// The `Investigation` `GetInvestigation` reports.
async fn investigation(
    router: &Router,
    region: &str,
    detector_id: &str,
    investigation_id: &str,
    case: &str,
) -> Value {
    let (status, body) = call_in(
        router,
        region,
        "GET",
        &format!("/detector/{detector_id}/investigation/{investigation_id}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected GetInvestigation to answer 200, got {status} {body}"
    );
    body["investigation"].clone()
}

/// The `InvestigationSummary` `ListInvestigations` reports for `investigation_id`.
async fn summary(
    router: &Router,
    region: &str,
    detector_id: &str,
    investigation_id: &str,
    case: &str,
) -> Value {
    let (status, body) = call_in(
        router,
        region,
        "POST",
        &format!("/detector/{detector_id}/investigation/list"),
        Some(json!({})),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected ListInvestigations to answer 200, got {status} {body}"
    );
    body["investigations"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a list of investigations, got {body}"))
        .iter()
        .find(|summary| summary["investigationId"] == json!(investigation_id))
        .cloned()
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected {investigation_id} to be listed, got {body}")
        })
}

/// Read the detector itself — a call whose only purpose here is to be one of the client's
/// own calls, which must leave the investigation where the clock had it.
async fn read_detector(router: &Router, region: &str, detector_id: &str, case: &str) {
    let (status, body) = call_in(
        router,
        region,
        "GET",
        &format!("/detector/{detector_id}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected GetDetector to answer 200, got {status} {body}"
    );
}

/// RULE 1, applied to one reported investigation: the phase it is in, asserting that the
/// three members that report the end of the analysis agree about it.
fn phase_of(case: &str, label: &str, investigation: &Value) -> Phase {
    let status = investigation["status"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?} ({label}): expected a status, got {investigation}"))
        .to_string();
    let start = investigation["startTime"].as_f64().unwrap_or_else(|| {
        panic!("case {case:?} ({label}): expected a startTime instant, got {investigation}")
    });
    assert!(
        start > 0.0,
        "case {case:?} ({label}): an initiated investigation started at a real instant, \
         got {investigation}"
    );
    let end = investigation["endTime"].as_f64();
    let error = investigation["error"].as_str();

    if status == INVESTIGATION_RUNNING {
        assert!(
            end.is_none() && error.is_none(),
            "case {case:?} ({label}): an investigation still {INVESTIGATION_RUNNING} has \
             not ended, so it reports no endTime and no error, got {investigation}"
        );
        return Phase::Running;
    }

    // The one terminal status an analysis this emulator could not run reaches: there is
    // no engine to produce a summary, so the run ends in the modelled failure state.
    assert_eq!(
        status, INVESTIGATION_FAILED,
        "case {case:?} ({label}): expected {INVESTIGATION_RUNNING} or \
         {INVESTIGATION_FAILED}, got {investigation}"
    );
    let end = end.unwrap_or_else(|| {
        panic!(
            "case {case:?} ({label}): an investigation that has ended reports the instant \
             it ended, got {investigation}"
        )
    });
    assert!(
        end >= start,
        "case {case:?} ({label}): expected an end instant at or after the start {start}, \
         got {end}"
    );
    let error = error.unwrap_or_else(|| {
        panic!(
            "case {case:?} ({label}): a {INVESTIGATION_FAILED} investigation carries the \
             reason it failed, got {investigation}"
        )
    });
    assert!(
        !error.is_empty() && error.len() <= MAX_ERROR_LENGTH,
        "case {case:?} ({label}): expected a reason within the modelled \
         {MAX_ERROR_LENGTH} characters, got {error:?}"
    );
    assert!(
        investigation["summary"].is_null()
            && investigation["riskLevel"].is_null()
            && investigation["confidence"].is_null()
            && investigation["risk"].is_null(),
        "case {case:?} ({label}): no analysis ran, so no assessment may be reported, got \
         {investigation}"
    );
    Phase::Ended
}

/// RULE 3: the two readers of one investigation report the same facts about it.
fn assert_readers_agree(case: &str, label: &str, got: &Value, listed: &Value) {
    assert_eq!(
        (&listed["status"], &listed["endTime"]),
        (&got["status"], &got["endTime"]),
        "case {case:?} ({label}): ListInvestigations and GetInvestigation must report one \
         investigation's status and end instant alike, got {listed} and {got}"
    );
    assert_eq!(
        listed["startTime"], got["startTime"],
        "case {case:?} ({label}): both readers report the instant the analysis started, \
         got {listed} and {got}"
    );
}

#[tokio::test]
async fn an_investigations_reported_end_follows_the_clock_and_no_call_of_the_clients_moves_it() {
    for Row {
        case,
        region,
        prompt,
        rounds,
    } in rows()
    {
        // A store per row, so one row's investigation cannot explain another's.
        let router = new_router();
        let detector_id = detector_in(&router, region, case).await;
        let id = initiate(&router, region, &detector_id, prompt, case).await;

        // The create answered an instant ago, so the analysis is still under way: this is
        // the state a client that reads straight back observes.
        let first = investigation(&router, region, &detector_id, &id, case).await;
        assert_eq!(
            phase_of(case, "the first read", &first),
            Phase::Running,
            "case {case:?}: an investigation read straight after the create is still \
             {INVESTIGATION_RUNNING}, got {first}"
        );
        assert_eq!(
            first["triggerPrompt"],
            json!(prompt),
            "case {case:?}: the read reports the prompt the create stored, got {first}"
        );

        // The client's own calls come next. None of them ends the analysis, so the phase
        // a read reports never goes back, and every read's members agree with each other
        // and with the list.
        let mut phase = Phase::Running;
        for round in 0..rounds {
            let label = &format!("round {round}");
            read_detector(&router, region, &detector_id, case).await;
            let listed = summary(&router, region, &detector_id, &id, case).await;
            let got = investigation(&router, region, &detector_id, &id, case).await;
            assert_readers_agree(case, label, &got, &listed);
            let seen = phase_of(case, label, &got);
            assert!(
                !(phase == Phase::Ended && seen == Phase::Running),
                "case {case:?} ({label}): an investigation the clock ended never runs \
                 again, got {got}"
            );
            phase = seen;
        }

        // Past the span the clock owed the end in: every member that reports it is in.
        tokio::time::sleep(past_the_analysis()).await;
        let ended = investigation(&router, region, &detector_id, &id, case).await;
        assert_eq!(
            phase_of(case, "past the span", &ended),
            Phase::Ended,
            "case {case:?}: past the span the clock owed, the analysis has ended, got \
             {ended}"
        );
        let listed = summary(&router, region, &detector_id, &id, case).await;
        assert_readers_agree(case, "past the span", &ended, &listed);

        // And reading it again changes nothing a later caller sees.
        let again = investigation(&router, region, &detector_id, &id, case).await;
        assert_eq!(
            again, ended,
            "case {case:?}: reading an ended investigation must not change what a later \
             read sees, got {ended} then {again}"
        );
    }
}
