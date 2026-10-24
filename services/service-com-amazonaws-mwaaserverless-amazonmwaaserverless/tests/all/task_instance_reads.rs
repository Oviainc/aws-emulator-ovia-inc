// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `GetTaskInstance`.
//!
//! The rule under test: `GetTaskInstance` reports the task-instance record the STORE
//! holds under the requested `TaskInstanceId` inside the requested run of the requested
//! workflow — every member from that record, the `WorkflowArn`/`RunId`/`WorkflowVersion`
//! from the run that owns it, `DurationInSeconds` derived from `StartedAt`/`EndedAt` and
//! null while the task is not complete — writing nothing; and a reference that resolves
//! to nothing (absent workflow, absent run, or a run holding no such instance) is the
//! modelled `ResourceNotFoundException`.
//!
//! No modelled operation of this service CREATES a task instance: one exists only once
//! something executes a workflow definition, and this emulator has no Apache Airflow
//! engine to execute one with, so every run it starts holds none. These tests therefore
//! seed the record into the store the handler reads (the same store `GetWorkflowRun`
//! reads) and drive the read over the real wire path, which is what this operation owns;
//! the absent-instance rows cover what a client actually sees today.
//!
//! The table spans what the rule generalises over: the `TaskInstanceStatus` value, which
//! optional members are present, whether the task has started/completed, the length of
//! the completed span, the number of `Xcom` entries and the form and length of the
//! `TaskInstanceId`.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use service_com_amazonaws_mwaaserverless_amazonmwaaserverless::state::{
    TaskInstance, UnresolvedEntity, LOOKUP_MISS_STATUS, RUN_STATUS_QUEUED,
    TASK_INSTANCE_STATUS_NONE,
};
use service_com_amazonaws_mwaaserverless_amazonmwaaserverless::ComAmazonawsMwaaserverlessAmazonmwaaserverlessState;

use crate::workflow_lifecycle::{role_arn, Harness};

/// Every `TaskInstanceStatus` value the model declares, in the model's own order
/// (`smithy_query(query="shape", shape="TaskInstanceStatus")`).
const TASK_INSTANCE_STATUSES: &[&str] = &[
    "QUEUED",
    "FAILED",
    "SCHEDULED",
    "RUNNING",
    "SUCCESS",
    "UP_FOR_RESCHEDULE",
    "UP_FOR_RETRY",
    "UPSTREAM_FAILED",
    "REMOVED",
    "RESTARTING",
    "DEFERRED",
    "NONE",
    "CANCELLED",
    "TIMEOUT",
];

/// A fixed instant plus `offset_millis`, so a seeded timestamp is deterministic and a
/// test never races the wall clock.
fn at(offset_millis: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_millis(1_700_000_000_000 + offset_millis)
}

/// The epoch seconds a `SystemTime` [`at`] built represents.
fn expected_seconds(offset_millis: u64) -> f64 {
    (1_700_000_000_000.0 + offset_millis as f64) / 1000.0
}

/// A timestamp member as epoch seconds, whichever wire form the protocol rendered:
/// a number, or the ISO 8601 string the member's `timestampFormat: date-time` names.
fn epoch_seconds(value: &serde_json::Value) -> Option<f64> {
    if let Some(number) = value.as_f64() {
        return Some(number);
    }
    let parsed = ::aws_smithy_types::DateTime::from_str(
        value.as_str()?,
        ::aws_smithy_types::date_time::Format::DateTime,
    )
    .ok()?;
    Some(parsed.as_secs_f64())
}

/// A task-instance record with every optional member empty: the state of an instance
/// that has not started.
fn instance(id: &str, status: &str) -> TaskInstance {
    TaskInstance {
        task_instance_id: id.to_string(),
        task_id: None,
        status: status.to_string(),
        operator_name: None,
        attempt_number: None,
        started_at: None,
        ended_at: None,
        modified_at: at(0),
        error_message: None,
        log_stream: None,
        xcom: BTreeMap::new(),
    }
}

