// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the EVENT-trigger slice of `CreateTrigger`: the trigger kind an
//! EventBridge rule drives, and the `EventBatchingCondition` that decides when the events
//! it is handed fire it.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler.
//!
//! The rule under test: `CreateTrigger` is a SERVED operation for every well-formed
//! definition the model admits — including an `EVENT` trigger carrying an
//! `EventBatchingCondition` — so it answers 200 with the trigger's name, persists the
//! condition as submitted, and `GetTrigger` then reports `Type`, the server-set `State`
//! and that condition unchanged; no such request is answered with a status or an error
//! code outside the operation's modelled set. A condition whose numbers fall outside the
//! modelled bounds is refused at the boundary and nothing is stored.
//!
//! The rows differ along what the rule generalises over: the `BatchSize` (each published
//! bound of `range 1..100`, one in between, and one past each bound), the `BatchWindow`
//! (absent, each published bound of `range 1..900`, one in between, and one past each
//! bound), whether the trigger belongs to a workflow, whether the create starts it, how
//! many actions it initiates and of which kind, and whether it carries tags.
//!
//! Source of the two ranges and of both members' documentation: `com.amazonaws.glue#
//! EventBatchingCondition` in this crate's Smithy model — `BatchSize` is required with
//! `range 1..100` ("Number of events that must be received from Amazon EventBridge before
//! EventBridge event trigger fires") and `BatchWindow` is optional with `range 1..900`
//! ("Window of time in seconds after which EventBridge event trigger fires. Window starts
//! when first event is received").

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_glue_awsglue::state::{
    messages, TRIGGER_STATE_ACTIVATED, TRIGGER_STATE_CREATED,
};

use crate::error_messages::assert_refusal;
use crate::wire::{call, emulator, error_code, error_message, ok};

/// The job an event trigger's action runs. An action names a job of the Region the create
/// is served in, so the suite registers it first.
const JOB: &str = "event-batching-target-job";

/// A second job, so a row can initiate more than one action.
const OTHER_JOB: &str = "event-batching-other-job";

/// The crawler an action can name instead of a job, so a row can carry two actions of
/// different kinds.
const CRAWLER: &str = "event-batching-target-crawler";

/// The workflow an event trigger belongs to: the resource an EventBridge delivery would
/// be counted against.
const WORKFLOW: &str = "event-batching-workflow";

/// A second workflow, so the table can hold two event triggers batching independently —
/// the pair a client builds when it compares a batched workflow with an unbatched one.
const OTHER_WORKFLOW: &str = "event-batching-other-workflow";

/// Register the workflows, jobs and crawler the definitions below name, so a create's
/// workflow and action targets resolve.
async fn register_targets(router: &MultiServiceRouter) {
    for workflow in [WORKFLOW, OTHER_WORKFLOW] {
        ok(router, "CreateWorkflow", &json!({ "Name": workflow })).await;
    }
    for job in [JOB, OTHER_JOB] {
        ok(
            router,
            "CreateJob",
            &json!({
                "Name": job,
                "Role": "arn:aws:iam::000000000000:role/sim2real-glue-job-role",
                "Command": {
                    "Name": "pythonshell",
                    "PythonVersion": "3.9",
                    "ScriptLocation": "s3://etl-scripts/noop.py",
                },
            }),
        )
        .await;
    }
    ok(
        router,
        "CreateCrawler",
        &json!({
            "Name": CRAWLER,
            "Role": "arn:aws:iam::000000000000:role/sim2real-crawler-role",
            "Targets": { "S3Targets": [{ "Path": "s3://sim2real-glue-crawler/data/" }] },
        }),
    )
    .await;
}

/// The `Trigger` structure `GetTrigger` reports for `name`.
async fn get_trigger(router: &MultiServiceRouter, name: &str) -> Value {
    let body = ok(router, "GetTrigger", &json!({ "Name": name })).await;
    body.get("Trigger")
        .cloned()
        .unwrap_or_else(|| panic!("GetTrigger must report a Trigger, got {body}"))
}

