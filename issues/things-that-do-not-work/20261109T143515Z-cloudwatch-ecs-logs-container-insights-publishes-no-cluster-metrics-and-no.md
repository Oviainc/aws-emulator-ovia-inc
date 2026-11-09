Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__ecs__logs.container-insights-publishes-cluster-metrics-and-performance-logs cloudwatch__ecs__logs.obs-ecs-2-fargate-service-metrics-dimensioned-per-service-not-per-cluster
Issue-Key: cloudwatch__ecs__logs.container-insights-publishes-cluster-metrics-and-performance-logs|_poll_until#24|TimeoutError|raise TimeoutError(f"{description}: the run budget ended first; not graded")
Issue-Key: cloudwatch__ecs__logs.obs-ecs-2-fargate-service-metrics-dimensioned-per-service-not-per-cluster|_poll_until#13|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Commit: 31f7b518324c5238b816f5187d2718623b5667f6

# Container Insights publishes no cluster metrics and no per-service AWS/ECS series

> _Note: this issue was identified against the code base at commit `31f7b518324c5238b816f5187d2718623b5667f6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutMetricData

## Description
Cause 1 of 1: a cluster with containerInsights enabled publishes none of the ECS/ContainerInsights metric set, and a Fargate service publishes no AWS/ECS series dimensioned per service.
AWS: with Container Insights enabled, the cluster publishes the ECS/ContainerInsights namespace (ContainerInstanceCount, CpuReserved, CpuUtilized, EphemeralStorageReserved, EphemeralStorageUtilized, MemoryReserved, MemoryUtilized, NetworkRxBytes, NetworkTxBytes, ServiceCount, StorageReadBytes, StorageWriteBytes, TaskCount) plus performance log events, and a Fargate service publishes AWS/ECS CPUUtilization dimensioned by ClusterName+ServiceName (not only by ClusterName).
Emulator: no metric of that set ever appears for the cluster, and 'AWS/ECS CPUUtilization datapoints for [{ClusterName}, {ServiceName}] not met within 600s. Last result: []'.
Why: Container Insights dashboards, per-service alarms and capacity decisions have no data at all, while the cluster reports insights as enabled.
Fix: the Container Insights metric/performance-log publisher and the per-service AWS/ECS metric dimensions in the ecs crate.
Traces: both checks in trace_ids.

## Evidence
'TimeoutError: the full ECS/ContainerInsights metric set for cluster <name>: [...13 metric names...]: the run budget ended first; not graded' and 'AssertionError: AWS/ECS CPUUtilization datapoints for [{'Name': 'ClusterName', ...}, {'Name': 'ServiceName', ...}] not met within 600s. Last result: []'.

## Steps to Reproduce
Replay cloudwatch__ecs__logs.container-insights-publishes-cluster-metrics-and-performance-logs and .obs-ecs-2-fargate-service-metrics-dimensioned-per-service-not-per-cluster.