/// One row: a seeded record, the offsets its timestamps were built from, and the
/// `DurationInSeconds` the reader must derive for it.
struct Case {
    name: String,
    task: TaskInstance,
    started_offset: Option<u64>,
    ended_offset: Option<u64>,
    modified_offset: u64,
    expected_duration: Option<i32>,
}

impl Case {
    fn new(name: &str, task: TaskInstance) -> Self {
        Self {
            name: name.to_string(),
            task,
            started_offset: None,
            ended_offset: None,
            modified_offset: 0,
            expected_duration: None,
        }
    }

    /// Give the record a start, an optional end, and the duration the span implies.
    fn span(mut self, started: u64, ended: Option<u64>, expected_duration: Option<i32>) -> Self {
        self.task.started_at = Some(at(started));
        self.started_offset = Some(started);
        self.task.ended_at = ended.map(at);
        self.ended_offset = ended;
        self.expected_duration = expected_duration;
        self
    }

    fn modified(mut self, offset: u64) -> Self {
        self.task.modified_at = at(offset);
        self.modified_offset = offset;
        self
    }
}

/// The rows. They differ along status, which optional members are set, whether the task
/// started or completed, the length of the span, the `Xcom` size and the id's form and
/// length — not merely along the id.
fn cases() -> Vec<Case> {
    // 255 characters, the maximum `IdString` allows
    // (`smithy_query(query="shape", shape="IdString")`: length 1..255).
    let longest_id = "t".repeat(255);

    let mut not_started = instance("t", TASK_INSTANCE_STATUS_NONE);
    not_started.task_id = Some("extract".to_string());

    let mut running = instance("extract.data_task-01", "RUNNING");
    running.task_id = Some("extract".to_string());
    running.operator_name = Some("PythonOperator".to_string());
    running.attempt_number = Some(1);
    running.log_stream = Some("extract.data_task-01.attempt-1".to_string());

    let mut succeeded = instance("transform-02", "SUCCESS");
    succeeded.task_id = Some("transform".to_string());
    succeeded.operator_name = Some("BashOperator".to_string());
    succeeded.attempt_number = Some(1);
    succeeded.xcom = BTreeMap::from([
        ("rows".to_string(), "4096".to_string()),
        ("sink".to_string(), "s3://bucket/key".to_string()),
    ]);

    let mut failed = instance("load-03", "FAILED");
    failed.task_id = Some("load".to_string());
    failed.operator_name = Some("SqlOperator".to_string());
    failed.attempt_number = Some(3);
    failed.error_message = Some("the load task exhausted its retries".to_string());
    failed.log_stream = Some("load-03.attempt-3".to_string());

    let mut upstream_failed = instance(&longest_id, "UPSTREAM_FAILED");
    upstream_failed.attempt_number = Some(i32::MAX);
    upstream_failed.xcom = BTreeMap::from([("reason".to_string(), "upstream".to_string())]);

    let deferred = instance("deferred.task_04", "DEFERRED");

    vec![
        // Not started: StartedAt, EndedAt and DurationInSeconds are all null.
        Case::new("not-started/no-optional-members", not_started),
        // Started, not complete: DurationInSeconds stays null.
        Case::new("running/started-not-complete", running).span(1_000, None, None),
        // Complete in exactly one second.
        Case::new("success/one-second-span", succeeded)
            .span(2_000, Some(3_000), Some(1))
            .modified(3_000),
        // A fractional span still reports the second it occupied (rounded up).
        Case::new("failed/sub-second-span", failed)
            .span(4_000, Some(4_001), Some(1))
            .modified(4_001),
        // A long span, and the longest id the model allows.
        Case::new("upstream-failed/hour-long-span", upstream_failed)
            .span(5_000, Some(3_605_000), Some(3600))
            .modified(3_605_000),
        // Ended at the very instant it started: a zero span is still "complete".
        Case::new("deferred/zero-span", deferred)
            .span(6_000, Some(6_000), Some(0))
            .modified(6_000),
    ]
}

