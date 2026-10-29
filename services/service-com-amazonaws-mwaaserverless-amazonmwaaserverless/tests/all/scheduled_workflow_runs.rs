// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what a workflow's automated trigger DOES once the service has
//! configured one: how many runs it produces, which version they execute, and when it
//! stops.
//!
//! The rule under test: a configured trigger owes one run per firing period
//! (`SCHEDULED_RUN_EVERY`), each dated from the instant it fell due, each reporting the
//! `RunType` that names automated execution (`smithy_query(query="shape",
//! shape="RunType")` declares `ON_DEMAND | SCHEDULED`); the trigger in force is always
//! the active version's, because "only one version of a workflow can have an active
//! schedule at any given time"; and `DeleteWorkflow`'s teardown removes it, because the
//! cleanup it performs includes "removing scheduled triggers".
//!
//! WHERE a trigger comes from — the schedule information the active version's snapshotted
//! YAML definition declares — is the subject of
//! [`crate::definition_declared_schedule_trigger`], so every workflow below is created
//! from a definition that really is an object in the emulated Amazon S3, through the same
//! router that suite drives.
//!
//! Every test drives the real wire path (`MultiServiceRouter::dispatch` over an
//! `awsJson1_0` request). A firing instant is a CLOCK field of the stored workflow, so
//! the tables drive it from the store — exactly the way the run-lifecycle suite drives a
//! run's deadlines — rather than racing the wall clock.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_mwaaserverless_amazonmwaaserverless::state::{
    Workflow, MAX_CATCHUP_FIRINGS_PER_CALL, MAX_CATCHUP_FIRINGS_PER_WORKFLOW, RUN_STATUS_QUEUED,
    RUN_STATUS_RUNNING, RUN_STATUS_STARTING, RUN_STATUS_SUCCESS, RUN_TYPE_SCHEDULED,
    SCHEDULED_RUN_EVERY,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::task_instance_reads::epoch_seconds;
use crate::workflow_definition_snapshot::{
    call, create_bucket, create_workflow_with_mode, definition_router_on, stage,
    with_stored_workflow, BUCKET,
};
use crate::workflow_lifecycle::role_arn;

/// A definition that DECLARES a schedule, so the service configures a trigger for the
/// version that snapshots it.
const DECLARES_A_SCHEDULE: &str = "name: nightly\nschedule: \"0 3 * * *\"\ntasks:\n  - id: extract\n";

/// A definition declaring no schedule at all, so the version that snapshots it is left to
/// be started by a caller.
const DECLARES_NO_SCHEDULE: &str = "name: nightly\ntasks:\n  - id: extract\n";

/// The `WorkflowRunStatus` values an automated run can be reported in, in the order the
/// clock owes them: it is admitted, waits for an executor, executes the tasks its
/// definition declares and completes.
const LIFECYCLE: &[&str] = &[
    RUN_STATUS_STARTING,
    RUN_STATUS_QUEUED,
    RUN_STATUS_RUNNING,
    RUN_STATUS_SUCCESS,
];

/// How closely two timestamps must agree to be the same instant on the wire, in seconds.
const TIMESTAMP_TOLERANCE: f64 = 0.002;

/// This service and the emulated Amazon S3 holding the definitions, on one engine this
/// suite also reads the stored firing instants from.
struct Scheduled {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Scheduled {
    /// A fresh emulator with the definition bucket created, so nothing one test stored
    /// can answer another's read.
    async fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let router = definition_router_on(Arc::clone(&engine));
        create_bucket(&router).await;
        Self { router, engine }
    }

    /// Store `definition` at `key` and create a workflow whose definition it is, and
    /// answer the workflow's ARN.
    async fn create(&self, name: &str, key: &str, definition: &str) -> String {
        stage(&self.router, key, definition.as_bytes()).await;
        create_workflow_with_mode(&self.router, name, key, None).await
    }
}

