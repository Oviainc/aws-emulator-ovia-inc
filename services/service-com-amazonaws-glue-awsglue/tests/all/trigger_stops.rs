// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Glue trigger-stop slice: `StopTrigger` and the `GetTrigger`
//! that reports the state it left the trigger in.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler.
//!
//! The rule under test: a stop addresses a trigger by name in the Region that holds it
//! and leaves it in the one modelled `TriggerState` of a stopped trigger (`DEACTIVATED`),
//! which every later read of that trigger reports, whatever state it was in before and
//! however often the stop is repeated; the rest of the stored definition — type, schedule,
//! predicate, actions — is untouched, so a later start takes the same trigger out of the
//! stopped state again, and a run a firing already began is left to its own lifecycle. A
//! stop for a name the request's Region holds no trigger under is refused with
//! `EntityNotFoundException`.
//!
//! The tables are rows along what the rule generalises over: the trigger's type (each of
//! the four modelled `TriggerType` values), the state the trigger was in when the stop
//! arrived (registered, started on creation, started by a call, already stopped), how many
//! actions it initiates and of which kind, and whether it belongs to a workflow at all.
//!
//! The due instants the workflow-run lifecycle turns on are moved THROUGH THE STORE rather
//! than slept for, so a run held in flight stays in flight for the whole test.

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::state::{
    messages, TRIGGER_STATE_ACTIVATED, TRIGGER_STATE_CREATED, TRIGGER_STATE_DEACTIVATED,
    WORKFLOW_RUN_STATUS_RUNNING,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::error_messages::assert_refusal;
use crate::job_runs::{emulator_with_engine, far_future};
use crate::trigger_starts::{
    register_targets, set_due_instant, stored_run_ids, trigger_state, CRAWLER, JOB, OTHER_JOB,
};
use crate::wire::{call_in_region, ok, REGION};

/// One row of the stop table: the trigger's definition, how it came to be started (if at
/// all), and the state a read must report before the stop arrives.
struct StopCase {
    /// What this row varies, named in every assertion it drives.
    case: &'static str,
    /// The `CreateTrigger` body, minus `Name` and `WorkflowName`.
    trigger: Value,
    /// Whether a `StartTrigger` call precedes the stop. A create that already started the
    /// trigger (`StartOnCreation`) needs none.
    start_first: bool,
    /// The `TriggerState` a read reports for this row before the stop.
    state_before: &'static str,
    /// The `TriggerState` a read reports after the stop is undone by a later start: the
    /// state a start leaves a trigger of this type in.
    state_after_restart: &'static str,
}

/// The rows: each modelled `TriggerType`, a trigger that was never started, one started by
/// its create and one started by a call, one and two actions, a job action and a crawler
/// one.
fn stop_cases() -> Vec<StopCase> {
    vec![
        StopCase {
            case: "on-demand, never started",
            trigger: json!({
                "Type": "ON_DEMAND",
                "Actions": [{ "JobName": JOB }],
            }),
            start_first: false,
            state_before: TRIGGER_STATE_CREATED,
            state_after_restart: TRIGGER_STATE_CREATED,
        },
        StopCase {
            case: "on-demand, started by a call",
            trigger: json!({
                "Type": "ON_DEMAND",
                "Actions": [{ "JobName": JOB }, { "CrawlerName": CRAWLER }],
            }),
            start_first: true,
            // An on-demand trigger has no armed state: the start FIRED it and left it in
            // the state it waits in.
            state_before: TRIGGER_STATE_CREATED,
            state_after_restart: TRIGGER_STATE_CREATED,
        },
        StopCase {
            // "This field is required when the trigger type is SCHEDULED."
            case: "scheduled, never started",
            trigger: json!({
                "Type": "SCHEDULED",
                "Schedule": "cron(15 12 * * ? *)",
                "Actions": [{ "JobName": JOB }],
            }),
            start_first: false,
            state_before: TRIGGER_STATE_CREATED,
            state_after_restart: TRIGGER_STATE_ACTIVATED,
        },
        StopCase {
            // "Set to true to start SCHEDULED and CONDITIONAL triggers when created."
            case: "scheduled, started on creation",
            trigger: json!({
                "Type": "SCHEDULED",
                "Schedule": "cron(0 */6 * * ? *)",
                "StartOnCreation": true,
                "Actions": [{ "CrawlerName": CRAWLER }],
            }),
            start_first: false,
            state_before: TRIGGER_STATE_ACTIVATED,
            state_after_restart: TRIGGER_STATE_ACTIVATED,
        },
        StopCase {
            // "This field is required when the trigger type is CONDITIONAL." The predicate
            // waits on a job no action of this trigger runs: a conditional trigger may not
            // name one job in both places.
            case: "conditional, started by a call",
            trigger: json!({
                "Type": "CONDITIONAL",
                "Predicate": {
                    "Conditions": [{
                        "LogicalOperator": "EQUALS",
                        "JobName": OTHER_JOB,
                        "State": "SUCCEEDED",
                    }],
                },
                "Actions": [{ "JobName": JOB }],
            }),
            start_first: true,
            state_before: TRIGGER_STATE_ACTIVATED,
            state_after_restart: TRIGGER_STATE_ACTIVATED,
        },
        StopCase {
            case: "conditional, started on creation",
            trigger: json!({
                "Type": "CONDITIONAL",
                "StartOnCreation": true,
                "Predicate": {
                    "Logical": "ANY",
                    "Conditions": [
                        { "LogicalOperator": "EQUALS", "JobName": OTHER_JOB, "State": "FAILED" },
                        { "CrawlState": "SUCCEEDED", "CrawlerName": CRAWLER },
                    ],
                },
                "Actions": [{ "JobName": JOB }, { "CrawlerName": CRAWLER }],
            }),
            start_first: false,
            state_before: TRIGGER_STATE_ACTIVATED,
            state_after_restart: TRIGGER_STATE_ACTIVATED,
        },
        StopCase {
            // "Batch condition that must be met … before EventBridge event trigger fires."
            case: "event, never started",
            trigger: json!({
                "Type": "EVENT",
                "EventBatchingCondition": { "BatchSize": 3, "BatchWindow": 900 },
                "Actions": [{ "JobName": JOB }],
            }),
            start_first: false,
            state_before: TRIGGER_STATE_CREATED,
            state_after_restart: TRIGGER_STATE_ACTIVATED,
        },
        StopCase {
            case: "event, started by a call",
            trigger: json!({
                "Type": "EVENT",
                "EventBatchingCondition": { "BatchSize": 1 },
                "Actions": [{ "CrawlerName": CRAWLER }],
            }),
            start_first: true,
            state_before: TRIGGER_STATE_ACTIVATED,
            state_after_restart: TRIGGER_STATE_ACTIVATED,
        },
    ]
}

/// Register the row's workflow and its trigger, start it when the row says a call does,
/// and answer the names they were registered under. `index` keeps each row's resources to
/// itself.
async fn register_case(
    router: &MultiServiceRouter,
    index: usize,
    row: &StopCase,
) -> (String, String) {
    let workflow = format!("trigger-stop-workflow-{index}");
    ok(router, "CreateWorkflow", &json!({ "Name": workflow })).await;

    let trigger = format!("trigger-stop-{index}");
    let mut body = row.trigger.clone();
    body["Name"] = json!(trigger);
    body["WorkflowName"] = json!(workflow);
    ok(router, "CreateTrigger", &body).await;

    if row.start_first {
        ok(router, "StartTrigger", &json!({ "Name": trigger })).await;
    }
    (workflow, trigger)
}

/// Stop `trigger` and answer the HTTP status and body.
async fn stop_trigger(router: &MultiServiceRouter, trigger: &str) -> (u16, Value) {
    call_in_region(router, REGION, "StopTrigger", &json!({ "Name": trigger })).await
}

/// The `Trigger` `GetTrigger` reports for `trigger`, with `State` removed: the part of the
/// definition a stop must leave exactly as it was.
async fn trigger_definition(router: &MultiServiceRouter, trigger: &str) -> Value {
    let body = ok(router, "GetTrigger", &json!({ "Name": trigger })).await;
    let mut reported = body["Trigger"].clone();
    if let Some(members) = reported.as_object_mut() {
        members.remove("State");
    }
    reported
}

// ---------------------------------------------------------------------------
// Stop, then read back
// ---------------------------------------------------------------------------

/// A stop answers 200 with the trigger's name and leaves the trigger in the stopped state
/// a later read reports, whatever state it was in before, leaving the rest of its
/// definition untouched.
#[tokio::test]
async fn a_stop_deactivates_the_trigger_whatever_state_it_was_in() {
    let (router, _engine) = emulator_with_engine();
    register_targets(&router).await;

    for (index, row) in stop_cases().iter().enumerate() {
        let case = row.case;
        let (_workflow, trigger) = register_case(&router, index, row).await;

        let before = trigger_state(&router, &trigger).await;
        assert_eq!(
            before, row.state_before,
            "case {case:?}: the stop must arrive at a trigger in {:?}, got {before:?}",
            row.state_before
        );
        let definition_before = trigger_definition(&router, &trigger).await;

        let (status, body) = stop_trigger(&router, &trigger).await;
        assert_eq!(
            status, 200,
            "case {case:?}: StopTrigger must answer 200, got {status} with {body}"
        );
        assert_eq!(
            body.get("Name").and_then(Value::as_str),
            Some(trigger.as_str()),
            "case {case:?}: the stop answers the name of the trigger it stopped, got {body}"
        );

        let after = trigger_state(&router, &trigger).await;
        assert_eq!(
            after, TRIGGER_STATE_DEACTIVATED,
            "case {case:?}: a stopped trigger is in {TRIGGER_STATE_DEACTIVATED}, got \
             {after:?}"
        );
        let definition_after = trigger_definition(&router, &trigger).await;
        assert_eq!(
            definition_after, definition_before,
            "case {case:?}: the stop must change only the trigger's state, got \
             {definition_after} for {definition_before}"
        );
    }
}

/// Stopping is idempotent — a second stop answers 200 and reports the same state — and the
/// stop is not a one-way door: a later start takes the trigger out of the stopped state and
/// into the state a start leaves a trigger of its type in, and stopping it once more
/// deactivates it again.
#[tokio::test]
async fn stopping_is_idempotent_and_a_later_start_undoes_it() {
    let (router, _engine) = emulator_with_engine();
    register_targets(&router).await;

    for (index, row) in stop_cases().iter().enumerate() {
        let case = row.case;
        let (_workflow, trigger) = register_case(&router, index, row).await;

        // Repeat the stop: each one answers 200 and leaves the same state, including the
        // row where the trigger was already stopped by the previous round.
        for stop in 1..=3 {
            let (status, body) = stop_trigger(&router, &trigger).await;
            assert_eq!(
                status, 200,
                "case {case:?}: stop {stop} must answer 200, got {status} with {body}"
            );
            let state = trigger_state(&router, &trigger).await;
            assert_eq!(
                state, TRIGGER_STATE_DEACTIVATED,
                "case {case:?}: after stop {stop} the trigger is in \
                 {TRIGGER_STATE_DEACTIVATED}, got {state:?}"
            );
        }

        ok(&router, "StartTrigger", &json!({ "Name": trigger })).await;
        let restarted = trigger_state(&router, &trigger).await;
        assert_eq!(
            restarted, row.state_after_restart,
            "case {case:?}: a stopped trigger starts again into {:?}, got {restarted:?}",
            row.state_after_restart
        );

        let (status, body) = stop_trigger(&router, &trigger).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the stop after the restart must answer 200, got {status} with \
             {body}"
        );
        let state = trigger_state(&router, &trigger).await;
        assert_eq!(
            state, TRIGGER_STATE_DEACTIVATED,
            "case {case:?}: the restarted trigger stops again into \
             {TRIGGER_STATE_DEACTIVATED}, got {state:?}"
        );
    }
}

