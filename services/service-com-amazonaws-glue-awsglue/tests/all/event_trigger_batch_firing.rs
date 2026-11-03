// SPDX-License-Identifier: CC0-1.0

//! What an event DELIVERED to a workflow does: the `EventBatchingCondition` accounting of
//! the workflow's EVENT trigger, and the workflow run a completed batch starts.
//!
//! The rule under test: an event handed to a workflow is counted against the
//! `EventBatchingCondition` of that workflow's EVENT trigger, and the trigger fires EXACTLY
//! ONCE per completed batch — "batch condition that must be met (specified number of events
//! received or batch time window expired) before EventBridge event trigger fires"
//! (`com.amazonaws.glue#EventBatchingCondition`), whose `BatchSize` is "number of events
//! that must be received from Amazon EventBridge before EventBridge event trigger fires"
//! (`@required`, `@range 1..100`) and whose `BatchWindow` is "window of time in seconds
//! after which EventBridge event trigger fires. Window starts when first event is received"
//! (`@range 1..900`). So a batch that has not filled starts NO run, the delivery that fills
//! it starts ONE, and a batch whose window has run out starts one with the events it has.
//! Each firing begins a run `GetWorkflowRuns` reports, carrying the workflow's own `Name`
//! and a `WorkflowRunId`.
//!
//! The rows differ along what the rule generalises over: the `BatchSize` (each published
//! bound, one in between, and a trigger carrying no condition at all), the number of events
//! delivered, and whether the window or the count is what completes the batch.
//!
//! The DELIVERY itself belongs to EventBridge — Glue serves no API for it, the ingress
//! being the `glue:notifyEvent` action a rule's target role carries — so the suite hands the
//! events to the store the way that ingress does, through
//! [`ComAmazonawsGlueAwsglueState::notify_workflow_event`], and then reads what a CLIENT
//! sees over the wire. The end-to-end path from `events:PutEvents` to a run is asserted in
//! the EventBridge crate's own suite.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_glue_awsglue::state::{
    service_state, WorkflowEventDelivery, TRIGGER_STATE_DEACTIVATED,
};

use crate::wire::{call, emulator_on, error_code, ok, REGION};

/// The job every trigger below initiates, so a firing has an action to begin.
const JOB: &str = "batch-firing-job";

/// One emulator, the engine it holds, and the job the triggers name.
fn given_emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(Arc::clone(&engine));
    (router, engine)
}

/// Register the job the trigger actions name.
async fn given_job(router: &MultiServiceRouter) {
    ok(
        router,
        "CreateJob",
        &json!({
            "Name": JOB,
            "Role": "arn:aws:iam::000000000000:role/glue-job-role",
            "Command": { "Name": "pythonshell", "ScriptLocation": "s3://scripts/noop.py" },
        }),
    )
    .await;
}

/// A workflow called `workflow` with an EVENT trigger on it, carrying `condition` as its
/// `EventBatchingCondition` (absent when the row names none).
async fn given_event_trigger(
    router: &MultiServiceRouter,
    workflow: &str,
    condition: Option<Value>,
) {
    ok(router, "CreateWorkflow", &json!({ "Name": workflow })).await;
    let mut request = json!({
        "Name": format!("{workflow}-evt"),
        "Type": "EVENT",
        "WorkflowName": workflow,
        "Actions": [{ "JobName": JOB }],
    });
    if let Some(condition) = condition {
        request["EventBatchingCondition"] = condition;
    }
    ok(router, "CreateTrigger", &request).await;
}

/// Hand ONE event to `workflow` the way the EventBridge ingress does, and answer what the
/// store made of it.
fn deliver(engine: &Arc<engine::InMemoryEngine>, workflow: &str) -> WorkflowEventDelivery {
    service_state(&**engine)
        .expect("the Glue store is reachable")
        .lock()
        .notify_workflow_event(REGION, workflow, SystemTime::now())
}

