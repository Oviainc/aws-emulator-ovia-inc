// SPDX-License-Identifier: CC0-1.0

//! The two members of an `Experiment` that FIS COMPUTES from the run rather than copying
//! from the template it was started from: `state.reason` and `experimentReport`.
//!
//! Every assertion here goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the same path a
//! client takes. Two rules are under test:
//!
//! * RULE (the state reason): `ExperimentState$reason` is "the reason for the state" of
//!   EVERY status FIS reports and is restricted to none of them, so an experiment pairs its
//!   status with a non-empty reason in every status it passes through — the transient
//!   `initiating`, `running` and `stopping` as well as the terminal ones — and the reason
//!   moves with the status rather than standing still. The one wording AWS publishes is the
//!   documented `StartExperiment` response's `{"status": "initiating", "reason": "Experiment
//!   is initiating."}`, which is asserted verbatim; for the rest, what the contract requires
//!   is a reason being there beside the status, within `ExperimentStatusReason`'s modelled
//!   `@length(0..512)`. The rule holds wherever an `ExperimentState` is reported, so it is
//!   asserted on `StartExperiment`, `GetExperiment`, `StopExperiment` and the summary
//!   `ListExperiments` carries.
//! * RULE (the report): an experiment started from a template that configured an experiment
//!   report carries an `experimentReport`, whose `state.status` is the state of the report
//!   GENERATION: `pending` while the experiment has not ended (there is nothing to report
//!   on), and once it has ended — however it ended — the report is written to the Amazon S3
//!   output the configuration names, so the generation is `completed` and
//!   `s3Reports` names the object that was written. A report configuration that names no
//!   output, or an output that will not take the report, leaves the generation in the
//!   modelled `failed` state with the code that says which failure it was. An experiment
//!   whose template configured no report carries no `experimentReport` at all.
//!
//! The rows differ along what those rules generalise over: how the run ENDED (its actions
//! elapsing, a `StopExperiment`, a target resolving to nothing under the mode that fails
//! for it), the form of the configured output prefix (a trailing slash, a nested prefix,
//! none at all), and whether the configuration names CloudWatch dashboards as data sources.
//!
//! # What the written report is
//!
//! A JSON document built from the experiment's own persisted record — its identity, the
//! lifecycle its run walked, the actions it carried out or skipped, the resources its
//! targets resolved to and the configuration that asked for the report. The suite reads the
//! object back out of S3 and asserts those values against the experiment the API reports,
//! so a report that was not computed from the record cannot pass. No dashboard snapshot and
//! no metric data is in it, because this emulator captures none, and the document says so.

use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_fis_faultinjectionsimulator::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::experiment_lifecycle::{
    create_template, emulator, get_experiment, send, start_experiment, stop_experiment, token,
    Answer, FALLBACK_REGION,
};

/// The reason AWS documents for a freshly started experiment: the `StartExperiment`
/// response in the FIS API reference pairs `"status": "initiating"` with this text.
const DOCUMENTED_INITIATING_REASON: &str = "Experiment is initiating.";

/// `ExperimentStatusReason`'s and `ExperimentReportReason`'s published ceiling
/// (`@length(min: 0, max: 512)`), spelled out here rather than imported from the crate so a
/// wrong bound in `src/state.rs` fails this suite.
const REASON_CEILING: usize = 512;

/// How long after a call that starts a transition the emulator owes it must have been
/// materialised: past the emulator's own non-zero span and well inside the shortest run.
const PAST_TRANSITION: Duration = Duration::from_millis(900);

/// How long a row may take to reach its terminal status: the whole compressed run window
/// plus the transitions around it and slack for a loaded machine.
const TERMINAL_BUDGET: Duration = Duration::from_secs(25);

/// A role ARN the emulator's IAM store holds no role for, so nothing about its trust policy
/// or its permissions refuses or fails a run here.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/fis-report";

/// The bucket every row that expects its report to be WRITTEN names, created in S3 before
/// the rows run.
const BUCKET: &str = "fis-reports";

/// The name the report object carries under `<prefix>/<experimentId>/`, spelled out here so
/// a changed key fails this suite.
const REPORT_OBJECT_NAME: &str = "experiment-report.json";

/// One emulator carrying FIS *and* S3 over one shared engine — what the assembled emulator
/// does. The report of an ended experiment is written by calling S3 in process, so a suite
/// that asserts the written report needs object storage answering.
fn emulator_with_object_storage() -> MultiServiceRouter {
    let engine: std::sync::Arc<engine::InMemoryEngine> =
        std::sync::Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(std::sync::Arc::clone(&engine)));
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        std::sync::Arc::clone(&engine),
    ));
    router
}

