// SPDX-License-Identifier: CC0-1.0

//! The `@idempotencyToken` REPLAY path of `StartExperiment`, which reports an experiment
//! this service already started instead of starting another one.
//!
//! RULE: an operation that REPORTS an experiment reports it with every write the clock
//! already owed that experiment materialised — the status transition that had fallen due,
//! and the experiment report an ENDED experiment that configured one owes — so any two
//! operations reporting one experiment at the same instant report the same state and the
//! same report, and the report object is in the configured output once either of them has
//! answered. A retry carrying a token that already started an experiment starts nothing:
//! it is one of those reporting calls.
//!
//! Every assertion goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the same path a
//! client takes.
//!
//! The rows differ along what the rule generalises over: how the run ENDED (its actions
//! elapsing, a `StopExperiment`), what the report configuration asks for (an output with a
//! nested prefix, an output with none, a configuration naming no output at all, no
//! configuration at all) and so which state the generation lands in (`completed`, the
//! modelled `failed`, or no report member at all), and WHEN the retry arrives (before the
//! run ended, and after it).
//!
//! The run is driven to its end through `ListExperiments`, whose summary carries no
//! `experimentReport`: that leaves the generation owed but unmaterialised, so the replay
//! under test is the first call that could materialise it and the written object is
//! attributable to it.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::experiment_lifecycle::{get_experiment, send, FALLBACK_REGION};
use crate::experiment_state_reason_and_report::{
    emulator_with_object_storage, s3, start_from, template_for, Ending, BUCKET,
    REPORT_OBJECT_NAME,
};

/// The statuses an experiment's run ENDS in, which is when the report generation falls due.
const TERMINAL_STATUSES: [&str; 4] = ["completed", "failed", "stopped", "cancelled"];

/// How long a row may take to reach its terminal status: the whole compressed run window
/// plus the transitions around it and slack for a loaded machine.
const TERMINAL_BUDGET: Duration = Duration::from_secs(25);

/// How long after the start a transition the emulator owes must have been materialised:
/// past the emulator's own non-zero span and well inside the shortest run.
const PAST_TRANSITION: Duration = Duration::from_millis(900);

/// What a row's template asks for by way of an experiment report.
#[derive(Debug, Clone, Copy)]
enum ReportConfig {
    /// An output naming the report bucket, under this prefix (`None` names no prefix).
    Written(Option<&'static str>),
    /// A configuration that asks for a report without naming anywhere to put it, which is
    /// the generation the crate fails with `outputNotConfigured`.
    NoOutput,
    /// No report configuration at all, so there is no report to generate.
    NotConfigured,
}

/// One row: what its template configures and how its run ends.
struct ReplayRow {
    case: &'static str,
    report: ReportConfig,
    ending: Ending,
}

impl ReplayRow {
    /// The `experimentReportConfiguration` this row's template carries, merged over the
    /// base template body.
    fn template_extra(&self) -> Value {
        match self.report {
            ReportConfig::Written(prefix) => {
                let mut s3_configuration = json!({ "bucketName": BUCKET });
                if let Some(prefix) = prefix {
                    s3_configuration["prefix"] = json!(prefix);
                }
                json!({
                    "experimentReportConfiguration": {
                        "outputs": { "s3Configuration": s3_configuration },
                        "preExperimentDuration": "PT1M",
                        "postExperimentDuration": "PT5M"
                    }
                })
            }
            ReportConfig::NoOutput => json!({
                "experimentReportConfiguration": { "preExperimentDuration": "PT1M" }
            }),
            ReportConfig::NotConfigured => json!({}),
        }
    }

    /// The key the report object is written under, or `None` for a row whose report is
    /// never written anywhere.
    fn object_key(&self, id: &str) -> Option<String> {
        match self.report {
            ReportConfig::Written(prefix) => Some(report_key(prefix, id)),
            ReportConfig::NoOutput | ReportConfig::NotConfigured => None,
        }
    }
}

/// The key the report of `id` takes under the output prefix `prefix`: the prefix, then the
/// experiment the report is about, then the object's name. Spelled out here rather than
/// taken from the crate, so a changed key fails this suite.
fn report_key(prefix: Option<&str>, id: &str) -> String {
    match prefix
        .map(|prefix| prefix.trim_matches('/'))
        .filter(|prefix| !prefix.is_empty())
    {
        Some(prefix) => format!("{prefix}/{id}/{REPORT_OBJECT_NAME}"),
        None => format!("{id}/{REPORT_OBJECT_NAME}"),
    }
}

/// The rows the rule is asserted over. See the suite docs for what they span.
fn replay_rows() -> Vec<ReplayRow> {
    vec![
        ReplayRow {
            case: "replay-elapsed-run-nested-prefix",
            report: ReportConfig::Written(Some("replay/2026/q1")),
            ending: Ending::Elapses,
        },
        ReplayRow {
            case: "replay-stopped-run-no-prefix",
            report: ReportConfig::Written(None),
            ending: Ending::Stopped,
        },
        ReplayRow {
            case: "replay-generation-that-failed",
            report: ReportConfig::NoOutput,
            ending: Ending::Elapses,
        },
        ReplayRow {
            case: "replay-without-a-report-configuration",
            report: ReportConfig::NotConfigured,
            ending: Ending::Elapses,
        },
    ]
}

/// The `state.status` `ListExperiments` reports for `id`, walking the pages until the
/// summary is found. The summary carries no `experimentReport`, so polling through it
/// leaves the report generation owed.
async fn listed_status(router: &MultiServiceRouter, case: &str, id: &str) -> String {
    let mut next_token: Option<String> = None;
    loop {
        let uri = match &next_token {
            Some(token) => format!("/experiments?maxResults=100&nextToken={token}"),
            None => "/experiments?maxResults=100".to_string(),
        };
        let answer = send(router, "GET", &uri, None, FALLBACK_REGION).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected the experiments to be listed, got {} with {:?}",
            answer.status, answer.body
        );
        let summary = answer
            .member("experiments")
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .find(|summary| summary["id"].as_str() == Some(id));
        if let Some(summary) = summary {
            return summary["state"]["status"]
                .as_str()
                .unwrap_or_default()
                .to_string();
        }
        match answer.body["nextToken"].as_str() {
            Some(token) => next_token = Some(token.to_string()),
            None => panic!("case {case:?}: {id} is not listed in {:?}", answer.body),
        }
    }
}

