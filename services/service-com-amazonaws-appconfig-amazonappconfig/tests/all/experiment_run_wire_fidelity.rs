// SPDX-License-Identifier: CC0-1.0

//! The wire form of an experiment-run document: the form a run's instants take on the
//! wire, and the audience share a run always reports.
//!
//! Two rules are under test, each one level more general than the recorded request that
//! exposed it:
//!
//! 1. Every member of a document this service answers about an experiment run that targets
//!    `Iso8601DateTime` (`@timestampFormat("date-time")`) goes onto the restJson1 wire as
//!    an RFC 3339 / ISO-8601 STRING, never as an epoch number, on every path that answers
//!    one — `StartedAt`, `UpdatedAt` and `EndedAt` of `ExperimentRun` (the start, the read,
//!    the update and the stop) and of `ExperimentRunSummary` (the run listing), and
//!    `OccurredAt` of `ExperimentRunEvent` (the timeline listing).
//!    `smithy_query(query="shape", shape="ExperimentRun")`,
//!    `shape="ExperimentRunSummary"` and `shape="ExperimentRunEvent"` report
//!    `"timestampFormat": "date-time"` on each of them.
//! 2. A run always delivers to a known share of the target audience, so
//!    `ExperimentRun$ExposurePercentage` is reported by every operation that answers a run
//!    and by every timeline entry: the share the request named, or zero for a start that
//!    named none — the setting `StartExperimentRun$ExposurePercentage` documents as the one
//!    that "validates the experiment before exposing production users". It is a
//!    `NullablePercentage`, range 0..100, so the zero is a value inside the published range
//!    rather than an absent member.
//!
//! The table spans what those rules generalise over: whether the start names an exposure at
//! all, and where in the published `NullablePercentage` range 0..100 it sits (both bounds, a
//! fraction below one, and values between); whether the start carries a description and
//! treatment overrides; how far an update then raises the exposure; and whether the stop
//! states a result. The recorded request — a start with no exposure at all — is one row of
//! it. Every assertion names its case and prints the actual value.

use aws_smithy_types::DateTime;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::appconfig_workflow::{router, Answer};
use crate::deployment_wire_fidelity::{iso_instant, member};
use crate::experiment_definitions::{scope, Scope};
use crate::list_experiment_run_events::list_events;
use crate::list_experiment_runs::list as list_runs;
use crate::start_experiment_run::{create_test_definition, get_run, id_of, start_run};
use crate::stop_experiment_run::stop_run;
use crate::update_experiment_run::update_run;

/// The share of the audience a run exposes when the start of it named none: the zero
/// `StartExperimentRun$ExposurePercentage` documents, which AppConfig materializes on the
/// run rather than leaving the member out.
const UNEXPOSED: f64 = 0.0;

/// One row: what a failing assertion calls it, the body the run is started from, the
/// exposure the started run must then report, the exposure an update raises it to (never
/// below the stored one, which the operation refuses), and the body the stop carries.
struct Row {
    case: &'static str,
    start: Value,
    exposure: f64,
    raised_to: f64,
    stop: Value,
}

/// The rows. `NullablePercentage` is `range 0..100`
/// (`smithy_query(query="operation", operation="StartExperimentRun")`), so the exposures
/// below are both published bounds, a fraction below the lower one's neighbour, and values
/// between — each paired with an update that raises it, because audience exposure can only
/// be increased.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a start that names no exposure at all",
            start: json!({}),
            exposure: UNEXPOSED,
            raised_to: 0.0,
            stop: json!({}),
        },
        Row {
            case: "a start that names no exposure, then raised to the upper range bound",
            start: json!({ "Description": "validated before exposing anyone" }),
            exposure: UNEXPOSED,
            raised_to: 100.0,
            stop: json!({
                "Result": {
                    "ExecutiveSummary": "the treatment won",
                    "ReasonsToLaunch": "conversion rose",
                },
            }),
        },
        Row {
            case: "an exposure written AT the lower range bound",
            start: json!({ "ExposurePercentage": 0.0 }),
            exposure: 0.0,
            raised_to: 0.5,
            stop: json!({}),
        },
        Row {
            case: "a fractional exposure",
            start: json!({
                "ExposurePercentage": 0.5,
                "TreatmentOverrides": { "Inline": { "entity-1": "t1", "entity-2": "c" } },
            }),
            exposure: 0.5,
            raised_to: 12.5,
            stop: json!({ "Result": { "ReasonsNotToLaunch": "the lift was inside the noise" } }),
        },
        Row {
            case: "an exposure between the bounds, with a description and overrides",
            start: json!({
                "Description": "half the beta tier",
                "ExposurePercentage": 50.0,
                "TreatmentOverrides": { "Inline": { "entity-7": "c" } },
            }),
            exposure: 50.0,
            raised_to: 50.0,
            stop: json!({ "Result": { "ExecutiveSummary": "inconclusive" } }),
        },
        Row {
            case: "an exposure AT the upper range bound",
            start: json!({ "ExposurePercentage": 100.0 }),
            exposure: 100.0,
            raised_to: 100.0,
            stop: json!({}),
        },
    ]
}