/// Send one path-style request to the emulator's S3 front door. S3 is a REST service: the
/// bucket and key live in the URI and the body is the object's own bytes, and the credential
/// scope names `s3`, which is what the router claims on.
async fn s3(router: &MultiServiceRouter, method: &str, path: &str) -> (u16, Vec<u8>) {
    let request = ::http::Request::builder()
        .method(method)
        .uri(path)
        .header("host", format!("s3.{FALLBACK_REGION}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{FALLBACK_REGION}/s3/\
                 aws4_request, SignedHeaders=host, Signature=0123456789abcdef"
            ),
        )
        .body(Bytes::new())
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    (
        response.status().as_u16(),
        response.body().as_ref().to_vec(),
    )
}

/// How a row's run ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ending {
    /// The actions elapse on the emulator's compressed run clock: `completed`.
    Elapses,
    /// `StopExperiment` is called on it: `stopping` -> `stopped`.
    Stopped,
    /// A target resolves to no resources under the documented default
    /// `emptyTargetResolutionMode` (`fail`): `failed`.
    EmptyTargetResolution,
}

impl Ending {
    /// The terminal `state.status` this ending reaches.
    fn terminal(self) -> &'static str {
        match self {
            Self::Elapses => "completed",
            Self::Stopped => "stopped",
            Self::EmptyTargetResolution => "failed",
        }
    }

    /// The `targets` a template needs for this ending.
    fn targets(self) -> Value {
        match self {
            // A target identified by TAG: this emulator holds no other service's resources
            // to resolve a tag against, so it resolves to nothing, which the documented
            // default mode fails the experiment for.
            Self::EmptyTargetResolution => json!({
                "taggedInstances": {
                    "resourceType": "aws:ec2:instance",
                    "resourceTags": { "Stage": "test" },
                    "selectionMode": "ALL"
                }
            }),
            _ => json!({}),
        }
    }

    /// The `actions` a template needs for this ending.
    fn actions(self) -> Value {
        match self {
            Self::EmptyTargetResolution => json!({
                "stopInstances": {
                    "actionId": "aws:ec2:stop-instances",
                    "parameters": { "startInstancesAfterDuration": "PT2M" },
                    "targets": { "Instances": "taggedInstances" }
                }
            }),
            // A wait long enough that the row is still running when it is polled, and
            // inside the 1..720 minutes FIS documents for `aws:fis:wait`.
            _ => json!({
                "wait": { "actionId": "aws:fis:wait", "parameters": { "duration": "PT5M" } }
            }),
        }
    }
}

/// Create a template for `case` with `extra` merged over the base body, and return its id.
async fn template_for(
    router: &MultiServiceRouter,
    case: &str,
    ending: Ending,
    extra: Value,
) -> String {
    let mut body = json!({
        "clientToken": token(&format!("state-report-{case}")),
        "description": format!("state reason and report: {case}"),
        "roleArn": ROLE_ARN,
        "stopConditions": [{ "source": "none" }],
        "targets": ending.targets(),
        "actions": ending.actions()
    });
    for (key, value) in extra.as_object().cloned().unwrap_or_default() {
        body[key] = value;
    }
    let created = create_template(router, body).await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: expected the template to be created, got {} with {:?}",
        created.status, created.body
    );
    created.member("experimentTemplate")["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no template id in {:?}", created.body))
        .to_string()
}

/// Start an experiment from `template_id` and return the answer.
async fn start_from(router: &MultiServiceRouter, case: &str, template_id: &str) -> Answer {
    let started = start_experiment(
        router,
        json!({
            "clientToken": token(&format!("state-report-start-{case}")),
            "experimentTemplateId": template_id
        }),
    )
    .await;
    assert_eq!(
        started.status, 200,
        "case {case:?}: expected the start to be accepted, got {} with {:?}",
        started.status, started.body
    );
    started
}

/// The `state` of `experiment`, asserted to carry a reason at all — the rule — and to stay
/// within the member's published ceiling. Returns that reason.
fn reason_of(case: &str, where_from: &str, experiment: &Value) -> String {
    let state = &experiment["state"];
    let status = state["status"].as_str().unwrap_or_default();
    assert!(
        !status.is_empty(),
        "case {case:?} ({where_from}): expected a state status, got {state:?}"
    );
    let reason = state["reason"].as_str().unwrap_or_else(|| {
        panic!(
            "case {case:?} ({where_from}): status {status:?} must name the reason for the \
             state, got {state:?}"
        )
    });
    assert!(
        !reason.is_empty() && reason.chars().count() <= REASON_CEILING,
        "case {case:?} ({where_from}): expected a reason within {REASON_CEILING} characters, \
         got {reason:?}"
    );
    reason.to_string()
}