/// The rows of the round-trip table: each one a whole `CreateTrigger` body of an EVENT
/// trigger, differing along the batching condition and the rest of the definition.
fn accepted_definitions() -> Vec<(&'static str, Value)> {
    vec![
        (
            // The lower published bound of `BatchSize`: a trigger that fires on the first
            // event it is handed.
            "batch size at the lower bound, no window",
            json!({
                "Name": "event-batch-size-1",
                "Type": "EVENT",
                "WorkflowName": WORKFLOW,
                "Actions": [{ "JobName": JOB }],
                "EventBatchingCondition": { "BatchSize": 1 },
            }),
        ),
        (
            // One in between, with the window at ITS upper bound: the condition a client
            // writes when the batch size alone is to decide the firing.
            "a batch of three with the window at its upper bound",
            json!({
                "Name": "event-batch-size-3",
                "Type": "EVENT",
                "WorkflowName": OTHER_WORKFLOW,
                "Actions": [{ "JobName": JOB }],
                "EventBatchingCondition": { "BatchSize": 3, "BatchWindow": 900 },
                "Description": "fires once three deliveries have arrived",
            }),
        ),
        (
            // The upper published bound of `BatchSize`, with the window at ITS lower one.
            "batch size at the upper bound, window at the lower bound",
            json!({
                "Name": "event-batch-size-100",
                "Type": "EVENT",
                "WorkflowName": WORKFLOW,
                "Actions": [{ "JobName": OTHER_JOB }, { "CrawlerName": CRAWLER }],
                "EventBatchingCondition": { "BatchSize": 100, "BatchWindow": 1 },
            }),
        ),
        (
            // A window in between, on a trigger the create also tags and starts: "True is
            // not supported for ON_DEMAND triggers" is the one exclusion, so an EVENT
            // trigger may be started on creation.
            "a window in between, tagged and started on creation",
            json!({
                "Name": "event-batch-window-60",
                "Type": "EVENT",
                "WorkflowName": WORKFLOW,
                "Actions": [
                    { "JobName": JOB },
                    { "JobName": OTHER_JOB, "Timeout": 30 },
                ],
                "EventBatchingCondition": { "BatchSize": 10, "BatchWindow": 60 },
                "StartOnCreation": true,
                "Tags": { "owner": "data-platform" },
            }),
        ),
        (
            // An event trigger belongs to a workflow only if it names one: the definition
            // is well formed without a `WorkflowName`, so the create serves it.
            "outside any workflow",
            json!({
                "Name": "event-batch-no-workflow",
                "Type": "EVENT",
                "Actions": [{ "CrawlerName": CRAWLER }],
                "EventBatchingCondition": { "BatchSize": 2, "BatchWindow": 300 },
            }),
        ),
    ]
}

// ---------------------------------------------------------------------------
// Create, then read back
// ---------------------------------------------------------------------------

/// An EVENT trigger carrying an `EventBatchingCondition` is SERVED: the create answers 200
/// with the trigger's name, and `GetTrigger` reports `Type` `EVENT`, the `State` the create
/// left the trigger in, and the condition exactly as submitted.
#[tokio::test]
async fn an_event_trigger_with_a_batching_condition_round_trips() {
    let router = emulator();
    register_targets(&router).await;

    for (case, request) in accepted_definitions() {
        let name = request["Name"].as_str().expect("the case names a trigger");

        let (status, created) = call(&router, "CreateTrigger", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateTrigger must serve a well-formed EVENT trigger, got \
             {status} with code {} and message {}",
            error_code(&created),
            error_message(&created)
        );
        assert_eq!(
            created.get("Name").and_then(Value::as_str),
            Some(name),
            "case {case:?}: the create answers with the trigger's name, got {created}"
        );

        let trigger = get_trigger(&router, name).await;

        // The axis under test: the condition is persisted member for member, so a reader
        // sees the batch size and window the create was given and no other numbers.
        assert_eq!(
            trigger.get("EventBatchingCondition"),
            request.get("EventBatchingCondition"),
            "case {case:?}: the condition must round-trip member for member, got {trigger}"
        );
        // The workflow the batched events would be counted against, which is absent for a
        // trigger that names none.
        assert_eq!(
            trigger.get("WorkflowName"),
            request.get("WorkflowName"),
            "case {case:?}: the trigger reports the workflow it belongs to, got {trigger}"
        );

        // The one member the service sets, and the type it was created as.
        let started = request
            .get("StartOnCreation")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let expected_state = if started {
            TRIGGER_STATE_ACTIVATED
        } else {
            TRIGGER_STATE_CREATED
        };
        assert_eq!(
            trigger.get("State").and_then(Value::as_str),
            Some(expected_state),
            "case {case:?}: expected State {expected_state}, got {trigger}"
        );
        assert_eq!(
            trigger.get("Type").and_then(Value::as_str),
            Some("EVENT"),
            "case {case:?}: the trigger keeps the type it was created as, got {trigger}"
        );
    }

    // Each create is its own record: every trigger of the table is still readable with
    // its own condition, so one row's create did not overwrite another's.
    for (case, request) in accepted_definitions() {
        let name = request["Name"].as_str().expect("the case names a trigger");
        let trigger = get_trigger(&router, name).await;
        assert_eq!(
            trigger.get("EventBatchingCondition"),
            request.get("EventBatchingCondition"),
            "case {case:?}: {name} must still report its own condition, got {trigger}"
        );
    }
}

