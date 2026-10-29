// SPDX-License-Identifier: CC0-1.0

//! Starting a recommendation report generation while one is already in flight, end to end
//! over HTTP.
//!
//! The rule under test is the one the service states for the report generation task it
//! runs one of at a time, not for one pair of calls: a
//! `StartRecommendationReportGeneration` issued while a report generation task of that
//! Region's portfolio has not reached a terminal status COALESCES onto that task — it is
//! answered with HTTP 200 and the id of the task already in flight, mints no second
//! record, and leaves that task exactly as it was — so N overlapping starts all answer the
//! ONE id the caller then polls to its terminal status, and none of them is answered with
//! the modelled `ConflictException`. The window is closed by the task reaching its terminal
//! status and by nothing a caller sends: a start made after that mints a task of its own.
//!
//! The tables therefore span what the rule generalises over: how MANY starts overlap (two,
//! three and five), what parameters each of them carries (every `OutputFormat` value, an
//! absent one, and filters of none, one and two groups), and the generation the burst falls
//! in (a second burst, after the clock has settled the first task, coalesces onto its own
//! task). The answer turns on the STORE, so no row of it may depend on what a request
//! holds — and the task in flight keeps the parameters of the call that minted it, which is
//! read back off the store rather than inferred from the answer.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_migrationhubstrategy_awsmigrationhubstrategyrecommendation::state::{
    ComAmazonawsMigrationhubstrategyAwsmigrationhubstrategyrecommendationState, COMPLETION_SPAN,
};

use crate::import_and_assess::{at, TEST_REGION};
use crate::recommendation_report::{
    emulator_with_engine, group, report_details, start_report, stored_task_count,
    CONFLICT_EXCEPTION, CONFLICT_STATUS, GROUP_NAMES, OUTPUT_FORMATS, REPORT_IN_PROGRESS,
    REPORT_SETTLED,
};

/// The start requests a burst is drawn from, differing along everything the operation lets
/// a caller vary: whether `outputFormat` is named at all and which value it carries, and
/// how many groups the filter names.
///
/// A burst of N takes the first N of these, so the row that minted the task and the rows
/// that coalesce onto it carry DIFFERENT parameters — which is what makes "the answer is
/// decided by the store, not by what the request holds" observable.
fn burst_requests() -> Vec<(&'static str, Value)> {
    vec![
        (
            "the default format named explicitly",
            json!({"outputFormat": OUTPUT_FORMATS[0]}),
        ),
        (
            // The `OutputFormat` entry the issue did not cite, with a filter.
            "the other output format, with one group",
            json!({
                "outputFormat": OUTPUT_FORMATS[1],
                "groupIdFilter": [group(GROUP_NAMES[0], "wave-1")],
            }),
        ),
        ("no members at all", json!({})),
        (
            "two groups, one of each GroupName",
            json!({
                "outputFormat": OUTPUT_FORMATS[1],
                "groupIdFilter": [
                    group(GROUP_NAMES[0], "wave-2"),
                    group(GROUP_NAMES[1], "MPA"),
                ],
            }),
        ),
        (
            "an empty groupIdFilter",
            json!({"outputFormat": OUTPUT_FORMATS[0], "groupIdFilter": []}),
        ),
    ]
}

/// The `outputFormat` the stored task of this Region's portfolio was minted with, read off
/// the same record the handler wrote.
///
/// There is one such task per burst (the rule under test), so this reports the only one.
fn stored_output_format(engine: &Arc<engine::InMemoryEngine>) -> String {
    let state =
        engine.state::<ComAmazonawsMigrationhubstrategyAwsmigrationhubstrategyrecommendationState>();
    let store = state.lock();
    store
        .region(TEST_REGION)
        .recommendation_reports
        .values()
        .map(|report| report.output_format.clone())
        .next()
        .unwrap_or_default()
}

/// What a request's `outputFormat` means for the task it would mint: the value it names,
/// or the format the operation documents as the default when it names none.
fn asked_format(request: &Value) -> String {
    match at(request, "/outputFormat").as_str() {
        Some(named) => {
            assert!(
                OUTPUT_FORMATS.contains(&named),
                "a burst row named an outputFormat OutputFormat does not declare: {named:?}"
            );
            named.to_string()
        }
        // `StartRecommendationReportGenerationRequest$outputFormat`: "The default format
        // is Microsoft Excel", which is `Excel` in `OutputFormat`.
        None => OUTPUT_FORMATS[0].to_string(),
    }
}

