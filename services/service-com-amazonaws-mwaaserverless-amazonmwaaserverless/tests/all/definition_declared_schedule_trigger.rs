// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for WHERE a workflow's automated trigger comes from.
//!
//! The rule under test: the service configures a workflow's automated trigger from the
//! schedule information its ACTIVE version's snapshotted YAML definition declares —
//! "when a workflow includes schedule information in its YAML definition, the service
//! automatically configures the appropriate triggers for automated execution"
//! (`smithy_query(query="doc", shape="GetWorkflowResponse",
//! member="ScheduleConfiguration")`) — so a workflow whose definition declares a cron
//! schedule is executed by the service itself and reports runs no caller started, while
//! one whose definition declares none, or holds no definition at all, is only ever
//! started by a caller. `TriggerMode` is a free string the model documents no value for
//! (`GenericString`, length 1..255), so it is carried and reported but decides no
//! trigger: the same definition behaves the same way under every mode, and no mode makes
//! a workflow without declared schedule information fire.
//!
//! The table spans what the rule generalises over: WHERE the schedule is declared (the
//! document root, the `dag` mapping, below the DAG level, nowhere), HOW it is spelled (a
//! five-field expression, a step expression, the former `schedule_interval` key, an
//! Airflow preset, prose that is no cron expression), the FORM of the definition (block
//! YAML, JSON) and whether there is a definition object to snapshot at all — crossed with
//! the `TriggerMode` values a caller may supply, including none.
//!
//! Every request goes over the real wire path through the same router the emulator boots,
//! with the Amazon S3 service mounted beside this one on ONE shared engine, because a
//! definition is snapshotted by calling S3 in process exactly as the assembled emulator
//! does. A firing instant is a stored clock field, so the tests seed it from the store
//! rather than racing the wall clock, and one test watches a real workflow fire on the
//! real clock.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_mwaaserverless_amazonmwaaserverless::state::{
    Workflow, RUN_TYPE_ON_DEMAND, RUN_TYPE_SCHEDULED, SCHEDULED_RUN_EVERY,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::workflow_definition_snapshot::{
    call, create_bucket, create_workflow_with_mode, definition_router_on, stage,
    with_stored_workflow, BUCKET,
};
use crate::workflow_lifecycle::role_arn;

/// One row of the declaration table: the definition staged for the workflow — `None` for
/// a row staging no object at all, so the version snapshots no definition — and the cron
/// expression the service holds for it, which is also what decides the trigger.
struct Row {
    case: &'static str,
    key: &'static str,
    definition: Option<&'static str>,
    cron: Option<&'static str>,
}

/// The rows. Every cron expression and preset below is read the way the definition reader
/// documents it (`crate::state::workflow_definition::declared_schedule`: the DAG-level
/// `schedule` / `schedule_interval` keys, at the document root or in the `dag` mapping,
/// with Apache Airflow's preset shorthands resolved).
const ROWS: &[Row] = &[
    Row {
        case: "a five-field cron expression at the document root",
        key: "dags/root.yaml",
        definition: Some("name: payroll\nschedule: \"30 4 * * *\"\ntasks:\n  - id: extract\n"),
        cron: Some("30 4 * * *"),
    },
    Row {
        case: "a cron expression inside the dag mapping",
        key: "dags/nested.yaml",
        definition: Some("dag:\n  dag_id: nested\n  schedule: 15 3 * * 1\ntasks: []\n"),
        cron: Some("15 3 * * 1"),
    },
    Row {
        case: "a step expression under the former schedule_interval key",
        key: "dags/legacy.yaml",
        definition: Some("schedule_interval: \"*/20 * * * *\"\ntasks:\n  - id: poll\n"),
        cron: Some("*/20 * * * *"),
    },
    Row {
        case: "an Airflow preset",
        key: "dags/preset.yaml",
        definition: Some("schedule: \"@monthly\"\ntasks: []\n"),
        cron: Some("0 0 1 * *"),
    },
    Row {
        case: "a definition written as JSON",
        key: "dags/flow.json",
        definition: Some("{\"schedule\": \"0 */2 * * *\", \"tasks\": []}"),
        cron: Some("0 */2 * * *"),
    },
    Row {
        case: "no schedule declared at all",
        key: "dags/manual.yaml",
        definition: Some("name: manual\ntasks:\n  - id: load\n"),
        cron: None,
    },
    Row {
        case: "a schedule value that is not a cron expression",
        key: "dags/prose.yaml",
        definition: Some("schedule: whenever you like\ntasks: []\n"),
        cron: None,
    },
    Row {
        case: "a schedule key below the DAG level",
        key: "dags/task-level.yaml",
        definition: Some("tasks:\n  - id: t\n    schedule: 0 1 * * *\n"),
        cron: None,
    },
    Row {
        case: "no definition object to snapshot",
        key: "dags/absent.yaml",
        definition: None,
        cron: None,
    },
];