/// The exposure a document reports, asserted to be a number inside the published
/// `NullablePercentage` range rather than an absent member.
fn exposure_of(case: &str, document: &str, body: &Value) -> f64 {
    let reported = member(body, "ExposurePercentage");
    let share = reported.as_f64().unwrap_or_else(|| {
        panic!(
            "case {case:?}: {document} must report the share of the audience the run exposes, \
             got {reported}"
        )
    });
    assert!(
        (0.0..=100.0).contains(&share),
        "case {case:?}: {document}'s ExposurePercentage must sit inside the modelled \
         NullablePercentage range 0..100, got {share}"
    );
    share
}

/// Assert that `document` reports `expected` as the audience share.
fn assert_exposure(case: &str, document: &str, body: &Value, expected: f64) {
    let share = exposure_of(case, document, body);
    assert!(
        (share - expected).abs() < 1e-6,
        "case {case:?}: expected {document} to report ExposurePercentage {expected}, got {share}"
    );
}

/// The instants ONE experiment-run document reports, each asserted to be on the wire as the
/// modelled `date-time` STRING: the run started and was last written, and ended only once
/// something ended it.
///
/// `names` are the members this document carries, so the same assertions cover the run
/// shape and the summary shape.
fn assert_run_instants(
    case: &str,
    document: &str,
    body: &Value,
    ended: bool,
) -> (DateTime, DateTime, Option<DateTime>) {
    let started_at = iso_instant(case, document, "StartedAt", &member(body, "StartedAt"));
    let updated_at = iso_instant(case, document, "UpdatedAt", &member(body, "UpdatedAt"));
    assert!(
        updated_at.as_secs_f64() >= started_at.as_secs_f64(),
        "case {case:?}: {document} cannot have been written before it started, got \
         {updated_at:?} before {started_at:?}"
    );
    let ended_at = if ended {
        let ended_at = iso_instant(case, document, "EndedAt", &member(body, "EndedAt"));
        assert!(
            ended_at.as_secs_f64() >= started_at.as_secs_f64(),
            "case {case:?}: {document} cannot have ended before it started, got {ended_at:?} \
             before {started_at:?}"
        );
        Some(ended_at)
    } else {
        assert_eq!(
            body.get("EndedAt"),
            None,
            "case {case:?}: {document} describes a run nothing has ended, so it reports no \
             EndedAt, got {:?}",
            body.get("EndedAt")
        );
        None
    };
    (started_at, updated_at, ended_at)
}

