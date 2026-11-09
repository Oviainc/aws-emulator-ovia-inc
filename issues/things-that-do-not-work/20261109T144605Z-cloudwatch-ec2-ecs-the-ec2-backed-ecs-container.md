Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__ec2__ecs.metrics-ec2-ecs-container-insights-running-task-count-follows-each-service-desired-count cloudwatch__ec2__ecs.metrics-ec2-ecs-update-service-negative-desired-count-rejected
Issue-Key: cloudwatch__ec2__ecs.metrics-ec2-ecs-container-insights-running-task-count-follows-each-service-desired-count|_poll_until#21|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch__ec2__ecs.metrics-ec2-ecs-update-service-negative-desired-count-rejected|_poll_until#23|Timeout|time.sleep(interval_s)
Issue-Commit: 5252be52e0cc9f5d73bee5aa197523afb271707f

# cloudwatch__ec2__ecs: the EC2-backed ECS Container Insights metric scenarios never reach an observable step (1 cause)

> _Note: this issue was identified against the code base at commit `5252be52e0cc9f5d73bee5aa197523afb271707f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMetricStatistics

## Description
Cause 1 of 1: both EC2-backed ECS metric scenarios run out of their 900s budget without producing a single observation, so neither the Container Insights RunningTaskCount series nor the UpdateService negative can be graded.

AWS: recorded behavior only for the end-to-end timing; the underlying contracts are modeled. Smithy (ecs): UpdateService must refuse desiredCount=-1 with InvalidParameterException (HTTP 400) — the refusal is immediate and needs no capacity. Smithy (cloudwatch): GetMetricStatistics/GetMetricData return datapoints for the ECS/ContainerInsights namespace, and RunningTaskCount for a service tracks its desiredCount once Container Insights is enabled on the cluster. Both scenarios depend on an EC2-backed cluster converging first.

Emulator: `Error: Check timed out after 900.0s` for both checks. The setup leg — an EC2 instance joining the cluster and the service placing tasks on it — never completes, so even the part that is a pure control-plane refusal (UpdateService with desiredCount=-1) is never reached, and no ECS/ContainerInsights datapoint is ever published.

Why: a client gets no signal at all from EC2-launch-type ECS plus CloudWatch: it cannot verify that its metrics flow, and it cannot verify that an invalid scale request is refused. A scenario that hangs for the full budget is indistinguishable from a broken client.

Fix: the convergence path these scenarios need — EC2→ECS container-instance registration and EC2 task placement in the ECS crate, plus the ECS/ContainerInsights metric publisher that feeds CloudWatch. The control-plane validation on UpdateService (desiredCount >= 0 → InvalidParameterException) must also be reachable without capacity.

Traces: cloudwatch__ec2__ecs.metrics-ec2-ecs-container-insights-running-task-count-follows-each-service-desired-count, cloudwatch__ec2__ecs.metrics-ec2-ecs-update-service-negative-desired-count-rejected.

## Evidence
Expected: an EC2-backed ECS service places tasks, CloudWatch GetMetricStatistics returns ECS/ContainerInsights RunningTaskCount datapoints that follow each service's desiredCount, and ecs UpdateService(desiredCount=-1) is refused with InvalidParameterException (HTTP 400).
Actual: `Error: Check timed out after 900.0s` on both checks — no datapoints and no refusal observed.

## Steps to Reproduce
1. Create an ECS cluster with containerInsights=enabled and launch an EC2 instance with ECS_CLUSTER user data.
2. Create an EC2-launch-type service with desiredCount=1 and poll ecs DescribeServices for runningCount.
3. Poll cloudwatch GetMetricStatistics for ECS/ContainerInsights RunningTaskCount.
4. Call ecs UpdateService(desiredCount=-1) and expect InvalidParameterException.
Observed: the scenario never advances past setup and is killed at 900s.
