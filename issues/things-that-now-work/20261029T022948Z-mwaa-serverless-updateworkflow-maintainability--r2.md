Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: d4e6bf007f35abe24a64d7f0825cf70aa7f81d5e

# The "workflow is DELETING, so refuse the write" ConflictException is built twice, once per handler, while the ValidationException half of the same rule is single-homed in state.rs

_Filed: 2026-10-29T02:29:48Z_
_Operation: UpdateWorkflow | Class: maintainability_

> _Note: this issue was identified against the code base at commit `d4e6bf007f35abe24a64d7f0825cf70aa7f81d5e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate states one rule for a write asked of a workflow whose teardown has started: the request is refused and nothing is written; an operation that models `ConflictException` refuses with that, and one that does not (`TagResource`/`UntagResource`) refuses with its `ValidationException`. The second half lives in `crate::state::tearing_down_resource(resource_arn, action)`, reached by both tag handlers with only the clause-completing `action` differing. The first half does not: `update_workflow::tearing_down` and `start_workflow_run::tearing_down` are two copies of the same builder — same message shape (`"Workflow {arn} is {WORKFLOW_STATUS_DELETING} and <action>"`), same `resource_id`, same `resource_type: "workflow"` — differing only in the clause and in which operation's error enum wraps the result. The two copies can drift (one gaining a resource_type or a message change the other does not), which is the same drift the already-fixed `page_size` and configuration-digest consolidations in this crate removed.

The fix is the shape the crate already uses: a `crate::state::tearing_down_conflict(resource_arn, action) -> ConflictException` beside `tearing_down_resource`, with each handler keeping only the one thing it cannot share — wrapping it in its own error variant. Every message stays byte-identical (the clause is passed in exactly as `tearing_down_resource` already takes it), so no response and no existing test changes.

## Evidence

Copy 1 — services/service-com-amazonaws-mwaaserverless-amazonmwaaserverless/src/ops/update_workflow.rs:363-375:
```
/// The modelled refusal for a workflow whose teardown has already started: it is on
/// its way out with every version it holds, so it cannot take a new one.
fn tearing_down(workflow_arn: &str) -> UpdateWorkflowError {
    UpdateWorkflowErrorModeled::ConflictException(ConflictException {
        message: format!(
            "Workflow {workflow_arn} is {} and cannot be updated",
            crate::state::WORKFLOW_STATUS_DELETING
        ),
        resource_id: workflow_arn.to_string(),
        resource_type: "workflow".to_string(),
    })
    .into()
}
```

Copy 2 — src/ops/start_workflow_run.rs:433-445:
```
/// The modelled refusal for a run asked of a workflow whose teardown has already
/// started: the cleanup stops executions rather than admitting new ones.
fn tearing_down(workflow_arn: &str) -> StartWorkflowRunError {
    StartWorkflowRunErrorModeled::ConflictException(ConflictException {
        message: format!(
            "Workflow {workflow_arn} is {} and cannot start a run",
            crate::state::WORKFLOW_STATUS_DELETING
        ),
        resource_id: workflow_arn.to_string(),
        resource_type: "workflow".to_string(),
    })
    .into()
}
```

The single-homed half of the SAME rule, whose own doc names the family both copies belong to — src/state.rs:2795-2814:
```
/// The modelled `ValidationException` for a change asked of a workflow whose
/// teardown has already started, where `action` completes the clause (`cannot be
/// tagged`).
///
/// A workflow in [`WORKFLOW_STATUS_DELETING`] is on its way out and accepts no
/// change ([`Workflow::is_deleting`]), so every operation that would write to it
/// refuses the request and writes nothing. An operation that models
/// `ConflictException` refuses such a write with it; one that does not —
/// `TagResource` and `UntagResource` model no conflict ...
pub fn tearing_down_resource(resource_arn: &str, action: &str) -> ValidationException {
    ValidationException {
        message: format!("Workflow {resource_arn} is {WORKFLOW_STATUS_DELETING} and {action}"),
```
reached by src/ops/tag_resource.rs:225 (`crate::state::tearing_down_resource(&workflow.arn, "cannot be tagged")`) and src/ops/untag_resource.rs:217 (`"cannot be untagged"`).

Tests that cover both copies and would be unchanged by consolidation: tests/all/workflow_deletion_teardown.rs:520-559 drives UpdateWorkflow and StartWorkflowRun against a DELETING workflow and asserts only `status == 409` and that `__type` contains `ConflictException`; tests/all/tag_writes_while_deleting.rs covers the ValidationException half.

## Steps to Reproduce

1. Read src/ops/update_workflow.rs:363-375 and src/ops/start_workflow_run.rs:433-445 side by side: the message template, resource_id and resource_type are identical; only the clause and the wrapping error enum differ.
2. Read src/state.rs:2795-2814: the same rule's ValidationException form is parameterised by `action` and reached by both tag handlers, so the crate already has the one-home shape for this refusal.
3. Change the refusal in one copy (for example add a field or reword the shared template) and observe that the other handler keeps the old form — the drift this duplication allows.
4. Run tests/all/workflow_deletion_teardown.rs: it asserts only the 409 status and the __type, so moving the builder into state.rs with the clause passed in leaves every response byte-identical and the tests untouched.
