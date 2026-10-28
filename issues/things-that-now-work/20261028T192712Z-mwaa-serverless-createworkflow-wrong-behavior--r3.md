Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: e8dc968dccb3edde6e1cac399927b1a6964cab15

# CreateWorkflow and UpdateWorkflow materialise the clock from an instant sampled before the S3 definition read, so a teardown that fell due during that read is not finished and the name it holds is still refused

_Filed: 2026-10-28T19:27:12Z_
_Operation: CreateWorkflow | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `e8dc968dccb3edde6e1cac399927b1a6964cab15`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Both write paths that snapshot the definition sample the wall clock BEFORE awaiting a call into the emulated Amazon S3 service, and then use that pre-await instant for `store.advance_clock(now)` and for every timestamp they stamp on the record they write. The other thirteen handlers sample the clock adjacent to the lock, with nothing awaited in between, so for them `advance_clock` really is "what the store would hold unpolled".

For these two it is not: any workflow teardown whose `WORKFLOW_DELETED_AFTER` deadline, or any scheduled firing, falls between the clock sample and the lock is NOT materialised, although it is already due by the time the handler looks at the store. The consequence is exactly the one the handler's own comment says it prevents: CreateWorkflow then finds the stale record still holding its name and refuses the create with `ConflictException` (409) on a name that is in fact free — the same request issued through any other operation first succeeds. The record's `created_at`/`modified_at`/`code_snapshotted_at` are also stamped from before the read, so a concurrent writer that took the lock during the read can have its later `modified_at` overwritten with the earlier instant.

## Evidence

`services/.../src/ops/create_workflow.rs:267-289` — the clock is read, then the S3 snapshot is awaited, and the stale instant drives the clock materialisation:
```rust
        let now = SystemTime::now();
        let fingerprint = request_fingerprint(&input);
...
        let definition = crate::state::workflow_definition::snapshot_definition(
            ctx, &region, &input.definition_s3_location,
        )
        .await;
        let mut store = state.lock();
...
        store.advance_clock(now);
```
and the stated intent that breaks, `create_workflow.rs:285-288`: "Finish every workflow teardown the clock already made due, so a name held by a workflow whose `DELETING` span has elapsed is free for this create and one still tearing down still holds it".

Same shape in `services/.../src/ops/update_workflow.rs:221-241` (`let now = SystemTime::now();` … `snapshot_definition(...).await;` … `let mut store = state.lock();` … `store.advance_clock(now);`).

The rule the shared helper states, `services/.../src/state.rs:1222-1225`: "Every handler that touches [`Self::workflows`] calls this FIRST, so what it answers is what the store would hold unpolled: a workflow never outlives its teardown, and a trigger never fails to fire, just because the next request happened to be a different operation."

The sibling handlers that do it the other way (clock sampled at the lock, nothing awaited in between): `get_workflow.rs:227-233` (`let mut store = state.lock(); ... store.advance_clock(SystemTime::now());`), `list_workflows.rs:184-189`, `tag_resource.rs:193-198`, `untag_resource.rs:185-190`, `list_tags_for_resource.rs:189-194`, and `delete_workflow.rs:207-213` (`let now = SystemTime::now(); let mut store = state.lock(); ... store.advance_clock(now);`).

The deadline that can elapse inside the window, `services/.../src/state.rs:119`: `pub const WORKFLOW_DELETED_AFTER: Duration = Duration::from_millis(500);`

Test coverage: `tests/all/workflow_deletion_teardown.rs:505-559` pins the DELETING-phase refusals by pinning `deletion_due_at` out of reach, and `tests/all/workflow_lifecycle.rs:437-450` pins the duplicate-name 409 — neither exercises a deadline that falls due during the definition read.

## Steps to Reproduce

1. CreateWorkflow with Name="recycled" and a DefinitionS3Location whose object exists; note the WorkflowArn.
2. DeleteWorkflow on that ARN (the record moves to DELETING and its removal falls due WORKFLOW_DELETED_AFTER = 500 ms later; the name stays taken until then).
3. Immediately CreateWorkflow again with Name="recycled", pointing DefinitionS3Location at an object whose in-process read takes longer than the remaining DELETING span.
4. Expected: the teardown is already due when the store is locked, so the name is free and the create succeeds (the handler's own stated rule).
5. Actual: `advance_clock` is driven with the instant sampled before the S3 read, the stale DELETING record is not reaped, and the create is refused 409 ConflictException "A workflow named recycled is DELETING and still holds that name".