/// A stop ends no workflow run a firing already began: the run keeps the status its own
/// lifecycle gives it, and the workflow keeps the runs it had.
#[tokio::test]
async fn a_stop_leaves_the_runs_a_firing_already_began_alone() {
    let (router, engine) = emulator_with_engine();
    register_targets(&router).await;

    // The rows differ along how many firings the workflow holds runs of before the stop.
    for (index, (case, firings)) in [
        ("one firing before the stop", 1usize),
        ("two firings before the stop", 2),
        ("three firings before the stop", 3),
    ]
    .into_iter()
    .enumerate()
    {
        let workflow = format!("stop-run-workflow-{index}");
        ok(
            &router,
            "CreateWorkflow",
            &json!({ "Name": workflow, "MaxConcurrentRuns": 10 }),
        )
        .await;
        let trigger = format!("stop-run-trigger-{index}");
        ok(
            &router,
            "CreateTrigger",
            &json!({
                "Name": trigger,
                "WorkflowName": workflow,
                "Type": "ON_DEMAND",
                "Actions": [{ "JobName": JOB }],
            }),
        )
        .await;

        for _ in 0..firings {
            ok(&router, "StartTrigger", &json!({ "Name": trigger })).await;
            // Hold every run the firings began in flight, so what is asserted after the
            // stop is what the stop did and not what the clock settled.
            set_due_instant(&engine, &workflow, far_future());
        }
        let before = stored_run_ids(&engine, &workflow);

        let (status, body) = stop_trigger(&router, &trigger).await;
        assert_eq!(
            status, 200,
            "case {case:?}: StopTrigger must answer 200, got {status} with {body}"
        );

        let after = stored_run_ids(&engine, &workflow);
        assert_eq!(
            after, before,
            "case {case:?}: the stop must leave the workflow's runs alone, got {after:?} \
             for {before:?}"
        );
        for run_id in &after {
            let read = ok(
                &router,
                "GetWorkflowRun",
                &json!({ "Name": workflow, "RunId": run_id }),
            )
            .await;
            assert_eq!(
                read["Run"].get("Status").and_then(Value::as_str),
                Some(WORKFLOW_RUN_STATUS_RUNNING),
                "case {case:?}: run {run_id} stays in flight across the stop, got {read}"
            );
        }
    }
}

