// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the READERS of a run's task instances agreeing with one another:
//! `GetTaskInstance`, `ListTaskInstances` and `GetWorkflowRun`'s `RunDetail.TaskInstances`.
//!
//! The rule under test: the three readers report ONE set of records — the instances of the
//! tasks the run's definition declared — so every identifier `RunDetail.TaskInstances`
//! names is one the listing pages over and one `GetTaskInstance` serves with 200, each
//! summary agrees member for member with the instance read on its own, and two reads of
//! the same settled instance answer identically, because a reporting operation only
//! materialises what the clock already owed. It holds for a run whichever way the run came
//! about: one a caller started, and one the workflow's own trigger fired.
//!
//! What a run EXECUTES, and the outcome each instance therefore reports, is the subject of
//! `run_execution_outcome`; this suite is about the three readers of those records never
//! disagreeing, which is what a client inspecting a run's tasks depends on.
//!
//! Every test drives the real wire path through the same router the emulator boots, with
//! the Amazon S3 service mounted beside this one on ONE shared engine, because a run's
//! tasks come from the definition its version snapshotted out of Amazon S3.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_mwaaserverless_amazonmwaaserverless::state::{
    RUN_TYPE_SCHEDULED, SCHEDULED_RUN_EVERY,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::run_execution_outcome::{read_task_instance, run_to_completion};
use crate::workflow_definition_snapshot::{
    call, create_bucket, create_workflow, definition_router, stage,
};

/// The definition the started-run test stages: three tasks, one of which waits for
/// another, so the run holds instances at more than one dependency level and the readers
/// have a set to agree on rather than a single record. It declares no schedule, so only a
/// caller starts a run of it.
const DEFINITION: &str = "tasks:\n  - id: extract\n    operator: BashOperator\n  \
                          - id: transform\n    operator: PythonOperator\n    \
                          dependencies: [extract]\n  - id: publish\n";

/// The same three tasks, under a definition that DECLARES A SCHEDULE: "when a workflow
/// includes schedule information in its YAML definition, the service automatically
/// configures the appropriate triggers for automated execution"
/// (`smithy_query(query="shape", shape="GetWorkflowResponse")`, `ScheduleConfiguration`),
/// so the service fires runs of it with no caller asking.
const SCHEDULED_DEFINITION: &str = "schedule: \"@daily\"\ntasks:\n  - id: extract\n    \
                                    operator: BashOperator\n  - id: transform\n    \
                                    operator: PythonOperator\n    dependencies: \
                                    [extract]\n  - id: publish\n";

/// The `TaskId`s [`DEFINITION`] declares.
const DECLARED_TASKS: &[&str] = &["extract", "transform", "publish"];

/// Every `TaskInstanceSummary` `ListTaskInstances` reports for a run, keyed by the
/// identifier it names, walked through the cursors the listing issues so a run holding
/// more than one page is read whole.
async fn listed_instances(
    router: &MultiServiceRouter,
    workflow_arn: &str,
    run_id: &str,
    case: &str,
) -> BTreeMap<String, Value> {
    let mut summaries: BTreeMap<String, Value> = BTreeMap::new();
    let mut token: Option<String> = None;
    loop {
        let mut body = json!({"WorkflowArn": workflow_arn, "RunId": run_id, "MaxResults": 100});
        if let Some(cursor) = &token {
            body["NextToken"] = Value::String(cursor.clone());
        }
        let (status, page) = call(router, "ListTaskInstances", body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListTaskInstances to succeed, got {status} {page:?}"
        );
        let items = page["TaskInstances"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected a TaskInstances list, got {page:?}"))
            .clone();
        for item in items {
            let id = item["TaskInstanceId"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: every summary names its instance, got {item:?}")
                })
                .to_string();
            assert!(
                summaries.insert(id.clone(), item).is_none(),
                "case {case:?}: the listing must not report instance {id:?} twice"
            );
        }
        match page["NextToken"].as_str() {
            Some(cursor) => token = Some(cursor.to_string()),
            None => break,
        }
    }
    summaries
}

