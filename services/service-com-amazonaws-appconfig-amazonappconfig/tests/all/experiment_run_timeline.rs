// SPDX-License-Identifier: CC0-1.0

//! What an experiment run's TIMELINE records, the order it is served in, and the refusal
//! class a run whose lifecycle has ended answers with — `UpdateExperimentRun`,
//! `ListExperimentRunEvents` and the reads that prove what was written, end to end over
//! HTTP.
//!
//! Three rules are under test, each stated one level above the single request that
//! exposed it:
//!
//! 1. every call that MOVES one of the states a run's timeline reports appends one entry
//!    of that move's own `ExperimentRunEventType`, occurring at the write and carrying the
//!    exposure and the overrides in force once the write had been applied; a call that
//!    moves none of them — a member omitted, or one restating the value the run already
//!    holds — appends nothing, and a description has no event type to be recorded under.
//! 2. the timeline is served MOST RECENT FIRST: `Items[0]` is the newest entry, the oldest
//!    is last, `OccurredAt` never rises down a page, and the first page of a walk under
//!    `MaxResults` carries the NEWEST entries — so paging through reads the whole feed in
//!    that order, each entry exactly once.
//! 3. an operation whose domain is a run still in flight refuses one that has reached the
//!    status its lifecycle ends in with HTTP 400 `BadRequestException` — the class for
//!    input no later request can make acceptable, not the 409 `ConflictException` a state
//!    that can still clear answers with — stating AWS's request-derived template, and
//!    writes nothing: neither the run nor its timeline moves, while the same patch against
//!    a run that IS in flight is accepted and persisted.
//! 4. an entry's place on the timeline is its own, so a walk resumes after the page it
//!    served whatever the run records meanwhile, and the feed a run acted on in a loop
//!    reports keeps the most recent entries this emulator retains rather than growing
//!    without limit.
//!
//! The tables span what those rules generalise over rather than one recorded request:
//! WHICH members an update carries and their values at the published bounds
//! (`Description` len 0..1024, `NullablePercentage` range 0..100, the inline
//! `TreatmentOverrides` map, from
//! `smithy_query(query="operation", operation="UpdateExperimentRun")`), whether the run
//! already holds the state being patched and whether the patch moves it at all, HOW MANY
//! entries the timeline holds and which actions put them there, WHICH page size the walk
//! asks for (both published bounds of `MaxResults`, range 1..50, and values between them),
//! WHICH run number the refused update addresses, and whether the resources are addressed
//! by minted id or by name (both are `Identifier` members, documented as the resource's
//! "ID or name").

use serde_json::{json, Value};
use service_com_amazonaws_appconfig_amazonappconfig::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::appconfig_workflow::{router, Answer};
use crate::experiment_definitions::scope;
use crate::list_experiment_run_events::{list_events, EVENT_TYPES};
use crate::start_experiment_run::{
    assert_refused, assert_running, create_test_definition, get_run, id_of, start_run, string_map,
};
use crate::stop_experiment_run::stop_run;
use crate::update_experiment_definition::instant;
use crate::update_experiment_run::update_run;

/// The published bounds of `MaxResults` on `ListExperimentRunEvents`, range 1..50
/// (`smithy_query(query="operation", operation="ListExperimentRunEvents")`).
const MIN_MAX_RESULTS: i64 = 1;
/// See [`MIN_MAX_RESULTS`].
const MAX_MAX_RESULTS: i64 = 50;

/// The `EventType` of one entry, checked against the values the model's enum accepts.
fn type_of(case: &str, entry: &Value) -> String {
    let reported = entry
        .get("EventType")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: an entry reported no EventType: {entry}"));
    assert!(
        EVENT_TYPES.contains(&reported),
        "case {case:?}: expected one of {EVENT_TYPES:?}, got {reported:?}"
    );
    reported.to_string()
}

/// The `EventType` of every entry a listing answered, in the order it answered them.
fn types_of(case: &str, entries: &[Value]) -> Vec<String> {
    entries.iter().map(|entry| type_of(case, entry)).collect()
}

/// The entries a listing's document reports (one that reports no `Items` at all reads as
/// empty: an answer about a run whose timeline holds no entry).
fn items(case: &str, page: &Value) -> Vec<Value> {
    match page.get("Items") {
        Some(Value::Array(entries)) => entries.clone(),
        None => Vec::new(),
        other => panic!("case {case:?}: expected Items to be a list, got {other:?}"),
    }
}

