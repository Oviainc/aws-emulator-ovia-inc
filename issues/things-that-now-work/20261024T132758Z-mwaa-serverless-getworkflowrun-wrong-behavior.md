Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: d9cda5a9b6bc2cdf59d194d1abe409fa5e2cae3f

# Every workflow run settles to FAILED with an emulator-authored ErrorMessage; RUNNING and SUCCESS are unreachable

_Filed: 2026-10-24T13:27:58Z_
_Operation: GetWorkflowRun | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `d9cda5a9b6bc2cdf59d194d1abe409fa5e2cae3f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: a run started on a READY workflow with a valid definition walks the modeled WorkflowRunStatus lifecycle and reaches a real outcome, so GetWorkflowRun reports RunDetail.RunState moving STARTING/QUEUED -> RUNNING -> SUCCESS (or FAILED/TIMEOUT when the DAG itself fails/times out), with RunDetail.CompletedOn and RunDetail.Duration set at completion and RunDetail.ErrorMessage null for a successful run (the model documents ErrorMessage as 'The error message if the workflow run failed. This value is null if the run completed successfully').

Actual: the run state machine in crate::state has exactly two transitions, STARTING -> QUEUED (after RUN_ADMITTED_AFTER) and QUEUED -> FAILED (after RUN_SETTLE_AFTER). The terminal branch unconditionally writes status = RUN_STATUS_FAILED and ErrorMessage = the emulator-authored constant NO_AIRFLOW_ENGINE ('The emulator accepted this run but has no Apache Airflow execution engine, so the workflow definition was never executed.'), plus a Duration computed over the admission span. RUNNING, SUCCESS, TIMEOUT, STOPPING and STOPPED are never written by any code path, so GetWorkflowRun (and the RunDetailSummary that ListWorkflowRuns derives from the same record) can only ever report STARTING, QUEUED or FAILED. Any client that starts a run and polls GetWorkflowRun for completion observes a failure AWS never produces, with a message string that exists only in this emulator, roughly 700ms after the start.

History: the run-execution gap was filed against StartWorkflowRun (issues/things-that-now-work/20261024T002400Z-mwaa-serverless-startworkflowrun-wrong-behavior.md) and marked fixed; that fix only introduced the STARTING -> QUEUED admission step and the OverrideParameters omission. The run-status half of that gap is still in the code and is currently untracked (the open GetTaskInstance issue explicitly covers only the task-instance half). This issue tracks it on the operation where it is observed.

## Evidence

Smithy model: WorkflowRunStatus = FAILED|QUEUED|RUNNING|STARTING|STOPPED|STOPPING|SUCCESS|TIMEOUT (smithy_query shape=WorkflowRunStatus); WorkflowRunDetail.ErrorMessage doc 'This value is null if the run completed successfully'; WorkflowRunDetail.CompletedOn/Duration docs 'null if the run is not complete' — i.e. a completed, successful run is a modeled outcome. Handler/state: src/ops/get_workflow_run.rs lines ~229-269 (reads run.status into RunDetail.RunState and run.error_message into RunDetail.ErrorMessage after calling run.settle(now)); src/state.rs WorkflowRun::settle lines ~232-256 (match on STARTING -> QUEUED, QUEUED -> RUN_STATUS_FAILED + NO_AIRFLOW_ENGINE, no other transition exists) and the constants RUN_ADMITTED_AFTER / RUN_SETTLE_AFTER / NO_AIRFLOW_ENGINE (src/state.rs lines ~52-69). grep over src/ shows RUN_STATUS_RUNNING/SUCCESS are never assigned. The repo's recorded behavioral trace set for this service already names the real lifecycle: mwaa-serverless.workflow-run-status-progresses-through-lifecycle-states, mwaa-serverless.workflow-run-success-end-to-end, mwaa-serverless.workflow-run-failure-records-error-message (listed in issues/things-that-now-work/20261022T212900Z-mwaa-serverless-s3-putpublicaccessblock-...md). Test coverage asserts the divergence rather than the AWS behavior: tests/all/run_lifecycle_progression.rs rows() pins expected_state = RUN_STATUS_FAILED with expected_error = NO_AIRFLOW_ENGINE for every row whose terminal change is due, and tests/all/workflow_lifecycle.rs::a_run_leaves_its_non_terminal_status_when_the_clock_makes_the_change_due asserts the same; no test anywhere mentions RUNNING or SUCCESS.

## Steps to Reproduce

1. CreateWorkflow with a valid Name, RoleArn and DefinitionS3Location pointing at a DAG that would succeed; note the WorkflowArn.
2. StartWorkflowRun on that WorkflowArn; note the RunId (Status = STARTING).
3. Call GetWorkflowRun with that WorkflowArn/RunId repeatedly for a few seconds.
4. Observe RunDetail.RunState go STARTING -> QUEUED -> FAILED and stop there, with RunDetail.ErrorMessage = 'The emulator accepted this run but has no Apache Airflow execution engine, so the workflow definition was never executed.' and a Duration covering only the admission span.
5. Observe that no sequence of calls ever reports RunState = RUNNING or RunState = SUCCESS, and that ErrorMessage is never null for a completed run, where the real service reports RUNNING while the DAG executes and SUCCESS with ErrorMessage null when it finishes.