/// A stop for a name the request's Region holds no trigger under is refused with the
/// modelled `EntityNotFoundException` naming what was asked for: a trigger is served only
/// through the endpoint that created it, so a trigger of another Region is such a name.
#[tokio::test]
async fn a_stop_for_a_trigger_this_region_does_not_hold_is_refused() {
    let (router, _engine) = emulator_with_engine();
    register_targets(&router).await;
    ok(
        &router,
        "CreateTrigger",
        &json!({
            "Name": "elsewhere-stop-trigger",
            "Type": "ON_DEMAND",
            "Actions": [{ "JobName": JOB }],
        }),
    )
    .await;

    // The names span what the template generalises over: a plain name, a single
    // character, a mixed-case one, punctuation, and the 255-character ceiling
    // `NameString` carries ("len 1..255", `smithy_query operation=StopTrigger`).
    for (case, name) in [
        ("a name nothing created", "no-such-trigger".to_string()),
        ("a single character", "x".to_string()),
        ("mixed case", "MissingTrigger".to_string()),
        ("punctuation and digits", "team.data_lake-2026/raw".to_string()),
        ("at the 255 ceiling", "n".repeat(255)),
    ] {
        assert_refusal(
            &router,
            case,
            "StopTrigger",
            &json!({ "Name": name }),
            "EntityNotFoundException",
            messages::TRIGGER_NOT_FOUND,
        )
        .await;
    }

    // The trigger created in this Region is not stoppable through another Region's
    // endpoint, and stopping it there leaves it untouched here.
    let (status, body) = call_in_region(
        &router,
        "us-west-2",
        "StopTrigger",
        &json!({ "Name": "elsewhere-stop-trigger" }),
    )
    .await;
    assert_eq!(
        status, 400,
        "another Region holds no such trigger, so the stop is refused, got {status} with \
         {body}"
    );
    let state = trigger_state(&router, "elsewhere-stop-trigger").await;
    assert_eq!(
        state, TRIGGER_STATE_CREATED,
        "the refused stop in another Region must not move this Region's trigger, got \
         {state:?}"
    );
}