/// Poll `GetExperiment` until the experiment is in a terminal status, and panic when
/// [`TERMINAL_BUDGET`] runs out first, so a row that never ends says so rather than
/// surfacing as a status mismatch.
async fn poll_until_terminal(router: &MultiServiceRouter, id: &str) -> Value {
    const INTERVAL: Duration = Duration::from_millis(50);
    let deadline = std::time::Instant::now() + TERMINAL_BUDGET;
    loop {
        let experiment = get_experiment(router, id).await.member("experiment").clone();
        let status = experiment["state"]["status"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        if ["completed", "failed", "stopped", "cancelled"].contains(&status.as_str()) {
            return experiment;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "{id}: expected a terminal status within {TERMINAL_BUDGET:?}, still \
             {status:?} in {experiment:?}"
        );
        tokio::time::sleep(INTERVAL).await;
    }
}

/// RULE (the state reason), over every status an experiment passes through and every
/// operation that reports an `ExperimentState`.
#[tokio::test]
async fn every_status_an_experiment_reports_names_the_reason_for_that_state() {
    // No row here configures a report, so FIS alone is enough.
    let router = emulator();
    let rows = [
        ("run-elapses", Ending::Elapses),
        ("stopped-by-request", Ending::Stopped),
        ("empty-target-resolution", Ending::EmptyTargetResolution),
    ];

    // Each row's experiment is started first, so the runs overlap instead of being waited
    // out one after another. The account's active-experiment quota is five.
    let mut started = Vec::new();
    for (case, ending) in rows {
        let template_id = template_for(&router, case, ending, json!({})).await;
        let answer = start_from(&router, case, &template_id).await;
        let experiment = answer.member("experiment").clone();
        assert_eq!(
            experiment["state"]["status"].as_str().unwrap_or_default(),
            "initiating",
            "case {case:?}: expected a started experiment to be initiating, got {experiment:?}"
        );
        // The one wording AWS publishes for this set.
        assert_eq!(
            reason_of(case, "StartExperiment", &experiment),
            DOCUMENTED_INITIATING_REASON,
            "case {case:?}: expected the documented initiating reason, got {experiment:?}"
        );
        let id = experiment["id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no experiment id in {experiment:?}"))
            .to_string();
        started.push((case, ending, id));
    }

    // Every row is READ while it is running: the transient status names its reason too,
    // and that reason is not the one the mint stamped.
    tokio::time::sleep(PAST_TRANSITION).await;
    let mut running_reasons = Vec::new();
    for (case, ending, id) in &started {
        let answer = get_experiment(&router, id).await;
        let experiment = answer.member("experiment").clone();
        assert_eq!(
            experiment["state"]["status"].as_str().unwrap_or_default(),
            "running",
            "case {case:?}: expected the experiment to be running, got {:?}",
            answer.body
        );
        let reason = reason_of(case, "GetExperiment while running", &experiment);
        assert_ne!(
            reason, DOCUMENTED_INITIATING_REASON,
            "case {case:?}: the reason moves with the status, got {reason:?} while running"
        );
        // The summary a list carries reports the same state as the read of the record.
        let listed = send(&router, "GET", "/experiments", None, FALLBACK_REGION).await;
        let summary = listed
            .member("experiments")
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: no experiments list in {:?}", listed.body))
            .iter()
            .find(|summary| summary["id"].as_str() == Some(id.as_str()))
            .unwrap_or_else(|| panic!("case {case:?}: {id} is not listed in {:?}", listed.body))
            .clone();
        assert_eq!(
            summary["state"], experiment["state"],
            "case {case:?}: expected the listed state to agree with the read one, \
             got {:?}",
            summary["state"]
        );
        running_reasons.push((*case, *ending, id.clone(), reason));
    }

    // The stop row is stopped here: `stopping` is a status FIS reports a reason for as much
    // as any other.
    for (case, ending, id, _) in &running_reasons {
        if *ending != Ending::Stopped {
            continue;
        }
        let stopped = stop_experiment(&router, id).await;
        assert_eq!(
            stopped.status, 200,
            "case {case:?}: expected HTTP 200 from StopExperiment, got {} with {:?}",
            stopped.status, stopped.body
        );
        let experiment = stopped.member("experiment").clone();
        assert_eq!(
            experiment["state"]["status"].as_str().unwrap_or_default(),
            "stopping",
            "case {case:?}: expected the stop to be in progress, got {experiment:?}"
        );
        reason_of(case, "StopExperiment", &experiment);
    }

    // And each row's terminal status names its own reason, which is not the running one.
    for (case, ending, id, running_reason) in &running_reasons {
        let experiment = poll_until_terminal(&router, id).await;
        assert_eq!(
            experiment["state"]["status"].as_str().unwrap_or_default(),
            ending.terminal(),
            "case {case:?}: expected the run to end {}, got {experiment:?}",
            ending.terminal()
        );
        let reason = reason_of(case, "GetExperiment once terminal", &experiment);
        assert_ne!(
            &reason, running_reason,
            "case {case:?}: the reason moves with the status, got {reason:?} once terminal"
        );
    }
}

