// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what a FIRED Glue trigger initiates: the job runs its actions
//! begin, read back through `GetJobRun` and `GetJobRuns`.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler.
//!
//! The rule under test: a trigger that FIRES initiates every action its definition names —
//! "Defines an action to be initiated by a trigger" (`Action`) — so each action naming a job
//! begins a run of that job, named as the run of that trigger (`JobRun$TriggerName`) and
//! carrying the overrides the action names over the job definition ("The job arguments used
//! when this trigger fires. For this job run, they replace the default arguments set in the
//! job definition itself" — `Action$Arguments`, and the same for `Timeout`,
//! `SecurityConfiguration` and `NotificationProperty`). A trigger that waits on something
//! outside the caller initiates nothing until that happens, and the ceiling a direct start
//! respects (`ExecutionProperty$MaxConcurrentRuns`) bounds a firing too.
//!
//! The tables are rows along what the rule generalises over: how many actions a trigger
//! names and of which kind (a job, a crawler, several of each), which overrides an action
//! carries, the trigger's type, whether it belongs to a workflow, which call fires it (the
//! trigger's own start or a run of the workflow it is the entry point of), and the
//! concurrency the job it names allows.
//!
//! The due instants the run lifecycle turns on are moved THROUGH THE STORE rather than
//! slept for, so a count is of what the firings recorded and not of what the clock settled.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::state::{
    JOB_RUN_ID_PREFIX, JOB_RUN_STATE_FAILED, JOB_RUN_STATE_STARTING,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::job_runs::{already_past, emulator_with_engine, far_future, job_body, set_due_instant};
use crate::wire::{call_in_region, ok, REGION};

/// The crawler an action can name instead of a job.
const CRAWLER: &str = "action-run-crawler";

/// Register `jobs` and the crawler an action can name, so the actions below resolve.
/// `extra` is applied to every `CreateJob` body, so a row can give its jobs a concurrency
/// ceiling or default arguments.
async fn register_targets(router: &MultiServiceRouter, jobs: &[&str], extra: Value) {
    for job in jobs {
        ok(router, "CreateJob", &job_body(job, extra.clone())).await;
    }
    ok(
        router,
        "CreateCrawler",
        &json!({
            "Name": CRAWLER,
            "Role": "arn:aws:iam::000000000000:role/action-run-crawler-role",
            "Targets": { "S3Targets": [{ "Path": "s3://action-runs/data/" }] },
        }),
    )
    .await;
}

