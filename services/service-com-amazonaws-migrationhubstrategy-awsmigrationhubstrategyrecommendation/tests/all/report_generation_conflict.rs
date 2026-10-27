// SPDX-License-Identifier: CC0-1.0

//! Starting a recommendation report generation while one is already running, end to end
//! over HTTP.
//!
//! The rule under test is the one the service states for a task it runs at most one of at
//! a time, not for one pair of calls: a `StartRecommendationReportGeneration` issued while
//! a report generation task of that Region's portfolio has not reached a terminal status
//! is REFUSED with the modelled `ConflictException` (HTTP 409) naming the task already
//! running — it mints nothing, answers no id, and leaves the running task exactly as it
//! was — so of N overlapping starts exactly ONE succeeds and the others are told their own
//! request was not acted on. The conflict is lifted by the task reaching its terminal
//! status, not by anything a caller sends.
//!
//! The tables therefore span what the rule generalises over: how MANY starts overlap (two,
//! three and five), what parameters each of them carries (every `OutputFormat` value, an
//! absent one, and filters of none, one and two groups), and the generation the burst falls
//! in (a second burst after the clock has settled the first task is refused the same way).
//! The refusal turns on the STORE, so no row of it may depend on what a request holds — and
//! the task that DID get minted keeps the parameters of the call that minted it, which is
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
/// A burst of N takes the first N of these, so the row that succeeds and the rows that are
/// refused carry DIFFERENT parameters — which is what makes "the second caller cannot tell
/// its request was never acted on" observable.
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

/// Of N starts issued inside one task's span exactly one succeeds: the first. Every later
/// one is refused with 409 `ConflictException` naming the task that first call minted,
/// mints nothing, and carries no id of its own — and the stored task keeps the parameters
/// of the call that minted it, not a later caller's.
#[tokio::test]
async fn exactly_one_of_several_overlapping_starts_succeeds() {
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
            let refused = start_report(&router, request).await;
            assert_eq!(
                refused.status, CONFLICT_STATUS,
                "case {case:?}: expected HTTP {CONFLICT_STATUS}, got {} with {:?}",
                refused.status, refused.body
            );
            assert_eq!(
                refused.error_code(),
                CONFLICT_EXCEPTION,
                "case {case:?}: expected {CONFLICT_EXCEPTION}, got {:?}",
                refused.body
            );
            let message = refused.string("/message");
            assert!(
                message.contains(&first_id) && message.contains(REPORT_IN_PROGRESS),
                "case {case:?}: the refusal must name the running task {first_id:?} and the \
                 {REPORT_IN_PROGRESS} status it holds, got {message:?}"
            );
            assert_eq!(
                at(&refused.body, "/id"),
                Value::Null,
                "case {case:?}: a refused start answers no task id, got {:?}",
                refused.body
            );
            assert_eq!(
                stored_task_count(&engine),
                1,
                "case {case:?}: a refused start must mint nothing, but the store holds {} tasks",
                stored_task_count(&engine)
            );
            assert_eq!(
                stored_output_format(&engine),
                expected_format,
                "case {case:?}: the running task keeps the format the call that minted it asked \
                 for ({expected_format:?}), got {:?}",
                stored_output_format(&engine)
            );
            observed += 1;
        }
    }
    assert!(
        observed > 0,
        "no row observed the conflict at all: expected at least one overlapping start to be \
         asserted inside the completion span"
    );
}

/// The conflict is lifted by the task reaching its terminal status and by nothing else: a
/// burst issued once the clock has settled the first task mints one new task and refuses
/// the rest, exactly as the first burst was treated.
#[tokio::test]
async fn the_conflict_is_lifted_by_the_clock_and_then_holds_again() {
    let (router, engine) = emulator_with_engine();

    for (generation, format) in OUTPUT_FORMATS.iter().enumerate() {
        let started_at = Instant::now();
        let started = start_report(&router, json!({"outputFormat": format})).await;
        assert_eq!(
            started.status, 200,
            "generation {generation}: with nothing running the start is accepted, got {} with \
             {:?}",
            started.status, started.body
        );
        let id = started.string("/id");
        assert_eq!(
            stored_task_count(&engine),
            generation + 1,
            "generation {generation}: each accepted start mints one task, but the store holds {}",
            stored_task_count(&engine)
        );

        if started_at.elapsed() < COMPLETION_SPAN {
            let refused = start_report(&router, json!({})).await;
            assert_eq!(
                (refused.status, refused.error_code()),
                (CONFLICT_STATUS, CONFLICT_EXCEPTION),
                "generation {generation}: a start inside the span is refused with \
                 {CONFLICT_STATUS} {CONFLICT_EXCEPTION}, got {} with {:?}",
                refused.status,
                refused.body
            );
            assert_eq!(
                stored_task_count(&engine),
                generation + 1,
                "generation {generation}: the refused start minted a task, the store holds {}",
                stored_task_count(&engine)
            );
        }

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