/// One report row: where its report goes, what its configuration names, and how its run
/// ends.
struct ReportRow {
    case: &'static str,
    /// The `prefix` the output names, or `None` for an output that names none.
    prefix: Option<&'static str>,
    /// The key the report object must be written under, with `{id}` standing for the
    /// experiment id. Spelled out per row rather than computed by the crate.
    key: &'static str,
    /// Whether the configuration names CloudWatch dashboards as data sources.
    dashboards: bool,
    ending: Ending,
}

/// The rows the report rule is asserted over. See the suite docs for what they span.
fn report_rows() -> Vec<ReportRow> {
    vec![
        ReportRow {
            case: "prefix-with-a-trailing-slash-and-dashboards",
            prefix: Some("r/"),
            key: "r/{id}/experiment-report.json",
            dashboards: true,
            ending: Ending::Elapses,
        },
        ReportRow {
            case: "nested-prefix-stopped-by-request",
            prefix: Some("reports/2026/q1"),
            key: "reports/2026/q1/{id}/experiment-report.json",
            dashboards: false,
            ending: Ending::Stopped,
        },
        ReportRow {
            case: "no-prefix-at-all",
            prefix: None,
            key: "{id}/experiment-report.json",
            dashboards: false,
            ending: Ending::Elapses,
        },
        ReportRow {
            case: "a-run-that-failed-still-has-its-report",
            prefix: Some("failed"),
            key: "failed/{id}/experiment-report.json",
            dashboards: true,
            ending: Ending::EmptyTargetResolution,
        },
    ]
}

/// The `experimentReportConfiguration` of one row.
fn report_configuration(row: &ReportRow, bucket: &str) -> Value {
    let mut s3_configuration = json!({ "bucketName": bucket });
    if let Some(prefix) = row.prefix {
        s3_configuration["prefix"] = json!(prefix);
    }
    let mut configuration = json!({
        "outputs": { "s3Configuration": s3_configuration },
        "preExperimentDuration": "PT1M",
        "postExperimentDuration": "PT5M"
    });
    if row.dashboards {
        configuration["dataSources"] = json!({
            "cloudWatchDashboards": [
                { "dashboardIdentifier": "arn:aws:cloudwatch::000000000000:dashboard/fis" }
            ]
        });
    }
    configuration
}