/// The `TriggerMode` values the rows are crossed with: none at all, the one value this
/// service once decided the trigger from, the mode naming caller-started execution, a
/// mode the service does not define, and the shortest value the member allows.
/// `TriggerMode` is `@length(1..255)`, pattern `^.*$`
/// (`smithy_query(query="operation", operation="CreateWorkflow")`), and the model
/// documents it only as "the trigger mode for the workflow execution" — no value of it
/// may change which workflows the service executes.
const TRIGGER_MODES: &[Option<&str>] = &[
    None,
    Some("SCHEDULED"),
    Some("ON_DEMAND"),
    Some("EVENT"),
    Some("M"),
];

/// The instant the store says this workflow's trigger fires next, and `None` for a
/// workflow the service does not execute on its own.
///
/// `pub(crate)` with the two helpers below, so a suite whose rule is about WHICH
/// declarations arm the trigger drives the same store and the same wire path rather than
/// building a second harness.
pub(crate) fn trigger(engine: &Arc<engine::InMemoryEngine>, arn: &str) -> Option<SystemTime> {
    with_stored_workflow(engine, arn, |workflow: &mut Workflow| {
        workflow.next_scheduled_run_at
    })
}

/// Put the workflow's next firing half a period in the past, so exactly one firing is
/// owed at the moment of the call that follows, and answer that firing's instant. A
/// workflow with no trigger has nothing to owe.
pub(crate) fn owe_one_firing(engine: &Arc<engine::InMemoryEngine>, arn: &str) -> Option<SystemTime> {
    let owed = SystemTime::now() - SCHEDULED_RUN_EVERY / 2;
    with_stored_workflow(engine, arn, |workflow: &mut Workflow| {
        workflow.next_scheduled_run_at = workflow.next_scheduled_run_at.map(|_| owed);
        workflow.next_scheduled_run_at
    })
}

/// The `WorkflowRuns` a listing reports for a workflow, over the wire.
pub(crate) async fn listed_runs(router: &MultiServiceRouter, arn: &str, case: &str) -> Vec<Value> {
    let (status, listed) = call(router, "ListWorkflowRuns", json!({"WorkflowArn": arn})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected ListWorkflowRuns to succeed, got {status} {listed:?}"
    );
    listed["WorkflowRuns"].as_array().cloned().unwrap_or_default()
}

