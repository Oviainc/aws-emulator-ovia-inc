Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: c9f3f250c6ee93c7fb0ade0d9c6dfd55a63e86ad

# GetTaskInstance can never return a task instance: no API path ever creates one, so every call answers 404

_Filed: 2026-10-24T09:03:06Z_
_Operation: GetTaskInstance | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `c9f3f250c6ee93c7fb0ade0d9c6dfd55a63e86ad`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: after StartWorkflowRun on a workflow whose definition declares tasks, the run materialises task instances, and GetTaskInstance(WorkflowArn, RunId, TaskInstanceId) answers 200 with the modeled members (Status drawn from TaskInstanceStatus, TaskId, OperatorName, AttemptNumber, StartedAt/EndedAt, DurationInSeconds, LogStream, Xcom). The model defines GetTaskInstance as a read over a per-run task-instance resource (GET /workflows/{WorkflowArn}/runs/{RunId}/tasks/{TaskInstanceId}) with a rich 200 payload, so a successful read must be reachable for a run the service accepted.

Actual: nothing in src/ops/ ever writes a task instance. StartWorkflowRun creates the run with `task_instances: BTreeMap::new()` (start_workflow_run.rs:330) and no handler, timer or settle step ever inserts into that map (crate::state::WorkflowRun::settle only rewrites run status). GetTaskInstance therefore always falls into its `unresolved_task_instance` branch and answers 404 ResourceNotFoundException for every (workflow, run, task) a client can reach through the API. The only way the stored map becomes non-empty is a test-only in-process seed (tests/all/task_instance_reads.rs::seed), which no HTTP client can perform. Consequence on the wire: the operation's entire 200 surface — and RunDetail.TaskInstances on GetWorkflowRun — is unreachable, so a client that starts a run and then inspects its tasks sees a not-found where AWS returns task detail.

This is the task-instance half of the run-execution gap that was filed against StartWorkflowRun (issues/.../20261024T002400Z-mwaa-serverless-startworkflowrun-wrong-behavior.md, marked fixed) but is still present in the code: that fix changed the initial run status/OverrideParameters emission and left task instances never created. No issue currently names GetTaskInstance, so the remaining symptom is untracked.

## Evidence

Smithy model: GetTaskInstance is a modeled @readonly operation whose GetTaskInstanceResponse declares WorkflowArn/RunId/TaskInstanceId (required) plus Status (TaskInstanceStatus: SCHEDULED|QUEUED|RUNNING|SUCCESS|FAILED|…), TaskId, OperatorName, AttemptNumber, StartedAt, EndedAt, ModifiedAt, DurationInSeconds, ErrorMessage, LogStream, Xcom; TaskInstanceSummary documents 'Summary information about a task instance within a workflow run'. Handler: services/service-com-amazonaws-mwaaserverless-amazonmwaaserverless/src/ops/get_task_instance.rs:248-256 (looks up run.task_instances and 404s on miss). Producer side: src/ops/start_workflow_run.rs:327-330 stores an always-empty task_instances map; src/state.rs:164-169 and 20-23 document that no modeled operation ever creates a task instance (NO_AIRFLOW_ENGINE). Test coverage: tests/all/task_instance_reads.rs exercises GetTaskInstance only by seeding records directly into the store (`seed`, line 253) — no test reaches a 200 through the HTTP surface, and tests/all/run_lifecycle_progression.rs asserts the divergent 'no task instance' outcome.

## Steps to Reproduce

1. CreateWorkflow with a valid Name, RoleArn and DefinitionS3Location; note the returned WorkflowArn.
2. StartWorkflowRun on that WorkflowArn; note the returned RunId.
3. Call GetWorkflowRun for that WorkflowArn/RunId and observe RunDetail.TaskInstances is absent/empty, so no TaskInstanceId is ever discoverable.
4. Call GetTaskInstance with that WorkflowArn/RunId and any TaskInstanceId (e.g. the DAG's task id).
5. Observe HTTP 404 ResourceNotFoundException for every attempt, where the real service returns 200 with the task instance's Status, TaskId, OperatorName, timestamps and DurationInSeconds.