/// RULE (the report): the report of an ENDED experiment is written to the configured output,
/// and the experiment names where it went.
#[tokio::test]
async fn an_ended_experiments_report_is_written_to_the_configured_output() {
    let router = emulator_with_object_storage();
    let (status, body) = s3(&router, "PUT", &format!("/{BUCKET}")).await;
    assert_eq!(
        status,
        200,
        "expected the report bucket to be created, got {status} with {:?}",
        String::from_utf8_lossy(&body)
    );

    let rows = report_rows();
    let mut started = Vec::new();
    for row in &rows {
        let template_id = template_for(
            &router,
            row.case,
            row.ending,
            json!({ "experimentReportConfiguration": report_configuration(row, BUCKET) }),
        )
        .await;
        let answer = start_from(&router, row.case, &template_id).await;
        let experiment = answer.member("experiment").clone();
        // Nothing has been generated yet: the experiment has not ended.
        let report = &experiment["experimentReport"];
        assert_eq!(
            report["state"]["status"].as_str().unwrap_or_default(),
            "pending",
            "case {:?}: a report is pending until its experiment ends, got {report:?}",
            row.case
        );
        assert!(
            report["state"]["reason"]
                .as_str()
                .is_some_and(|reason| !reason.is_empty()),
            "case {:?}: a report state names its reason too, got {report:?}",
            row.case
        );
        assert!(
            report["s3Reports"].is_null(),
            "case {:?}: nothing is named until a report has been written, got {report:?}",
            row.case
        );
        assert!(
            report["state"]["error"].is_null(),
            "case {:?}: a pending generation has not failed, got {report:?}",
            row.case
        );
        let id = experiment["id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {:?}: no experiment id in {experiment:?}", row.case))
            .to_string();
        started.push((row, id));
    }

    // The row that is stopped rather than left to elapse.
    for (row, id) in &started {
        if row.ending == Ending::Stopped {
            let stopped = stop_experiment(&router, id).await;
            assert_eq!(
                stopped.status, 200,
                "case {:?}: expected HTTP 200 from StopExperiment, got {} with {:?}",
                row.case, stopped.status, stopped.body
            );
        }
    }

    for (row, id) in &started {
        let experiment = poll_until_terminal(&router, id).await;
        assert_eq!(
            experiment["state"]["status"].as_str().unwrap_or_default(),
            row.ending.terminal(),
            "case {:?}: expected the run to end {}, got {experiment:?}",
            row.case,
            row.ending.terminal()
        );

        // Once the experiment has ended the report has been generated, and the experiment
        // names the object it was written to.
        let experiment = get_experiment(&router, id).await.member("experiment").clone();
        let report = experiment["experimentReport"].clone();
        assert_eq!(
            report["state"]["status"].as_str().unwrap_or_default(),
            "completed",
            "case {:?}: expected the report of an ended experiment to have been \
             generated, got {report:?}",
            row.case
        );
        assert!(
            report["state"]["error"].is_null(),
            "case {:?}: a generated report reports no error, got {report:?}",
            row.case
        );
        let key = row.key.replace("{id}", id);
        let s3_reports = report["s3Reports"]
            .as_array()
            .unwrap_or_else(|| panic!("case {:?}: no s3Reports in {report:?}", row.case))
            .clone();
        assert_eq!(
            s3_reports.len(),
            1,
            "case {:?}: expected one written report, got {s3_reports:?}",
            row.case
        );
        assert_eq!(
            s3_reports[0]["arn"].as_str().unwrap_or_default(),
            format!("arn:aws:s3:::{BUCKET}/{key}"),
            "case {:?}: expected the report arn to name the object it was written to, \
             got {s3_reports:?}",
            row.case
        );
        assert!(
            s3_reports[0]["reportType"]
                .as_str()
                .is_some_and(|report_type| !report_type.is_empty()),
            "case {:?}: a written report names its type, got {s3_reports:?}",
            row.case
        );

        // And the object is REALLY there, holding the experiment's own record.
        let (status, body) = s3(&router, "GET", &format!("/{BUCKET}/{key}")).await;
        assert_eq!(
            status,
            200,
            "case {:?}: expected the report object at s3://{BUCKET}/{key}, got {status} \
             with {:?}",
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
            "case {:?}: expected the report to be about this experiment, got {document:?}",
            row.case
        );
        assert_eq!(
            document["experiment"]["arn"], experiment["arn"],
            "case {:?}: expected the report to carry the experiment's arn, got {document:?}",
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
        assert_eq!(
            document["experiment"]["state"]["reason"], experiment["state"]["reason"],
            "case {:?}: expected the report to state the experiment's own reason, \
             got {document:?}",
            row.case
        );
        assert!(
            document["experiment"]["endTime"].is_string(),
            "case {:?}: an ended experiment's report states when it ended, got {document:?}",
            row.case
        );
        for name in row.ending.actions().as_object().cloned().unwrap_or_default().keys() {
            assert!(
                document["actions"][name].is_object(),
                "case {:?}: expected the report to carry the action {name:?}, got {document:?}",
                row.case
            );
        }
        let dashboards = document["reportConfiguration"]["dataSources"]["cloudWatchDashboards"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            dashboards.is_empty(),
            !row.dashboards,
            "case {:?}: expected the report to name the configured data sources, \
             got {document:?}",
            row.case
        );
        assert_eq!(
            document["reportConfiguration"]["postExperimentDuration"]
                .as_str()
                .unwrap_or_default(),
            "PT5M",
            "case {:?}: expected the report to carry the configured windows, got {document:?}",
            row.case
        );

        // Reading again generates nothing a second time: the report is the same object with
        // the same state, so a read changes nothing a later caller would not have seen.
        let again = get_experiment(&router, id).await.member("experiment").clone();
        assert_eq!(
            again["experimentReport"], report,
            "case {:?}: a second read must report the same generation, got {:?}",
            row.case, again["experimentReport"]
        );
    }
}