#[tokio::test]
async fn the_definition_s_declared_schedule_decides_the_trigger_whatever_the_trigger_mode() {
    for row in ROWS {
        for mode in TRIGGER_MODES {
            // A fresh emulator per case, so one case's workflow cannot answer another's
            // read.
            let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
            let router = definition_router_on(Arc::clone(&engine));
            create_bucket(&router).await;
            if let Some(definition) = row.definition {
                stage(&router, row.key, definition.as_bytes()).await;
            }
            let case = format!("{} / TriggerMode {mode:?}", row.case);
            let arn = create_workflow_with_mode(&router, "declaredSchedule", row.key, *mode).await;

            // What the service HOLDS for this workflow: the schedule its definition
            // declares, reported by the same read that reports the trigger mode.
            let (status, fetched) = call(&router, "GetWorkflow", json!({"WorkflowArn": arn})).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected GetWorkflow to succeed, got {status} {fetched:?}"
            );
            assert_eq!(
                fetched["ScheduleConfiguration"]["CronExpression"].as_str(),
                row.cron,
                "case {case:?}: expected the declared cron expression {:?}, got {:?}",
                row.cron,
                fetched["ScheduleConfiguration"]
            );
            assert_eq!(
                fetched["TriggerMode"].as_str(),
                *mode,
                "case {case:?}: the trigger mode is carried and reported unchanged, got {:?}",
                fetched["TriggerMode"]
            );

            // …and therefore WHETHER the service executes it on its own.
            let automated = row.cron.is_some();
            let (created_at, first_firing) =
                with_stored_workflow(&engine, &arn, |workflow: &mut Workflow| {
                    (workflow.created_at, workflow.next_scheduled_run_at)
                });
            assert_eq!(
                first_firing.is_some(),
                automated,
                "case {case:?}: expected the service to {} this workflow on its own, got \
                 a next firing of {first_firing:?}",
                if automated { "execute" } else { "not execute" }
            );

            if let Some(firing) = first_firing {
                // No firing is instantaneous: the first one falls a whole period after
                // the create, so a caller reading straight back sees the workflow it just
                // made rather than a run it never asked for.
                assert!(
                    firing >= created_at + SCHEDULED_RUN_EVERY,
                    "case {case:?}: expected the first firing a full {SCHEDULED_RUN_EVERY:?} \
                     after the create, got {firing:?} against {created_at:?}"
                );
                // Park it beyond this test's horizon, so what the listing below reports is
                // decided by the trigger and not by how long this test took to get here.
                with_stored_workflow(&engine, &arn, |workflow: &mut Workflow| {
                    workflow.next_scheduled_run_at =
                        Some(SystemTime::now() + Duration::from_secs(3_600));
                });
            }
            let runs = listed_runs(&router, &arn, &case).await;
            assert!(
                runs.is_empty(),
                "case {case:?}: a workflow's history is empty until its trigger fires, \
                 got {runs:?}"
            );

            // One owed firing produces exactly one run nobody started — and a workflow
            // with no declared schedule has no firing to owe, so its history stays empty
            // however far the clock runs.
            let owed = owe_one_firing(&engine, &arn);
            assert_eq!(
                owed.is_some(),
                automated,
                "case {case:?}: only a workflow the service executes has a firing to owe"
            );
            let runs = listed_runs(&router, &arn, &case).await;
            assert_eq!(
                runs.len(),
                usize::from(automated),
                "case {case:?}: expected {} automated run(s), got {runs:?}",
                usize::from(automated)
            );
            if let Some(owed) = owed {
                let run = &runs[0];
                assert_eq!(
                    run["RunType"].as_str(),
                    Some(RUN_TYPE_SCHEDULED),
                    "case {case:?}: a run the workflow's own trigger started reports \
                     {RUN_TYPE_SCHEDULED}, got {:?}",
                    run["RunType"]
                );
                assert_eq!(
                    trigger(&engine, &arn),
                    Some(owed + SCHEDULED_RUN_EVERY),
                    "case {case:?}: a fired trigger moves on to its next firing, got {:?}",
                    trigger(&engine, &arn)
                );

                // ROUND TRIP: the run the trigger produced is readable through the reader
                // every other run is read through, reporting the same type.
                let run_id = run["RunId"].as_str().unwrap_or_default();
                let (status, read_back) = call(
                    &router,
                    "GetWorkflowRun",
                    json!({"WorkflowArn": arn, "RunId": run_id}),
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected GetWorkflowRun to succeed, got {status} \
                     {read_back:?}"
                );
                assert_eq!(
                    read_back["RunType"].as_str(),
                    Some(RUN_TYPE_SCHEDULED),
                    "case {case:?}: the reader reports the type the run carries, got {:?}",
                    read_back["RunType"]
                );
            }

            // A run the CALLER started is the caller's own under every definition and
            // every mode, so neither relabels an explicit start.
            let (status, started) =
                call(&router, "StartWorkflowRun", json!({"WorkflowArn": arn})).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected StartWorkflowRun to succeed, got {status} {started:?}"
            );
            let started_id = started["RunId"].as_str().unwrap_or_default().to_string();
            let (status, started) = call(
                &router,
                "GetWorkflowRun",
                json!({"WorkflowArn": arn, "RunId": started_id}),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected GetWorkflowRun to succeed, got {status} {started:?}"
            );
            assert_eq!(
                started["RunType"].as_str(),
                Some(RUN_TYPE_ON_DEMAND),
                "case {case:?}: a run the caller started reports {RUN_TYPE_ON_DEMAND}, got {:?}",
                started["RunType"]
            );
        }
    }
}

