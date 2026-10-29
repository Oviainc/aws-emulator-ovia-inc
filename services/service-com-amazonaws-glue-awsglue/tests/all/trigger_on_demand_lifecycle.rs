// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the whole on-demand trigger lifecycle: `CreateTrigger`,
//! `StartTrigger`, and the `GetJobRuns` and `GetTrigger` reads that report what the start
//! did — the slice a client drives when it kicks off Glue work through a trigger instead of
//! calling `StartJobRun` itself.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler.
//!
//! The rule under test: starting a trigger that waits on nothing but its caller FIRES it
//! once, and a fire initiates every action the definition names ("the actions initiated by
//! this trigger when it fires" — `CreateTriggerRequest$Actions`), so each action naming a
//! job begins ONE run of that job ("the name of a job to be run" — `Action$JobName`), which
//! a later `GetJobRuns` of that job reports; the start leaves such a trigger in the
//! `TriggerState` it waits in rather than in the armed state of a started trigger, since it
//! has nothing to wait in once it has fired ("True is not supported for `ON_DEMAND`
//! triggers" — `CreateTriggerRequest$StartOnCreation`). None of this turns on the trigger
//! belonging to a workflow, which `Trigger$WorkflowName` leaves optional. A trigger that
//! waits on something outside the caller — its schedule, its predicate or the events it
//! batches — is left ARMED by a start and initiates nothing until that happens.
//!
//! The rows differ along what the rule generalises over: the trigger's type (each modelled
//! `TriggerType`), how many actions the definition names and of which kind (one job, two
//! jobs, the same job twice, a crawler beside a job, a crawler alone), whether the trigger
//! belongs to a workflow, and HOW MANY TIMES it is fired — one fire begins one run per job
//! action, so N fires leave exactly N runs per job action.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::state::{
    JOB_RUN_ID_PREFIX, TRIGGER_STATE_ACTIVATED, TRIGGER_STATE_CREATED,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::job_runs::{emulator_with_engine, job_body};
use crate::trigger_action_runs::job_runs;
use crate::trigger_starts::trigger_state;
use crate::wire::{call_in_region, ok, REGION};

/// The crawler an action can name instead of a job, so a row can carry an action of the
/// other kind. One crawler, shared: which crawler it is is not an axis of the rule.
const CRAWLER: &str = "on-demand-lifecycle-crawler";

/// A concurrency ceiling high enough that no row below is bounded by it — the ceiling that
/// bounds a firing has its own rows in [`crate::trigger_action_runs`].
const HEADROOM: i64 = 10;

/// What an action of a row names: one of the row's own two jobs, or the shared crawler.
#[derive(Clone, Copy, Debug)]
enum Target {
    /// The row's first job.
    Job,
    /// The row's second job, so a row can initiate actions naming different jobs.
    OtherJob,
    /// [`CRAWLER`]: an action that names no job at all.
    Crawler,
}

/// One row: a trigger definition, how many times it is started, and what each start must
/// leave behind.
struct FireCase {
    /// What this row varies, named in every assertion it drives.
    case: &'static str,
    /// The `CreateTrigger` members beyond `Name`, `Actions` and `WorkflowName`, built from
    /// the row's own first job name so a predicate can wait on a job that exists.
    definition: fn(&str) -> Value,
    /// Whether the trigger belongs to a workflow: the rule holds either way.
    in_workflow: bool,
    /// The actions the definition names, in order.
    actions: &'static [Target],
    /// How many times the trigger is started.
    fires: usize,
    /// The `TriggerState` a read reports after EVERY start: the state a start of this type
    /// leaves the trigger in.
    state_after: &'static str,
    /// How many runs ONE start begins of the row's first job and of its second.
    per_fire: (usize, usize),
}

