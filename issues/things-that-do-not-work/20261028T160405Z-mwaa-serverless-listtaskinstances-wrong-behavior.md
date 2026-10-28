Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 5f1c460e5544105d602e680f733bf81495110ec8

# A task instance's OperatorName is reported verbatim from the definition, unchecked against the member's modelled 1..255 length

_Filed: 2026-10-28T16:04:05Z_
_Operation: ListTaskInstances | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `5f1c460e5544105d602e680f733bf81495110ec8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every other string this crate derives from caller-supplied definition content and then REPORTS is held to the length the model declares for the member that carries it: a run's `ErrorMessage` is clipped to `MAX_ERROR_MESSAGE_CHARS` ("the modelled length of the `GenericString` it targets"), and a declared task identifier is run through the framework's constraint walk over `M_TASK_INSTANCE_ID` (`IdString`, 1..255) by `reportable_task_id`, with a definition that declares an out-of-range identifier failing the whole run.

`OperatorName` is the one definition-derived reported string that gets neither. The value of the task entry's `operator` / `operator_name` / `type` key is taken as the YAML scalar writes it into `TaskDeclaration::operator_name`, copied into `PlannedTask::operator_name`, stored on the `TaskInstance`, and handed straight to the response by both `ListTaskInstances::summarize` and `GetTaskInstance`. The definition content may be up to `MAX_DEFINITION_BYTES` (16 KiB), so a task declaring `operator:` with 300 characters — or with an empty string — makes these two reads answer with a `TaskInstanceSummary$OperatorName` / `GetTaskInstanceResponse$OperatorName` outside the `@length(min: 1, max: 255)` the model declares for it, which is exactly the divergence the sibling two strings are checked to avoid.

## Evidence

The modelled bound (`smithy_query(query="shape", shape="TaskInstanceSummary")`): `"OperatorName": {"target": "GenericString", "length": {"min": 1, "max": 255}, "pattern": "^.*$", "doc": "<p>The name of the Apache Airflow operator used for this task instance.</p>"}`.

The crate holds its OTHER two definition-derived reported strings to their modelled lengths — `services/service-com-amazonaws-mwaaserverless-amazonmwaaserverless/src/state/workflow_execution.rs:48-51`:
```
/// The most characters a run's `ErrorMessage` carries: the modelled length of the
/// `GenericString` it targets (`smithy_query(query="shape", shape="WorkflowRunDetail")`
/// declares `ErrorMessage` 1..255).
const MAX_ERROR_MESSAGE_CHARS: usize = 255;
```
and `src/state/workflow_execution.rs:226-235`:
```
/// Whether a declared task identifier is one this service can REPORT: `TaskInstanceId`
/// and `TaskId` both target the modelled `IdString`, so one check covers the identifier a
/// run's instance is stored under and the task it names.
...
fn reportable_task_id(task_id: &str) -> bool {
    super::validate_members(&[(&M_TASK_INSTANCE_ID, ConstrainedValue::Text(task_id))]).is_empty()
}
```
The operator name is read with no such check, `src/state/workflow_definition.rs:78-81`:
```
/// The keys a task entry names the operator that runs it under — what
/// `TaskInstanceSummary$OperatorName` ("The name of the Apache Airflow operator used for
/// this task instance") reports.
const TASK_OPERATOR_KEYS: &[&str] = &["operator", "operator_name", "type"];
```
stored as written, `src/state/workflow_definition.rs:682`: `TaskField::Operator => task.operator_name = Some(value),`, carried into the plan at `src/state/workflow_execution.rs:220`: `operator_name: task.operator_name.clone(),`, and reported unchanged by `src/ops/list_task_instances.rs:323` (`operator_name: task.operator_name.clone(),`) and `src/ops/get_task_instance.rs:284` (`operator_name: task.operator_name.clone(),`).

Tests cover only short operator names: `tests/all/task_instance_listing.rs` and `tests/all/run_task_instances.rs` declare ordinary operator spellings, so no case exercises the boundary the other two strings have one for.

## Steps to Reproduce

1. Put a definition object in the emulated S3 whose first document declares `tasks:` with one entry carrying `operator: <a 300-character scalar>` (the whole file stays well inside MAX_DEFINITION_BYTES).
2. CreateWorkflow pointing DefinitionS3Location at that object.
3. StartWorkflowRun on the workflow and wait until the run leaves QUEUED so its task instances are recorded.
4. Call ListTaskInstances (and GetTaskInstance) for that run.
5. Observe the answered OperatorName is the full 300-character value, past the @length(min: 1, max: 255) the model declares for TaskInstanceSummary$OperatorName; repeat with `operator: ""` to get an empty OperatorName, below the same member's minimum. Compare with a 300-character `operator` key spelled as a task id, which reportable_task_id refuses outright.