/// A run's whole timeline, as the listing serves it when nothing bounds the page.
async fn timeline(
    case: &str,
    router: &MultiServiceRouter,
    application: &str,
    definition: &str,
    run: i64,
) -> Vec<Value> {
    let answer = list_events(router, application, definition, run, "").await;
    items(case, &accepted(case, &answer))
}

/// Every entry of a run's timeline, read `size` at a time so a feed longer than one page
/// is reported whole, in the order the pages reported it.
async fn walk_timeline(
    case: &str,
    router: &MultiServiceRouter,
    application: &str,
    definition: &str,
    run: i64,
    size: i64,
) -> Vec<Value> {
    let mut walked: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    // A bound on the walk itself, so a token that never settles cannot spin here: far more
    // pages than the timeline this service retains can fill.
    for _ in 0..=state::MAX_RETAINED_EXPERIMENT_RUN_EVENTS {
        let query = match &token {
            Some(next) => format!("max_results={size}&next_token={next}"),
            None => format!("max_results={size}"),
        };
        let answer = list_events(router, application, definition, run, &query).await;
        let page = accepted(case, &answer);
        walked.extend(items(case, &page));
        token = page
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if token.is_none() {
            break;
        }
    }
    assert_eq!(
        token, None,
        "case {case:?}: the walk must end, got a token left over: {token:?}"
    );
    walked
}

