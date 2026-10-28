Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 9f84f44c6767c5c874b7cc6e16fb6469b8a0f678

# Report OperatorName only when the declared value fits the modelled GenericString

_Filed: 2026-10-28T23:39:30Z_
_Operation: GetTaskInstance | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `9f84f44c6767c5c874b7cc6e16fb6469b8a0f678`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The operator name a workflow definition declares is carried straight from the YAML into the task instance and reported verbatim by GetTaskInstance (and, from the same record, by ListTaskInstances), with no check that the value is one the modelled member can carry. Both `GetTaskInstanceResponse$OperatorName` and `TaskInstanceSummary$OperatorName` target `GenericString`, which the model declares `@length(min: 1, max: 255)`.

So a definition whose task entry declares `operator: ""` produces a task instance that GetTaskInstance answers with `"OperatorName": ""` — a value below the modelled minimum of 1 — and one declaring a 300-character operator name is reported with all 300 characters, past the modelled maximum of 255. Nothing refuses the task, nothing elides the member, and nothing clips it.

This is the ONE value this service reports out of the caller's definition file without the "is this reportable?" step the very same two modules apply to every other such value: the declared task identifier is refused through the framework's own constraint walk over the modelled member schema, the run's `ErrorMessage` is clipped to the modelled 255 characters, and the snapshotted definition content is dropped when the object is empty precisely because the member it feeds is modelled with a minimum length of 1. Only `operator_name` is passed through unexamined, and neither module carries a comment, constant or test justifying that one exception (contrast `MAX_DEFINITION_BYTES`, where the crate does state and justify its divergence from a modelled length).

## Evidence

Model (smithy_query query="shape" shape="TaskInstanceSummary,GetTaskInstanceResponse"):
  TaskInstanceSummary: `"OperatorName": {"target": "GenericString", "length": {"min": 1, "max": 255}, "pattern": "^.*$", "doc": "<p>The name of the Apache Airflow operator used for this task instance.</p>"}`
  GetTaskInstanceResponse: `"OperatorName": {"target": "GenericString", "length": {"min": 1, "max": 255}, "pattern": "^.*$"}`

The unchecked path — the declared scalar becomes the reported value, unchanged, at every hop:
  services/.../src/state/workflow_definition.rs:682 — `TaskField::Operator => task.operator_name = Some(value),`
  services/.../src/state/workflow_execution.rs:220 — `operator_name: task.operator_name.clone(),` (inside `plan_tasks`, building `PlannedTask`)
  services/.../src/state.rs:463 — `operator_name: task.operator_name.clone(),` (inside `WorkflowRun::start_declared_tasks`)
  services/.../src/ops/get_task_instance.rs:284 — `operator_name: task.operator_name.clone(),`
  services/.../src/ops/list_task_instances.rs:323 — `operator_name: task.operator_name.clone(),`

The same rule the crate DOES apply to every sibling value read out of the same definition:
  services/.../src/state/workflow_execution.rs:226-235 — "/// Whether a declared task identifier is one this service can REPORT: `TaskInstanceId` and `TaskId` both target the modelled `IdString`, so one check covers the identifier ... The check is the framework's own constraint walk over that member's schema" / `fn reportable_task_id(task_id: &str) -> bool { super::validate_members(&[(&M_TASK_INSTANCE_ID, ConstrainedValue::Text(task_id))]).is_empty() }`, used at workflow_execution.rs:146-151 to refuse the whole graph (`"Task id {task_id:?} has to be made of at most 255 alphanumeric characters..."`).
  services/.../src/state/workflow_execution.rs:48-51, 237-244 — "/// The most characters a run's `ErrorMessage` carries: the modelled length of the `GenericString` it targets ... declares `ErrorMessage` 1..255" / `fn clipped(message: String) -> String`, "so a long task identifier in it cannot push the reported reason past its declared length".
  services/.../src/state/workflow_definition.rs:225-229 — "// `WorkflowDefinition` is modelled with a minimum length of 1, so an object holding no bytes holds no definition content to report." / `if content.is_empty() { return None; }`

Test coverage: tests/all/task_instance_reads.rs and tests/all/task_instance_listing.rs exercise OperatorName only with ordinary operator names; tests/all/run_version_task_graph.rs pins the unreportable-task-id refusal. No test asserts anything about an empty or over-long declared operator name, so no stated reason covers the divergence.

## Steps to Reproduce

1. Put a definition object in the emulated S3 whose first document declares one task with an empty operator: "tasks:\n  - id: t1\n    operator: \"\"\n".
2. CreateWorkflow with DefinitionS3Location pointing at that object, then StartWorkflowRun on the returned WorkflowArn.
3. Poll GetWorkflowRun until the run reports RUNNING, so the task instances its definition declared have been recorded.
4. Call GetTaskInstance with that WorkflowArn/RunId and TaskInstanceId="t1". Expected: no OperatorName at all (the declared value is not one the modelled member can carry, the same conclusion the crate reaches for an empty definition object), Actual: the response carries "OperatorName": "", below GenericString's modelled @length(min: 1).
5. Repeat with "operator: <300 'a' characters>": GetTaskInstance (and ListTaskInstances) report all 300 characters, past GenericString's modelled max of 255.
