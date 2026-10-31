Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 4a5766ed2caf698065daa3a6a1a461ee27489a64

# The stepTimeoutInMinutes range branch is implemented twice, once per handler

_Filed: 2026-10-28T17:29:40Z_
_Operation: UpdateJobExecution | Class: maintainability_

> _Note: this issue was identified against the code base at commit `4a5766ed2caf698065daa3a6a1a461ee27489a64`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The documented 1..10080 bound on `stepTimeoutInMinutes` is checked by two handlers of this crate, each with its own copy of the branch: `UpdateJobExecution` accepts the range plus `CANCEL_STEP_TIMER_MINUTES`, `StartNextPendingJobExecution` accepts the range only (correctly, since only `UpdateJobExecution$stepTimeoutInMinutes` documents -1). The RULE common to both — `(MIN_STEP_TIMEOUT_MINUTES..=MAX_STEP_TIMEOUT_MINUTES).contains(&minutes)` refused as `InvalidRequestException` — is written out twice, so the two copies can drift while each handler's refusal keeps looking right in isolation.

The response-visible parts (the two different message texts, the two different operation error channels) can stay exactly as they are: one helper beside `check_status_details` in `crate::state` that takes whether the cancel value is accepted and returns the same `Err(String)` each handler reports today leaves every status code and message byte-identical.

## Evidence

Copy 1 — services/.../src/ops/update_job_execution.rs:229-242:
```
        if let Some(minutes) = input.step_timeout_in_minutes {
            let acceptable = minutes == CANCEL_STEP_TIMER_MINUTES
                || (MIN_STEP_TIMEOUT_MINUTES..=MAX_STEP_TIMEOUT_MINUTES).contains(&minutes);
            if !acceptable {
                return Err(UpdateJobExecutionErrorModeled::InvalidRequestException(
```

Copy 2 — services/.../src/ops/start_next_pending_job_execution.rs:184-197:
```
        if let Some(minutes) = input.step_timeout_in_minutes {
            if !(MIN_STEP_TIMEOUT_MINUTES..=MAX_STEP_TIMEOUT_MINUTES).contains(&minutes) {
                return Err(
                    StartNextPendingJobExecutionErrorModeled::InvalidRequestException(
```

Both copies are covered by one existing test, which drives the bound through BOTH operations — tests/all/job_execution_lifecycle.rs:1182-1263:
```
/// RULE: `stepTimeoutInMinutes` is honoured inside its published range (1..10080)
/// and refused 400 outside it; `-1` cancels the step timer on
/// `UpdateJobExecution`, which is the only operation that documents it.
#[tokio::test]
async fn the_step_timeout_is_honoured_inside_its_published_range_and_refused_outside() {
```
with rows `for minutes in [MIN_STEP_TIMEOUT - 1, MAX_STEP_TIMEOUT + 1, -1, -100]` asserting 400 + `InvalidRequestException` from `$next` (:1210-1223) and 400 from the update, plus -1 accepted by the update only (:1232-1253). The shared constants already live in one place (services/common/src/iot_job_executions.rs:196-203); only the branch is duplicated.

## Steps to Reproduce

1. Read services/.../src/ops/update_job_execution.rs:229-242 and services/.../src/ops/start_next_pending_job_execution.rs:184-197 side by side: the same `(MIN_STEP_TIMEOUT_MINUTES..=MAX_STEP_TIMEOUT_MINUTES).contains(&minutes)` refusal is written twice.
2. Note that the two copies already differ (only the update accepts `CANCEL_STEP_TIMER_MINUTES`), so a change to the shared range rule has to be made in two places and can silently apply to one operation only.
3. Confirm the behaviour is pinned by one test exercising both handlers: `cargo test -p service-com-amazonaws-iotjobsdataplane-iotlaserthingjobmanagerexternalservice the_step_timeout_is_honoured_inside_its_published_range_and_refused_outside`.
