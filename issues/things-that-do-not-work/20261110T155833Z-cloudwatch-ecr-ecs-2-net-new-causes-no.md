Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__ecr__ecs.ecs-service-cpu-utilization-published-to-cloudwatch cloudwatch__ecr__ecs.ecs-service-memory-utilization-published-to-cloudwatch cloudwatch__ecr__ecs.ecs-cluster-container-insights-metrics-visible-in-cloudwatch cloudwatch__ecr__ecs.ecs-metrics-absent-without-container-insights-enabled
Issue-Key: cloudwatch__ecr__ecs.ecs-service-cpu-utilization-published-to-cloudwatch|_poll_metric_datapoints#7|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch__ecr__ecs.ecs-service-memory-utilization-published-to-cloudwatch|_poll_metric_datapoints#7|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch__ecr__ecs.ecs-cluster-container-insights-metrics-visible-in-cloudwatch|_poll_metric_datapoints#8|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch__ecr__ecs.ecs-metrics-absent-without-container-insights-enabled|_poll_metric_datapoints#12|Timeout|time.sleep(interval_s)
Issue-Commit: b11785bf9bf8af57a00ad7e4d814b1c5f1d66117

# cloudwatch__ecr__ecs: 2 net-new causes — no AWS/ECS service metrics and no ECS/ContainerInsights metrics are ever published for a running ECS service, so metric polls exhaust the whole check budget

> _Note: this issue was identified against the code base at commit `b11785bf9bf8af57a00ad7e4d814b1c5f1d66117`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMetricStatistics

## Description
Two root causes in the ECS -> CloudWatch metric-publication path, both grounded in failing behavioral checks listed under the suite heading `cloudwatch__ecr__ecs`. Each must be closed separately.

NOTE on an existing open issue: 20261109T141609Z-cloudwatch-ecr-ecs-2-net-new-causes-ecs.md describes neighbouring symptoms and lists these check ids, but it does NOT carry their current replay keys — it recorded them failing with an AssertionError at the empty-datapoint assert (e.g. `run#8|AssertionError|assert points`), whereas they now consume the entire harness budget and end as `Check timed out after 600.0s`, so the suite does not report them as owned. The divergence below is therefore refiled with the keys the current replay produces; a fixer closing the older issue must also replay the four checks named here.

