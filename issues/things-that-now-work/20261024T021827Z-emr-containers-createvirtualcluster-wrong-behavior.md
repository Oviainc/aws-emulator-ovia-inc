Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 743fc0a4792edb6d196f368afdefaae152d33ff0

# schedulerConfiguration is stored but never enforced, and VirtualCluster.schedulerStatus is never reported

_Filed: 2026-10-24T02:18:27Z_
_Operation: CreateVirtualCluster | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `743fc0a4792edb6d196f368afdefaae152d33ff0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

A virtual cluster created with `schedulerConfiguration` has documented runtime semantics the emulator does not implement:

* `maxConcurrentJobRuns` - "The maximum number of job runs that can be in the RUNNING state at any time for the virtual cluster. As running slots free up, queued job runs start automatically." The emulator moves every accepted run PENDING -> RUNNING on a fixed 400 ms timer regardless of how many runs are already RUNNING, so a cluster with `maxConcurrentJobRuns: 1` reports 20 simultaneously RUNNING runs and nothing is ever queued.
* `VirtualCluster$schedulerStatus` - "The current in-queue and concurrent job-run counts for the virtual cluster" (`currentInQueueJobRuns`, `currentConcurrentJobRuns`, both default 0). The emulator hardcodes this member to absent, so a caller can never read the cluster's capacity usage even when a scheduler configuration is set and runs exist.

Expected: a run submitted while `maxConcurrentJobRuns` RUNNING slots are taken stays PENDING until a slot frees, and DescribeVirtualCluster reports schedulerStatus counts computed from the stored runs.
Actual: unlimited concurrency, schedulerStatus always omitted.

## Evidence

Smithy model (smithy_query shape SchedulerConfiguration / SchedulerStatus / VirtualCluster): the member docs quoted above define concurrency gating and the reported counts; `SchedulerStatus` members carry @default 0. Implementation: src/state.rs `VirtualClusterRecord::to_virtual_cluster` returns `scheduler_status: None` with a comment admitting "this slice persists a schedulerConfiguration without scheduling against it (the queue-depth and concurrency limits the shape documents are not enforced yet)"; src/ops/start_job_run.rs lines 260-286 always sets `PendingTransition::due(JOB_RUNNING)` without consulting the cluster's scheduler_configuration or counting RUNNING runs. No test asserts concurrency gating or a schedulerStatus body (tests/all/virtual_cluster_job_run_loop.rs only round-trips the submitted schedulerConfiguration).

## Steps to Reproduce

1. POST /virtualclusters with {"schedulerConfiguration":{"maxConcurrentJobRuns":1,"maxInQueueJobRuns":100}, ...}
2. POST /virtualclusters/{id}/jobruns three times with distinct clientTokens and a valid jobDriver
3. Wait ~1s, then GET /virtualclusters/{id}/jobruns -> all three runs report state RUNNING, exceeding maxConcurrentJobRuns=1
4. GET /virtualclusters/{id} -> the response contains no `schedulerStatus`, so currentConcurrentJobRuns / currentInQueueJobRuns cannot be observed