/// Of N starts issued inside one task's span every one answers the SAME id: the first
/// mints the task, each later one coalesces onto it with HTTP 200, mints nothing and
/// disturbs nothing — and the task keeps the parameters of the call that minted it, not a
/// later caller's. The one task the burst coalesced onto then reaches its terminal status.
#[tokio::test]
async fn overlapping_starts_coalesce_onto_the_task_in_flight() {
    let mut observed = 0usize;
    for burst in [2usize, 3, 5] {
        let rows = burst_requests();
        assert!(
            burst <= rows.len(),
            "case {burst} overlapping starts: the table holds only {} rows",
            rows.len()
        );
        let (router, engine) = emulator_with_engine();
        let started_at = Instant::now();

        let (first_case, first_request) = rows[0].clone();
        let expected_format = asked_format(&first_request);
        let first = start_report(&router, first_request).await;
        assert_eq!(
            first.status, 200,
            "case {burst} overlapping starts: the first start is accepted, got {} with {:?} \
             ({first_case})",
            first.status, first.body
        );
        let first_id = first.string("/id");

        for (case, request) in rows.into_iter().take(burst).skip(1) {
            let case = format!("{burst} overlapping starts, then {case}");
            if started_at.elapsed() >= COMPLETION_SPAN {
                // The clock has already settled the first task, so this call no longer
                // overlaps it: the row would prove nothing and is skipped rather than
                // asserted wrongly.
                break;
            }
            let again = start_report(&router, request).await;
            assert_eq!(
                again.status, 200,
                "case {case:?}: a start made while a task is in flight is accepted (HTTP 200, \
                 never HTTP {CONFLICT_STATUS}), got {} with {:?}",
                again.status, again.body
            );
            assert_eq!(
                again.error_code(),
                "",
                "case {case:?}: a repeat of the start is not answered with \
                 {CONFLICT_EXCEPTION}, got {:?}",
                again.body
            );
            assert_eq!(
                again.string("/id"),
                first_id,
                "case {case:?}: expected the id of the task already in flight ({first_id:?}), \
                 got {:?}",
                again.body
            );
            assert_eq!(
                stored_task_count(&engine),
                1,
                "case {case:?}: a coalesced start must mint nothing, but the store holds {} tasks",
                stored_task_count(&engine)
            );
            assert_eq!(
                stored_output_format(&engine),
                expected_format,
                "case {case:?}: the task in flight keeps the format the call that minted it asked \
                 for ({expected_format:?}), got {:?}",
                stored_output_format(&engine)
            );
            // And the task is left exactly as it was: still the one the first call minted,
            // still in progress, reported once.
            let running = report_details(&router, &first_id).await;
            assert_eq!(
                (
                    running.status,
                    running.string("/recommendationReportDetails/status")
                ),
                (200, REPORT_IN_PROGRESS.to_string()),
                "case {case:?}: the coalesced start disturbed the task in flight, got {:?}",
                running.body
            );
            observed += 1;
        }

        // Nothing is done here but wait: no call drives the transition.
        std::thread::sleep(COMPLETION_SPAN + Duration::from_millis(200));

        let settled = report_details(&router, &first_id).await;
        assert_eq!(
            settled.string("/recommendationReportDetails/status"),
            REPORT_SETTLED,
            "case {burst} overlapping starts: the id every call answered settles on \
             {REPORT_SETTLED}, got {:?}",
            settled.body
        );
        assert_eq!(
            stored_task_count(&engine),
            1,
            "case {burst} overlapping starts: the burst is one task in all, but the store holds \
             {} tasks",
            stored_task_count(&engine)
        );
    }
    assert!(
        observed > 0,
        "no row observed a coalesced start at all: expected at least one overlapping start to be \
         asserted inside the completion span"
    );
}

/// The window is closed by the task reaching its terminal status and by nothing else: a
/// start issued once the clock has settled the previous task mints a task of its own with a
/// fresh id, and a start issued inside that new task's span coalesces onto it exactly as
/// the first generation's did.
#[tokio::test]
async fn the_coalescing_window_is_closed_by_the_clock_and_then_opens_again() {
    let (router, engine) = emulator_with_engine();
    let mut minted: Vec<String> = Vec::new();

    for (generation, format) in OUTPUT_FORMATS.iter().enumerate() {
        let started_at = Instant::now();
        let started = start_report(&router, json!({"outputFormat": format})).await;
        assert_eq!(
            started.status, 200,
            "generation {generation}: with nothing in flight the start is accepted, got {} with \
             {:?}",
            started.status, started.body
        );
        let id = started.string("/id");
        assert!(
            !minted.contains(&id),
            "generation {generation}: a start made once the earlier task settled mints a task of \
             its own, but it answered {id:?} again ({minted:?})"
        );
        assert_eq!(
            stored_task_count(&engine),
            generation + 1,
            "generation {generation}: each start made with nothing in flight mints one task, but \
             the store holds {}",
            stored_task_count(&engine)
        );

        if started_at.elapsed() < COMPLETION_SPAN {
            let again = start_report(&router, json!({})).await;
            assert_eq!(
                (again.status, again.string("/id")),
                (200, id.clone()),
                "generation {generation}: a start inside the span coalesces onto {id:?} with HTTP \
                 200, got {} with {:?}",
                again.status,
                again.body
            );
            assert_eq!(
                stored_task_count(&engine),
                generation + 1,
                "generation {generation}: the coalesced start minted a task, the store holds {}",
                stored_task_count(&engine)
            );
        }
        minted.push(id.clone());

        // Nothing is done here but wait: no call drives the transition.
        std::thread::sleep(COMPLETION_SPAN + Duration::from_millis(200));

        let settled = report_details(&router, &id).await;
        assert_eq!(
            settled.string("/recommendationReportDetails/status"),
            REPORT_SETTLED,
            "generation {generation}: once the span has passed the task is {REPORT_SETTLED}, got \
             {:?}",
            settled.body
        );
    }

    assert_eq!(
        stored_task_count(&engine),
        OUTPUT_FORMATS.len(),
        "each generation's task is kept, but the store holds {} tasks",
        stored_task_count(&engine)
    );
}
