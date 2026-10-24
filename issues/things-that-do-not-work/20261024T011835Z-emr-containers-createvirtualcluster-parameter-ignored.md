Issue-Severity: MEDIUM
Issue-Gap-Class: parameter-ignored
Issue-Commit: 1f316ce023d643c000ad0b8fb7b5799232d706fd

# schedulerConfiguration is stored but never enforced: queue-depth and concurrency limits have no effect on StartJobRun

_Filed: 2026-10-24T01:18:35Z_
_Operation: CreateVirtualCluster | Class: parameter-ignored_

> _Note: this issue was identified against the code base at commit `1f316ce023d643c000ad0b8fb7b5799232d706fd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected (AWS): `schedulerConfiguration` on a virtual cluster is enforced. Per the model's own documentation, `maxInQueueJobRuns` is "the maximum number of job runs that can be in the PENDING or SUBMITTED state at any time for the virtual cluster. When the queue is full, the service rejects StartJobRun requests with a ValidationException", and `maxConcurrentJobRuns` is "the maximum number of job runs that can be in the RUNNING state at any time ... As running slots free up, queued job runs start automatically".

Actual (emulator): the configuration is persisted on the record and echoed back by `DescribeVirtualCluster`, but no handler ever reads it. `StartJobRun` admits an unbounded number of runs and every one of them is moved to RUNNING on the clock regardless of `maxConcurrentJobRuns`, and a full queue never produces the documented `ValidationException`. The only refusal `StartJobRun` can produce is the emulator's own MAX_JOB_RUNS retention cap (2000), which is unrelated to the configured limits.

## Evidence

Spec: `SchedulerConfiguration` member docs (smithy_query shape=SchedulerConfiguration) state the ValidationException-on-full-queue rule and the queued-then-auto-start rule; ranges are maxInQueueJobRuns 100..10000 and maxConcurrentJobRuns 1..10000. `StartJobRun` models `ValidationException` (HTTP 400).

Handler: src/ops/create_virtual_cluster.rs line 222 persists `scheduler_configuration: input.scheduler_configuration`; src/ops/start_job_run.rs never reads `cluster.scheduler_configuration` (its only capacity check is `state.reserve_job_run()`, the emulator retention cap, lines 247-255). src/state.rs lines 281-284 admits "this slice persists a `schedulerConfiguration` without scheduling against it (the queue-depth and concurrency limits the shape documents are not enforced yet)".

Tests: no test in tests/all/ submits more runs than a configured limit; the member is only checked for round-tripping.

## Steps to Reproduce

1. POST /virtualclusters with {name, clientToken, containerProvider:{id,type:EKS}, schedulerConfiguration:{maxConcurrentJobRuns:1, maxInQueueJobRuns:100}}; keep the cluster id.
2. POST /virtualclusters/{clusterId}/jobruns three times with distinct clientTokens and a sparkSubmitJobDriver.
3. Wait ~1s, then GET /virtualclusters/{clusterId}/jobruns.
4. Observed: all three runs are RUNNING at once. Expected: at most maxConcurrentJobRuns (1) RUNNING, the rest held PENDING and started as slots free up.
5. Drive PENDING runs past maxInQueueJobRuns and call StartJobRun again: observed HTTP 200, expected HTTP 400 ValidationException for a full queue.
