Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: 1f316ce023d643c000ad0b8fb7b5799232d706fd

# DescribeVirtualCluster never reports schedulerStatus, so the cluster's in-queue/concurrent job-run counts are unobservable

_Filed: 2026-10-24T01:18:35Z_
_Operation: DescribeVirtualCluster | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `1f316ce023d643c000ad0b8fb7b5799232d706fd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected (AWS): `VirtualCluster.schedulerStatus` reports "the current in-queue and concurrent job-run counts for the virtual cluster" — `currentInQueueJobRuns` (runs in PENDING or SUBMITTED) and `currentConcurrentJobRuns` (runs in RUNNING) — for a cluster whose scheduler capacity is in use.

Actual (emulator): the field is hardcoded absent. `VirtualClusterRecord::to_virtual_cluster` sets `scheduler_status: None` unconditionally, even though the emulator holds every job run of the cluster in state and could count PENDING/RUNNING runs exactly. A caller that reads the cluster to learn how much scheduler capacity is in use always sees nothing.

## Evidence

Spec: `VirtualCluster.schedulerStatus` targets `SchedulerStatus` with `currentInQueueJobRuns` (PENDING or SUBMITTED) and `currentConcurrentJobRuns` (RUNNING), both NonNegativeInteger with default 0 (smithy_query shape=VirtualCluster,SchedulerStatus).

Handler/state: src/state.rs line 297 — `scheduler_status: None` in `to_virtual_cluster`, with the comment that reporting it "would be reporting work nothing did"; the per-cluster job-run index (`cluster_job_runs`, src/state.rs ~line 937) already makes the counts computable.

Tests: tests/all/virtual_cluster_job_run_loop.rs asserts the cluster view's id/name/arn/state/containerProvider/createdAt/tags round-trip but makes no assertion about schedulerStatus, so the gap is uncovered.

## Steps to Reproduce

1. POST /virtualclusters with {name, clientToken, containerProvider:{id,type:EKS}, schedulerConfiguration:{maxConcurrentJobRuns:10}}; keep the cluster id.
2. POST /virtualclusters/{clusterId}/jobruns twice with distinct clientTokens and a sparkSubmitJobDriver.
3. GET /virtualclusters/{clusterId} immediately (runs still PENDING) and again after ~1s (runs RUNNING).
4. Observed: the virtualCluster object has no schedulerStatus member in either call. Expected: schedulerStatus.currentInQueueJobRuns = 2 then currentConcurrentJobRuns = 2, computed from the stored job runs.