/// Assert the three readers of `run_id`'s task instances report ONE set of records, and
/// that reading one twice answers identically.
async fn assert_readers_agree(
    router: &MultiServiceRouter,
    workflow_arn: &str,
    run_id: &str,
    workflow_version: &str,
    case: &str,
) {
    let finished = run_to_completion(router, workflow_arn, run_id, case).await;
    let detailed: BTreeSet<String> = finished["RunDetail"]["TaskInstances"]
        .as_array()
        .map(|ids| {
            ids.iter()
                .filter_map(|id| id.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        detailed.len(),
        DECLARED_TASKS.len(),
        "case {case:?}: the run must name an instance per declared task, got {detailed:?}"
    );

    let summaries = listed_instances(router, workflow_arn, run_id, case).await;
    let listed: BTreeSet<String> = summaries.keys().cloned().collect();
    assert_eq!(
        listed, detailed,
        "case {case:?}: the listing and GetWorkflowRun must name the same instances, got \
         {listed:?} and {detailed:?}"
    );

    let mut tasks: BTreeSet<String> = BTreeSet::new();
    for (id, summary) in &summaries {
        let read = read_task_instance(router, workflow_arn, run_id, id, case).await;

        // The three members the OWNING RUN decides, read from the run by both readers
        // rather than echoed from the request.
        for (member, expected) in [
            ("WorkflowArn", workflow_arn),
            ("RunId", run_id),
            ("WorkflowVersion", workflow_version),
        ] {
            assert_eq!(
                read[member].as_str(),
                Some(expected),
                "case {case:?}/{id}: expected {member} {expected:?}, got {:?}",
                read[member]
            );
            assert_eq!(
                summary[member], read[member],
                "case {case:?}/{id}: the summary and the instance must agree on {member}, \
                 got {:?} and {:?}",
                summary[member], read[member]
            );
        }

        // Every member the SUMMARY carries is the one the instance itself reports, so a
        // client that pages and a client that reads see one record. `Status` and the
        // derived `DurationInSeconds` are asserted PRESENT first, so the agreement below
        // cannot be satisfied by two absent members.
        for member in ["Status", "DurationInSeconds"] {
            assert!(
                !read[member].is_null(),
                "case {case:?}/{id}: a settled instance reports {member}, got {:?}",
                read[member]
            );
        }
        for member in [
            "TaskInstanceId",
            "Status",
            "OperatorName",
            "DurationInSeconds",
        ] {
            assert_eq!(
                summary[member], read[member],
                "case {case:?}/{id}: the summary and the instance must agree on {member}, \
                 got {:?} and {:?}",
                summary[member], read[member]
            );
        }

        let task_id = read["TaskId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}/{id}: an instance names its task, got {read:?}"))
            .to_string();
        assert!(
            DECLARED_TASKS.contains(&task_id.as_str()),
            "case {case:?}/{id}: expected an instance of a declared task {DECLARED_TASKS:?}, \
             got {task_id:?}"
        );
        assert!(
            tasks.insert(task_id.clone()),
            "case {case:?}: the run must hold ONE instance per task, got two for {task_id:?}"
        );

        // A reporting operation changes nothing a later caller sees: the second read of a
        // settled instance answers exactly the same.
        let again = read_task_instance(router, workflow_arn, run_id, id, case).await;
        assert_eq!(
            again, read,
            "case {case:?}/{id}: a settled instance must not move between reads, got \
             {again:?} after {read:?}"
        );
    }
    let declared: BTreeSet<String> = DECLARED_TASKS.iter().map(|id| (*id).to_string()).collect();
    assert_eq!(
        tasks, declared,
        "case {case:?}: the instances must be of exactly the declared tasks, got {tasks:?}"
    );
}

/// The `WorkflowVersion` a workflow currently reports.
///
/// `pub(crate)` so another suite whose rule is about WHICH version a run executes reads
/// it through the same call rather than keeping a second copy of it.
pub(crate) async fn current_version(router: &MultiServiceRouter, arn: &str, case: &str) -> String {
    let (status, fetched) = call(router, "GetWorkflow", json!({"WorkflowArn": arn})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected GetWorkflow to succeed, got {status} {fetched:?}"
    );
    fetched["WorkflowVersion"]
        .as_str()
        .expect("a stored workflow reports its version")
        .to_string()
}

#[tokio::test]
async fn every_reader_of_a_started_run_s_task_instances_reports_the_same_records() {
    let case = "a run a caller started";
    let router = definition_router();
    create_bucket(&router).await;
    let key = "dags/readers.yaml";
    stage(&router, key, DEFINITION.as_bytes()).await;
    let arn = create_workflow(&router, "taskInstanceReaders", key).await;
    let version = current_version(&router, &arn, case).await;

    let (status, started) = call(&router, "StartWorkflowRun", json!({"WorkflowArn": arn})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected StartWorkflowRun to succeed, got {status} {started:?}"
    );
    let run_id = started["RunId"]
        .as_str()
        .expect("a started run has a RunId")
        .to_string();

    assert_readers_agree(&router, &arn, &run_id, &version, case).await;
}

#[tokio::test]
async fn every_reader_of_a_triggered_run_s_task_instances_reports_the_same_records() {
    let case = "a run the workflow's own trigger fired";
    let router = definition_router();
    create_bucket(&router).await;
    let key = "dags/triggered.yaml";
    stage(&router, key, SCHEDULED_DEFINITION.as_bytes()).await;

    // A workflow whose DEFINITION declares a schedule, which is what the service
    // configures a trigger from, so the run under test is one no caller started.
    let arn = create_workflow(&router, "triggeredTaskInstances", key).await;
    let version = current_version(&router, &arn, case).await;

    // Wait for the firing the trigger owes, which is what records a run of this kind.
    let deadline = SystemTime::now() + SCHEDULED_RUN_EVERY + Duration::from_secs(10);
    let mut run_id = None;
    while SystemTime::now() < deadline {
        let (status, listed) = call(&router, "ListWorkflowRuns", json!({"WorkflowArn": arn})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListWorkflowRuns to succeed, got {status} {listed:?}"
        );
        run_id = listed["WorkflowRuns"]
            .as_array()
            .and_then(|runs| {
                runs.iter()
                    .find(|run| run["RunType"].as_str() == Some(RUN_TYPE_SCHEDULED))
                    .and_then(|run| run["RunId"].as_str().map(str::to_string))
            });
        if run_id.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let run_id = run_id.unwrap_or_else(|| {
        panic!("case {case:?}: the trigger must have fired a run within {SCHEDULED_RUN_EVERY:?}")
    });

    assert_readers_agree(&router, &arn, &run_id, &version, case).await;
}