/// The instant the stored workflow's trigger fires next, and `None` for a workflow the
/// service does not execute on its own.
fn trigger(scheduled: &Scheduled, arn: &str) -> Option<SystemTime> {
    with_stored_workflow(&scheduled.engine, arn, |workflow: &mut Workflow| {
        workflow.next_scheduled_run_at
    })
}

/// Put the workflow's next firing `firings` firings plus half a period in the past, so
/// exactly `firings` of them are owed at the moment of the call that follows. Returns
/// the instant of the OLDEST owed firing.
fn owe_firings(scheduled: &Scheduled, arn: &str, firings: u32) -> SystemTime {
    let behind = SCHEDULED_RUN_EVERY * firings - SCHEDULED_RUN_EVERY / 2;
    let oldest = SystemTime::now() - behind;
    with_stored_workflow(&scheduled.engine, arn, |workflow: &mut Workflow| {
        workflow.next_scheduled_run_at = Some(oldest);
    });
    oldest
}

/// The `WorkflowRuns` a listing reports for a workflow, over the wire.
async fn listed_runs(scheduled: &Scheduled, arn: &str, case: &str) -> Vec<Value> {
    let (status, listed) = call(
        &scheduled.router,
        "ListWorkflowRuns",
        json!({"WorkflowArn": arn}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected ListWorkflowRuns to succeed, got {status} {listed:?}"
    );
    listed["WorkflowRuns"].as_array().cloned().unwrap_or_default()
}

/// Assert `member` of `value` is `expected` epoch seconds.
fn assert_instant(value: &Value, member: &str, expected: SystemTime, case: &str) {
    let want = expected
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("an instant after the epoch")
        .as_secs_f64();
    let got = epoch_seconds(&value[member]);
    assert!(
        got.is_some_and(|seconds| (seconds - want).abs() < TIMESTAMP_TOLERANCE),
        "case {case:?}: expected {member} at {want} epoch seconds, got {:?}",
        value[member]
    );
}

#[tokio::test]
async fn a_trigger_left_behind_reports_one_run_per_firing_it_owed() {
    // The rows differ along how many firings the clock owes at once: one, a handful, and
    // a trigger that has been behind for several periods.
    for owed in [1_u32, 2, 3, 7] {
        let scheduled = Scheduled::new().await;
        let arn = scheduled
            .create("owedFirings", "dags/owed.yaml", DECLARES_A_SCHEDULE)
            .await;
        let case = format!("{owed} owed firing(s)");

        let oldest = owe_firings(&scheduled, &arn, owed);
        let runs = listed_runs(&scheduled, &arn, &case).await;
        assert_eq!(
            runs.len(),
            owed as usize,
            "case {case:?}: every owed firing produces its own run, got {runs:?}"
        );

        let mut seen_ids: Vec<&str> = Vec::new();
        for (index, run) in runs.iter().enumerate() {
            let expected_at = oldest + SCHEDULED_RUN_EVERY * u32::try_from(index).unwrap_or(0);
            assert_eq!(
                run["RunType"].as_str(),
                Some(RUN_TYPE_SCHEDULED),
                "case {case:?}: run {index} was started by the trigger, got {:?}",
                run["RunType"]
            );
            // The listing is chronological, so the i-th run is the i-th firing and
            // carries that firing's instant rather than the instant of the read.
            assert_instant(
                &run["RunDetailSummary"],
                "CreatedOn",
                expected_at,
                &format!("{case} / run {index}"),
            );
            let status = run["RunDetailSummary"]["Status"].as_str().unwrap_or_default();
            assert!(
                LIFECYCLE.contains(&status),
                "case {case:?}: run {index} reports a status of the lifecycle \
                 {LIFECYCLE:?}, got {status:?}"
            );
            let id = run["RunId"].as_str().unwrap_or_default();
            assert!(
                !seen_ids.contains(&id),
                "case {case:?}: every firing mints its own RunId, got {id:?} twice"
            );
            seen_ids.push(id);
        }

        // A read changes nothing a later caller sees: with the trigger parked beyond the
        // horizon of this test, the second listing answers exactly what the first did.
        with_stored_workflow(&scheduled.engine, &arn, |workflow: &mut Workflow| {
            workflow.next_scheduled_run_at = Some(SystemTime::now() + Duration::from_secs(3_600));
        });
        let again = listed_runs(&scheduled, &arn, &case).await;
        assert_eq!(
            again, runs,
            "case {case:?}: a read must not change what a later caller sees, got {again:?} \
             after {runs:?}"
        );
    }
}

#[tokio::test]
async fn the_trigger_in_force_is_the_active_version_s_and_a_delete_removes_it() {
    let scheduled = Scheduled::new().await;
    let key = "dags/followed.yaml";
    let arn = scheduled
        .create("triggerFollowsVersion", key, DECLARES_NO_SCHEDULE)
        .await;
    assert!(
        trigger(&scheduled, &arn).is_none(),
        "case created without a declared schedule: the service executes no workflow whose \
         definition declares none"
    );

    /// The update body: the same definition location, snapshotted afresh.
    fn update(arn: &str, key: &str) -> Value {
        json!({
            "WorkflowArn": arn,
            "DefinitionS3Location": {"Bucket": BUCKET, "ObjectKey": key},
            "RoleArn": role_arn(),
        })
    }

    // An update onto a definition that declares a schedule configures the trigger, and
    // the run it later produces executes the version that declared it.
    stage(&scheduled.router, key, DECLARES_A_SCHEDULE.as_bytes()).await;
    let (status, updated) = call(&scheduled.router, "UpdateWorkflow", update(&arn, key)).await;
    assert_eq!(
        status, 200,
        "case updated onto a declared schedule: expected UpdateWorkflow to succeed, got \
         {status} {updated:?}"
    );
    let active_version = updated["WorkflowVersion"]
        .as_str()
        .expect("an update reports the version it appended")
        .to_string();
    assert!(
        trigger(&scheduled, &arn).is_some(),
        "case updated onto a declared schedule: the new version's trigger is the one in \
         force"
    );

    let fired_at = owe_firings(&scheduled, &arn, 1);
    let runs = listed_runs(&scheduled, &arn, "updated onto a declared schedule").await;
    assert_eq!(
        runs.len(),
        1,
        "case updated onto a declared schedule: the owed firing produces one run, got \
         {runs:?}"
    );
    assert_eq!(
        runs[0]["WorkflowVersion"].as_str(),
        Some(active_version.as_str()),
        "case updated onto a declared schedule: the firing runs the version whose \
         definition declared the schedule, got {:?}",
        runs[0]["WorkflowVersion"]
    );
    assert_instant(
        &runs[0]["RunDetailSummary"],
        "CreatedOn",
        fired_at,
        "updated onto a declared schedule",
    );

    // An update back onto a definition declaring no schedule disables it: the runs
    // already produced stay, and no further one appears however long the clock runs.
    stage(&scheduled.router, key, DECLARES_NO_SCHEDULE.as_bytes()).await;
    let (status, updated) = call(&scheduled.router, "UpdateWorkflow", update(&arn, key)).await;
    assert_eq!(
        status, 200,
        "case updated off the declared schedule: expected UpdateWorkflow to succeed, got \
         {status} {updated:?}"
    );
    assert!(
        trigger(&scheduled, &arn).is_none(),
        "case updated off the declared schedule: scheduling is disabled with the version \
         that declared it"
    );
    tokio::time::sleep(SCHEDULED_RUN_EVERY * 2).await;
    let after = listed_runs(&scheduled, &arn, "updated off the declared schedule").await;
    assert_eq!(
        after.len(),
        1,
        "case updated off the declared schedule: the history keeps the run the trigger \
         produced and gains no other, got {after:?}"
    );

    // A delete's cleanup removes the scheduled triggers, so a workflow on its way out
    // fires no further run.
    let deleted_arn = scheduled
        .create("deletedSchedule", "dags/deleted.yaml", DECLARES_A_SCHEDULE)
        .await;
    assert!(
        trigger(&scheduled, &deleted_arn).is_some(),
        "setup: the created workflow is one the service executes"
    );
    let (status, deleted) = call(
        &scheduled.router,
        "DeleteWorkflow",
        json!({"WorkflowArn": deleted_arn}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case deleted: expected DeleteWorkflow to succeed, got {status} {deleted:?}"
    );
    assert!(
        trigger(&scheduled, &deleted_arn).is_none(),
        "case deleted: the teardown removes the workflow's scheduled triggers"
    );
    let during_teardown = listed_runs(&scheduled, &deleted_arn, "deleted").await;
    assert!(
        during_teardown.is_empty(),
        "case deleted: a workflow tearing down produces no new run, got {during_teardown:?}"
    );
}

/// Every run of a workflow in one page, so a ceiling rather than the default page size
/// is what bounds what a test sees.
async fn whole_history(scheduled: &Scheduled, arn: &str, case: &str) -> Vec<Value> {
    let (status, listed) = call(
        &scheduled.router,
        "ListWorkflowRuns",
        json!({"WorkflowArn": arn, "MaxResults": 100}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected ListWorkflowRuns to succeed, got {status} {listed:?}"
    );
    listed["WorkflowRuns"].as_array().cloned().unwrap_or_default()
}

/// The `RunId` of each run, in the order the listing reported them.
fn run_ids(runs: &[Value]) -> Vec<String> {
    runs.iter()
        .map(|run| run["RunId"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// The work ONE request performs catching a trigger up is bounded: a call materialises
/// at most [`MAX_CATCHUP_FIRINGS_PER_WORKFLOW`] firings for a workflow, parks the
/// trigger AT the oldest firing it did not materialise rather than skipping it, and the
/// next call continues from there — so a gap costs one caller a bounded amount of work
/// and still produces exactly one run per firing.
#[tokio::test]
async fn one_call_catches_a_trigger_up_by_at_most_the_per_call_ceiling() {
    let scheduled = Scheduled::new().await;
    let arn = scheduled
        .create("cappedCatchUp", "dags/capped.yaml", DECLARES_A_SCHEDULE)
        .await;

    // More owed firings than one call will materialise.
    let owed = MAX_CATCHUP_FIRINGS_PER_WORKFLOW + 4;
    let case =
        format!("{owed} owed firings against a ceiling of {MAX_CATCHUP_FIRINGS_PER_WORKFLOW}");
    let oldest = owe_firings(&scheduled, &arn, u32::try_from(owed).unwrap_or_default());

    let first = whole_history(&scheduled, &arn, &case).await;
    assert_eq!(
        first.len(),
        MAX_CATCHUP_FIRINGS_PER_WORKFLOW,
        "case {case:?}: one call must materialise at most the per-workflow ceiling, got {} \
         runs",
        first.len()
    );

    // Nothing was skipped: the trigger stands at the firing after the last one this call
    // recorded, which is still in the past.
    let stopped_at = oldest
        + SCHEDULED_RUN_EVERY
            * u32::try_from(MAX_CATCHUP_FIRINGS_PER_WORKFLOW).unwrap_or_default();
    assert_eq!(
        trigger(&scheduled, &arn),
        Some(stopped_at),
        "case {case:?}: a capped call must leave the trigger at the firing it stopped at"
    );

    // The next call continues the catch-up, so every firing the first call owed has by
    // now produced its own run, and the runs the first call recorded are still there.
    let second = whole_history(&scheduled, &arn, &case).await;
    assert!(
        second.len() >= owed,
        "case {case:?}: the next call must continue the catch-up to at least {owed} runs, got \
         {}",
        second.len()
    );
    let ids = run_ids(&second);
    let (carried, added) = ids.split_at(first.len());
    assert_eq!(
        carried,
        run_ids(&first).as_slice(),
        "case {case:?}: the runs the first call recorded must still be the oldest ones"
    );
    for id in added {
        assert!(
            !carried.contains(id),
            "case {case:?}: every firing mints its own RunId, got {id:?} twice"
        );
    }
}

/// The per-call ceiling bounds the work, it does not drop a trigger: the triggers are
/// served oldest owed firing first, so a workflow one call's budget did not reach holds
/// the oldest firing in the store and is the first the next call serves.
#[tokio::test]
async fn the_per_call_budget_cannot_starve_a_trigger() {
    let scheduled = Scheduled::new().await;

    // One workflow more than the whole-store budget covers at the per-workflow ceiling,
    // each owing more firings than that ceiling, so the budget really is spent before
    // the last of them is reached.
    let workflows = MAX_CATCHUP_FIRINGS_PER_CALL / MAX_CATCHUP_FIRINGS_PER_WORKFLOW + 1;
    let owed = MAX_CATCHUP_FIRINGS_PER_WORKFLOW + 2;
    let case = format!("{workflows} triggers owing {owed} firings each");
    let mut arns: Vec<String> = Vec::with_capacity(workflows);
    for index in 0..workflows {
        let arn = scheduled
            .create(
                &format!("starved{index}"),
                &format!("dags/starved-{index}.yaml"),
                DECLARES_A_SCHEDULE,
            )
            .await;
        arns.push(arn);
    }
    // Owed only once every workflow exists: a create is itself a request, and a request
    // materialises what the clock owes, so owing inside the creation loop would let the
    // later creates catch the earlier triggers up before the budget is ever tested.
    for arn in &arns {
        owe_firings(&scheduled, arn, u32::try_from(owed).unwrap_or_default());
    }

    /// How many runs each workflow holds AS THE STORE HOLDS IT, in creation order.
    ///
    /// Read from the store rather than over the wire: every request materialises what
    /// the clock owes before it answers, so a listing call would itself move the
    /// catch-up along and could not show where one call stopped.
    fn histories(scheduled: &Scheduled, arns: &[String]) -> Vec<usize> {
        arns.iter()
            .map(|arn| {
                with_stored_workflow(&scheduled.engine, arn, |workflow: &mut Workflow| {
                    workflow.runs.len()
                })
            })
            .collect()
    }

    // The first call spends the budget, so at least one trigger is left untouched: the
    // bound is doing something here.
    let (status, listed) =
        call(&scheduled.router, "ListWorkflows", json!({"MaxResults": 100})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected ListWorkflows to succeed, got {status} {listed:?}"
    );
    let after_one = histories(&scheduled, &arns);
    assert!(
        after_one.contains(&0),
        "case {case:?}: the whole-store budget must stop one call short of every trigger, \
         got {after_one:?}"
    );

    // And no trigger stays behind: within a few further calls every one of them has
    // caught up to the firings it owed.
    let mut counts = after_one;
    for _ in 0..workflows {
        if counts.iter().all(|held| *held >= owed) {
            break;
        }
        let (status, listed) =
            call(&scheduled.router, "ListWorkflows", json!({"MaxResults": 100})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListWorkflows to succeed, got {status} {listed:?}"
        );
        counts = histories(&scheduled, &arns);
    }
    assert!(
        counts.iter().all(|held| *held >= owed),
        "case {case:?}: every trigger must catch up to the {owed} firings it owed, got \
         {counts:?}"
    );

    // Every caught-up trigger's history is one run per firing, readable back over the
    // wire: the bound changed how much one call does, not what a firing produces.
    for arn in &arns {
        let runs = whole_history(&scheduled, arn, &case).await;
        let ids = run_ids(&runs);
        let mut unique = ids.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(
            unique.len(),
            ids.len(),
            "case {case:?}: every firing mints its own RunId, got {ids:?}"
        );
    }
}
