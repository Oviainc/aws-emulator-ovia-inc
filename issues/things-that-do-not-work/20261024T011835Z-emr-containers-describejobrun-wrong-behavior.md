Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 1f316ce023d643c000ad0b8fb7b5799232d706fd

# A job run never reaches COMPLETED: DescribeJobRun reports RUNNING forever, with no finishedAt, stateDetails or retryPolicyExecution

_Filed: 2026-10-24T01:18:35Z_
_Operation: DescribeJobRun | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `1f316ce023d643c000ad0b8fb7b5799232d706fd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected (AWS): a submitted job run walks PENDING -> (SUBMITTED) -> RUNNING -> COMPLETED (or FAILED), so a poll-until-terminal loop on `DescribeJobRun` eventually observes a terminal `state` together with `finishedAt`, and a run that was submitted with `retryPolicyConfiguration` reports `retryPolicyExecution.currentAttemptCount` while it executes.

Actual (emulator): the only transitions the state machine owes are PENDING -> RUNNING (on `StartJobRun`) and CANCEL_PENDING -> CANCELLED (on `CancelJobRun`). A run that is never cancelled stays `RUNNING` indefinitely, so `finishedAt` is never set, `stateDetails` is always absent, `COMPLETED`/`FAILED` are states no code path ever writes, and `retryPolicyExecution` is hardcoded to `None` even when `retryPolicyConfiguration` was persisted. Any client or test that waits for a job run to finish (the normal EMR-on-EKS workflow) hangs until it times out, and `ListJobRuns?states=COMPLETED` can never match anything.

## Evidence

Spec: `JobRunState` enum = PENDING, SUBMITTED, RUNNING, FAILED, CANCELLED, CANCEL_PENDING, COMPLETED; `JobRun` declares `finishedAt`, `stateDetails`, `failureReason` and `retryPolicyExecution` (`RetryPolicyExecution.currentAttemptCount` @required), i.e. the model describes a run that terminates and reports how it terminated.

Handler/state: src/ops/start_job_run.rs lines 282-284 queues only `PendingTransition::due(JOB_RUNNING)`; src/state.rs declares JOB_COMPLETED/JOB_FAILED but no handler writes them (the comment at `JOB_SUBMITTED` admits no scheduler runs), and `JobRunRecord::to_job_run` (src/state.rs lines 352-372) hardcodes `failure_reason: None` and `retry_policy_execution: None`.

Tests: tests/all/virtual_cluster_job_run_loop.rs:667-760 only covers PENDING -> RUNNING -> CANCEL_PENDING -> CANCELLED; no test asserts a run ever completes, so the gap is uncovered.

## Steps to Reproduce

1. POST /virtualclusters with {name, clientToken, containerProvider:{id,type:EKS}}; keep the cluster id.
2. POST /virtualclusters/{clusterId}/jobruns with {clientToken:"c1", jobDriver:{sparkSubmitJobDriver:{entryPoint:"s3://b/j.py"}}, retryPolicyConfiguration:{maxAttempts:3}}.
3. Poll GET /virtualclusters/{clusterId}/jobruns/{id} repeatedly for several seconds.
4. Observed: state stays RUNNING forever; finishedAt, stateDetails and retryPolicyExecution never appear. Expected: the run eventually reports a terminal state (COMPLETED) with finishedAt set, and retryPolicyExecution.currentAttemptCount while running.
5. GET /virtualclusters/{clusterId}/jobruns?states=COMPLETED: observed always empty.
