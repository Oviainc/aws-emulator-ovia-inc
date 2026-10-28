Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 7959e6ca7cb81d5c32ad32f8e1e29786f97ad065

# TagResource and UntagResource write tags onto a workflow whose teardown has already started, which the crate's own rule says accepts no change

_Filed: 2026-10-28T10:01:51Z_
_Operation: TagResource | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `7959e6ca7cb81d5c32ad32f8e1e29786f97ad065`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate states one rule for a workflow in `DELETING`: it "is on its way out and no longer accepts a change" (`Workflow::is_deleting`, src/state.rs:714-718). Two writers enforce it — `UpdateWorkflow` refuses a `DELETING` workflow with the modelled `ConflictException`, and `StartWorkflowRun` does the same for a new run — and the test suite's own module doc restates the rule as a property of the service ("A workflow in `DELETING` is on its way out, so it accepts no change").

The two remaining writers never consult the status at all:

* `TagResource` resolves the workflow and then inserts straight into `workflow.tags`;
* `UntagResource` resolves the workflow and then removes from `workflow.tags`.

So a caller that issues `DeleteWorkflow` and then `TagResource` gets HTTP 200, and `ListTagsForResource` / a later read report the new tag map for a workflow that is being torn down — a change "applied" to a record the same crate says can take none, which then silently disappears when `reap_deleted_workflows` removes the record (src/state.rs:1090-1113). `UntagResource` likewise reports success for removing a key from a record on its way out. Both paths skip a state gate the crate already implements for its other two writers; neither operation models `ConflictException`, but each models `ValidationException`, which is the error this same handler already uses for a service-level rule that is not a member constraint (`crate::state::too_many_tags`, src/state.rs:2748-2769, used at src/ops/tag_resource.rs:222-227).

## Evidence

The rule, in this crate's own state module — src/state.rs:714-718:
```
/// Whether this workflow's teardown has already been started, so it is on its
/// way out and no longer accepts a change.
pub fn is_deleting(&self) -> bool {
    self.deletion_due_at.is_some()
}
```
The sibling writers that enforce it:
* src/ops/update_workflow.rs:264-266 — `if workflow.is_deleting() { return Err(tearing_down(&workflow.arn)); }`, whose comment reads "A workflow whose teardown has started is going away with every version it holds, so a new version cannot be appended to it. That is what the modelled `ConflictException` is for".
* src/ops/start_workflow_run.rs:308-313 — `if store.workflow_in(&region, &input.workflow_arn).is_some_and(crate::state::Workflow::is_deleting) { return Err(tearing_down(&input.workflow_arn)); }`.

The writers that skip it:
* src/ops/tag_resource.rs:201-233 — `let workflow = store.workflow_in_mut(&region, &input.resource_arn).ok_or_else(...)?;` … `for (key, value) in input.tags { workflow.tags.insert(key, value); }` — no status test anywhere in the handler; the only refusal it adds is the tag-count ceiling (lines 216-227).
* src/ops/untag_resource.rs:193-211 — `let workflow = store.workflow_in_mut(&region, &input.resource_arn).ok_or_else(...)?;` … `for key in &input.tag_keys { workflow.tags.remove(key); }`.

Test coverage: tests/all/workflow_deletion_teardown.rs:19-23 states the rule — "A workflow in `DELETING` is on its way out, so it accepts no change: `UpdateWorkflow` and `StartWorkflowRun` refuse it with the modelled `ConflictException`" — and the suite that exercises it, `a_workflow_being_deleted_accepts_no_change` (tests/all/workflow_deletion_teardown.rs:488-559), drives exactly three rows: `UpdateWorkflow`, `StartWorkflowRun` and `CreateWorkflow`. No test in tests/all calls `TagResource` or `UntagResource` against a workflow in `DELETING` (tests/all/resource_tagging.rs and tests/all/resource_untagging.rs tag only `READY` workflows), so nothing pins today's accepting behaviour.

## Steps to Reproduce

1. Boot the emulator and point an airflow-serverless client at it.
2. CreateWorkflow(Name='TeardownTags', DefinitionS3Location={'Bucket':'defs','ObjectKey':'dags/d.yaml'}, RoleArn='arn:aws:iam::000000000000:role/wf-exec', Tags={'Team':'a'}) and note the returned WorkflowArn.
3. DeleteWorkflow(WorkflowArn=<arn>) and immediately GetWorkflow(WorkflowArn=<arn>) — WorkflowStatus is 'DELETING'.
4. UpdateWorkflow(WorkflowArn=<arn>, DefinitionS3Location={'Bucket':'defs','ObjectKey':'dags/late.yaml'}, RoleArn='arn:aws:iam::000000000000:role/wf-exec') — refused 409 ConflictException, the documented rule.
5. TagResource(ResourceArn=<arn>, Tags={'Added':'while-deleting'}) — observe HTTP 200, and ListTagsForResource(<arn>) now reports the new key on a workflow that is tearing down.
6. UntagResource(ResourceArn=<arn>, TagKeys=['Team']) — observe HTTP 200 and the key removed from the same record, which the clock then discards entirely a moment later.