/// The runs `GetJobRuns` reports for `job`, newest first.
pub(crate) async fn job_runs(router: &MultiServiceRouter, job: &str) -> Vec<Value> {
    let page = ok(router, "GetJobRuns", &json!({ "JobName": job })).await;
    page.get("JobRuns")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// Fire `trigger` through `StartTrigger`, asserting the start was served.
async fn start_trigger(router: &MultiServiceRouter, case: &str, trigger: &str) {
    let (status, body) =
        call_in_region(router, REGION, "StartTrigger", &json!({ "Name": trigger })).await;
    assert_eq!(
        status, 200,
        "case {case:?}: StartTrigger must answer 200, got {status} with {body}"
    );
}

// ---------------------------------------------------------------------------
// What a firing initiates
// ---------------------------------------------------------------------------

/// A firing begins one run of every job its actions name, and nothing for an action that
/// names no job; a trigger that waits on something outside the caller begins none.
///
/// Each row is a trigger definition plus, per job, how many runs one start of it must
/// begin. The rows differ along the number of actions, their kind, the trigger's type and
/// whether the same job is named twice.
#[tokio::test]
async fn a_firing_begins_a_run_of_every_job_its_actions_name() {
    const JOB: &str = "action-run-job";
    const OTHER_JOB: &str = "action-run-other-job";

    let (router, _engine) = emulator_with_engine();
    // A ceiling high enough that no row of this table is bounded by it; the ceiling has its
    // own test below.
    register_targets(
        &router,
        &[JOB, OTHER_JOB],
        json!({ "ExecutionProperty": { "MaxConcurrentRuns": 10 } }),
    )
    .await;

    // (case, trigger members beyond Name, runs of JOB, runs of OTHER_JOB)
    let cases: Vec<(&str, Value, usize, usize)> = vec![
        (
            "one job action",
            json!({ "Type": "ON_DEMAND", "Actions": [{ "JobName": JOB }] }),
            1,
            0,
        ),
        (
            "a job and a crawler action",
            json!({
                "Type": "ON_DEMAND",
                "Actions": [{ "JobName": JOB }, { "CrawlerName": CRAWLER }],
            }),
            1,
            0,
        ),
        (
            "two job actions naming different jobs",
            json!({
                "Type": "ON_DEMAND",
                "Actions": [{ "JobName": JOB }, { "JobName": OTHER_JOB }],
            }),
            1,
            1,
        ),
        (
            // Two actions naming the SAME job initiate two runs of it: an action is what
            // is initiated, not a job.
            "two job actions naming one job",
            json!({
                "Type": "ON_DEMAND",
                "Actions": [{ "JobName": JOB }, { "JobName": JOB, "Timeout": 30 }],
            }),
            2,
            0,
        ),
        (
            "only a crawler action",
            json!({ "Type": "ON_DEMAND", "Actions": [{ "CrawlerName": CRAWLER }] }),
            0,
            0,
        ),
        (
            // "This field is required when the trigger type is SCHEDULED": a scheduled
            // trigger fires on its cron expression, so starting it initiates nothing.
            "scheduled: fires on its schedule",
            json!({
                "Type": "SCHEDULED",
                "Schedule": "cron(15 12 * * ? *)",
                "Actions": [{ "JobName": JOB }],
            }),
            0,
            0,
        ),
        (
            "conditional: fires on its predicate",
            json!({
                "Type": "CONDITIONAL",
                "Predicate": {
                    "Conditions": [
                        { "LogicalOperator": "EQUALS", "JobName": JOB, "State": "SUCCEEDED" },
                    ],
                },
                "Actions": [{ "JobName": OTHER_JOB }],
            }),
            0,
            0,
        ),
        (
            "event: fires on the events it batches",
            json!({
                "Type": "EVENT",
                "EventBatchingCondition": { "BatchSize": 3 },
                "Actions": [{ "JobName": JOB }, { "JobName": OTHER_JOB }],
            }),
            0,
            0,
        ),
    ];

    for (index, (case, definition, from_job, from_other)) in cases.iter().enumerate() {
        let before: BTreeMap<&str, usize> = [
            (JOB, job_runs(&router, JOB).await.len()),
            (OTHER_JOB, job_runs(&router, OTHER_JOB).await.len()),
        ]
        .into_iter()
        .collect();

        let trigger = format!("action-run-trigger-{index}");
        let mut body = definition.clone();
        body["Name"] = json!(trigger);
        ok(&router, "CreateTrigger", &body).await;
        start_trigger(&router, case, &trigger).await;

        for (job, begun) in [(JOB, *from_job), (OTHER_JOB, *from_other)] {
            let runs = job_runs(&router, job).await;
            let expected = before[job] + begun;
            assert_eq!(
                runs.len(),
                expected,
                "case {case:?}: the firing must leave job {job:?} with {expected} run(s), got \
                 {}",
                runs.len()
            );
            // Every run the firing began says which trigger began it, carries a minted run
            // id, and is in flight: nothing has executed it yet.
            for run in runs.iter().take(begun) {
                assert_eq!(
                    run.get("TriggerName").and_then(Value::as_str),
                    Some(trigger.as_str()),
                    "case {case:?}: a run a firing began names its trigger, got {run}"
                );
                let id = run
                    .get("Id")
                    .and_then(Value::as_str)
                    .unwrap_or_else(|| panic!("case {case:?}: a run carries an Id, got {run}"));
                assert!(
                    id.starts_with(JOB_RUN_ID_PREFIX),
                    "case {case:?}: a job run id carries the {JOB_RUN_ID_PREFIX:?} prefix, got \
                     {id:?}"
                );
                assert_eq!(
                    run.get("JobRunState").and_then(Value::as_str),
                    Some(JOB_RUN_STATE_STARTING),
                    "case {case:?}: the run the firing began is in flight, got {run}"
                );
            }
        }
    }
}

/// A run a firing began carries the action's own overrides over the job definition, and the
/// job's configuration everywhere the action names nothing.
///
/// The rows differ along which members the action overrides: none at all, the arguments,
/// the timeout, the security configuration, the notification property, and all of them at
/// once.
#[tokio::test]
async fn an_actions_overrides_are_applied_over_the_job_definition() {
    let (router, _engine) = emulator_with_engine();

    // (case, action members beyond JobName, the JobRun members the firing must produce)
    let cases: Vec<(&str, Value, Value)> = vec![
        (
            "no overrides at all",
            json!({}),
            json!({
                // The job's own timeout, "defaulted to 2,880 minutes for Glue version 4.0
                // and earlier, or 480 minutes for Glue version 5.0 and later"
                // (`Action$Timeout`), which is the version these jobs are created at.
                "Timeout": 480,
                "Arguments": { "--job-language": "python" },
                "SecurityConfiguration": Value::Null,
                "NotificationProperty": Value::Null,
            }),
        ),
        (
            "arguments only",
            json!({ "Arguments": { "--day": "2026-10-26", "--mode": "backfill" } }),
            json!({
                "Timeout": 480,
                // The action's arguments replace the job's default of the same name and
                // are added beside the ones it does not name.
                "Arguments": {
                    "--job-language": "python",
                    "--day": "2026-10-26",
                    "--mode": "backfill",
                },
            }),
        ),
        (
            "a default argument overridden",
            json!({ "Arguments": { "--job-language": "scala" } }),
            json!({ "Arguments": { "--job-language": "scala" } }),
        ),
        (
            "the timeout only",
            json!({ "Timeout": 45 }),
            json!({ "Timeout": 45, "Arguments": { "--job-language": "python" } }),
        ),
        (
            "the security configuration only",
            json!({ "SecurityConfiguration": "action-security-config" }),
            json!({ "Timeout": 480, "SecurityConfiguration": "action-security-config" }),
        ),
        (
            "the notification property only",
            json!({ "NotificationProperty": { "NotifyDelayAfter": 7 } }),
            json!({ "NotificationProperty": { "NotifyDelayAfter": 7 } }),
        ),
        (
            "every override at once",
            json!({
                "Arguments": { "--day": "2026-01-01" },
                "Timeout": 10_080,
                "SecurityConfiguration": "action-security-config",
                "NotificationProperty": { "NotifyDelayAfter": 1 },
            }),
            json!({
                "Timeout": 10_080,
                "Arguments": { "--job-language": "python", "--day": "2026-01-01" },
                "SecurityConfiguration": "action-security-config",
                "NotificationProperty": { "NotifyDelayAfter": 1 },
            }),
        ),
    ];

    for (index, (case, overrides, expected)) in cases.iter().enumerate() {
        let job = format!("override-job-{index}");
        ok(
            &router,
            "CreateJob",
            &job_body(
                &job,
                json!({
                    "DefaultArguments": { "--job-language": "python" },
                    "ExecutionProperty": { "MaxConcurrentRuns": 5 },
                }),
            ),
        )
        .await;

        let mut action = overrides.clone();
        action["JobName"] = json!(job.clone());
        let trigger = format!("override-trigger-{index}");
        ok(
            &router,
            "CreateTrigger",
            &json!({ "Name": trigger, "Type": "ON_DEMAND", "Actions": [action] }),
        )
        .await;
        start_trigger(&router, case, &trigger).await;

        let runs = job_runs(&router, &job).await;
        assert_eq!(
            runs.len(),
            1,
            "case {case:?}: the firing must begin exactly one run, got {runs:?}"
        );
        let run_id = runs[0]
            .get("Id")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: a run carries an Id, got {}", runs[0]))
            .to_string();
        // The run the firing began is the run the single-run read reports under that id.
        let read = ok(
            &router,
            "GetJobRun",
            &json!({ "JobName": job, "RunId": run_id }),
        )
        .await;
        let reported = read
            .get("JobRun")
            .unwrap_or_else(|| panic!("case {case:?}: GetJobRun must report a JobRun, got {read}"));
        for (member, value) in expected.as_object().expect("the expectations are an object") {
            assert_eq!(
                reported.get(member).unwrap_or(&Value::Null),
                value,
                "case {case:?}: expected {member} {value}, got {reported}"
            );
        }
        assert_eq!(
            reported.get("JobName").and_then(Value::as_str),
            Some(job.as_str()),
            "case {case:?}: the run is a run of the job its action named, got {reported}"
        );
    }
}

/// The ceiling a job's execution property names bounds a firing as it bounds a direct
/// start: a firing takes a free slot and begins nothing once they are all held, and the
/// next firing begins a run again once the clock has finished the runs in flight.
///
/// The rows differ along the ceiling: one run at a time, two, three, and a stored zero,
/// which the service holds at the documented minimum of one.
#[tokio::test]
async fn a_firing_is_bounded_by_the_jobs_concurrency_ceiling() {
    let (router, engine) = emulator_with_engine();

    for (index, (case, ceiling, allowed)) in [
        ("one run at a time", 1, 1usize),
        ("two runs at a time", 2, 2),
        ("three runs at a time", 3, 3),
        ("a stored zero is held at one", 0, 1),
    ]
    .into_iter()
    .enumerate()
    {
        let job = format!("ceiling-job-{index}");
        ok(
            &router,
            "CreateJob",
            &job_body(
                &job,
                json!({ "ExecutionProperty": { "MaxConcurrentRuns": ceiling } }),
            ),
        )
        .await;
        let trigger = format!("ceiling-action-trigger-{index}");
        ok(
            &router,
            "CreateTrigger",
            &json!({
                "Name": trigger,
                "Type": "ON_DEMAND",
                "Actions": [{ "JobName": job }],
            }),
        )
        .await;

        // Fire past the ceiling, holding every run already begun in flight.
        for firing in 1..=allowed + 2 {
            start_trigger(&router, case, &trigger).await;
            set_due_instant(&engine, &job, far_future());
            let runs = job_runs(&router, &job).await;
            let expected = firing.min(allowed);
            assert_eq!(
                runs.len(),
                expected,
                "case {case:?}: after {firing} firing(s) the job must hold {expected} run(s), \
                 got {}",
                runs.len()
            );
        }

        // Once the clock has finished the runs in flight, the next firing begins a run
        // again: the ceiling is on runs in flight, not on firings ever made.
        set_due_instant(&engine, &job, already_past());
        start_trigger(&router, case, &trigger).await;
        let runs = job_runs(&router, &job).await;
        assert_eq!(
            runs.len(),
            allowed + 1,
            "case {case:?}: the freed slot took another run, got {}",
            runs.len()
        );
        let settled: Vec<&Value> = runs
            .iter()
            .filter(|run| run.get("JobRunState").and_then(Value::as_str) == Some(JOB_RUN_STATE_FAILED))
            .collect();
        assert_eq!(
            settled.len(),
            allowed,
            "case {case:?}: the runs the clock finished come to rest in \
             {JOB_RUN_STATE_FAILED}, got {runs:?}"
        );
        // The jobs of this suite point at a script no Amazon S3 here serves, so a run that
        // came to rest says exactly that.
        for run in settled {
            crate::job_runs::assert_script_was_unreadable(case, run);
        }
    }
}

/// A run of a workflow begun by a caller initiates the actions of the trigger it begins at,
/// exactly as that trigger's own start does — whatever type that entry point is, since a
/// workflow run starts from it rather than waiting for it.
///
/// The rows differ along the entry point's type and how many job actions it names.
#[tokio::test]
async fn a_workflow_run_initiates_its_entry_triggers_actions() {
    const JOB: &str = "workflow-entry-job";
    const OTHER_JOB: &str = "workflow-entry-other-job";

    let (router, engine) = emulator_with_engine();
    register_targets(
        &router,
        &[JOB, OTHER_JOB],
        json!({ "ExecutionProperty": { "MaxConcurrentRuns": 10 } }),
    )
    .await;

    // (case, trigger members beyond Name and WorkflowName, runs of JOB, runs of OTHER_JOB)
    let cases: Vec<(&str, Value, usize, usize)> = vec![
        (
            "an on-demand entry point with one job action",
            json!({ "Type": "ON_DEMAND", "Actions": [{ "JobName": JOB }] }),
            1,
            0,
        ),
        (
            "an on-demand entry point with a job and a crawler action",
            json!({
                "Type": "ON_DEMAND",
                "Actions": [{ "JobName": JOB }, { "CrawlerName": CRAWLER }],
            }),
            1,
            0,
        ),
        (
            "a scheduled entry point with two job actions",
            json!({
                "Type": "SCHEDULED",
                "Schedule": "cron(0 */6 * * ? *)",
                "Actions": [{ "JobName": JOB }, { "JobName": OTHER_JOB }],
            }),
            1,
            1,
        ),
        (
            "an event entry point with one job action",
            json!({
                "Type": "EVENT",
                "EventBatchingCondition": { "BatchSize": 1 },
                "Actions": [{ "JobName": OTHER_JOB }],
            }),
            0,
            1,
        ),
    ];

    for (index, (case, definition, from_job, from_other)) in cases.iter().enumerate() {
        let before: BTreeMap<&str, usize> = [
            (JOB, job_runs(&router, JOB).await.len()),
            (OTHER_JOB, job_runs(&router, OTHER_JOB).await.len()),
        ]
        .into_iter()
        .collect();

        let workflow = format!("entry-workflow-{index}");
        ok(&router, "CreateWorkflow", &json!({ "Name": workflow })).await;
        let trigger = format!("entry-trigger-{index}");
        let mut body = definition.clone();
        body["Name"] = json!(trigger);
        body["WorkflowName"] = json!(workflow.clone());
        ok(&router, "CreateTrigger", &body).await;

        let started = ok(&router, "StartWorkflowRun", &json!({ "Name": workflow })).await;
        assert!(
            started.get("RunId").and_then(Value::as_str).is_some(),
            "case {case:?}: StartWorkflowRun must answer a RunId, got {started}"
        );

        for (job, begun) in [(JOB, *from_job), (OTHER_JOB, *from_other)] {
            // Hold the runs this row began in flight, so the next row counts only its own.
            set_due_instant(&engine, job, far_future());
            let runs = job_runs(&router, job).await;
            let expected = before[job] + begun;
            assert_eq!(
                runs.len(),
                expected,
                "case {case:?}: the workflow run must leave job {job:?} with {expected} run(s), \
                 got {}",
                runs.len()
            );
            for run in runs.iter().take(begun) {
                assert_eq!(
                    run.get("TriggerName").and_then(Value::as_str),
                    Some(trigger.as_str()),
                    "case {case:?}: a run the entry point began names that trigger, got {run}"
                );
            }
        }
    }
}
