// SPDX-License-Identifier: CC0-1.0

//! `CancelFlowExecutions`, end to end over HTTP.
//!
//! The rule under test: cancelling applies to the runs of the named flow that have not
//! finished — such a run moves to `CancelStarted` and the clock then owes it `Canceled`
//! — while every run id the request names that this flow cannot cancel (one it has
//! never recorded, one belonging to another flow, or one already in a terminal status)
//! comes back in `invalidExecutions`; a request that names no run at all ends every
//! unfinished run of that flow and reports none back.
//!
//! The harness (`call`, `Answer`, the store seeds) is the one `flow_lifecycle` already
//! drives the service with, so these cases reach the service exactly the way a client
//! does.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_appflow_sandstoneconfigurationservicelambda::state::{
    service_state, ExecutionStatus, EXECUTION_SETTLES_AFTER,
};
use smithy_server_runtime::MultiServiceRouter;

use super::flow_lifecycle::{
    appflow, appflow_with_engine, call, map_task, on_demand_trigger, s3_destination, s3_source,
    stored_execution, stored_flow, Answer, AFTER_RUN_SETTLES,
};

/// The run ids a served answer reported back as ones it could not cancel, sorted — the
/// model declares `invalidExecutions` a list and says nothing about its order. An absent
/// member is an empty list: no run was reported back.
fn invalid_executions(answer: &Answer) -> Vec<String> {
    let mut reported: Vec<String> = answer
        .get("invalidExecutions")
        .as_array()
        .map(|ids| {
            ids.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    reported.sort();
    reported
}

/// The status of each of a flow's runs, keyed by run id, read back through
/// `DescribeFlowExecutionRecords` — the operation a client reads a run's status with, so
/// the status asserted here is the one a client sees.
async fn run_statuses(router: &MultiServiceRouter, flow_name: &str) -> BTreeMap<String, String> {
    let records = call(
        router,
        "/describe-flow-execution-records",
        &json!({ "flowName": flow_name }),
    )
    .await;
    assert!(
        records.is_ok(),
        "expected DescribeFlowExecutionRecords for {flow_name:?} to succeed, got {} {:?}",
        records.status,
        records.body
    );
    records
        .get("flowExecutions")
        .as_array()
        .map(|runs| {
            runs.iter()
                .filter_map(|run| {
                    let id = run.get("executionId").and_then(Value::as_str)?;
                    let status = run.get("executionStatus").and_then(Value::as_str)?;
                    Some((id.to_string(), status.to_string()))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The definition every flow in these cases is created from: on-demand, S3 to S3, one
/// field mapping — the cancellation rule is about a flow's RUNS, not its definition.
fn flow_definition(flow_name: &str) -> Value {
    json!({
        "flowName": flow_name,
        "triggerConfig": on_demand_trigger(),
        "sourceFlowConfig": s3_source("cancel-in"),
        "destinationFlowConfigList": [s3_destination("cancel-out")],
        "tasks": [map_task("id", "id")]
    })
}

/// One row of the cancellation table: the status a recorded run carries when the
/// cancellation arrives, and whether this flow can cancel a run in that status.
struct RunCase {
    execution_id: &'static str,
    status: ExecutionStatus,
    cancellable: bool,
}

/// The cancellation table. Its rows differ along the one thing the rule generalises
/// over: the status the run carries — every value the model's `ExecutionStatus` enum
/// declares (`smithy_query(query="shape", shape="ExecutionStatus")`), so the table spans
/// the class rather than the single status the recorded case happened to carry.
fn run_cases() -> Vec<RunCase> {
    vec![
        RunCase {
            execution_id: "run-in-progress",
            status: ExecutionStatus::InProgress,
            cancellable: true,
        },
        RunCase {
            execution_id: "run-cancel-started",
            status: ExecutionStatus::CancelStarted,
            cancellable: true,
        },
        RunCase {
            execution_id: "run-canceled",
            status: ExecutionStatus::Canceled,
            cancellable: false,
        },
        RunCase {
            execution_id: "run-errored",
            status: ExecutionStatus::Error,
            cancellable: false,
        },
        RunCase {
            execution_id: "run-successful",
            status: ExecutionStatus::Successful,
            cancellable: false,
        },
    ]
}

/// A run id no flow in the store has ever recorded.
const UNRECORDED_RUN: &str = "run-that-was-never-recorded";

/// The run of a SECOND flow, which the first flow's cancellation must neither cancel nor
/// claim as its own.
const BYSTANDER_RUN: &str = "run-of-another-flow";

#[tokio::test]
async fn cancelling_named_runs_cancels_the_unfinished_ones_and_reports_the_rest() {
    let (router, engine) = appflow_with_engine();
    let flow_name = "cancel-table-flow";
    let bystander_flow = "cancel-bystander-flow";

    // Seeded straight into the store rather than created over HTTP: a flow can hold only
    // ONE run in progress when it is driven through `StartFlow`, and what this table is
    // about is the status a run carries when the cancellation reaches it.
    {
        let state = service_state(&*engine).expect("the AppFlow store is on this engine");
        let mut store = state.lock();
        let first_started = SystemTime::now();

        let mut flow = stored_flow(flow_name);
        for (index, case) in run_cases().iter().enumerate() {
            let mut execution = stored_execution(
                case.execution_id,
                first_started + Duration::from_millis(index as u64),
            );
            execution.execution_status = case.status;
            // An unfinished run is exactly one the clock still owes a terminal status.
            execution.settles_at = case
                .cancellable
                .then(|| SystemTime::now() + EXECUTION_SETTLES_AFTER);
            flow.record_execution(execution);
        }
        store.flows.insert(flow_name.to_string(), flow);

        let mut bystander = stored_flow(bystander_flow);
        let mut running = stored_execution(BYSTANDER_RUN, first_started);
        running.execution_status = ExecutionStatus::InProgress;
        running.settles_at = Some(SystemTime::now() + EXECUTION_SETTLES_AFTER);
        bystander.record_execution(running);
        store.flows.insert(bystander_flow.to_string(), bystander);
    }

    let named: Vec<String> = run_cases()
        .iter()
        .map(|case| case.execution_id.to_string())
        .chain([UNRECORDED_RUN.to_string(), BYSTANDER_RUN.to_string()])
        .collect();
    let cancelled = call(
        &router,
        "/cancel-flow-executions",
        &json!({ "flowName": flow_name, "executionIds": named }),
    )
    .await;
    assert!(
        cancelled.is_ok(),
        "expected CancelFlowExecutions to be served, got {} {:?}",
        cancelled.status,
        cancelled.body
    );

    let mut expected_invalid: Vec<String> = run_cases()
        .iter()
        .filter(|case| !case.cancellable)
        .map(|case| case.execution_id.to_string())
        .chain([UNRECORDED_RUN.to_string(), BYSTANDER_RUN.to_string()])
        .collect();
    expected_invalid.sort();
    assert_eq!(
        invalid_executions(&cancelled),
        expected_invalid,
        "case {:?}: expected exactly the runs this flow could not cancel to be reported back, got {:?}",
        "every run status, plus an unrecorded run and another flow's run",
        cancelled.get("invalidExecutions")
    );

    // Every run the request could cancel has STARTED cancelling; every other run is
    // untouched.
    let statuses = run_statuses(&router, flow_name).await;
    for case in run_cases() {
        let expected = if case.cancellable {
            ExecutionStatus::CancelStarted
        } else {
            case.status
        };
        assert_eq!(
            statuses.get(case.execution_id).map(String::as_str),
            Some(expected.as_str()),
            "case {:?}: expected the {:?} run to read as {}, got {:?}",
            case.execution_id,
            case.status.as_str(),
            expected.as_str(),
            statuses.get(case.execution_id)
        );
    }

    // The other flow's run belongs to the other flow: naming it changed nothing.
    let bystander_statuses = run_statuses(&router, bystander_flow).await;
    assert_eq!(
        bystander_statuses.get(BYSTANDER_RUN).map(String::as_str),
        Some(ExecutionStatus::InProgress.as_str()),
        "case {:?}: expected another flow's run to keep running, got {:?}",
        BYSTANDER_RUN,
        bystander_statuses.get(BYSTANDER_RUN)
    );

    // Asking again for the runs whose cancellation is under way is the same request
    // repeated, so none of them is reported back as one this flow cannot cancel.
    let under_way: Vec<String> = run_cases()
        .iter()
        .filter(|case| case.cancellable)
        .map(|case| case.execution_id.to_string())
        .collect();
    let repeated = call(
        &router,
        "/cancel-flow-executions",
        &json!({ "flowName": flow_name, "executionIds": under_way }),
    )
    .await;
    assert_eq!(
        invalid_executions(&repeated),
        Vec::<String>::new(),
        "case {:?}: expected a repeated cancellation to report nothing back, got {:?}",
        "the runs already cancelling",
        repeated.get("invalidExecutions")
    );

    // The clock owed each cancelling run its `Canceled` status; the next read
    // materialises it, and the runs nobody cancelled stay where they were.
    std::thread::sleep(AFTER_RUN_SETTLES);
    let settled = run_statuses(&router, flow_name).await;
    for case in run_cases() {
        let expected = if case.cancellable {
            ExecutionStatus::Canceled
        } else {
            case.status
        };
        assert_eq!(
            settled.get(case.execution_id).map(String::as_str),
            Some(expected.as_str()),
            "case {:?}: expected the settled run to read as {}, got {:?}",
            case.execution_id,
            expected.as_str(),
            settled.get(case.execution_id)
        );
    }
}

#[tokio::test]
async fn cancelling_without_naming_runs_ends_every_unfinished_run_of_that_flow() {
    let router = appflow();
    let cancelled_flow = "cancel-all-flow";
    let untouched_flow = "cancel-all-bystander-flow";

    let mut runs: BTreeMap<&str, String> = BTreeMap::new();
    for flow_name in [cancelled_flow, untouched_flow] {
        let created = call(&router, "/create-flow", &flow_definition(flow_name)).await;
        assert!(
            created.is_ok(),
            "case {flow_name:?}: expected CreateFlow to succeed, got {} {:?}",
            created.status,
            created.body
        );
        let started = call(
            &router,
            "/start-flow",
            &json!({ "flowName": flow_name }),
        )
        .await;
        assert!(
            started.is_ok(),
            "case {flow_name:?}: expected StartFlow to succeed, got {} {:?}",
            started.status,
            started.body
        );
        runs.insert(flow_name, started.string("executionId").to_string());
    }

    let cancelled = call(
        &router,
        "/cancel-flow-executions",
        &json!({ "flowName": cancelled_flow }),
    )
    .await;
    assert!(
        cancelled.is_ok(),
        "expected a cancellation that names no run to be served, got {} {:?}",
        cancelled.status,
        cancelled.body
    );
    assert_eq!(
        invalid_executions(&cancelled),
        Vec::<String>::new(),
        "case {:?}: a request that names no run has none to report back, got {:?}",
        "executionIds omitted",
        cancelled.get("invalidExecutions")
    );

    let cancelling = run_statuses(&router, cancelled_flow).await;
    assert_eq!(
        cancelling.get(&runs[cancelled_flow]).map(String::as_str),
        Some(ExecutionStatus::CancelStarted.as_str()),
        "case {:?}: expected the flow's own run to start cancelling, got {:?}",
        cancelled_flow,
        cancelling.get(&runs[cancelled_flow])
    );
    let untouched = run_statuses(&router, untouched_flow).await;
    assert_eq!(
        untouched.get(&runs[untouched_flow]).map(String::as_str),
        Some(ExecutionStatus::InProgress.as_str()),
        "case {:?}: expected another flow's run to keep running, got {:?}",
        untouched_flow,
        untouched.get(&runs[untouched_flow])
    );

    // Each run ends in the status the clock owed it: the cancelled one is `Canceled`,
    // and the one nobody cancelled reaches its own terminal status instead.
    std::thread::sleep(AFTER_RUN_SETTLES);
    let settled_cancelled = run_statuses(&router, cancelled_flow).await;
    assert_eq!(
        settled_cancelled
            .get(&runs[cancelled_flow])
            .map(String::as_str),
        Some(ExecutionStatus::Canceled.as_str()),
        "case {:?}: expected the cancellation to complete, got {:?}",
        cancelled_flow,
        settled_cancelled.get(&runs[cancelled_flow])
    );
    let settled_untouched = run_statuses(&router, untouched_flow).await;
    assert_eq!(
        settled_untouched
            .get(&runs[untouched_flow])
            .map(String::as_str),
        Some(ExecutionStatus::Successful.as_str()),
        "case {:?}: expected an uncancelled run to settle on its own terminal status, got {:?}",
        untouched_flow,
        settled_untouched.get(&runs[untouched_flow])
    );
}

#[tokio::test]
async fn cancelling_runs_a_flow_does_not_have() {
    let router = appflow();

    let unknown = call(
        &router,
        "/cancel-flow-executions",
        &json!({ "flowName": "no-such-flow", "executionIds": [UNRECORDED_RUN] }),
    )
    .await;
    assert_eq!(
        (unknown.status, unknown.error.as_str()),
        (404, "ResourceNotFoundException"),
        "case {:?}: expected a flow that does not exist to be refused, got {} {:?}",
        "an unknown flow",
        unknown.status,
        unknown.body
    );

    let flow_name = "never-run-flow";
    let created = call(&router, "/create-flow", &flow_definition(flow_name)).await;
    assert!(
        created.is_ok(),
        "expected CreateFlow to succeed, got {} {:?}",
        created.status,
        created.body
    );

    let named = call(
        &router,
        "/cancel-flow-executions",
        &json!({ "flowName": flow_name, "executionIds": [UNRECORDED_RUN, BYSTANDER_RUN] }),
    )
    .await;
    assert!(
        named.is_ok(),
        "expected naming runs a flow never recorded to be served, got {} {:?}",
        named.status,
        named.body
    );
    let mut expected_invalid = vec![UNRECORDED_RUN.to_string(), BYSTANDER_RUN.to_string()];
    expected_invalid.sort();
    assert_eq!(
        invalid_executions(&named),
        expected_invalid,
        "case {:?}: expected every named run to be reported back, got {:?}",
        "a flow with no runs at all",
        named.get("invalidExecutions")
    );

    let unnamed = call(
        &router,
        "/cancel-flow-executions",
        &json!({ "flowName": flow_name }),
    )
    .await;
    assert!(
        unnamed.is_ok(),
        "expected cancelling a flow with nothing running to be served, got {} {:?}",
        unnamed.status,
        unnamed.body
    );
    assert_eq!(
        invalid_executions(&unnamed),
        Vec::<String>::new(),
        "case {:?}: expected nothing to be reported back, got {:?}",
        "a flow with no runs and no named run",
        unnamed.get("invalidExecutions")
    );
}
