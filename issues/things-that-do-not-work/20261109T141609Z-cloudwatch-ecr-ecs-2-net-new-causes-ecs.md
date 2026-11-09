Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__ecr__ecs.ecs-service-cpu-utilization-published-to-cloudwatch cloudwatch__ecr__ecs.ecs-service-memory-utilization-published-to-cloudwatch cloudwatch__ecr__ecs.ecs-cluster-container-insights-metrics-visible-in-cloudwatch cloudwatch__ecr__ecs.ecs-metrics-absent-without-container-insights-enabled cloudwatch__ecr__ecs.cloudwatch-alarm-on-ecs-service-running-task-count
Issue-Key: cloudwatch__ecr__ecs.ecs-service-cpu-utilization-published-to-cloudwatch|run#8|AssertionError|assert points, (
Issue-Key: cloudwatch__ecr__ecs.ecs-service-memory-utilization-published-to-cloudwatch|run#8|AssertionError|assert points, "no MemoryUtilization datapoints appeared in AWS/ECS for the running service"
Issue-Key: cloudwatch__ecr__ecs.ecs-cluster-container-insights-metrics-visible-in-cloudwatch|run#9|AssertionError|assert running_points, (
Issue-Key: cloudwatch__ecr__ecs.ecs-metrics-absent-without-container-insights-enabled|run#13|AssertionError|assert enabled_points, "positive control failed: CI-enabled cluster produced no ECS/ContainerInsights data"
Issue-Key: cloudwatch__ecr__ecs.cloudwatch-alarm-on-ecs-service-running-task-count|run#13|AssertionError|assert state == "OK", f"alarm never reached OK once the service had a real running task (state={state})"
Issue-Commit: 5252be52e0cc9f5d73bee5aa197523afb271707f

# cloudwatch__ecr__ecs: 2 net-new causes — ECS publishes no AWS/ECS service metrics and no ECS/ContainerInsights metrics for a running service, so metric-driven alarms never leave ALARM

> _Note: this issue was identified against the code base at commit `5252be52e0cc9f5d73bee5aa197523afb271707f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMetricStatistics

## Description
Two root causes in the ECS→CloudWatch metric path. Both are in the cloudwatch__ecr__ecs service and must each be closed.

Cause 1 of 2: no AWS/ECS CPUUtilization or MemoryUtilization datapoints are published for a service with running tasks.
AWS: while an ECS service has running tasks, the AWS/ECS namespace receives CPUUtilization and MemoryUtilization datapoints dimensioned by ClusterName and ServiceName, so GetMetricStatistics/GetMetricData over the elapsed period returns at least one datapoint. Recorded behavior only (these metrics are emitted by the service, not modelled in Smithy).
Emulator: the AWS/ECS namespace has no datapoints at all for the running service — GetMetricStatistics returns an empty Datapoints list for both metrics for the whole window the check polls.
Why: a client that sizes, scales or alarms on ECS service utilisation sees a service that reports no load ever; target-tracking/step scaling and dashboards are blind.
Fix: the ECS service/task runtime's metric publication into CloudWatch (AWS/ECS namespace emission), plus CloudWatch's storage of those datapoints.
Traces: cloudwatch__ecr__ecs.ecs-service-cpu-utilization-published-to-cloudwatch, cloudwatch__ecr__ecs.ecs-service-memory-utilization-published-to-cloudwatch.

Cause 2 of 2: a cluster with Container Insights enabled publishes no ECS/ContainerInsights metrics, so an alarm on RunningTaskCount never leaves ALARM.
AWS: with containerInsights enabled on the cluster, the ECS/ContainerInsights namespace receives cluster/service metrics including RunningTaskCount, and an alarm whose threshold the running task count satisfies transitions to OK once datapoints arrive (while a cluster WITHOUT Container Insights publishes none — that is the negative control the same check pairs it with).
Emulator: no ECS/ContainerInsights datapoints appear for the Container-Insights-enabled cluster (the negative control's positive leg fails the same way), and a CloudWatch alarm on that metric stays in ALARM forever even after the service has a real running task.
Why: Container Insights is unobservable, and any alarm built on it is permanently wrong: a client waiting for OK (a deployment gate, an autoscaling action) waits forever and a client that treats ALARM as an outage gets a false alarm.
Fix: the ECS cluster Container Insights metric publisher (ECS/ContainerInsights namespace emission) and the CloudWatch alarm state evaluator that must re-evaluate once datapoints exist.
Traces: cloudwatch__ecr__ecs.ecs-cluster-container-insights-metrics-visible-in-cloudwatch, cloudwatch__ecr__ecs.ecs-metrics-absent-without-container-insights-enabled, cloudwatch__ecr__ecs.cloudwatch-alarm-on-ecs-service-running-task-count.

## Evidence
1) ecs-service-cpu-utilization-published-to-cloudwatch — AssertionError: no CPUUtilization datapoints appeared in AWS/ECS for the running service
2) ecs-service-memory-utilization-published-to-cloudwatch — AssertionError: no MemoryUtilization datapoints appeared in AWS/ECS for the running service
3) ecs-cluster-container-insights-metrics-visible-in-cloudwatch — AssertionError: no ECS/ContainerInsights RunningTaskCount datapoints appeared
4) ecs-metrics-absent-without-container-insights-enabled — AssertionError: positive control failed: CI-enabled cluster produced no ECS/ContainerInsights data
5) cloudwatch-alarm-on-ecs-service-running-task-count — AssertionError: alarm never reached OK once the service had a real running task (state=ALARM)

Expected: at least one datapoint in AWS/ECS (CPUUtilization, MemoryUtilization) and in ECS/ContainerInsights (RunningTaskCount) for the running service, and the alarm in OK.

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. Re-replay the checks in trace_ids (service cloudwatch__ecr__ecs).
3. Or: create a cluster with containerInsights enabled, run a service with desiredCount>=1, then poll GetMetricStatistics for AWS/ECS CPUUtilization/MemoryUtilization (dimensions ClusterName/ServiceName) and ECS/ContainerInsights RunningTaskCount, and DescribeAlarms on an alarm built over RunningTaskCount.