/// Assert a 200 answer and hand back its document.
fn accepted(case: &str, answer: &Answer) -> Value {
    assert_eq!(
        answer.status,
        200,
        "case {case:?}: expected 200, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    answer.json()
}

/// Assert that `entries` are ordered newest first: no entry occurred before the one after
/// it.
fn assert_descending(case: &str, entries: &[Value]) {
    let occurred: Vec<f64> = entries
        .iter()
        .map(|entry| instant(case, "OccurredAt", entry.get("OccurredAt")))
        .collect();
    for pair in occurred.windows(2) {
        assert!(
            pair[0] >= pair[1],
            "case {case:?}: expected OccurredAt to descend down the feed, got {occurred:?}"
        );
    }
}

/// Rule 1: an update appends one entry per state it moved, and nothing for a state it left
/// where it was.
#[tokio::test]
async fn an_update_records_one_entry_for_each_state_it_moved() {
    let router = router();
    let scope = scope(&router, "moved").await;
    let one_override = json!({ "Inline": string_map([("entity-1", "T1")]) });

    // Rows differ along what the run already holds, which members the patch carries and
    // whether each of them MOVES the state it addresses.
    let rows: Vec<(&str, Value, Value, Vec<&str>)> = vec![
        (
            "an exposure raised",
            json!({ "ExposurePercentage": 5.0 }),
            json!({ "ExposurePercentage": 25.0 }),
            vec!["EXPOSURE_UPDATED"],
        ),
        (
            "an exposure raised from the lower published bound to the upper one",
            json!({ "ExposurePercentage": 0.0 }),
            json!({ "ExposurePercentage": 100.0 }),
            vec!["EXPOSURE_UPDATED"],
        ),
        (
            "an exposure a run started without one is given",
            json!({}),
            json!({ "ExposurePercentage": 12.5 }),
            vec!["EXPOSURE_UPDATED"],
        ),
        (
            "an exposure restating the one in force",
            json!({ "ExposurePercentage": 40.0 }),
            json!({ "ExposurePercentage": 40.0 }),
            vec![],
        ),
        (
            "a description, which no event type records",
            json!({ "ExposurePercentage": 10.0, "Description": "before" }),
            json!({ "Description": "after" }),
            vec![],
        ),
        (
            "a description at the upper length bound of nothing else",
            json!({ "ExposurePercentage": 10.0 }),
            json!({ "Description": "x".repeat(1024) }),
            vec![],
        ),
        (
            "a patch carrying nothing at all",
            json!({ "ExposurePercentage": 10.0 }),
            json!({}),
            vec![],
        ),
        (
            "overrides a run that held none is given",
            json!({}),
            json!({ "TreatmentOverrides": one_override.clone() }),
            vec!["OVERRIDES_UPDATED"],
        ),
        (
            "overrides replaced by a different assignment",
            json!({ "TreatmentOverrides": one_override.clone() }),
            json!({ "TreatmentOverrides": { "Inline": string_map([
                ("entity-1", "C"),
                ("entity-2", "T1"),
            ]) } }),
            vec!["OVERRIDES_UPDATED"],
        ),
        (
            "overrides restating the assignment in force",
            json!({ "TreatmentOverrides": one_override.clone() }),
            json!({ "TreatmentOverrides": one_override.clone() }),
            vec![],
        ),
        (
            "the exposure and the overrides moved by one call",
            json!({ "ExposurePercentage": 1.0 }),
            json!({
                "Description": "both",
                "ExposurePercentage": 2.5,
                "TreatmentOverrides": { "Inline": string_map([("entity-9", "T1")]) },
            }),
            vec!["EXPOSURE_UPDATED", "OVERRIDES_UPDATED"],
        ),
    ];

    for (index, (case, start, patch, appended)) in rows.into_iter().enumerate() {
        // One definition per row: a definition delivers one run at a time.
        let name = format!("moved-{index}");
        let definition = create_test_definition(&router, &scope, &name).await;
        let definition_id = id_of(case, &definition);
        let created = start_run(&router, &scope.application, &definition_id, &start).await;
        let started = assert_running(case, &created, 1, &definition);

        let before = timeline(case, &router, &scope.application, &definition_id, 1).await;
        assert_eq!(
            types_of(case, &before),
            vec!["RUN_STARTED".to_string()],
            "case {case:?}: the start is the only action performed so far, got {before:?}"
        );
        assert_eq!(
            instant(case, "OccurredAt", before[0].get("OccurredAt")),
            instant(case, "StartedAt", started.get("StartedAt")),
            "case {case:?}: the start's entry occurred when the run started, got {:?}",
            before[0]
        );

        let answered = update_run(&router, &scope.application, &definition_id, 1, &patch).await;
        let updated = accepted(case, &answered);

        let after = timeline(case, &router, &scope.application, &definition_id, 1).await;
        assert_eq!(
            after.len(),
            before.len() + appended.len(),
            "case {case:?}: expected {} entries on the timeline, got {after:?}",
            before.len() + appended.len()
        );
        // Newest first, so what this update appended is at the HEAD of the feed. Two moves
        // made by ONE call occurred at the same instant, so the entries are compared as a
        // set: the rule is which moves were recorded, not how one instant is broken up.
        let mut recorded = types_of(case, &after[..appended.len()]);
        recorded.sort();
        let mut expected: Vec<String> = appended.iter().map(|kind| kind.to_string()).collect();
        expected.sort();
        assert_eq!(
            recorded, expected,
            "case {case:?}: expected the moves {expected:?} to be recorded, got {after:?}"
        );
        assert_descending(case, &after);

        for entry in &after[..appended.len()] {
            // "The exposure percentage at the time of the event" and "The treatment
            // overrides at the time of the event"
            // (`smithy_query(query="shape", shape="ExperimentRunEvent")`), which for an
            // update's entry is what the update left in force.
            assert_eq!(
                instant(case, "OccurredAt", entry.get("OccurredAt")),
                instant(case, "UpdatedAt", updated.get("UpdatedAt")),
                "case {case:?}: the entry occurred when the record was written, got {entry}"
            );
            for member in ["ExposurePercentage", "TreatmentOverrides"] {
                assert_eq!(
                    entry.get(member),
                    updated.get(member),
                    "case {case:?}: expected the entry's {member} to be the run's {:?}, got {:?}",
                    updated.get(member),
                    entry.get(member)
                );
            }
            assert_eq!(
                entry.get("TriggeredBy").and_then(Value::as_str),
                Some("USER"),
                "case {case:?}: an update a caller asked for is triggered by USER, got {entry}"
            );
            assert!(
                entry
                    .get("Description")
                    .and_then(Value::as_str)
                    .is_some_and(|text| !text.is_empty()),
                "case {case:?}: expected a description of the event, got {entry}"
            );
        }

        // The timeline is a record of what happened, not a projection of the run as it is
        // now: the entry the start wrote is reported unchanged however the run moved since,
        // and it is the oldest entry, so it tails the feed.
        assert_eq!(
            after.last(),
            before.first(),
            "case {case:?}: expected the start's entry to tail the feed unchanged, got {after:?}"
        );

        // What the update wrote is what the run's own reader reports.
        let read = get_run(&router, &scope.application, &definition_id, 1).await;
        assert_eq!(
            accepted(case, &read),
            updated,
            "case {case:?}: expected GetExperimentRun to report {updated}, got {}",
            read.json()
        );
    }
}

/// Rule 2: the feed is newest first, and a walk under any page size reads it in that order,
/// each entry exactly once.
#[tokio::test]
async fn the_timeline_is_served_most_recent_first_however_it_is_paged() {
    let router = router();
    let scope = scope(&router, "newest").await;
    let raise = json!({ "ExposurePercentage": 25.0 });
    let reassign = json!({ "TreatmentOverrides": { "Inline": string_map([("entity-1", "T1")]) } });

    // Rows differ along HOW MANY entries the timeline holds and which actions put them
    // there. Each action is its own call, so the order is the order the calls happened in,
    // reversed.
    let rows: Vec<(&str, Vec<Value>, bool, Vec<&str>)> = vec![
        ("only the start", vec![], false, vec!["RUN_STARTED"]),
        (
            "one update",
            vec![raise.clone()],
            false,
            vec!["EXPOSURE_UPDATED", "RUN_STARTED"],
        ),
        (
            "the start and the stop",
            vec![],
            true,
            vec!["RUN_STOPPED", "RUN_STARTED"],
        ),
        (
            "one update and the stop",
            vec![raise.clone()],
            true,
            vec!["RUN_STOPPED", "EXPOSURE_UPDATED", "RUN_STARTED"],
        ),
        (
            "two updates and the stop",
            vec![raise.clone(), reassign.clone()],
            true,
            vec![
                "RUN_STOPPED",
                "OVERRIDES_UPDATED",
                "EXPOSURE_UPDATED",
                "RUN_STARTED",
            ],
        ),
    ];

    for (index, (case, updates, stopped, expected)) in rows.into_iter().enumerate() {
        let name = format!("newest-{index}");
        let definition = create_test_definition(&router, &scope, &name).await;
        let definition_id = id_of(case, &definition);
        let created = start_run(
            &router,
            &scope.application,
            &definition_id,
            &json!({ "ExposurePercentage": 5.0 }),
        )
        .await;
        assert_running(case, &created, 1, &definition);
        for patch in &updates {
            let answered = update_run(&router, &scope.application, &definition_id, 1, patch).await;
            accepted(case, &answered);
        }
        if stopped {
            let ended = stop_run(&router, &scope.application, &definition_id, 1, &json!({})).await;
            accepted(case, &ended);
        }

        let whole = timeline(case, &router, &scope.application, &definition_id, 1).await;
        assert_eq!(
            types_of(case, &whole),
            expected
                .iter()
                .map(|kind| kind.to_string())
                .collect::<Vec<String>>(),
            "case {case:?}: expected the feed newest first, got {whole:?}"
        );
        assert_descending(case, &whole);

        // The same feed read a page at a time: the FIRST page carries the newest entries,
        // and the walk reports every entry exactly once in the same order. Page sizes span
        // both published bounds of `MaxResults` and values between them.
        for size in [MIN_MAX_RESULTS, 2, 3, MAX_MAX_RESULTS] {
            let case = format!("{case}, paged {size} at a time");
            let mut walked: Vec<Value> = Vec::new();
            let mut token: Option<String> = None;
            // One page per entry is the most a walk can need, plus one for the last
            // request: a bound, so a token that never settles cannot spin here.
            for _ in 0..=whole.len() {
                let query = match &token {
                    Some(next) => format!("max_results={size}&next_token={next}"),
                    None => format!("max_results={size}"),
                };
                let answer =
                    list_events(&router, &scope.application, &definition_id, 1, &query).await;
                let page = accepted(&case, &answer);
                let entries = items(&case, &page);
                assert!(
                    entries.len() <= size as usize,
                    "case {case:?}: a page carries at most {size} entries, got {}",
                    entries.len()
                );
                if walked.is_empty() {
                    assert_eq!(
                        entries.first(),
                        whole.first(),
                        "case {case:?}: the first page must open on the newest entry {:?}, got \
                         {:?}",
                        whole.first(),
                        entries.first()
                    );
                }
                walked.extend(entries);
                token = page
                    .get("NextToken")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                if token.is_none() {
                    break;
                }
            }
            assert_eq!(
                token, None,
                "case {case:?}: the walk must end, got a token left over: {token:?}"
            );
            assert_eq!(
                walked, whole,
                "case {case:?}: expected the paged walk to report {whole:?}, got {walked:?}"
            );
        }
    }
}

/// Rule 3: a run its lifecycle has finished with takes no update — HTTP 400
/// `BadRequestException` with AWS's template, writing nothing — while a run still in flight
/// takes the same patch and persists it.
#[tokio::test]
async fn an_update_of_a_terminal_run_is_a_bad_request_and_writes_nothing() {
    const LABEL: &str = "terminal";
    let router = router();
    let scope = scope(&router, LABEL).await;
    let application_name = format!("app-{LABEL}");
    let name = format!("{LABEL}-definition");
    let definition = create_test_definition(&router, &scope, &name).await;
    let definition_id = id_of(&name, &definition);
    let overrides = json!({ "Inline": string_map([("entity-1", "T1")]) });

    // The refusal is exercised on TWO run numbers of the same definition, so the number the
    // template renders is not one row's constant: run 1 is stopped first, then run 2 serves
    // the positive control before it, too, is stopped.
    for run in [1_i64, 2] {
        let started = start_run(
            &router,
            &scope.application,
            &definition_id,
            &json!({ "ExposurePercentage": 10.0 }),
        )
        .await;
        let running = assert_running(&format!("run {run}"), &started, run, &definition);

        if run == 2 {
            // The positive control: a run still in flight takes the patch, reports the
            // status of a running experiment and the new exposure, and persists both.
            let case = "the positive control: a run still in flight";
            let answered = update_run(
                &router,
                &scope.application,
                &definition_id,
                run,
                &json!({ "ExposurePercentage": 20.0 }),
            )
            .await;
            let updated = accepted(case, &answered);
            assert_eq!(
                updated.get("Status").and_then(Value::as_str),
                Some("RUNNING"),
                "case {case:?}: expected the run to still be RUNNING, got {updated}"
            );
            assert_eq!(
                updated.get("ExposurePercentage").and_then(Value::as_f64),
                Some(20.0),
                "case {case:?}: expected the exposure the update asked for, got {updated}"
            );
            let read = get_run(&router, &scope.application, &definition_id, run).await;
            assert_eq!(
                accepted(case, &read),
                updated,
                "case {case:?}: expected GetExperimentRun to report {updated}, got {}",
                read.json()
            );
            assert_ne!(
                updated.get("ExposurePercentage"),
                running.get("ExposurePercentage"),
                "case {case:?}: the control must move the exposure, got {updated}"
            );
        }

        let ended = stop_run(&router, &scope.application, &definition_id, run, &json!({})).await;
        let terminal = accepted(&format!("the stop of run {run}"), &ended);
        let events_before = timeline(
            &format!("the stop of run {run}"),
            &router,
            &scope.application,
            &definition_id,
            run,
        )
        .await;

        // Rows differ along what the refused patch carries — each member alone, all of them
        // together, and none at all — and along whether the resources are addressed by
        // minted id or by name.
        for (shape, by_name, patch) in [
            ("a new exposure", false, json!({ "ExposurePercentage": 20.0 })),
            (
                "a new description",
                true,
                json!({ "Description": "must not be stored" }),
            ),
            (
                "new overrides",
                false,
                json!({ "TreatmentOverrides": overrides.clone() }),
            ),
            ("nothing at all", true, json!({})),
            (
                "every member it takes",
                false,
                json!({
                    "Description": "nor this",
                    "ExposurePercentage": 100.0,
                    "TreatmentOverrides": overrides.clone(),
                    "DeploymentParameters": { "Tags": { "owner": "nobody" } },
                }),
            ),
        ] {
            let case = format!("run {run} carrying {shape}");
            let (application, addressed) = if by_name {
                (application_name.clone(), name.clone())
            } else {
                (scope.application.clone(), definition_id.clone())
            };

            let refused = update_run(&router, &application, &addressed, run, &patch).await;
            assert_refused(&case, &refused, 400, "BadRequestException");
            let expected = format!(
                "Cannot update experiment run {run} for experiment definition {addressed} \
                 because it is already terminal."
            );
            assert_eq!(
                refused.string("Message").as_deref(),
                Some(expected.as_str()),
                "case {case:?}: expected the message {expected:?}, got {:?}",
                refused.string("Message")
            );

            // The refusal left no effect: the run stands exactly as the stop left it, and
            // nothing was recorded on its timeline.
            let read = get_run(&router, &scope.application, &definition_id, run).await;
            assert_eq!(
                accepted(&case, &read),
                terminal,
                "case {case:?}: a refused update must write nothing, expected {terminal}, got {}",
                read.json()
            );
            let events_after =
                timeline(&case, &router, &scope.application, &definition_id, run).await;
            assert_eq!(
                events_after, events_before,
                "case {case:?}: a refused update records nothing, expected {events_before:?}, \
                 got {events_after:?}"
            );
        }
    }
}

/// One entry's place on the timeline is its own: a walk resumes where it left off whatever
/// the run records meanwhile, and the feed a run acted on in a loop reports keeps the most
/// recent entries rather than growing without limit.
#[tokio::test]
async fn a_walk_resumes_where_it_left_off_and_the_timeline_keeps_the_newest_entries() {
    const LABEL: &str = "bounded";
    let router = router();
    let scope = scope(&router, LABEL).await;
    let definition = create_test_definition(&router, &scope, LABEL).await;
    let definition_id = id_of(LABEL, &definition);
    let created = start_run(
        &router,
        &scope.application,
        &definition_id,
        &json!({ "ExposurePercentage": 5.0 }),
    )
    .await;
    assert_running(LABEL, &created, 1, &definition);

    // Each call assigns a different entity, so each one MOVES the overrides and records
    // one entry: the timeline grows by one per call.
    let reassign = |entity: usize| {
        let key = format!("entity-{entity}");
        json!({ "TreatmentOverrides": { "Inline": string_map([(key.as_str(), "T1")]) } })
    };
    for entity in 0..2 {
        let answered = update_run(
            &router,
            &scope.application,
            &definition_id,
            1,
            &reassign(entity),
        )
        .await;
        accepted("an assignment moved before the walk", &answered);
    }
    let case = "a token minted while the run was still being acted on";
    let before = timeline(case, &router, &scope.application, &definition_id, 1).await;
    assert_eq!(
        before.len(),
        3,
        "case {case:?}: the start and two moved assignments are three entries, got {before:?}"
    );

    // A page of one, then another action on the run, then the token: the walk continues at
    // the entry that followed the page, not at the one recorded since and not at the page
    // over again.
    let first = accepted(
        case,
        &list_events(
            &router,
            &scope.application,
            &definition_id,
            1,
            &format!("max_results={MIN_MAX_RESULTS}"),
        )
        .await,
    );
    assert_eq!(
        items(case, &first),
        before[..1].to_vec(),
        "case {case:?}: the first page must open on the newest entry {:?}, got {:?}",
        before.first(),
        items(case, &first)
    );
    let token = first
        .get("NextToken")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| {
            panic!("case {case:?}: entries were left over, so a token must come back: {first}")
        });
    let answered = update_run(&router, &scope.application, &definition_id, 1, &reassign(2)).await;
    accepted("an assignment moved while the walk was open", &answered);
    let resumed = accepted(
        case,
        &list_events(
            &router,
            &scope.application,
            &definition_id,
            1,
            &format!("max_results={MAX_MAX_RESULTS}&next_token={token}"),
        )
        .await,
    );
    assert_eq!(
        items(case, &resumed),
        before[1..].to_vec(),
        "case {case:?}: expected the walk to resume at {:?}, got {:?}",
        &before[1..],
        items(case, &resumed)
    );

    // Past the retention bound the feed keeps the most recent entries: the oldest, the
    // start's own among them, are dropped rather than held forever.
    let kept = state::MAX_RETAINED_EXPERIMENT_RUN_EVENTS;
    for entity in 3..=kept + 3 {
        let answered = update_run(
            &router,
            &scope.application,
            &definition_id,
            1,
            &reassign(entity),
        )
        .await;
        accepted("an assignment moved past the retention bound", &answered);
    }
    let case = "a run acted on past the retention bound";
    let whole = walk_timeline(
        case,
        &router,
        &scope.application,
        &definition_id,
        1,
        MAX_MAX_RESULTS,
    )
    .await;
    assert_eq!(
        whole.len(),
        kept,
        "case {case:?}: expected the newest {kept} entries, got {}",
        whole.len()
    );
    assert_descending(case, &whole);
    assert!(
        types_of(case, &whole)
            .iter()
            .all(|kind| kind == "OVERRIDES_UPDATED"),
        "case {case:?}: every entry still held was recorded by a moved assignment, got {:?}",
        types_of(case, &whole)
    );
    assert_eq!(
        whole.first().and_then(|entry| entry.get("TreatmentOverrides")),
        reassign(kept + 3).get("TreatmentOverrides"),
        "case {case:?}: expected the newest entry to report the last assignment, got {:?}",
        whole.first()
    );
}

