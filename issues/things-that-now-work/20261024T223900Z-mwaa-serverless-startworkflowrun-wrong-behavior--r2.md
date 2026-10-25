Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 5b32bce31d5638057695fa0f3e53c870e68936d8

# A StartWorkflowRun ClientToken replay reports the run's stale status, skipping the settle every sibling reader applies

_Filed: 2026-10-24T22:39:00Z_
_Operation: StartWorkflowRun | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `5b32bce31d5638057695fa0f3e53c870e68936d8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The idempotency replay path of `StartWorkflowRun` reads the stored run through an immutable lookup and reports `run.status` without calling `WorkflowRun::settle`, which every other handler that reports a run's status in this crate calls first. Because settle materialises the status changes the clock has ALREADY made due (the crate's own doctrine: the status a reader reports must be "the one the run would have reached unpolled"), a replay issued after `RUN_ADMITTED_AFTER`/`RUN_SETTLE_AFTER` have elapsed answers `STARTING` for a run that `GetWorkflowRun` — called a moment later on the same stored record — reports as `QUEUED` or `FAILED`. Two reads of the same run disagree, and the earlier one is the stale answer.

## Evidence

Replay path without settle, `src/ops/start_workflow_run.rs:278-282`:
```
                let run = store
                    .workflow_in(&region, workflow_arn)
                    .and_then(|workflow| workflow.runs.get(run_id))
                    .ok_or_else(unavailable)?;
                return Ok(response_for(run));
```
and `response_for` reports the stored status verbatim (`src/ops/start_workflow_run.rs:443-448`): `status: Some(run.status.clone())`.

The sibling readers, `src/ops/get_workflow_run.rs:235-236`:
```
        // The ONE write a read may make: the transitions the clock already owed.
        run.settle(now);
```
`src/ops/list_workflow_runs.rs:259-261`:
```
        for run in workflow.runs.values_mut() {
            run.settle(now);
        }
```
The rule they implement, `src/state.rs:240-245`:
```
    /// This is the ONLY write a reporting operation may make: at `now` each
    /// transition below was owed whether or not anybody called, so the status a
    /// reader reports is the one the run would have reached unpolled.
```

Test coverage: `tests/all/workflow_lifecycle.rs:912` (`starting_a_run_is_idempotent_and_an_unknown_workflow_is_refused`) compares only `RunId` between the first answer and the replay (line 947), so the replayed `Status` is unpinned; `tests/all/run_lifecycle_progression.rs` exercises settle only through `GetWorkflowRun`.

## Steps to Reproduce

1. CreateWorkflow {Name: "w", ...}.
2. StartWorkflowRun {WorkflowArn: <arn>, ClientToken: "t1"} — answers Status STARTING.
3. Wait longer than RUN_ADMITTED_AFTER + RUN_SETTLE_AFTER (700 ms).
4. Repeat StartWorkflowRun {WorkflowArn: <arn>, ClientToken: "t1"} — the replay answers Status STARTING.
5. GetWorkflowRun {WorkflowArn: <arn>, RunId: <same run>} — answers FAILED, contradicting the replay just served.