/// Drive `id` to its terminal status through `ListExperiments` alone, and report that
/// status.
async fn poll_until_listed_terminal(router: &MultiServiceRouter, case: &str, id: &str) -> String {
    const INTERVAL: Duration = Duration::from_millis(50);
    let deadline = Instant::now() + TERMINAL_BUDGET;
    loop {
        let status = listed_status(router, case, id).await;
        if TERMINAL_STATUSES.contains(&status.as_str()) {
            return status;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected {id} to reach a terminal status within \
             {TERMINAL_BUDGET:?}, still {status:?}"
        );
        tokio::time::sleep(INTERVAL).await;
    }
}

/// RULE, over every row: the retry that replays a token reports the experiment with the
/// report generation the clock owed it materialised, and agrees with the sibling read.
#[tokio::test]
async fn a_token_replay_reports_the_experiment_report_the_clock_already_owed() {
    let router = emulator_with_object_storage();
    let (status, body) = s3(&router, "PUT", &format!("/{BUCKET}")).await;
    assert_eq!(
        status,
        200,
        "expected the report bucket to be created, got {status} with {:?}",
        String::from_utf8_lossy(&body)
    );

    let rows = replay_rows();
    let mut started = Vec::new();
    for row in &rows {
        let template_id =
            template_for(&router, row.case, row.ending, row.template_extra()).await;
        let answer = start_from(&router, row.case, &template_id).await;
        let experiment = answer.member("experiment").clone();
        let id = experiment["id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {:?}: no experiment id in {experiment:?}", row.case))
            .to_string();
        started.push((row, template_id, id));
    }

    // A retry that arrives BEFORE the run ended replays the same experiment and owes no
    // report yet: there is nothing to report on until the experiment ends.
    tokio::time::sleep(PAST_TRANSITION).await;
    for (row, template_id, id) in &started {
        let replay = start_from(&router, row.case, template_id).await;
        let experiment = replay.member("experiment").clone();
        assert_eq!(
            experiment["id"].as_str().unwrap_or_default(),
            id.as_str(),
            "case {:?}: a replay reports the experiment the token started, got \
             {experiment:?}",
            row.case
        );
        let status = experiment["state"]["status"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            !TERMINAL_STATUSES.contains(&status.as_str()),
            "case {:?}: expected the run to still be under way, got {status:?}",
            row.case
        );
        let report = experiment["experimentReport"].clone();
        match row.report {
            ReportConfig::NotConfigured => assert!(
                report.is_null(),
                "case {:?}: an experiment that configured no report carries none, got \
                 {report:?}",
                row.case
            ),
            _ => {
                assert_eq!(
                    report["state"]["status"].as_str().unwrap_or_default(),
                    "pending",
                    "case {:?}: a report is pending until its experiment ends, got {report:?}",
                    row.case
                );
                assert!(
                    report["s3Reports"].is_null(),
                    "case {:?}: nothing is named until a report has been written, got \
                     {report:?}",
                    row.case
                );
            }
        }
        if let Some(key) = row.object_key(id) {
            let (status, _) = s3(&router, "GET", &format!("/{BUCKET}/{key}")).await;
            assert_ne!(
                status, 200,
                "case {:?}: no report may be written while the experiment is still \
                 running (s3://{BUCKET}/{key})",
                row.case
            );
        }
    }

    // The row that is stopped rather than left to elapse.
    for (row, _, id) in &started {
        if row.ending != Ending::Stopped {
            continue;
        }
        let stopped = crate::experiment_lifecycle::stop_experiment(&router, id).await;
        assert_eq!(
            stopped.status, 200,
            "case {:?}: expected HTTP 200 from StopExperiment, got {} with {:?}",
            row.case, stopped.status, stopped.body
        );
    }

    for (row, template_id, id) in &started {
        // Driven to its end through the listing, which reports no report: the generation
        // is owed and nothing has materialised it when the replay below arrives.
        let status = poll_until_listed_terminal(&router, row.case, id).await;
        assert_eq!(
            status,
            row.ending.terminal(),
            "case {:?}: expected the run to end {}, got {status:?}",
            row.case,
            row.ending.terminal()
        );
        if let Some(key) = row.object_key(id) {
            let (status, _) = s3(&router, "GET", &format!("/{BUCKET}/{key}")).await;
            assert_ne!(
                status, 200,
                "case {:?}: nothing may have written s3://{BUCKET}/{key} before the \
                 replay under test",
                row.case
            );
        }

        // THE REPLAY: the retry a client sends, arriving after the run ended.
        let replay = start_from(&router, row.case, template_id).await;
        let experiment = replay.member("experiment").clone();
        assert_eq!(
            experiment["id"].as_str().unwrap_or_default(),
            id.as_str(),
            "case {:?}: a replay reports the experiment the token started, got \
             {experiment:?}",
            row.case
        );
        assert_eq!(
            experiment["state"]["status"].as_str().unwrap_or_default(),
            row.ending.terminal(),
            "case {:?}: expected the replay to report the status the clock owed, got \
             {experiment:?}",
            row.case
        );
        let report = experiment["experimentReport"].clone();
        match row.report {
            ReportConfig::Written(_) => {
                assert_eq!(
                    report["state"]["status"].as_str().unwrap_or_default(),
                    "completed",
                    "case {:?}: expected the replay to report the generation the clock \
                     owed an ended experiment, got {report:?}",
                    row.case
                );
                assert!(
                    report["state"]["error"].is_null(),
                    "case {:?}: a generated report reports no error, got {report:?}",
                    row.case
                );
                let key = row
                    .object_key(id)
                    .unwrap_or_else(|| panic!("case {:?}: this row writes a report", row.case));
                let s3_reports = report["s3Reports"]
                    .as_array()
                    .cloned()
                    .unwrap_or_else(|| panic!("case {:?}: no s3Reports in {report:?}", row.case));
                assert_eq!(
                    s3_reports.len(),
                    1,
                    "case {:?}: expected one written report, got {s3_reports:?}",
                    row.case
                );
                assert_eq!(
                    s3_reports[0]["arn"].as_str().unwrap_or_default(),
                    format!("arn:aws:s3:::{BUCKET}/{key}"),
                    "case {:?}: expected the report arn to name the object it was written \
                     to, got {s3_reports:?}",
                    row.case
                );

                // And the object the replay named is really there, holding this
                // experiment's own record — written by the replay, which is the only call
                // that could have.
                let (status, body) = s3(&router, "GET", &format!("/{BUCKET}/{key}")).await;
                assert_eq!(
                    status,
                    200,
                    "case {:?}: expected the replay to have written s3://{BUCKET}/{key}, \
                     got {status} with {:?}",
                    row.case,
                    String::from_utf8_lossy(&body)
                );
                let document: Value = serde_json::from_slice(&body).unwrap_or_else(|error| {
                    panic!(
                        "case {:?}: the report object is not JSON ({error}): {:?}",
                        row.case,
                        String::from_utf8_lossy(&body)
                    )
                });
                assert_eq!(
                    document["experiment"]["id"].as_str().unwrap_or_default(),
                    id.as_str(),
                    "case {:?}: expected the report to be about this experiment, got \
                     {document:?}",
                    row.case
                );
                assert_eq!(
                    document["experiment"]["state"]["status"]
                        .as_str()
                        .unwrap_or_default(),
                    row.ending.terminal(),
                    "case {:?}: expected the report to state the status the run ended in, \
                     got {document:?}",
                    row.case
                );
            }
            ReportConfig::NoOutput => {
                assert_eq!(
                    report["state"]["status"].as_str().unwrap_or_default(),
                    "failed",
                    "case {:?}: expected the replay to report the failed generation, got \
                     {report:?}",
                    row.case
                );
                assert_eq!(
                    report["state"]["error"]["code"].as_str().unwrap_or_default(),
                    "outputNotConfigured",
                    "case {:?}: expected the code that says which failure it was, got \
                     {report:?}",
                    row.case
                );
                assert!(
                    report["s3Reports"].is_null(),
                    "case {:?}: a failed generation names no written report, got {report:?}",
                    row.case
                );
            }
            ReportConfig::NotConfigured => assert!(
                report.is_null(),
                "case {:?}: an ended experiment that configured no report still carries \
                 none, got {report:?}",
                row.case
            ),
        }

        // The sibling read at the same instant reports the same report, and generates
        // nothing a second time.
        let read = get_experiment(&router, id).await.member("experiment").clone();
        assert_eq!(
            read["experimentReport"], report,
            "case {:?}: expected the read and the replay to report one report, got {:?}",
            row.case, read["experimentReport"]
        );
        assert_eq!(
            read["state"], experiment["state"],
            "case {:?}: expected the read and the replay to report one state, got {:?}",
            row.case, read["state"]
        );
    }
}
