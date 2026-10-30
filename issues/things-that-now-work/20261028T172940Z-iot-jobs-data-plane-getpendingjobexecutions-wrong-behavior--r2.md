Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 4a5766ed2caf698065daa3a6a1a461ee27489a64

# Nested JobExecutionSummary/JobExecution members with no value are left out of the body instead of reported as JSON null

_Filed: 2026-10-28T17:29:40Z_
_Operation: GetPendingJobExecutions | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `4a5766ed2caf698065daa3a6a1a461ee27489a64`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This service states ONE response rule in `src/ops/types/response_document.rs` and in `src/ops/types/mod.rs`: a 2xx body "carries EVERY member its operation's output shape models, and reports a member it has no value for as JSON `null` rather than leaving the key out". The third of the three recorded cases the module cites as evidence for that rule is a NESTED member: "a queued summary in `GET /things/{thingName}/jobs` carries `\"startedAt\": null` for an execution that has not started".

The implementation only completes the TOP-LEVEL document. `Complete::fill_absent` walks `self.shape.members()` — the operation's own `OUTPUT_SCHEMA` members (`inProgressJobs`, `queuedJobs`) — and writes `null` for the ones the output did not write. A nested struct is handed straight to the inner serializer (`write_struct` passes through), and that serializer, by this same module's own description, "skips an `Option` member holding `None`". So a `JobExecutionSummary` whose `started_at` is `None` reaches the wire WITHOUT a `startedAt` key at all, which is exactly the case the module quotes as the rule's evidence.

The same hole applies to the nested `execution` of `DescribeJobExecution` / `StartNextPendingJobExecution` for every `JobExecution` member the projection leaves `None` other than the two omissions `crate::state::to_job_execution` documents as deliberate (`thingName`, `statusDetails`) — e.g. `startedAt` and `approximateSecondsBeforeTimedOut` on a queued execution with no timer armed.

Neither existing test distinguishes "absent key" from "null": `serde_json`'s `Index` yields `Value::Null` for a missing key, so the lifecycle assertion `summary["startedAt"].is_null()` passes either way, and `response_document_completeness.rs` iterates only the operation's top-level member list.

## Evidence

The rule and its evidence, stated in the crate's own module doc — services/.../src/ops/types/response_document.rs:5-20:
```
//! The IoT Jobs device API answers every 2xx with a JSON object that carries EVERY
//! member its operation's output shape models, and reports a member it has no value
//! for as JSON `null` rather than leaving the key out. The generated serializer does
//! the opposite: it skips an `Option` member holding `None`, ...
//! * a queued summary in `GET /things/{thingName}/jobs` carries `"startedAt": null`
//!   for an execution that has not started.
```
restated at services/.../src/ops/types/mod.rs:19-21:
```
/// How every operation of this service writes its success response: as a document
/// carrying every modelled output member, JSON `null` where there is no value.
```

The code under that doc completes only the operation's OWN members — services/.../src/ops/types/response_document.rs:107-117:
```
    fn fill_absent(&mut self) -> ::std::result::Result<(), SerdeError> {
        for member in self.shape.members() {
```
with nesting explicitly passed through unchanged (response_document.rs:28-31):
```
//! Nested structures are written exactly as the handler's projection built them (see
//! `crate::state::to_job_execution` / `to_execution_state`, ...
```

The nested member in question is an `Option`, so the generated serializer drops it — services/.../src/ops/types/get_pending_job_executions_types.rs:29-36:
```
pub struct JobExecutionSummary {
    pub job_id: Option<String>,
    pub queued_at: i64,
    pub started_at: Option<i64>,
```
filled from the record at services/.../src/state.rs:147-156 (`started_at: record.started_at`, `None` until a start stamps it) and handed to `ResponseDocument` by the handler at services/.../src/ops/get_pending_job_executions.rs:181-195.

Test coverage does not pin the difference: tests/all/job_execution_lifecycle.rs:266-269
```
        assert!(
            summary["startedAt"].is_null(),
            "case {case:?}: a queued execution has not started, got {summary}"
        );
```
(`serde_json` indexing an ABSENT key also yields `Null`), and tests/all/response_document_completeness.rs:43 + :154-171 iterate only `PENDING_MEMBERS = &["inProgressJobs", "queuedJobs"]`.

## Steps to Reproduce

1. Register a command in the shared IoT command index (as `tests/all/response_document_completeness.rs` does) and POST /command-executions with `targetArn = arn:aws:iot:us-east-1:111122223333:thing/laser-1` to queue one execution for thing `laser-1`.
2. GET /things/laser-1/jobs and read the RAW response body (not a parsed `Value` indexed by key).
3. Observe the single entry of `queuedJobs` has no `startedAt` key at all, while response_document.rs:19-20 states the recorded body carries `"startedAt": null` for an execution that has not started.
4. GET /things/laser-1/jobs/$next and observe the same for the nested `execution`: no `startedAt` and no `approximateSecondsBeforeTimedOut` key, rather than those members reported as null.