Cause 1 of 2: a Fargate service with running tasks publishes no AWS/ECS CPUUtilization or MemoryUtilization datapoints.
AWS: while an ECS service has running tasks, ECS publishes service-level CPUUtilization and MemoryUtilization into the AWS/ECS namespace dimensioned by ClusterName + ServiceName; CloudWatch GetMetricStatistics over the elapsed window then returns a non-empty `Datapoints` list (Smithy: cloudwatch#GetMetricStatistics input Namespace/MetricName/Dimensions/StartTime/EndTime/Period/Statistics, output `Datapoints`; the metric emission itself is recorded behavior only — these metrics are produced by ECS and are not modelled in Smithy). A busy container (a CPU spin loop) yields Average > 0.
Emulator: nothing is ever published for the service. GetMetricStatistics for AWS/ECS CPUUtilization and MemoryUtilization with the service's ClusterName/ServiceName dimensions keeps answering 200 with an empty `Datapoints` list for the whole polling window, so the checks poll (interval 30s, budget 600s) until the harness budget ends and the check is reported as `Check timed out after 600.0s` with no datapoint ever seen.
Why: a client that sizes, scales or alarms on ECS service utilisation sees a service that reports no load at any time — target-tracking and step scaling policies never fire, dashboards stay empty, and any wait-for-datapoint loop (a deployment gate, a capacity check) hangs instead of completing.
Fix: the ECS service/task runtime's metric publication into CloudWatch (AWS/ECS namespace emission for a running service) together with CloudWatch's storage/retrieval of those datapoints in GetMetricStatistics.
Traces: cloudwatch__ecr__ecs.ecs-service-cpu-utilization-published-to-cloudwatch, cloudwatch__ecr__ecs.ecs-service-memory-utilization-published-to-cloudwatch.

Cause 2 of 2: a cluster created with the containerInsights setting enabled publishes no ECS/ContainerInsights metrics.
AWS: Smithy: ecs#ClusterSetting.name is the enum ecs#ClusterSettingName with the single value `containerInsights`, and its value `enabled`/`enhanced` "turn[s] on CloudWatch Container Insights ... for a cluster". With it enabled, the ECS/ContainerInsights namespace receives cluster/service metrics including RunningTaskCount (Maximum >= the number of running tasks) and CpuUtilized (Average > 0 for a busy task), dimensioned by ClusterName + ServiceName; a cluster WITHOUT the setting publishes none into that namespace while still appearing in the classic AWS/ECS namespace. Metric emission is recorded behavior only.
Emulator: no ECS/ContainerInsights datapoints appear for the Container-Insights-enabled cluster at all — GetMetricStatistics for RunningTaskCount and CpuUtilized returns an empty `Datapoints` list for the full window, so both the positive check and the positive control of the negative-delivery check poll until the 600s harness budget ends (`Check timed out after 600.0s`). The negative half cannot even be evaluated, because the control leg never produces data.
Why: Container Insights is entirely unobservable — the cluster setting has no effect, a client cannot distinguish an insights-enabled cluster from a disabled one, and anything built on those metrics (alarms, autoscaling, dashboards, cost/utilisation reporting) is permanently blind and waits forever for its first datapoint.
Fix: the ECS cluster Container Insights metric publisher (ECS/ContainerInsights namespace emission, gated on the cluster's containerInsights setting) and CloudWatch's datapoint store that GetMetricStatistics reads.
Traces: cloudwatch__ecr__ecs.ecs-cluster-container-insights-metrics-visible-in-cloudwatch, cloudwatch__ecr__ecs.ecs-metrics-absent-without-container-insights-enabled.

## Evidence
Cause 1:
- cloudwatch__ecr__ecs.ecs-service-cpu-utilization-published-to-cloudwatch — Error: Check timed out after 600.0s. The check creates an execution role, a cluster (insights=false), registers a FARGATE task definition running `sh -c "i=0; while true; do i=$((i+1)); done"`, creates a service with desiredCount=1, waits for a running task, then polls GetMetricStatistics(Namespace="AWS/ECS", MetricName="CPUUtilization", Dimensions=[ClusterName, ServiceName], timeout_s=600, interval_s=30) and asserts `points` non-empty and some Average > 0. Expected: at least one datapoint with Average > 0. Actual: Datapoints stays empty for the entire window and the check never gets past the poll.
- cloudwatch__ecr__ecs.ecs-service-memory-utilization-published-to-cloudwatch — same shape for MetricName="MemoryUtilization" (assert message "no MemoryUtilization datapoints appeared in AWS/ECS for the running service"); Error: Check timed out after 600.0s.

Cause 2:
- cloudwatch__ecr__ecs.ecs-cluster-container-insights-metrics-visible-in-cloudwatch — Error: Check timed out after 600.0s. Cluster created with containerInsights enabled; polls GetMetricStatistics(Namespace="ECS/ContainerInsights", MetricName="RunningTaskCount", Dimensions=[ServiceName, ClusterName], Statistics=["Maximum"], timeout_s=600) and then CpuUtilized (Average). Expected: RunningTaskCount datapoints with Maximum >= 1 and CpuUtilized datapoints with Average > 0 (assert messages "no ECS/ContainerInsights RunningTaskCount datapoints appeared" / "no ECS/ContainerInsights CpuUtilized datapoints appeared"). Actual: empty Datapoints throughout.
- cloudwatch__ecr__ecs.ecs-metrics-absent-without-container-insights-enabled — Error: Check timed out after 600.0s. Runs two otherwise identical clusters (insights disabled vs enabled); the enabled cluster is the positive control ("positive control failed: CI-enabled cluster produced no ECS/ContainerInsights data") and never yields a CpuUtilized datapoint, so the effect-absent assertion on the disabled cluster is never reached.

In all four, GetMetricStatistics itself answers 200 — it simply has no datapoints to return, for either namespace, at any point in the window.

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. Re-replay the checks listed in trace_ids (suite heading cloudwatch__ecr__ecs).
3. Or by hand: create an IAM role trusting ecs-tasks.amazonaws.com with AmazonECSTaskExecutionRolePolicy; CreateCluster with settings [{name: containerInsights, value: enabled}] and a second cluster without that setting; RegisterTaskDefinition (FARGATE, awsvpc, cpu 256 / memory 512) whose container runs a CPU spin loop; CreateService with desiredCount=1 in each cluster and wait for a running task.
4. Poll cloudwatch GetMetricStatistics(Namespace="AWS/ECS", MetricName="CPUUtilization" and "MemoryUtilization", Dimensions=[{ClusterName},{ServiceName}], Period=60, Statistics=["Average"]) over the elapsed window: Datapoints is always empty (Cause 1).
5. Poll cloudwatch GetMetricStatistics(Namespace="ECS/ContainerInsights", MetricName="RunningTaskCount" (Statistics=["Maximum"]) and "CpuUtilized" (Statistics=["Average"]), Dimensions=[{ServiceName},{ClusterName}]) for the insights-enabled cluster: Datapoints is always empty, so the enabled/disabled clusters are indistinguishable (Cause 2).
