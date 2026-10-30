// SPDX-License-Identifier: CC0-1.0

//! `UpdateSafetyLeverState` under the rule its sibling suite
//! [`crate::report_follows_whoever_ends_the_run`] states: an operation that materialises the
//! END of an experiment's run also performs the report generation that end makes due, writing
//! the report to the Amazon S3 output the experiment's own configuration names.
//!
//! The lever gets a suite of its own because it is the one call that settles EVERY experiment
//! the store holds, so one engage can materialise several clock-owed ends at once — and the
//! report of each of those runs is owed by that same call. A client that engages the lever as
//! the "stop everything" control therefore never has to call another operation for the report
//! of an ended experiment to exist, and two callers at one instant cannot disagree about
//! whether it does.
//!
//! The rows differ along what the rule generalises over: how long the run lasted, the form of
//! the configured output prefix (none, a plain one, a nested one with a trailing slash) and
//! whether a report was configured at all. The lever is engaged ONCE for all of them, so the
//! row count also varies how many ends a single call materialises.
//!
//! Each case calls only the lever (and Amazon S3, to see whether the object is there): no
//! `GetExperiment`, no `ListExperiments` and no `StopExperiment` runs between the start of a
//! run and the appearance of its report, so the report a case finds can only have been
//! written by the lever.
//!
//! The second test is the rule from the other side: the lever writes nothing for an experiment
//! whose run has NOT ended, and writes the report once a later call of the same operation
//! materialises the end the lever itself set in motion.
//!
//! The lever request, the report configuration and the post-read generation assertions come
//! from the sibling suite rather than being copied here.

use std::time::Duration;

use serde_json::{json, Value};

use crate::experiment_lifecycle::get_experiment;
use crate::experiment_state_reason_and_report::{
    emulator_with_object_storage, given_report_bucket, start_from, template_for, Ending, BUCKET,
};
use crate::report_follows_whoever_ends_the_run::{
    assert_engaged, assert_generation_names_object, engage_lever, report_configuration,
    report_object,
};

/// How long the rows' runs are left to elapse with NOTHING called.
///
/// The emulator compresses a run against the `duration` its actions name and floors a short
/// one, so the longest row ([`LONGEST_DURATION`]) is over well inside this span, with slack
/// for the transition into `running` and for a loaded machine. Nothing may be called while it
/// passes — that is what these cases establish — so it is a sleep and not a poll.
const RUN_ELAPSES_AFTER: Duration = Duration::from_secs(13);

/// The longest `duration` a row's `aws:fis:wait` names, inside the 1 to 720 minutes FIS
/// documents for that action's `duration`. See [`RUN_ELAPSES_AFTER`].
const LONGEST_DURATION: &str = "PT10M";

/// How long after the lever asked an experiment to stop that stop must have been
/// materialised: past the emulator's own non-zero transition span.
const PAST_STOP_TRANSITION: Duration = Duration::from_millis(900);

/// Where a row's report must land, and what its template configured for it.
enum Reporting {
    /// An `experimentReportConfiguration` naming an S3 output with this `prefix` (`None` for
    /// an output that names none), whose report must be written under `key`, with `{id}`
    /// standing for the experiment id.
    Configured {
        prefix: Option<&'static str>,
        key: &'static str,
    },
    /// No `experimentReportConfiguration` at all: this experiment owes no report, so the end
    /// of its run makes no generation due and nothing is written for it.
    NotConfigured,
}

impl Reporting {
    /// The key the report of experiment `id` is looked for under. For a row that configured
    /// none, this is where a report WOULD have landed had one been owed, so the case can
    /// assert that nothing was written.
    fn probe_key(&self, id: &str) -> String {
        match self {
            Self::Configured { key, .. } => key.replace("{id}", id),
            Self::NotConfigured => format!("{id}/experiment-report.json"),
        }
    }
}

