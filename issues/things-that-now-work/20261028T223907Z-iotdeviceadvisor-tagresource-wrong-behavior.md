Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: 67d3ef025eb7c6b921e8b895d06c7f32bf688664

# TagResource records the audit outcome "tagged" for a call that changed nothing, unlike every sibling mutating handler

_Filed: 2026-10-28T22:39:07Z_
_Operation: TagResource | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `67d3ef025eb7c6b921e8b895d06c7f32bf688664`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every mutating handler in this crate opens an `AuditedCall` and reports how the call ended, and `AuditedCall::ended` documents that value as "what it changed, or the no-op it was". Four of the five sibling handlers honour that distinction explicitly: `DeleteSuiteDefinition` reports `absent` vs `deleted`, `StopSuiteRun` reports `stopping` vs `unchanged-already-stopping-or-finished`, `UntagResource` reports how many of the named keys it actually removed, and `CreateSuiteDefinition` reports `idempotent-replay` for a retry that minted nothing.

`TagResource` is the one handler that reports a single unconditional `tagged`, although it has a no-op path that the model explicitly allows: `TagResourceRequest$tags` is `TagMap`, `len 0..50` and required, so `{"tags": {}}` is a legal request. On that request `merge_tags` returns `Ok(())` through its `if tags.is_empty() { return Ok(()); }` short-circuit WITHOUT creating or touching a tag row — the store is byte-for-byte unchanged — and the handler then writes an audit record claiming the resource was `tagged`. The audit trail is what the crate offers for making a mutation attributable after the fact (state.rs:782-799), so a record asserting a change that provably did not happen is a wrong record, and it is inconsistent with the four siblings that distinguish the same situation.

## Evidence

The rule the outcome value is defined by — services/service-com-amazonaws-iotdeviceadvisor-iotsenateservice/src/state.rs:872-875:
```
    /// How a call that was NOT refused ended — what it changed, or the no-op it was.
    pub fn ended(&mut self, outcome: impl Into<String>) {
        self.outcome = outcome.into();
    }
```
The no-op path that still records a change — src/state.rs:617-619 (`merge_tags`):
```
        if tags.is_empty() {
            return Ok(());
        }
```
(and its doc at src/state.rs:602-603: "An empty row is dropped rather than kept, so a resource with no tags has no row in either direction.")

The handler — src/ops/tag_resource.rs:189-207, where the only non-refusal outcome is unconditional:
```
        if let Err(would_hold) = store.merge_tags(
            &input.resource_arn,
            tags,
            crate::state::MAX_TAGS_PER_RESOURCE,
        ) {
            ...
        }

        trail.ended("tagged");
```
The siblings that DO distinguish a no-op in this same crate:
* src/ops/delete_suite_definition.rs:158 `let mut outcome = "absent";` … :175 `outcome = "deleted";`
* src/ops/stop_suite_run.rs:174-180 `let outcome = if run.is_stoppable() { … "stopping" } else { "unchanged-already-stopping-or-finished" };`
* src/ops/untag_resource.rs:199-202 `trail.ended(format!("untagged {removed} of {} key(s)", input.tag_keys.len()));`
* src/ops/create_suite_definition.rs:214 `trail.ended("idempotent-replay");`

Test coverage: tests/all/audit_trail.rs:10-16 states the table's scope — "a mutating call leaves EXACTLY ONE record saying how it ended … The table spans the class that rule generalises over: every mutating operation this service serves, and for each one every way its call can end — completed, a no-op, and each refusal it answers" — but the TagResource rows (tests/all/audit_trail.rs:432 `"tagged"`, :572-577 `"tagged"` for a merge up to the limit, :595-600 the refusal) contain NO no-op case, so this path is unpinned rather than deliberately chosen.

## Steps to Reproduce

1. Start the emulator and create a suite definition with `POST /suiteDefinitions`, noting its `suiteDefinitionArn`.
2. Capture the `emulator::audit` tracing target (as tests/all/audit_trail.rs does).
3. Call `POST /tags/{suiteDefinitionArn}` with the body `{"tags": {}}` — legal, since `TagMap` is `len 0..50`.
4. Observe HTTP 200 and an audit record with `outcome=tagged`, while `GET /tags/{suiteDefinitionArn}` still answers `{}` and the store holds no tag row for that ARN: nothing was changed.
5. Compare with `DELETE /tags/{arn}?tagKeys=absent` (records `untagged 0 of 1 key(s)`) and `POST /suiteDefinitions/{id}/suiteRuns/{runId}/stop` on a finished run (records `unchanged-already-stopping-or-finished`) — the siblings report the no-op, TagResource claims a change.