/// The rows.
fn fire_cases() -> Vec<FireCase> {
    vec![
        FireCase {
            // The recorded case: an on-demand trigger with a single job action and no
            // workflow at all, started once.
            case: "on-demand, one job action, outside any workflow, fired once",
            definition: |_job| json!({ "Type": "ON_DEMAND" }),
            in_workflow: false,
            actions: &[Target::Job],
            fires: 1,
            state_after: TRIGGER_STATE_CREATED,
            per_fire: (1, 0),
        },
        FireCase {
            // Each start is its own fire, so the runs accumulate one per start.
            case: "on-demand, one job action, outside any workflow, fired three times",
            definition: |_job| json!({ "Type": "ON_DEMAND" }),
            in_workflow: false,
            actions: &[Target::Job],
            fires: 3,
            state_after: TRIGGER_STATE_CREATED,
            per_fire: (1, 0),
        },
        FireCase {
            case: "on-demand, two job actions naming different jobs, fired twice",
            definition: |_job| json!({ "Type": "ON_DEMAND", "Description": "two jobs" }),
            in_workflow: false,
            actions: &[Target::Job, Target::OtherJob],
            fires: 2,
            state_after: TRIGGER_STATE_CREATED,
            per_fire: (1, 1),
        },
        FireCase {
            // An ACTION is what a fire initiates, not a job, so two actions naming one job
            // begin two runs of it per fire.
            case: "on-demand, the same job named by two actions, fired twice",
            definition: |_job| json!({ "Type": "ON_DEMAND" }),
            in_workflow: false,
            actions: &[Target::Job, Target::Job],
            fires: 2,
            state_after: TRIGGER_STATE_CREATED,
            per_fire: (2, 0),
        },
        FireCase {
            // An action naming a crawler initiates a CRAWL, which is the lifecycle
            // `StartCrawler` owns and which this crate does not hold yet, so it begins no
            // job run — the job action beside it still begins its own.
            case: "on-demand, a job action beside a crawler action, fired twice",
            definition: |_job| json!({ "Type": "ON_DEMAND" }),
            in_workflow: false,
            actions: &[Target::Job, Target::Crawler],
            fires: 2,
            state_after: TRIGGER_STATE_CREATED,
            per_fire: (1, 0),
        },
        FireCase {
            case: "on-demand, only a crawler action, fired twice",
            definition: |_job| json!({ "Type": "ON_DEMAND" }),
            in_workflow: false,
            actions: &[Target::Crawler],
            fires: 2,
            state_after: TRIGGER_STATE_CREATED,
            per_fire: (0, 0),
        },
        FireCase {
            // `Trigger$WorkflowName` is optional, so belonging to a workflow changes
            // neither the runs the fire begins nor the state it leaves behind.
            case: "on-demand, two job actions, inside a workflow, fired twice",
            definition: |_job| json!({ "Type": "ON_DEMAND" }),
            in_workflow: true,
            actions: &[Target::Job, Target::OtherJob],
            fires: 2,
            state_after: TRIGGER_STATE_CREATED,
            per_fire: (1, 1),
        },
        FireCase {
            // "This field is required when the trigger type is SCHEDULED": a scheduled
            // trigger waits for its cron expression, so a start only arms it.
            case: "scheduled, started twice",
            definition: |_job| {
                json!({ "Type": "SCHEDULED", "Schedule": "cron(15 12 * * ? *)" })
            },
            in_workflow: false,
            actions: &[Target::Job],
            fires: 2,
            state_after: TRIGGER_STATE_ACTIVATED,
            per_fire: (0, 0),
        },
        FireCase {
            // "A predicate to specify when the new trigger should fire": a conditional
            // trigger waits on the run state of the job its condition names.
            case: "conditional, started twice",
            definition: |job| {
                json!({
                    "Type": "CONDITIONAL",
                    "Predicate": {
                        "Conditions": [{
                            "LogicalOperator": "EQUALS",
                            "JobName": job,
                            "State": "SUCCEEDED",
                        }],
                    },
                })
            },
            in_workflow: true,
            actions: &[Target::OtherJob],
            fires: 2,
            state_after: TRIGGER_STATE_ACTIVATED,
            per_fire: (0, 0),
        },
        FireCase {
            // "Batch condition that must be met … before EventBridge event trigger
            // fires": an event trigger waits for the events it batches.
            case: "event, started once",
            definition: |_job| {
                json!({
                    "Type": "EVENT",
                    "EventBatchingCondition": { "BatchSize": 3, "BatchWindow": 900 },
                })
            },
            in_workflow: false,
            actions: &[Target::Job, Target::OtherJob],
            fires: 1,
            state_after: TRIGGER_STATE_ACTIVATED,
            per_fire: (0, 0),
        },
    ]
}

/// Register the crawler the rows' crawler actions name, so those actions resolve.
async fn register_crawler(router: &MultiServiceRouter) {
    ok(
        router,
        "CreateCrawler",
        &json!({
            "Name": CRAWLER,
            "Role": "arn:aws:iam::000000000000:role/on-demand-lifecycle-crawler-role",
            "Targets": { "S3Targets": [{ "Path": "s3://on-demand-lifecycle/data/" }] },
        }),
    )
    .await;
}

/// Register one row's own jobs, workflow and trigger, and answer the two job names and the
/// trigger name. Each row owns its jobs, so a run count is of the runs that row's own
/// starts began.
async fn register_case(
    router: &MultiServiceRouter,
    index: usize,
    row: &FireCase,
) -> (String, String, String) {
    let job = format!("on-demand-job-{index}");
    let other_job = format!("on-demand-other-job-{index}");
    for name in [&job, &other_job] {
        ok(
            router,
            "CreateJob",
            &job_body(
                name,
                json!({ "ExecutionProperty": { "MaxConcurrentRuns": HEADROOM } }),
            ),
        )
        .await;
    }

    let actions: Vec<Value> = row
        .actions
        .iter()
        .map(|target| match target {
            Target::Job => json!({ "JobName": job }),
            Target::OtherJob => json!({ "JobName": other_job }),
            Target::Crawler => json!({ "CrawlerName": CRAWLER }),
        })
        .collect();

    let trigger = format!("on-demand-trigger-{index}");
    let mut body = (row.definition)(&job);
    body["Name"] = json!(trigger);
    body["Actions"] = json!(actions);
    if row.in_workflow {
        let workflow = format!("on-demand-workflow-{index}");
        ok(
            router,
            "CreateWorkflow",
            &json!({ "Name": workflow, "MaxConcurrentRuns": HEADROOM }),
        )
        .await;
        body["WorkflowName"] = json!(workflow);
    }
    ok(router, "CreateTrigger", &body).await;
    (job, other_job, trigger)
}