/// RULE (the report), on the two ways the generation can FAIL: the configuration names no
/// output, and the named output will not take the report.
#[tokio::test]
async fn a_report_that_cannot_be_written_reports_the_modelled_failure() {
    let router = emulator_with_object_storage();
    for (case, configuration, expected_code) in [
        (
            // `ExperimentReportConfiguration$outputs` is optional, so a configuration can
            // ask for a report without naming anywhere to put it.
            "no-output-configured",
            json!({ "preExperimentDuration": "PT1M" }),
            "outputNotConfigured",
        ),
        (
            // A bucket nothing created: the write is refused, which is a failure of the
            // generation rather than of any request.
            "output-bucket-does-not-exist",
            json!({
                "outputs": { "s3Configuration": { "bucketName": "no-such-report-bucket" } }
            }),
            "outputNotWritten",
        ),
    ] {
        let template_id = template_for(
            &router,
            case,
            Ending::Elapses,
            json!({ "experimentReportConfiguration": configuration }),
        )
        .await;
        let started = start_from(&router, case, &template_id).await;
        let id = started.member("experiment")["id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no experiment id in {:?}", started.body))
            .to_string();

        let experiment = poll_until_terminal(&router, &id).await;
        assert_eq!(
            experiment["state"]["status"].as_str().unwrap_or_default(),
            "completed",
            "case {case:?}: the report is the experiment's, not the run's: the run still \
             completes, got {experiment:?}"
        );
        let report = get_experiment(&router, &id)
            .await
            .member("experiment")["experimentReport"]
            .clone();
        assert_eq!(
            report["state"]["status"].as_str().unwrap_or_default(),
            "failed",
            "case {case:?}: expected the generation to have failed, got {report:?}"
        );
        assert_eq!(
            report["state"]["error"]["code"].as_str().unwrap_or_default(),
            expected_code,
            "case {case:?}: expected the code that says which failure it was, got {report:?}"
        );
        let reason = report["state"]["reason"].as_str().unwrap_or_default();
        assert!(
            !reason.is_empty() && reason.chars().count() <= REASON_CEILING,
            "case {case:?}: expected a reason within {REASON_CEILING} characters, \
             got {reason:?}"
        );
        assert!(
            report["s3Reports"].is_null(),
            "case {case:?}: a failed generation names no written report, got {report:?}"
        );
    }
}

/// RULE (the report), on the experiment that configured none: there is no report to
/// generate, so the member is absent for the whole of its lifecycle.
#[tokio::test]
async fn an_experiment_that_configured_no_report_carries_no_experiment_report() {
    let router = emulator_with_object_storage();
    // The bucket a configured report WOULD go to exists, so the closing assertion below
    // tests the handler rather than the absence of a bucket.
    let (status, body) = s3(&router, "PUT", &format!("/{BUCKET}")).await;
    assert_eq!(
        status,
        200,
        "expected the report bucket to be created, got {status} with {:?}",
        String::from_utf8_lossy(&body)
    );
    let case = "no-report-configuration";
    let template_id = template_for(&router, case, Ending::Elapses, json!({})).await;
    let started = start_from(&router, case, &template_id).await;
    let experiment = started.member("experiment").clone();
    assert!(
        experiment["experimentReport"].is_null(),
        "case {case:?}: expected no report for an experiment that configured none, \
         got {experiment:?}"
    );
    let id = experiment["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no experiment id in {experiment:?}"))
        .to_string();

    let ended = poll_until_terminal(&router, &id).await;
    assert_eq!(
        ended["state"]["status"].as_str().unwrap_or_default(),
        "completed",
        "case {case:?}: expected the run to complete, got {ended:?}"
    );
    assert!(
        ended["experimentReport"].is_null(),
        "case {case:?}: an ended experiment that configured no report still carries none, \
         got {ended:?}"
    );
    // And nothing was written anywhere: the object key an experiment's report would take is
    // not in object storage.
    let (status, _) = s3(&router, "GET", &format!("/{BUCKET}/{id}/{REPORT_OBJECT_NAME}")).await;
    assert_ne!(
        status, 200,
        "case {case:?}: no report may be written for an experiment that configured none"
    );
}
