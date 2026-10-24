Issue-Severity: MEDIUM
Issue-Gap-Class: event-propagation
Issue-Commit: 7805cfa6f36158072c58d31caff72f5961880244

# Workflow run logs are never delivered to the configured CloudWatch Logs log group and LogStream is never populated

_Filed: 2026-10-24T07:17:57Z_
_Operation: StartWorkflowRun | Class: event-propagation_

> _Note: this issue was identified against the code base at commit `7805cfa6f36158072c58d31caff72f5961880244`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: a workflow created with a `LoggingConfiguration.LogGroupName` has its run/worker/task-level output exported to that CloudWatch Logs log group — the model documents `LoggingConfiguration` as "The configuration for workflow logging. Specifies the CloudWatch log group where workflow execution logs are stored. Amazon Managed Workflows for Apache Airflow Serverless automatically exports worker logs and task-level information to the specified log group in your account using remote logging functionality" — and each task instance names the stream it wrote to, via `GetTaskInstanceResponse.LogStream` ("The CloudWatch log stream name for this task instance execution"). So after StartWorkflowRun, a client inspecting the emulated CloudWatch Logs service (DescribeLogStreams / GetLogEvents on that log group) finds streams for the run, and GetTaskInstance reports a non-null LogStream pointing at one of them.

Actual: the mwaa-serverless crate has no cross-service interaction whatsoever. `LoggingConfiguration` is accepted by CreateWorkflow/UpdateWorkflow and cloned into `crate::state::WorkflowSnapshot`, where it is only ever read back out by GetWorkflow; nothing in `src/` ever touches the CloudWatch Logs emulator (no log group is created, no log stream is opened, no event is put). `TaskInstance::log_stream` exists in state but is only ever set by test fixtures, so GetTaskInstance's `LogStream` is always null for anything a client can produce through the API. A client that starts a run and then goes looking for its execution logs in the log group it configured finds the log group absent/empty, where AWS has worker and task logs there.

Fix direction: when a run is started (and as its task instances are produced), deliver the run's log output to the emulated CloudWatch Logs service under the workflow's configured `LogGroupName` — creating the log group/streams as the real remote-logging integration does — and record the resulting stream name on the task-instance record so GetTaskInstance/ListTaskInstances report it. This is distinct from, but downstream of, the already-filed run-execution gap (issue `20261024T002400Z-mwaa-serverless-startworkflowrun-wrong-behavior`, class wrong-behavior): even the FAILED settlement the emulator does produce emits no log delivery at all.

## Evidence

Smithy model: `CreateWorkflowRequest$LoggingConfiguration` doc ("automatically exports worker logs and task-level information to the specified log group in your account using remote logging functionality"); `LoggingConfiguration$LogGroupName` doc ("The name of the CloudWatch log group where workflow execution logs are stored"); `GetTaskInstanceResponse$LogStream` / `TaskInstanceSummary$LogStream` doc ("The CloudWatch log stream name for this task instance execution"). Handler/state evidence: `src/ops/create_workflow.rs` lines ~333-343 (logging_configuration cloned into the snapshot and nothing else), `src/ops/update_workflow.rs` lines ~268-278 (same), `src/ops/start_workflow_run.rs` lines ~303-337 (the run record is built and inserted with no log-group side effect; `task_instances: BTreeMap::new()`), `src/state.rs` line ~223 (`LogStream`: the CloudWatch log stream this instance logs to — a field only populated from outside the API). A repo-wide grep over this crate's `src/` for cloudwatch/logs/cross-service handles returns only doc comments and `definition_s3_location` field names — the crate reaches no other emulated service, although the repository does ship a CloudWatch Logs emulator (`services/service-com-amazonaws-cloudwatchlogs-logs-20140328`). Test coverage: `tests/all/task_instance_reads.rs` seeds `TaskInstance` values (including `log_stream`) straight into `crate::state` via its `seed()` helper precisely because no operation produces them; no test asserts any CloudWatch Logs delivery. No existing issue mentions logging/CloudWatch for this service (the open StartWorkflowRun issue is about run status/task instances only).

## Steps to Reproduce

1. Call CreateWorkflow with a valid Name, RoleArn and DefinitionS3Location plus LoggingConfiguration={"LogGroupName": "/aws/mwaa-serverless/demo"}; note the returned WorkflowArn.
2. Call StartWorkflowRun on that WorkflowArn and note the RunId.
3. Wait a second, then call GetWorkflowRun and GetTaskInstance for the run.
4. Query the emulated CloudWatch Logs service for log group "/aws/mwaa-serverless/demo" (DescribeLogGroups / DescribeLogStreams / GetLogEvents).
5. Observe the log group does not exist and holds no streams or events, and that no task instance (hence no LogStream) is reachable, where the real service exports worker and task-level logs to that log group and reports the stream name in GetTaskInstance.LogStream.