/// One row: the run its template describes and the report that run's end makes due.
struct Row {
    case: &'static str,
    /// The `duration` the row's one `aws:fis:wait` action names — what the length of the run
    /// is computed from, so the rows' runs differ in length.
    duration: &'static str,
    reporting: Reporting,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "output-names-no-prefix",
            duration: "PT1M",
            reporting: Reporting::Configured {
                prefix: None,
                key: "{id}/experiment-report.json",
            },
        },
        Row {
            case: "plain-prefix",
            duration: "PT6M",
            reporting: Reporting::Configured {
                prefix: Some("swept"),
                key: "swept/{id}/experiment-report.json",
            },
        },
        Row {
            case: "nested-prefix-with-a-trailing-slash",
            duration: LONGEST_DURATION,
            reporting: Reporting::Configured {
                prefix: Some("swept/nested/2026/"),
                key: "swept/nested/2026/{id}/experiment-report.json",
            },
        },
        Row {
            case: "no-report-configured",
            duration: "PT1M",
            reporting: Reporting::NotConfigured,
        },
    ]
}

/// The `actions` a row's template carries: one wait of that row's own duration.
fn wait_actions(duration: &str) -> Value {
    json!({ "wait": { "actionId": "aws:fis:wait", "parameters": { "duration": duration } } })
}

/// The template body a row needs: its own wait, and its report configuration when it has one.
fn template_body(row: &Row) -> Value {
    let mut extra = json!({ "actions": wait_actions(row.duration) });
    if let Reporting::Configured { prefix, .. } = row.reporting {
        extra["experimentReportConfiguration"] = report_configuration(prefix, BUCKET);
    }
    extra
}

/// RULE: engaging the safety lever materialises the end of every run that had already
/// elapsed, so the same call writes the report each of those ends makes due.
///
/// The runs elapse with NOTHING called — no read of an experiment, no listing, no stop — and
/// then the lever is engaged ONCE for all of the rows, so a report a row finds can only have
/// been written by that single call.
#[tokio::test]
async fn engaging_the_lever_writes_the_report_of_every_run_that_had_elapsed() {
    let router = emulator_with_object_storage();
    given_report_bucket(&router).await;

    // Every row's experiment is started first, so their runs overlap and ONE lever call ends
    // up materialising all of their ends. The account's active-experiment quota is five,
    // which these rows stay inside.
    let mut started = Vec::new();
    for row in rows() {
        let template_id =
            template_for(&router, row.case, Ending::Elapses, template_body(&row)).await;
        let experiment = start_from(&router, row.case, &template_id)
            .await
            .member("experiment")
            .clone();
        let id = experiment["id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {:?}: no experiment id in {experiment:?}", row.case))
            .to_string();
        let report = experiment["experimentReport"].clone();
        match row.reporting {
            Reporting::Configured { .. } => assert_eq!(
                report["state"]["status"].as_str().unwrap_or_default(),
                "pending",
                "case {:?}: a configured report is pending until its run ends, got {report:?}",
                row.case
            ),
            Reporting::NotConfigured => assert!(
                report.is_null(),
                "case {:?}: an experiment that configured no report carries none, got {report:?}",
                row.case
            ),
        }
        started.push((row, id));
    }

    // The runs elapse with nothing called at all, so no operation has materialised the end of
    // any of them and no report of any of them exists yet.
    tokio::time::sleep(RUN_ELAPSES_AFTER).await;
    for (row, id) in &started {
        let key = row.reporting.probe_key(id);
        let found = report_object(&router, row.case, &key).await;
        assert!(
            found.is_none(),
            "case {:?}: expected no report at s3://{BUCKET}/{key} while no operation had \
             materialised the end of {id}, got {found:?}",
            row.case
        );
    }

    // ONE call, and the only FIS call since the starts.
    assert_engaged(
        "engaging-the-lever",
        &engage_lever(&router, "stop it all").await,
        "engaging",
    );

    for (row, id) in &started {
        let key = row.reporting.probe_key(id);
        let document = report_object(&router, row.case, &key).await;
        match row.reporting {
            Reporting::Configured { .. } => {
                let document = document.unwrap_or_else(|| {
                    panic!(
                        "case {:?}: expected engaging the lever to write the report of the \
                         ended experiment {id} to s3://{BUCKET}/{key}, found no object",
                        row.case
                    )
                });
                assert_eq!(
                    document["experiment"]["id"].as_str().unwrap_or_default(),
                    id.as_str(),
                    "case {:?}: expected the report to be about {id}, got {document:?}",
                    row.case
                );
                // The end the lever materialised is the one the CLOCK owed — the run elapsed
                // before the lever was touched — and not a stop the lever forced.
                assert_eq!(
                    document["experiment"]["state"]["status"]
                        .as_str()
                        .unwrap_or_default(),
                    "completed",
                    "case {:?}: expected the report to state the status the elapsed run ended \
                     in, got {:?}",
                    row.case,
                    document["experiment"]["state"]
                );
            }
            Reporting::NotConfigured => assert!(
                document.is_none(),
                "case {:?}: an experiment that configured no report owes nothing, so the lever \
                 writes nothing for it, got {document:?}",
                row.case
            ),
        }
    }

    // And each experiment itself now reports the generation the lever already performed,
    // naming the one object it wrote — so this read has nothing left to write.
    for (row, id) in &started {
        let key = row.reporting.probe_key(id);
        let read = get_experiment(&router, id).await.member("experiment").clone();
        assert_eq!(
            read["state"]["status"].as_str().unwrap_or_default(),
            "completed",
            "case {:?}: expected the elapsed run to have ended completed, got {read:?}",
            row.case
        );
        match row.reporting {
            Reporting::Configured { .. } => {
                assert_generation_names_object(row.case, &read["experimentReport"], &key)
            }
            Reporting::NotConfigured => assert!(
                read["experimentReport"].is_null(),
                "case {:?}: an experiment that configured no report carries none however its \
                 run ended, got {read:?}",
                row.case
            ),
        }
    }
}