/// The runs `GetWorkflowRuns` reports for `workflow`, as a client reads them: a workflow
/// with no run at all is refused with `EntityNotFoundException`, which is the same ZERO.
async fn runs_of(router: &MultiServiceRouter, case: &str, workflow: &str) -> Vec<Value> {
    let (status, body) = call(router, "GetWorkflowRuns", &json!({ "Name": workflow })).await;
    if status == 400 {
        assert_eq!(
            error_code(&body),
            "EntityNotFoundException",
            "case {case:?}: a run history with nothing in it is the modelled miss, got {body}"
        );
        return Vec::new();
    }
    assert_eq!(
        status, 200,
        "case {case:?}: GetWorkflowRuns answered {status} {body}"
    );
    body["Runs"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: GetWorkflowRuns reports a Runs list, got {body}"))
}

/// Every run of `workflow` names that workflow and carries a run id.
fn assert_runs_are_well_formed(case: &str, workflow: &str, runs: &[Value]) {
    for run in runs {
        assert_eq!(
            run.get("Name").and_then(Value::as_str),
            Some(workflow),
            "case {case:?}: a run reports the workflow it belongs to, got {run}"
        );
        let run_id = run.get("WorkflowRunId").and_then(Value::as_str);
        assert!(
            run_id.is_some_and(|id| !id.is_empty()),
            "case {case:?}: a run carries a non-empty WorkflowRunId, got {run}"
        );
    }
}

/// A batch fires EXACTLY ONCE per `BatchSize` events delivered — over a TABLE whose rows
/// differ along the batch size (each published bound of `@range 1..100`, one in between,
/// and a trigger with no condition at all) and along how many events arrive.
#[tokio::test]
async fn an_event_trigger_fires_once_its_batch_size_is_reached() {
    // (case, the trigger's EventBatchingCondition, the number of runs expected after
    //  each of three deliveries)
    let rows: [(&str, Option<Value>, [usize; 3]); 5] = [
        (
            // The lower published bound of `BatchSize`: every event completes its own
            // batch, so every delivery starts a run.
            "batch size at the lower bound",
            Some(json!({ "BatchSize": 1 })),
            [1, 2, 3],
        ),
        (
            // No condition at all: nothing is batched, so the trigger fires per event, as
            // the lower bound of the batch size does.
            "no batching condition",
            None,
            [1, 2, 3],
        ),
        (
            "a batch of two",
            Some(json!({ "BatchSize": 2, "BatchWindow": 900 })),
            [0, 1, 1],
        ),
        (
            // The recorded shape of the defect: deliveries 1 and 2 start nothing and
            // delivery 3 starts exactly one run.
            "a batch of three",
            Some(json!({ "BatchSize": 3, "BatchWindow": 900 })),
            [0, 0, 1],
        ),
        (
            // The upper published bound: three events are nowhere near a batch, so the
            // workflow still has no run.
            "batch size at the upper bound",
            Some(json!({ "BatchSize": 100 })),
            [0, 0, 0],
        ),
    ];

    for (case, condition, expected) in rows {
        let (router, engine) = given_emulator();
        given_job(&router).await;
        let workflow = format!("wf-{}", case.replace(' ', "-"));
        given_event_trigger(&router, &workflow, condition).await;

        // Nothing has been delivered yet, so nothing has run.
        let before = runs_of(&router, case, &workflow).await;
        assert!(
            before.is_empty(),
            "case {case:?}: a workflow handed no event has no run, got {before:?}"
        );

        for (delivery, expected_runs) in expected.iter().enumerate() {
            let outcome = deliver(&engine, &workflow);
            let runs = runs_of(&router, case, &workflow).await;
            assert_eq!(
                runs.len(),
                *expected_runs,
                "case {case:?}: after delivery {} expected {expected_runs} run(s), got {} \
                 ({outcome:?})",
                delivery + 1,
                runs.len()
            );
            assert_runs_are_well_formed(case, &workflow, &runs);
        }
    }
}

/// A batch whose WINDOW has run out fires with the events it holds, even though the batch
/// size was never reached: the window starts when the first event is received, so the
/// firing is due one window after that event and a later read reports the run it began.
///
/// The window is the lower published bound of `BatchWindow` (`@range 1..900`), one second,
/// which is what makes the wait below short; the suite waits in REAL time because the
/// firing is owed by the clock rather than by a call.
#[tokio::test]
async fn a_batch_whose_window_ran_out_fires_with_the_events_it_has() {
    let case = "batch of three with a one-second window";
    let (router, engine) = given_emulator();
    given_job(&router).await;
    let workflow = "wf-window-expiry";
    given_event_trigger(
        &router,
        workflow,
        Some(json!({ "BatchSize": 3, "BatchWindow": 1 })),
    )
    .await;

    // One event: the batch of three is not complete, so nothing fires yet.
    let outcome = deliver(&engine, workflow);
    assert!(
        matches!(outcome, WorkflowEventDelivery::Counted { received: 1, batch_size: 3 }),
        "case {case:?}: the first event opens the batch, got {outcome:?}"
    );
    let runs = runs_of(&router, case, workflow).await;
    assert!(
        runs.is_empty(),
        "case {case:?}: an incomplete batch starts no run, got {runs:?}"
    );

    // Past the window: the firing was due one second after the first event, and the read
    // materialises it.
    std::thread::sleep(Duration::from_millis(1_200));
    let runs = runs_of(&router, case, workflow).await;
    assert_eq!(
        runs.len(),
        1,
        "case {case:?}: a window that ran out fires once with the events it holds, got \
         {runs:?}"
    );
    assert_runs_are_well_formed(case, workflow, &runs);

    // The batch closed with that firing, so the window does not fire a second run: a later
    // read reports the one run and no more.
    std::thread::sleep(Duration::from_millis(1_200));
    let runs = runs_of(&router, case, workflow).await;
    assert_eq!(
        runs.len(),
        1,
        "case {case:?}: a closed batch fires nothing further, got {runs:?}"
    );
}

/// What a delivery to a workflow that cannot receive it answers: a name no workflow holds,
/// a workflow with no EVENT trigger, and a workflow whose EVENT trigger a stop deactivated.
/// None of them starts a run.
#[tokio::test]
async fn a_delivery_a_workflow_cannot_receive_starts_no_run() {
    let (router, engine) = given_emulator();
    given_job(&router).await;

    // A name this account holds no workflow under.
    let outcome = deliver(&engine, "no-such-workflow");
    assert_eq!(
        outcome,
        WorkflowEventDelivery::NoWorkflow,
        "case {:?}: expected no workflow, got {outcome:?}",
        "a workflow that does not exist"
    );

    // A workflow whose only trigger is an ON_DEMAND one: no batch exists for the event.
    ok(&router, "CreateWorkflow", &json!({ "Name": "wf-on-demand" })).await;
    ok(
        &router,
        "CreateTrigger",
        &json!({
            "Name": "wf-on-demand-trg",
            "Type": "ON_DEMAND",
            "WorkflowName": "wf-on-demand",
            "Actions": [{ "JobName": JOB }],
        }),
    )
    .await;
    let outcome = deliver(&engine, "wf-on-demand");
    assert_eq!(
        outcome,
        WorkflowEventDelivery::NoEventTrigger,
        "case {:?}: expected no EVENT trigger, got {outcome:?}",
        "a workflow with no EVENT trigger"
    );
    let runs = runs_of(&router, "a workflow with no EVENT trigger", "wf-on-demand").await;
    assert!(
        runs.is_empty(),
        "case {:?}: no run is started, got {runs:?}",
        "a workflow with no EVENT trigger"
    );

    // A workflow whose EVENT trigger has been stopped: the delivery is accepted and
    // nothing fires.
    let workflow = "wf-stopped";
    given_event_trigger(&router, workflow, Some(json!({ "BatchSize": 1 }))).await;
    let stopped = ok(
        &router,
        "StopTrigger",
        &json!({ "Name": format!("{workflow}-evt") }),
    )
    .await;
    assert_eq!(
        stopped.get("Name").and_then(Value::as_str),
        Some(format!("{workflow}-evt").as_str()),
        "the stop answers with the trigger's name, got {stopped}"
    );
    let read = ok(
        &router,
        "GetTrigger",
        &json!({ "Name": format!("{workflow}-evt") }),
    )
    .await;
    assert_eq!(
        read["Trigger"].get("State").and_then(Value::as_str),
        Some(TRIGGER_STATE_DEACTIVATED),
        "the stopped trigger is deactivated, got {read}"
    );
    let outcome = deliver(&engine, workflow);
    assert_eq!(
        outcome,
        WorkflowEventDelivery::Deactivated,
        "case {:?}: expected the delivery to be accepted and not fire, got {outcome:?}",
        "a workflow whose EVENT trigger is stopped"
    );
    let runs = runs_of(&router, "a workflow whose EVENT trigger is stopped", workflow).await;
    assert!(
        runs.is_empty(),
        "case {:?}: a stopped trigger starts no run, got {runs:?}",
        "a workflow whose EVENT trigger is stopped"
    );
}