/// Two event triggers of the same shape but different `BatchSize`, one per workflow, are
/// two independent records: each reports its own condition, so a client can tell the
/// batched arm from the unbatched one.
#[tokio::test]
async fn event_triggers_of_two_workflows_keep_their_own_conditions() {
    let router = emulator();
    register_targets(&router).await;

    // The rows differ along the one member under test: the batch size each arm asks for.
    for (case, workflow, trigger, batch_size) in [
        ("the batched arm", WORKFLOW, "batched-evt", 3),
        ("the unbatched twin", OTHER_WORKFLOW, "twin-evt", 1),
    ] {
        ok(
            &router,
            "CreateTrigger",
            &json!({
                "Name": trigger,
                "Type": "EVENT",
                "WorkflowName": workflow,
                "Actions": [{ "JobName": JOB }],
                "EventBatchingCondition": { "BatchSize": batch_size, "BatchWindow": 900 },
            }),
        )
        .await;

        let read = get_trigger(&router, trigger).await;
        assert_eq!(
            read["EventBatchingCondition"].get("BatchSize").and_then(Value::as_i64),
            Some(batch_size),
            "case {case:?}: the trigger reports the batch size it was created with, got {read}"
        );
        assert_eq!(
            read.get("WorkflowName").and_then(Value::as_str),
            Some(workflow),
            "case {case:?}: the trigger reports the workflow it belongs to, got {read}"
        );
    }

    // Neither create disturbed the other's record.
    for (case, trigger, batch_size) in [
        ("the batched arm after both creates", "batched-evt", 3),
        ("the unbatched twin after both creates", "twin-evt", 1),
    ] {
        let read = get_trigger(&router, trigger).await;
        assert_eq!(
            read["EventBatchingCondition"].get("BatchSize").and_then(Value::as_i64),
            Some(batch_size),
            "case {case:?}: expected BatchSize {batch_size}, got {read}"
        );
    }
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// A batching condition outside the modelled bounds is refused at the boundary with a 400,
/// and nothing is stored: a later read of that name finds no trigger.
///
/// The rows are one step past each published bound of `BatchSize` (`range 1..100`) and of
/// `BatchWindow` (`range 1..900`), plus the zero each range excludes.
#[tokio::test]
async fn a_batching_condition_outside_the_modelled_range_is_refused() {
    let router = emulator();
    register_targets(&router).await;

    for (index, (case, condition)) in [
        ("batch size below the lower bound", json!({ "BatchSize": 0 })),
        (
            "batch size one past the upper bound",
            json!({ "BatchSize": 101 }),
        ),
        (
            "batch size far past the upper bound",
            json!({ "BatchSize": 1_000 }),
        ),
        (
            "window below the lower bound",
            json!({ "BatchSize": 3, "BatchWindow": 0 }),
        ),
        (
            "window one past the upper bound",
            json!({ "BatchSize": 3, "BatchWindow": 901 }),
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let name = format!("out-of-range-condition-{index}");
        let (status, body) = call(
            &router,
            "CreateTrigger",
            &json!({
                "Name": name,
                "Type": "EVENT",
                "WorkflowName": WORKFLOW,
                "Actions": [{ "JobName": JOB }],
                "EventBatchingCondition": condition,
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a condition outside the modelled range is refused, got {status} \
             with {body}"
        );
        assert!(
            error_code(&body).ends_with("ValidationException"),
            "case {case:?}: a modelled constraint is refused at the boundary, got {} with {}",
            error_code(&body),
            error_message(&body)
        );

        // The refused create stored nothing, so the name is still free and the read of it
        // is the service's own absence refusal.
        assert_refusal(
            &router,
            case,
            "GetTrigger",
            &json!({ "Name": name }),
            "EntityNotFoundException",
            messages::TRIGGER_NOT_FOUND,
        )
        .await;
    }
}