/// Start `trigger` once, asserting the start was served and answered the name it started.
async fn fire(router: &MultiServiceRouter, case: &str, start: usize, trigger: &str) {
    let (status, body) =
        call_in_region(router, REGION, "StartTrigger", &json!({ "Name": trigger })).await;
    assert_eq!(
        status, 200,
        "case {case:?}: start {start} must answer 200, got {status} with {body}"
    );
    assert_eq!(
        body.get("Name").and_then(Value::as_str),
        Some(trigger),
        "case {case:?}: start {start} answers the name of the trigger it started, got {body}"
    );
}

/// Assert `job` holds exactly `expected` runs, each of that job, each begun by `trigger`,
/// and each under its own minted id.
async fn assert_runs(
    router: &MultiServiceRouter,
    case: &str,
    job: &str,
    trigger: &str,
    expected: usize,
) {
    let runs = job_runs(router, job).await;
    assert_eq!(
        runs.len(),
        expected,
        "case {case:?}: job {job:?} must hold {expected} run(s), got {}",
        runs.len()
    );
    let mut ids = BTreeSet::new();
    for run in &runs {
        assert_eq!(
            run.get("JobName").and_then(Value::as_str),
            Some(job),
            "case {case:?}: a run the fire began is a run of the job its action named, got \
             {run}"
        );
        assert_eq!(
            run.get("TriggerName").and_then(Value::as_str),
            Some(trigger),
            "case {case:?}: a run the fire began names the trigger that began it, got {run}"
        );
        let id = run
            .get("Id")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: a run carries an Id, got {run}"));
        assert!(
            id.starts_with(JOB_RUN_ID_PREFIX),
            "case {case:?}: a job run id carries the {JOB_RUN_ID_PREFIX:?} prefix, got {id:?}"
        );
        assert!(
            ids.insert(id.to_string()),
            "case {case:?}: every run carries its own id, got {id:?} twice in {runs:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// What a start does, per trigger type
// ---------------------------------------------------------------------------

/// Every start of a trigger that waits on nothing but its caller begins one run of each job
/// its actions name and leaves it in the state it waits in; a start of a trigger that waits
/// on something outside the caller begins no run and arms it instead.
#[tokio::test]
async fn each_fire_begins_one_run_per_job_action_and_settles_the_trigger_back() {
    let (router, _engine) = emulator_with_engine();
    register_crawler(&router).await;

    for (index, row) in fire_cases().iter().enumerate() {
        let case = row.case;
        let (job, other_job, trigger) = register_case(&router, index, row).await;

        // The create did not ask for the trigger to be started, and nothing has fired it
        // yet, so neither job holds a run.
        for name in [&job, &other_job] {
            assert_runs(&router, case, name, &trigger, 0).await;
        }

        for start in 1..=row.fires {
            fire(&router, case, start, &trigger).await;

            let state = trigger_state(&router, &trigger).await;
            assert_eq!(
                state, row.state_after,
                "case {case:?}: after start {start} the trigger must be in {:?}, got {state:?}",
                row.state_after
            );

            for (name, per_fire) in [(&job, row.per_fire.0), (&other_job, row.per_fire.1)] {
                assert_runs(&router, case, name, &trigger, start * per_fire).await;
            }
        }
    }
}

/// The state a fired trigger comes to rest in, and the runs its fire began, are what every
/// later READ reports: a read neither re-fires the trigger nor moves it, so repeated reads
/// report the same state and the same run count.
#[tokio::test]
async fn reading_a_fired_trigger_neither_re_fires_it_nor_moves_it() {
    let (router, _engine) = emulator_with_engine();
    register_crawler(&router).await;

    for (index, row) in fire_cases().iter().enumerate() {
        let case = row.case;
        let (job, other_job, trigger) = register_case(&router, index, row).await;
        fire(&router, case, 1, &trigger).await;

        // Read the trigger and its jobs' runs several times over: what the first read
        // reported is what every later one reports.
        for read in 1..=4 {
            let state = trigger_state(&router, &trigger).await;
            assert_eq!(
                state, row.state_after,
                "case {case:?}: read {read} must report {:?}, got {state:?}",
                row.state_after
            );
            for (name, per_fire) in [(&job, row.per_fire.0), (&other_job, row.per_fire.1)] {
                assert_runs(&router, case, name, &trigger, per_fire).await;
            }
        }
    }
}
