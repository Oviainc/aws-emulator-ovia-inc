Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: c7879b242a52aaa0fabb95009fbb05c28e2cce49

# DeleteWorkflow's teardown never stops the workflow's in-flight runs

_Filed: 2026-10-25T20:13:12Z_
_Operation: DeleteWorkflow | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `c7879b242a52aaa0fabb95009fbb05c28e2cce49`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

DeleteWorkflow starts an asynchronous teardown (the workflow moves to DELETING and its removal falls due WORKFLOW_DELETED_AFTER later), but the teardown performs none of the run cleanup the operation's own model documentation names: the workflow's stored runs are left exactly as they were, with the status deadline they already carried.

So a run that was STARTING/QUEUED when the workflow was deleted keeps progressing on its own schedule while the workflow reports DELETING: WorkflowRun::settle moves it STARTING -> QUEUED -> FAILED and stamps the emulator's NO_AIRFLOW_ENGINE text into ErrorMessage. A client that starts a run, deletes the workflow and then polls GetWorkflowRun (or ListWorkflowRuns, which settles every run of the workflow) during the DELETING window observes QUEUED/FAILED, i.e. a run the delete was supposed to have stopped still being admitted and then failing, instead of the STOPPING/STOPPED the crate already models and implements for a stopped run.

This is not a missing feature: the crate already owns the helper (`WorkflowRun::begin_stop`, called by StopWorkflowRun) and DeleteWorkflow already holds a mutable handle on the workflow (it calls `workflow.begin_deletion(now)` on it), and the sibling StartWorkflowRun handler cites this very documentation clause as its reason to refuse NEW runs on a DELETING workflow — while the existing runs the clause is actually about are never touched.

## Evidence

Smithy model, DeleteWorkflow documentation (smithy_query query="doc" shape="DeleteWorkflow"):
"Deletes a workflow and all its versions. ... Amazon Managed Workflows for Apache Airflow Serverless ensures that all associated resources are properly cleaned up, including stopping any running executions, removing scheduled triggers, and cleaning up execution history."

The handler's teardown touches only the workflow record — services/service-com-amazonaws-mwaaserverless-amazonmwaaserverless/src/ops/delete_workflow.rs:257-267:
```
            // Deleting every version of a workflow deletes the workflow, and the
            // docs describe that as a cleanup over its associated resources —
            // "stopping any running executions, removing scheduled triggers, and
            // cleaning up execution history". So the record is NOT dropped inside
            // this request: it moves to `DELETING` and the clock owes its removal
            ...
                None => workflow.begin_deletion(now),
```
and `begin_deletion` itself states that nothing else is changed — src/state.rs:489-504:
```
    /// Start the teardown `DeleteWorkflow` describes: the workflow moves to
    /// [`WORKFLOW_STATUS_DELETING`] and its removal falls due
    /// [`WORKFLOW_DELETED_AFTER`] later. Nothing else is dropped here — the
    /// versions, runs, name and tokens go with the record when the removal lands,
    /// so a read during the teardown still reports the workflow.
```
The stop the crate already implements, unused by the delete path — src/state.rs:351-369:
```
    /// Start the shutdown `StopWorkflowRun` describes: a run that has not finished
    /// moves to [`RUN_STATUS_STOPPING`], and its arrival in [`RUN_STATUS_STOPPED`]
    /// falls due [`RUN_STOPPED_AFTER`] later ...
    pub fn begin_stop(&mut self, now: SystemTime) -> bool {
```
called only from src/ops/stop_workflow_run.rs:221 (`run.begin_stop(now);`).

The sibling handler that quotes the same clause but applies it only to new runs — src/ops/start_workflow_run.rs:301-313:
```
        // Run the requested version, defaulting to the workflow's latest. A workflow
        // whose teardown has started is not one a run can be admitted to — the
        // cleanup "stop[s] any running executions" — so starting a NEW run on it is
        // the modelled `ConflictException` ...
```
What the run does instead, after the delete — src/state.rs:312-329 (`settle`): `RUN_STATUS_STARTING => QUEUED`, then `RUN_STATUS_QUEUED => { self.status = RUN_STATUS_FAILED...; self.error_message = Some(NO_AIRFLOW_ENGINE.to_string()); }`, and GetWorkflowRun applies it on every read (src/ops/get_workflow_run.rs:235-236: `// The ONE write a read may make: the transitions the clock already owed.` / `run.settle(now);`).

Test coverage: tests/all/workflow_deletion_teardown.rs quotes the same clause in its header (lines 5-10) and its rows carry 0, 1 and 2 runs, but the only run assertion during DELETING is readability — lines 397-409: `"case {:?}: a run of a workflow still being cleaned up is still readable, got {status} {run:?}"` — and lines 433-438 only assert the runs are gone once the record is removed. No test pins the status of a run while its workflow is DELETING, so nothing in the crate justifies the runs continuing to be admitted and failed.

## Steps to Reproduce

1. Create a workflow: CreateWorkflow {Name:"w1", DefinitionS3Location:{Bucket:"defs",ObjectKey:"dags/v1.yaml"}, RoleArn:<valid role arn>} and keep the WorkflowArn.
2. Start a run: StartWorkflowRun {WorkflowArn} and keep the RunId (it answers Status=STARTING).
3. Immediately call DeleteWorkflow {WorkflowArn} (no WorkflowVersion), which answers 200 and leaves the workflow in WorkflowStatus=DELETING.
4. While the workflow still reports DELETING (within WORKFLOW_DELETED_AFTER = 500ms, src/state.rs:100), poll GetWorkflowRun {WorkflowArn, RunId}.
5. Expected: the deleted workflow's running execution has been stopped — the run reports STOPPING and then STOPPED, the states StopWorkflowRun already produces through WorkflowRun::begin_stop.
6. Actual: the run is untouched by the delete and keeps its pre-delete schedule — it reports QUEUED and then FAILED with RunDetail.ErrorMessage = the emulator's NO_AIRFLOW_ENGINE text, i.e. the delete admitted and failed an execution it documents as stopped.
