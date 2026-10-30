Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 5cb591631d698c9098c00693287b08686d77b0d1

# UpdateJobExecution's state-transition and version-mismatch refusals omit the executionState the model documents in the error body

_Filed: 2026-10-28T18:03:15Z_
_Operation: UpdateJobExecution | Class: error-response_

> _Note: this issue was identified against the code base at commit `5cb591631d698c9098c00693287b08686d77b0d1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

UpdateJobExecution has three refusal paths that the model documents as carrying the execution's CURRENT state back to the caller, and all three answer with a message string only.

1. The 409 `InvalidStateTransitionException` — raised for a terminal execution (update_job_execution.rs:301-314) and for a refused transition (update_job_execution.rs:323-332) — is constructed as `InvalidStateTransitionException { message: Some(...) }`. The model's own documentation for that error says the body ALSO contains the `executionState` field. Nothing in the 409 body reports status, statusDetails or versionNumber, so a device that is refused has no way to learn the state it was refused against other than issuing a separate read (which, for a terminal execution, DescribeJobExecution itself refuses 410 — describe_job_execution.rs:209-216).

2. The 400 `InvalidRequestException` for a mismatched `expectedVersion` (update_job_execution.rs:282-294) carries only the two version numbers in prose, although the member's own documentation says an ErrorResponse "that contains the current job execution status data is returned" and states the point of it: "(This makes it unnecessary to perform a separate DescribeJobExecution request in order to obtain the job execution status data.)" The crate's own helper for that comment, `crate::state::version_mismatch_message`, repeats the justification ("so a device can re-read and retry without a separate `DescribeJobExecution`") while delivering only the numbers.

The crate already holds the projection the documented field needs — `crate::state::to_execution_state(record)` — and the SUCCESS path of this very handler calls it (update_job_execution.rs:364-367); the refusal paths, which the model says must carry the same data, do not. The modelled error struct in this crate carries only `message` (src/ops/types/update_job_execution_types.rs:43-47), so closing this means the error body this operation serialises has to carry the documented `executionState` object as well, not just the message.

## Evidence

Model text that itself states the behaviour — `smithy_query(query="doc", shape="InvalidStateTransitionException")`:
```
"An update attempted to change the job execution to a state that is invalid because of the job execution's current state (for example, an attempt to change a request in state SUCCESS to state IN_PROGRESS). In this case, the body of the error message also contains the executionState field."
```
and `smithy_query(query="doc", shape="UpdateJobExecutionRequest", member="expectedVersion")`:
```
"Optional. The expected current version of the job execution. ... If the version of the job execution stored in Jobs does not match, the update is rejected with a VersionMismatch error, and an ErrorResponse that contains the current job execution status data is returned. (This makes it unnecessary to perform a separate DescribeJobExecution request in order to obtain the job execution status data.)"
```

The handler's refusals carry a message and nothing else — services/service-com-amazonaws-iotjobsdataplane-iotlaserthingjobmanagerexternalservice/src/ops/update_job_execution.rs:301-313:
```
        if record.is_terminal() {
            return Err(
                UpdateJobExecutionErrorModeled::InvalidStateTransitionException(
                    InvalidStateTransitionException {
                        message: Some(crate::state::already_finished_message(
```
and :323-331:
```
        if let Some(reason) = record.transition_refusal(&input.status) {
            return Err(
                UpdateJobExecutionErrorModeled::InvalidStateTransitionException(
                    InvalidStateTransitionException {
                        message: Some(reason),
```
and the version guard at :282-292:
```
        if let Some(expected) = input.expected_version {
            if expected != record.version_number {
                return Err(UpdateJobExecutionErrorModeled::InvalidRequestException(
                    InvalidRequestException {
                        message: Some(crate::state::version_mismatch_message(
```

The helper the success path uses for exactly this data, which no refusal path calls — src/state.rs:160-166:
```
pub fn to_execution_state(record: &JobExecutionRecord) -> JobExecutionState {
    JobExecutionState {
        status: Some(record.status.clone()),
        status_details: reported_status_details(record),
        version_number: record.version_number,
```
called only at src/ops/update_job_execution.rs:364-367:
```
                execution_state: input
                    .include_job_execution_state
                    .unwrap_or(false)
                    .then(|| crate::state::to_execution_state(record)),
```

The crate's comment that repeats the documented purpose while shipping only numbers — src/state.rs:90-95:
```
/// The message of the `InvalidRequestException` (HTTP 400) an `expectedVersion` that
/// does not match the stored version is refused with: it names BOTH versions ... so a device can re-read
/// and retry without a separate `DescribeJobExecution`.
pub fn version_mismatch_message(expected: i64, stored: i64) -> String {
```

No existing test pins the 409/400 BODY: every assertion checks only status and code — tests/all/job_execution_state_machine.rs:251-252 (`"InvalidStateTransitionException", "case {case:?}: expected InvalidStateTransitionException, got {body}"`), tests/all/job_execution_lifecycle.rs:767-768 and tests/all/clock_driven_timeout_is_materialised.rs:425 (`(409, "InvalidStateTransitionException")`); none reads `executionState` out of an error body. The modelled struct that would have to carry it — src/ops/types/update_job_execution_types.rs:43-47:
```
#[smithy_macros::smithy(shape = "com.amazonaws.iotjobsdataplane#InvalidStateTransitionException")]
#[derive(Debug, Clone, Default)]
pub struct InvalidStateTransitionException {
    pub message: Option<String>,
}
```

## Steps to Reproduce

1. Register a command in the shared IoT command index and POST /command-executions with `targetArn = arn:aws:iot:us-east-1:111122223333:thing/laser-1` to queue one execution; note the returned `executionId`.
2. POST /things/laser-1/jobs/{executionId} with `{"status": "SUCCEEDED"}` to drive the execution terminal (200).
3. POST /things/laser-1/jobs/{executionId} again with `{"status": "IN_PROGRESS"}`. Observe HTTP 409 with a body of `{"__type": "InvalidStateTransitionException", "message": "..."}` and NO `executionState` field, although the model's documentation for that error states "the body of the error message also contains the executionState field".
4. Repeat on a live execution: POST /things/laser-1/jobs/{executionId2} with `{"status": "SUCCEEDED", "expectedVersion": 7}` against a stored version of 1. Observe HTTP 400 whose body carries only the two version numbers in prose, with none of the "current job execution status data" the `expectedVersion` documentation says the ErrorResponse contains — so a client must issue the separate DescribeJobExecution the documentation says is unnecessary.