/// Create a workflow and start a run under it, and report
/// `(WorkflowArn, RunId, WorkflowVersion)`.
async fn stored_run(harness: &Harness, name: &str) -> (String, String, String) {
    let (status, created) = harness
        .call(
            "CreateWorkflow",
            serde_json::json!({
                "Name": name,
                "DefinitionS3Location": {"Bucket": "defs", "ObjectKey": "dags/probe.py"},
                "RoleArn": role_arn(),
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "setup: expected CreateWorkflow to succeed, got {status} {created:?}"
    );
    let arn = created["WorkflowArn"]
        .as_str()
        .expect("setup: a created workflow has an ARN")
        .to_string();
    // The run executes the workflow's latest version, which for a freshly created
    // workflow is the one the create reported.
    let version = created["WorkflowVersion"]
        .as_str()
        .expect("setup: a created workflow has a version")
        .to_string();

    let (status, started) = harness
        .call("StartWorkflowRun", serde_json::json!({"WorkflowArn": arn}))
        .await;
    assert_eq!(
        status, 200,
        "setup: expected StartWorkflowRun to succeed, got {status} {started:?}"
    );
    let run_id = started["RunId"]
        .as_str()
        .expect("setup: a started run has a RunId")
        .to_string();
    (arn, run_id, version)
}

/// Put `task` into the run's store. Nothing in this service's API creates a task
/// instance (no Apache Airflow engine executes a definition here), so seeding the
/// store is the only way to exercise the read this operation owns.
fn seed(harness: &Harness, workflow_arn: &str, run_id: &str, task: TaskInstance) {
    harness
        .engine()
        .state::<ComAmazonawsMwaaserverlessAmazonmwaaserverlessState>()
        .lock()
        .workflows
        .get_mut(workflow_arn)
        .expect("setup: the seeded workflow is stored")
        .runs
        .get_mut(run_id)
        .expect("setup: the started run is stored")
        .task_instances
        .insert(task.task_instance_id.clone(), task);
}

/// Assert an optional string member: equal to `expected`, or absent when it is `None`.
fn assert_optional_text(
    body: &serde_json::Value,
    member: &str,
    expected: Option<&str>,
    case: &str,
) {
    match expected {
        Some(text) => assert_eq!(
            body[member].as_str(),
            Some(text),
            "case {case:?}: expected {member} {text:?}, got {:?}",
            body[member]
        ),
        None => assert!(
            body[member].is_null(),
            "case {case:?}: expected {member} absent, got {:?}",
            body[member]
        ),
    }
}

/// Assert a timestamp member: the instant `offset` names, or absent when it is `None`.
fn assert_optional_timestamp(
    body: &serde_json::Value,
    member: &str,
    offset: Option<u64>,
    case: &str,
) {
    match offset {
        Some(offset) => {
            let expected = expected_seconds(offset);
            let got = epoch_seconds(&body[member]);
            assert!(
                got.is_some_and(|seconds| (seconds - expected).abs() < 0.002),
                "case {case:?}: expected {member} at {expected} epoch seconds, got {:?}",
                body[member]
            );
        }
        None => assert!(
            body[member].is_null(),
            "case {case:?}: expected {member} absent, got {:?}",
            body[member]
        ),
    }
}

#[tokio::test]
async fn a_stored_task_instance_is_reported_member_for_member() {
    let harness = Harness::new("us-west-2");
    let (arn, run_id, version) = stored_run(&harness, "taskInstanceReads").await;

    for case in cases() {
        let Case {
            name,
            task,
            started_offset,
            ended_offset,
            modified_offset,
            expected_duration,
        } = case;
        let expected = task.clone();
        seed(&harness, &arn, &run_id, task);

        let (status, answered) = harness
            .call(
                "GetTaskInstance",
                serde_json::json!({
                    "WorkflowArn": arn,
                    "RunId": run_id,
                    "TaskInstanceId": expected.task_instance_id,
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected GetTaskInstance to succeed, got {status} {answered:?}"
        );

        // The three members the OWNING RUN decides, read from the run and not echoed
        // from the request.
        assert_eq!(
            answered["WorkflowArn"].as_str(),
            Some(arn.as_str()),
            "case {name:?}: expected the run's WorkflowArn, got {:?}",
            answered["WorkflowArn"]
        );
        assert_eq!(
            answered["RunId"].as_str(),
            Some(run_id.as_str()),
            "case {name:?}: expected the run's RunId, got {:?}",
            answered["RunId"]
        );
        assert_eq!(
            answered["WorkflowVersion"].as_str(),
            Some(version.as_str()),
            "case {name:?}: expected the version the run executes, got {:?}",
            answered["WorkflowVersion"]
        );

        assert_eq!(
            answered["TaskInstanceId"].as_str(),
            Some(expected.task_instance_id.as_str()),
            "case {name:?}: expected the stored TaskInstanceId, got {:?}",
            answered["TaskInstanceId"]
        );
        assert_eq!(
            answered["Status"].as_str(),
            Some(expected.status.as_str()),
            "case {name:?}: expected the stored Status, got {:?}",
            answered["Status"]
        );
        assert_optional_text(&answered, "TaskId", expected.task_id.as_deref(), &name);
        assert_optional_text(
            &answered,
            "OperatorName",
            expected.operator_name.as_deref(),
            &name,
        );
        assert_optional_text(
            &answered,
            "ErrorMessage",
            expected.error_message.as_deref(),
            &name,
        );
        assert_optional_text(
            &answered,
            "LogStream",
            expected.log_stream.as_deref(),
            &name,
        );
        assert_eq!(
            answered["AttemptNumber"].as_i64(),
            expected.attempt_number.map(i64::from),
            "case {name:?}: expected AttemptNumber {:?}, got {:?}",
            expected.attempt_number,
            answered["AttemptNumber"]
        );

        assert_optional_timestamp(&answered, "StartedAt", started_offset, &name);
        assert_optional_timestamp(&answered, "EndedAt", ended_offset, &name);
        assert_optional_timestamp(&answered, "ModifiedAt", Some(modified_offset), &name);

        // Derived, not stored: null for exactly the instances that have not completed.
        assert_eq!(
            answered["DurationInSeconds"].as_i64(),
            expected_duration.map(i64::from),
            "case {name:?}: expected DurationInSeconds {expected_duration:?}, got {:?}",
            answered["DurationInSeconds"]
        );

        if expected.xcom.is_empty() {
            assert!(
                answered["Xcom"].is_null(),
                "case {name:?}: expected Xcom absent for an instance that exchanged no \
                 cross-task data, got {:?}",
                answered["Xcom"]
            );
        } else {
            let xcom = answered["Xcom"].as_object().unwrap_or_else(|| {
                panic!("case {name:?}: expected an Xcom map, got {:?}", answered)
            });
            assert_eq!(
                xcom.len(),
                expected.xcom.len(),
                "case {name:?}: expected {} Xcom entries, got {xcom:?}",
                expected.xcom.len()
            );
            for (key, value) in &expected.xcom {
                assert_eq!(
                    xcom.get(key).and_then(serde_json::Value::as_str),
                    Some(value.as_str()),
                    "case {name:?}: expected Xcom[{key:?}] {value:?}, got {:?}",
                    xcom.get(key)
                );
            }
        }
    }
}

#[tokio::test]
async fn every_modelled_status_round_trips() {
    let harness = Harness::new("eu-west-1");
    let (arn, run_id, _) = stored_run(&harness, "taskInstanceStatuses").await;

    for (index, status) in TASK_INSTANCE_STATUSES.iter().enumerate() {
        let id = format!("status.{index}");
        seed(&harness, &arn, &run_id, instance(&id, status));
        let (http_status, answered) = harness
            .call(
                "GetTaskInstance",
                serde_json::json!({
                    "WorkflowArn": arn,
                    "RunId": run_id,
                    "TaskInstanceId": id,
                }),
            )
            .await;
        assert_eq!(
            http_status, 200,
            "case {status:?}: expected GetTaskInstance to succeed, got {http_status} {answered:?}"
        );
        assert_eq!(
            answered["Status"].as_str(),
            Some(*status),
            "case {status:?}: expected the stored Status, got {:?}",
            answered["Status"]
        );
    }
}

#[tokio::test]
async fn an_unresolvable_task_instance_is_a_lookup_miss_naming_it() {
    let harness = Harness::new("ap-south-1");
    let (arn, run_id, _) = stored_run(&harness, "taskInstanceMisses").await;
    seed(
        &harness,
        &arn,
        &run_id,
        instance("present.task-01", "SUCCESS"),
    );

    // A workflow ARN of the modelled form that no store ever held
    // (`smithy_query(query="shape", shape="WorkflowArn")`).
    let ghost_arn = "arn:aws:airflow-serverless:ap-south-1:999999999999:workflow/absent-0123456789";

    // Rows: which reference is absent, over `RunId`/`TaskInstanceId` values of
    // differing form and length (all valid `IdString`s).
    let rows: Vec<(&str, String, String, String)> = vec![
        (
            "absent-workflow",
            ghost_arn.to_string(),
            run_id.clone(),
            "present.task-01".to_string(),
        ),
        (
            "absent-run",
            arn.clone(),
            "run-absent0000000".to_string(),
            "present.task-01".to_string(),
        ),
        (
            "absent-instance/short-id",
            arn.clone(),
            run_id.clone(),
            "a".to_string(),
        ),
        (
            "absent-instance/punctuated-id",
            arn.clone(),
            run_id.clone(),
            "absent.task_09-XYZ".to_string(),
        ),
        (
            "absent-instance/longest-id",
            arn.clone(),
            run_id.clone(),
            format!("{}0123456789", "a".repeat(245)),
        ),
    ];

    for (name, workflow_arn, run, task_instance_id) in rows {
        let (status, answered) = harness
            .call(
                "GetTaskInstance",
                serde_json::json!({
                    "WorkflowArn": workflow_arn,
                    "RunId": run,
                    "TaskInstanceId": task_instance_id,
                }),
            )
            .await;
        assert_eq!(
            status, LOOKUP_MISS_STATUS,
            "case {name:?}: expected HTTP {LOOKUP_MISS_STATUS}, got {status} {answered:?}"
        );
        assert!(
            answered["__type"]
                .as_str()
                .is_some_and(|kind| kind.contains("ResourceNotFoundException")),
            "case {name:?}: expected a ResourceNotFoundException, got {:?}",
            answered["__type"]
        );
        assert_eq!(
            answered["Message"].as_str(),
            Some(UnresolvedEntity::TaskInstance.message()),
            "case {name:?}: expected the message naming the entity this operation \
             resolves, got {:?}",
            answered["Message"]
        );
        assert_eq!(
            answered["ResourceId"].as_str(),
            Some(task_instance_id.as_str()),
            "case {name:?}: expected ResourceId to name the task instance looked up, \
             got {:?}",
            answered["ResourceId"]
        );
        assert_eq!(
            answered["ResourceType"].as_str(),
            Some(UnresolvedEntity::TaskInstance.resource_type()),
            "case {name:?}: expected the task-instance ResourceType, got {:?}",
            answered["ResourceType"]
        );
    }

    // The task-instance refusal stays tellable apart from every other entity's.
    for other in [
        UnresolvedEntity::Workflow,
        UnresolvedEntity::LatestWorkflowVersion,
        UnresolvedEntity::NamedWorkflowVersion,
        UnresolvedEntity::WorkflowRun,
    ] {
        assert_ne!(
            UnresolvedEntity::TaskInstance.message(),
            other.message(),
            "case entity messages: the task-instance message must differ from {other:?}'s"
        );
        assert_ne!(
            UnresolvedEntity::TaskInstance.resource_type(),
            other.resource_type(),
            "case entity types: the task-instance ResourceType must differ from {other:?}'s"
        );
    }

    // The instance that IS stored still reads, after all those refusals.
    let (status, answered) = harness
        .call(
            "GetTaskInstance",
            serde_json::json!({
                "WorkflowArn": arn,
                "RunId": run_id,
                "TaskInstanceId": "present.task-01",
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case stored instance: expected GetTaskInstance to succeed, got {status} {answered:?}"
    );

    // And once the workflow holding the run is gone, so is the instance.
    let (status, deleted) = harness
        .call("DeleteWorkflow", serde_json::json!({"WorkflowArn": arn}))
        .await;
    assert_eq!(
        status, 200,
        "setup: expected DeleteWorkflow to succeed, got {status} {deleted:?}"
    );
    let (status, answered) = harness
        .call(
            "GetTaskInstance",
            serde_json::json!({
                "WorkflowArn": arn,
                "RunId": run_id,
                "TaskInstanceId": "present.task-01",
            }),
        )
        .await;
    assert_eq!(
        status, LOOKUP_MISS_STATUS,
        "case after-delete: expected HTTP {LOOKUP_MISS_STATUS}, got {status} {answered:?}"
    );
}

#[tokio::test]
async fn reading_a_task_instance_changes_nothing_a_later_caller_sees() {
    let harness = Harness::new("us-east-1");
    let (arn, run_id, _) = stored_run(&harness, "taskInstanceReporting").await;

    // Two instances, so the id list a later reader sees has an order to preserve.
    let ids = ["b.second-task", "a.first-task"];
    for id in ids {
        seed(&harness, &arn, &run_id, instance(id, "SCHEDULED"));
    }

    let request = serde_json::json!({
        "WorkflowArn": arn,
        "RunId": run_id,
        "TaskInstanceId": ids[0],
    });
    let (first_status, first) = harness.call("GetTaskInstance", request.clone()).await;
    let (second_status, second) = harness.call("GetTaskInstance", request).await;
    assert_eq!(
        (first_status, second_status),
        (200, 200),
        "case repeated read: expected both reads to succeed, got {first_status} {first:?} \
         then {second_status} {second:?}"
    );
    assert_eq!(
        first, second,
        "case repeated read: a reporting operation must answer identically, got {first:?} \
         then {second:?}"
    );

    // The read moved no run status and consumed nothing: the run is still the `QUEUED`
    // its start left it in, and it still holds exactly the seeded instances.
    let (run_state, error_message, stored_ids) = {
        let state = harness
            .engine()
            .state::<ComAmazonawsMwaaserverlessAmazonmwaaserverlessState>();
        let store = state.lock();
        let run = store
            .workflows
            .get(&arn)
            .and_then(|workflow| workflow.runs.get(&run_id))
            .expect("the started run is stored");
        (
            run.status.clone(),
            run.error_message.clone(),
            run.task_instance_ids(),
        )
    };
    assert_eq!(
        run_state, RUN_STATUS_QUEUED,
        "case reporting read: GetTaskInstance must not move the run's status, got \
         {run_state:?}"
    );
    assert_eq!(
        error_message, None,
        "case reporting read: GetTaskInstance must not fail the run, got {error_message:?}"
    );
    let mut expected_ids: Vec<String> = ids.iter().map(|id| (*id).to_string()).collect();
    expected_ids.sort();
    assert_eq!(
        stored_ids, expected_ids,
        "case reporting read: the run must still hold exactly the seeded instances, \
         got {stored_ids:?}"
    );

    // The other reader of the same records agrees: `GetWorkflowRun` lists those ids.
    let (http_status, run) = harness
        .call(
            "GetWorkflowRun",
            serde_json::json!({"WorkflowArn": arn, "RunId": run_id}),
        )
        .await;
    assert_eq!(
        http_status, 200,
        "case cross-reader: expected GetWorkflowRun to succeed, got {http_status} {run:?}"
    );
    let listed: Vec<String> = run["RunDetail"]["TaskInstances"]
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| entry.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        listed, expected_ids,
        "case cross-reader: GetWorkflowRun must list the ids GetTaskInstance reads, \
         got {listed:?}"
    );
}
