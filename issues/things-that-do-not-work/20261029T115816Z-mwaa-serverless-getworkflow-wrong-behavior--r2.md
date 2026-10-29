Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 0577155811cbe97e717443f83f93d91d777f503b

# GetWorkflow reports CodeSnapshottedAt for a workflow that carries no code artifacts

_Filed: 2026-10-29T11:58:16Z_
_Operation: GetWorkflow | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `0577155811cbe97e717443f83f93d91d777f503b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`GetWorkflow` reports `CodeSnapshottedAt` unconditionally, filled with the active version's creation instant, even for a workflow whose `CreateWorkflow`/`UpdateWorkflow` supplied no `Code` at all. The same response then reports `Code` as absent, so one response simultaneously says "this workflow has no code artifacts" and "its code artifacts were copied at T".

The value is not a copy instant in any sense: `crate::state::Workflow::promote_newest_version` assigns `code_snapshotted_at` from the version record's `created_at`, and nothing in the crate ever reads the `Code` S3 location — `input.code` is only cloned into the version snapshot. That is in contrast to the *definition* location, which the same handler family genuinely reads through the crate's own in-process S3 client (`workflow_definition::snapshot_definition`). So for a workflow created without `Code`, the member reports a timestamp for an event that both did not happen and has no subject.

The crate's own convention for an optional output member with nothing behind it is to omit it (quoted from two sibling readers below). `CodeSnapshottedAt` is the one member that breaks that convention. The narrow defect: report `CodeSnapshottedAt` only when the version being reported actually froze a `Code` location, and leave it absent otherwise — exactly as `Code` itself is left absent.

## Evidence

Model (smithy_query doc, GetWorkflowResponse$CodeSnapshottedAt): "<p>The time at which the code artifacts were copied for this workflow, in ISO 8601 date-time format.</p>"
Model (smithy_query doc, CreateWorkflowRequest$Code): "<p>The location of code artifacts in Amazon S3 for the workflow. The service copies the code from this location at the time of the request.</p>"

Handler — the member is emitted whatever the record holds, right beside the `Code` it reports as absent:
`services/.../src/ops/get_workflow.rs:277-283`
```
            code: snapshot.code.clone(),
            // The definition snapshot is taken when a version is created.
            code_snapshotted_at: Some(crate::state::timestamp(if requested.is_some() {
                record.created_at
            } else {
                workflow.code_snapshotted_at
            })),
```
Note the comment itself explains the value by appealing to the DEFINITION snapshot, not to a code copy.

What the stored value actually is — the version's creation instant:
`services/.../src/state.rs:807`
```
        self.code_snapshotted_at = self.versions[newest].created_at;
```

The `Code` location is never read, while the definition location IS read through the crate's own S3 client:
`services/.../src/ops/create_workflow.rs:383` -> `            code: input.code.clone(),`
`services/.../src/ops/create_workflow.rs:274-279` ->
```
        let definition = crate::state::workflow_definition::snapshot_definition(
            ctx,
            &region,
            &input.definition_s3_location,
        )
        .await;
```

The sibling convention this path skips (same crate):
`services/.../src/ops/get_workflow_run.rs:245-247`
```
            // Absent, like every other unset optional member, for a run the caller
            // started with no parameter overrides.
            override_parameters: (!run.override_parameters.is_empty()).then(|| {
```
`services/.../src/ops/get_task_instance.rs:294-296`
```
            // Absent, like every other unset optional member, for an instance that
            // exchanged no cross-task data.
            xcom: (!task.xcom.is_empty()).then(|| {
```

Test coverage: the suites create workflows with no `Code` at all, e.g. `services/.../tests/all/deleted_workflow_run_cleanup.rs:70-76`
```
            serde_json::json!({
                "Name": name,
                "DefinitionS3Location": {"Bucket": "defs", "ObjectKey": "dags/v1.yaml"},
                "RoleArn": role_arn(),
            }),
```
and `services/.../tests/all/workflow_lifecycle.rs` reads those workflows back with `GetWorkflow`; no existing test asserts that `CodeSnapshottedAt` is present for a workflow without `Code`, so leaving it absent in that case changes no pinned expectation.

## Steps to Reproduce

1. Call CreateWorkflow with only `Name`, `DefinitionS3Location` and `RoleArn` — no `Code` member (the same request shape the crate's own tests use).
2. Call GetWorkflow with the returned `WorkflowArn`.
3. Observe the response omits `Code` (nothing was supplied, nothing was copied) yet carries `CodeSnapshottedAt`, equal to the response's `CreatedAt`.
4. Compare with the modelled meaning of the member: "The time at which the code artifacts were copied for this workflow" — no code artifact exists, and the crate never reads a `Code` location, so no copy ever occurred.