/// RULE, from the other side of the same sweep: the generation follows the END of the run, so
/// the lever writes nothing for an experiment that has not ended — and writes the report once
/// the end it set in motion is itself materialised.
#[tokio::test]
async fn the_lever_writes_the_report_of_the_stop_it_caused_once_that_stop_has_landed() {
    let case = "lever-stops-a-run-under-way";
    let router = emulator_with_object_storage();
    given_report_bucket(&router).await;

    let template_id = template_for(
        &router,
        case,
        Ending::Elapses,
        json!({
            "actions": wait_actions(LONGEST_DURATION),
            "experimentReportConfiguration": report_configuration(Some("stops"), BUCKET)
        }),
    )
    .await;
    let id = start_from(&router, case, &template_id)
        .await
        .member("experiment")["id"]
        .as_str()
        .expect("a started experiment names its id")
        .to_string();
    let key = format!("stops/{id}/experiment-report.json");

    // The lever is engaged while the run is still under way: the stop it asks for is on its
    // way, so the run has NOT ended and no report is owed yet.
    assert_engaged(
        case,
        &engage_lever(&router, "stop the run under way").await,
        "engaging",
    );
    let found = report_object(&router, case, &key).await;
    assert!(
        found.is_none(),
        "case {case:?}: a run that has not ended owes no report, got {found:?} at \
         s3://{BUCKET}/{key}"
    );

    // Once the stop has fallen due, the next call of the same operation materialises that
    // end — and with it the report that end makes due. The lever's own engage has taken
    // effect by now, which is a span of its own and not the experiment's.
    tokio::time::sleep(PAST_STOP_TRANSITION).await;
    assert_engaged(
        case,
        &engage_lever(&router, "stop everything again").await,
        "engaged",
    );
    let document = report_object(&router, case, &key).await.unwrap_or_else(|| {
        panic!(
            "case {case:?}: expected the lever to write the report of {id} once the stop it \
             caused had landed, found no object at s3://{BUCKET}/{key}"
        )
    });
    assert_eq!(
        document["experiment"]["id"].as_str().unwrap_or_default(),
        id.as_str(),
        "case {case:?}: expected the report to be about {id}, got {document:?}"
    );
    assert_eq!(
        document["experiment"]["state"]["status"]
            .as_str()
            .unwrap_or_default(),
        "stopped",
        "case {case:?}: expected the report to state the status the stop ended the run in, \
         got {:?}",
        document["experiment"]["state"]
    );

    let report = get_experiment(&router, &id).await.member("experiment")["experimentReport"].clone();
    assert_generation_names_object(case, &report, &key);
}