#[tokio::test]
async fn the_trigger_follows_the_definition_the_active_version_froze() {
    // The rows differ along what the UPDATE does to the declared schedule: adds one where
    // the create declared none, removes the one the create declared, replaces it with
    // another, and leaves a workflow declaring none without one.
    let rows = [
        (
            "a schedule added by the update",
            "tasks:\n  - id: only\n",
            false,
            "schedule: \"0 5 * * *\"\ntasks:\n  - id: only\n",
            true,
        ),
        (
            "the declared schedule removed by the update",
            "schedule: \"@hourly\"\ntasks: []\n",
            true,
            "tasks:\n  - id: only\n",
            false,
        ),
        (
            "one declared schedule replaced by another",
            "schedule: \"0 6 * * 5\"\ntasks: []\n",
            true,
            "schedule: \"45 1 * * *\"\ntasks: []\n",
            true,
        ),
        (
            "no schedule declared by either version",
            "name: manual\ntasks: []\n",
            false,
            "name: manual\ntasks:\n  - id: load\n",
            false,
        ),
    ];

    for (index, (case, first, first_automated, second, second_automated)) in
        rows.into_iter().enumerate()
    {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let router = definition_router_on(Arc::clone(&engine));
        create_bucket(&router).await;
        let key = format!("dags/followed-{index}.yaml");
        stage(&router, &key, first.as_bytes()).await;

        // The create carries the mode that once decided the trigger on its own, so each
        // row shows the DEFINITION deciding it against a constant mode.
        let arn = create_workflow_with_mode(&router, "followDefinition", &key, Some("SCHEDULED"))
            .await;
        assert_eq!(
            trigger(&engine, &arn).is_some(),
            first_automated,
            "case {case:?}: the created version's definition decides its trigger, got {:?}",
            trigger(&engine, &arn)
        );

        // A new version snapshots the object as it stands now, and the trigger in force is
        // that version's: "only one version of a workflow can have an active schedule at
        // any given time".
        stage(&router, &key, second.as_bytes()).await;
        let (status, updated) = call(
            &router,
            "UpdateWorkflow",
            json!({
                "WorkflowArn": arn,
                "DefinitionS3Location": {"Bucket": BUCKET, "ObjectKey": key},
                "RoleArn": role_arn(),
                "TriggerMode": "SCHEDULED",
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected UpdateWorkflow to succeed, got {status} {updated:?}"
        );
        let active_version = updated["WorkflowVersion"]
            .as_str()
            .expect("an update reports the version it appended")
            .to_string();
        assert_eq!(
            trigger(&engine, &arn).is_some(),
            second_automated,
            "case {case:?}: the promoted version's definition decides the trigger, got {:?}",
            trigger(&engine, &arn)
        );

        // What the trigger then produces is a run of the version that declared the
        // schedule, and a workflow with no trigger produces nothing at all.
        let runs_before = listed_runs(&router, &arn, case).await.len();
        owe_one_firing(&engine, &arn);
        let runs = listed_runs(&router, &arn, case).await;
        assert_eq!(
            runs.len(),
            runs_before + usize::from(second_automated),
            "case {case:?}: expected {} further automated run(s), got {runs:?}",
            usize::from(second_automated)
        );
        if second_automated {
            let latest = runs.last().expect("the listing reports the run just fired");
            assert_eq!(
                latest["WorkflowVersion"].as_str(),
                Some(active_version.as_str()),
                "case {case:?}: the firing runs the version whose definition declared the \
                 schedule, got {:?}",
                latest["WorkflowVersion"]
            );
        }
    }
}

#[tokio::test]
async fn a_workflow_whose_definition_declares_a_schedule_fires_on_the_real_clock() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = definition_router_on(Arc::clone(&engine));
    create_bucket(&router).await;
    stage(
        &router,
        "dags/declared.yaml",
        b"schedule: \"0 7 * * *\"\ntasks:\n  - id: extract\n",
    )
    .await;
    stage(&router, "dags/undeclared.yaml", b"tasks:\n  - id: extract\n").await;

    // Neither create names a TriggerMode: the definition is the whole input.
    let declared =
        create_workflow_with_mode(&router, "realDeclared", "dags/declared.yaml", None).await;
    let undeclared =
        create_workflow_with_mode(&router, "realUndeclared", "dags/undeclared.yaml", None).await;

    // Poll the way a client waiting for the schedule does. Nothing but the clock drives
    // the trigger, so the poll neither hurries it nor holds it back.
    let deadline = SystemTime::now() + Duration::from_secs(10);
    let mut produced: Vec<Value> = Vec::new();
    while SystemTime::now() < deadline {
        produced = listed_runs(&router, &declared, "real clock").await;
        if !produced.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        !produced.is_empty(),
        "case real clock: a workflow whose definition declares a schedule must report a \
         run within {:?} of being created, got none",
        Duration::from_secs(10)
    );
    for run in &produced {
        assert_eq!(
            run["RunType"].as_str(),
            Some(RUN_TYPE_SCHEDULED),
            "case real clock: nobody started this run, so it reports {RUN_TYPE_SCHEDULED}, \
             got {:?}",
            run["RunType"]
        );
        assert_eq!(
            run["WorkflowArn"].as_str(),
            Some(declared.as_str()),
            "case real clock: the run belongs to the workflow whose trigger fired, got {:?}",
            run["WorkflowArn"]
        );
    }

    let idle = listed_runs(&router, &undeclared, "real clock / no declaration").await;
    assert!(
        idle.is_empty(),
        "case real clock: a workflow whose definition declares no schedule produces no \
         run of its own, got {idle:?}"
    );
}
