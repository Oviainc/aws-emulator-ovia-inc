Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: aeaddd52642475574d853bf8e089296dd7f2f86c

# A started workflow run never executes: it always settles to FAILED with an emulator-authored error and no task instances

_Filed: 2026-10-24T00:24:00Z_
_Operation: StartWorkflowRun | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `aeaddd52642475574d853bf8e089296dd7f2f86c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: StartWorkflowRun on a READY workflow with a valid definition starts a run that progresses through the modeled WorkflowRunStatus lifecycle (QUEUED/STARTING -> RUNNING -> SUCCESS) and accumulates task instances, so a later GetWorkflowRun reports RunState=SUCCESS, CompletedOn/Duration set, ErrorMessage null, and RunDetail.TaskInstances naming the tasks of the DAG (the model's WorkflowRunDetail documents ErrorMessage as 'null if the run completed successfully' and TaskInstances as 'A list of task instance IDs that are part of this workflow run').

Actual: every run the emulator accepts is doomed. `StartWorkflowRun` stores the run with status QUEUED and `settles_at = now + RUN_SETTLE_AFTER` (500ms); `WorkflowRun::settle` then rewrites the status to the constant `RUN_STATUS_FAILED` and sets ErrorMessage to the emulator-authored string `NO_AIRFLOW_ENGINE` ('The emulator accepted this run but has no Apache Airflow execution engine, so the workflow definition was never executed.'). No intermediate RUNNING state is ever observable, SUCCESS is unreachable, and `task_instances` is initialised to `Vec::new()` and never appended to, so RunDetail.TaskInstances is always empty. Secondary symptom in the same response: GetWorkflowRun always emits `OverrideParameters` (an empty map when the run carried none) rather than omitting the member.

A client that starts a run and polls for completion therefore always sees a failure with a message AWS never emits, and nothing downstream (task instances, run success, run duration) can be exercised.

## Evidence

Smithy model: WorkflowRunStatus = FAILED|QUEUED|RUNNING|STARTING|STOPPED|STOPPING|SUCCESS|TIMEOUT; WorkflowRunDetail.ErrorMessage doc 'The error message if the workflow run failed. This value is null if the run completed successfully'; WorkflowRunDetail.TaskInstances doc 'A list of task instance IDs that are part of this workflow run'. Handler/state: services/service-com-amazonaws-mwaaserverless-amazonmwaaserverless/src/ops/start_workflow_run.rs (run built with status RUN_STATUS_QUEUED, task_instances: Vec::new(), settles_at) and src/state.rs `WorkflowRun::settle` (sets RUN_STATUS_FAILED + NO_AIRFLOW_ENGINE, lines ~148-162), plus the constants RUN_SETTLE_AFTER/NO_AIRFLOW_ENGINE (~lines 48-64). Behavioral traces for this service already name the real lifecycle: `mwaa-serverless.workflow-run-status-progresses-through-lifecycle-states`, `mwaa-serverless.workflow-run-success-end-to-end`, `mwaa-serverless.workflow-run-failure-records-error-message`, `mwaa-serverless.list-task-instances-for-never-started-run-returns-empty` (see issues/things-that-now-work/20261022T212900Z-mwaa-serverless-s3-putpublicaccessblock-...md). Test coverage: tests/all/workflow_lifecycle.rs `a_run_leaves_its_non_terminal_status_when_the_clock_makes_the_change_due` asserts the divergent RunState == "FAILED"; no test asserts RUNNING or SUCCESS.

## Steps to Reproduce

1. CreateWorkflow with a valid Name, RoleArn and DefinitionS3Location; note the returned WorkflowArn.
2. StartWorkflowRun on that WorkflowArn; note the RunId and Status (QUEUED).
3. Wait ~1 second and call GetWorkflowRun with the same WorkflowArn/RunId.
4. Observe RunDetail.RunState == FAILED and RunDetail.ErrorMessage == 'The emulator accepted this run but has no Apache Airflow execution engine...' with RunDetail.TaskInstances == [], where the real service reports a run that moved RUNNING -> SUCCESS with task instances and no ErrorMessage.