/// Every entry of a run's timeline, asserted to report its instant as the modelled
/// `date-time` STRING and the audience share in force at it.
async fn assert_timeline(
    router: &MultiServiceRouter,
    case: &str,
    application: &str,
    definition: &str,
    expected_exposure: f64,
) {
    let answer = list_events(router, application, definition, 1, "").await;
    assert_eq!(
        answer.status,
        200,
        "case {case:?}: ListExperimentRunEvents must answer 200, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    let body = answer.json();
    let entries = member(&body, "Items");
    let entries = entries
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: the timeline must carry Items, got {body}"));
    assert!(
        !entries.is_empty(),
        "case {case:?}: a run that has been acted on has a timeline, got {body}"
    );
    for (index, entry) in entries.iter().enumerate() {
        let document = format!("ListExperimentRunEvents' Items[{index}]");
        iso_instant(case, &document, "OccurredAt", &member(entry, "OccurredAt"));
        exposure_of(case, &document, entry);
    }
    // The newest entry is first (`EventLog`'s own ordering rule, which this listing
    // follows), so the head of the feed reports the share in force after the last write.
    assert_exposure(
        case,
        "the newest timeline entry",
        &entries[0],
        expected_exposure,
    );
}

/// The summary the run listing reports for run 1 of `definition`.
async fn summary_of(
    router: &MultiServiceRouter,
    case: &str,
    application: &str,
    definition: &str,
) -> Value {
    let answer = list_runs(router, application, definition, &[]).await;
    assert_eq!(
        answer.status,
        200,
        "case {case:?}: ListExperimentRuns must answer 200, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    let body = answer.json();
    let items = member(&body, "Items");
    let items = items
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: the listing must carry Items, got {body}"));
    items
        .into_iter()
        .find(|summary| member(summary, "Run").as_i64() == Some(1))
        .unwrap_or_else(|| panic!("case {case:?}: the listing must report run 1, got {body}"))
}

/// Assert the status a document reports for the run, and answer the body.
fn document(case: &str, document: &str, answer: &Answer, expected_status: u16) -> Value {
    assert_eq!(
        answer.status,
        expected_status,
        "case {case:?}: {document} must answer {expected_status}, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    answer.json()
}

/// Every path that answers an experiment run reports its instants as ISO-8601 strings and
/// the share of the audience it exposes: the start, the read, the update, the stop, the run
/// listing's summary and the timeline's entries.
#[tokio::test]
async fn an_experiment_run_reports_iso_8601_instants_and_its_audience_share() {
    let router = router();
    let scope: Scope = scope(&router, "run-wire").await;

    for (index, row) in rows().into_iter().enumerate() {
        let case = row.case;
        // One definition per row: a definition delivers one experiment at a time.
        let definition = create_test_definition(&router, &scope, &format!("run-wire-{index}")).await;
        let definition_id = id_of(case, &definition);

        // The start.
        let started = document(
            case,
            "StartExperimentRun",
            &start_run(&router, &scope.application, &definition_id, &row.start).await,
            201,
        );
        let (started_at, updated_at, _) =
            assert_run_instants(case, "StartExperimentRun", &started, false);
        assert_eq!(
            updated_at.as_secs_f64(),
            started_at.as_secs_f64(),
            "case {case:?}: nothing has written the run since the start, so its UpdatedAt is \
             its StartedAt, got {updated_at:?} and {started_at:?}"
        );
        assert_exposure(case, "StartExperimentRun", &started, row.exposure);

        // The read answers the stored run, so it reports the same instants in the same wire
        // form and the same share.
        let read = document(
            case,
            "GetExperimentRun",
            &get_run(&router, &scope.application, &definition_id, 1).await,
            200,
        );
        assert_run_instants(case, "GetExperimentRun", &read, false);
        assert_eq!(
            read, started,
            "case {case:?}: GetExperimentRun must report the run the start wrote {started}, \
             got {read}"
        );

        // The listing reports the same run as a summary, whose instants are the run's own
        // and in the same wire form.
        let summary = summary_of(&router, case, &scope.application, &definition_id).await;
        assert_run_instants(case, "ListExperimentRuns' summary", &summary, false);
        for name in ["StartedAt", "UpdatedAt"] {
            assert_eq!(
                member(&summary, name),
                member(&started, name),
                "case {case:?}: a summary reports the run's own {name}, got {} for {}",
                member(&summary, name),
                member(&started, name)
            );
        }

        // The timeline reports the start of the run, at the instant it started and under the
        // share the start wrote.
        assert_timeline(
            &router,
            case,
            &scope.application,
            &definition_id,
            row.exposure,
        )
        .await;

        // The update writes the record: a raised exposure is reported, and the instants stay
        // strings.
        let updated = document(
            case,
            "UpdateExperimentRun",
            &update_run(
                &router,
                &scope.application,
                &definition_id,
                1,
                &json!({ "ExposurePercentage": row.raised_to }),
            )
            .await,
            200,
        );
        assert_run_instants(case, "UpdateExperimentRun", &updated, false);
        assert_exposure(case, "UpdateExperimentRun", &updated, row.raised_to);
        assert_timeline(
            &router,
            case,
            &scope.application,
            &definition_id,
            row.raised_to,
        )
        .await;

        // The stop ends the run, so the fourth instant of the shape is reported — as a
        // string too — and the share the run was delivering is kept.
        let stopped = document(
            case,
            "StopExperimentRun",
            &stop_run(&router, &scope.application, &definition_id, 1, &row.stop).await,
            200,
        );
        let (_, stopped_updated_at, ended_at) =
            assert_run_instants(case, "StopExperimentRun", &stopped, true);
        let ended_at = ended_at.expect("a stopped run reports the instant it ended");
        assert_eq!(
            stopped_updated_at.as_secs_f64(),
            ended_at.as_secs_f64(),
            "case {case:?}: the stop wrote the record as it ended it, expected UpdatedAt \
             {ended_at:?}, got {stopped_updated_at:?}"
        );
        assert_exposure(case, "StopExperimentRun", &stopped, row.raised_to);

        // And every path that reports the finished run reports that fourth instant the same
        // way.
        let read_back = document(
            case,
            "GetExperimentRun after the stop",
            &get_run(&router, &scope.application, &definition_id, 1).await,
            200,
        );
        assert_run_instants(case, "GetExperimentRun after the stop", &read_back, true);
        assert_eq!(
            read_back, stopped,
            "case {case:?}: GetExperimentRun must report the stopped run {stopped}, got \
             {read_back}"
        );
        let ended_summary = summary_of(&router, case, &scope.application, &definition_id).await;
        assert_run_instants(
            case,
            "ListExperimentRuns' summary after the stop",
            &ended_summary,
            true,
        );
        assert_eq!(
            member(&ended_summary, "EndedAt"),
            member(&stopped, "EndedAt"),
            "case {case:?}: a summary reports the run's own EndedAt, got {} for {}",
            member(&ended_summary, "EndedAt"),
            member(&stopped, "EndedAt")
        );
        assert_timeline(
            &router,
            case,
            &scope.application,
            &definition_id,
            row.raised_to,
        )
        .await;
    }
}
