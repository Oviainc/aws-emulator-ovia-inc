Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__ecs__logs.obs-ecs-2-fargate-service-metrics-dimensioned-per-service-not-per-cluster
Issue-Key: cloudwatch__ecs__logs.obs-ecs-2-fargate-service-metrics-dimensioned-per-service-not-per-cluster|_poll_until#13|Timeout|time.sleep(interval_s)
Issue-Commit: f4970b2d8a83c111b3a18edc79ceb1ab4d4e7ac8

# cloudwatch__ecs__logs: AWS/ECS service metrics are never dimensioned per ClusterName+ServiceName, so the per-service vs per-cluster alarm contrast reaches no verdict (1 cause)

> _Note: this issue was identified against the code base at commit `f4970b2d8a83c111b3a18edc79ceb1ab4d4e7ac8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutMetricAlarm

## Description
Cause 1 of 1: a Fargate service's AWS/ECS CPU/memory series never appear, so neither the ClusterName+ServiceName alarm nor its ClusterName-only twin can be evaluated.
AWS: AWS/ECS CPUUtilization/MemoryUtilization for a Fargate service exist only as ClusterName+ServiceName series in Percent; an alarm on that dimension pair transitions to ALARM/OK on the published data, while an alarm on ClusterName alone stays INSUFFICIENT_DATA because no such series is published. Recorded behavior only for the datapoint values.
Emulator: the check produces no verdict at all — 'Error: Check timed out after 600.0s'. No series and no alarm state transition is ever observed.
Why: a customer cannot tell which dimensions ECS metrics are published under, so alarms and scaling policies are written against dimensions that silently never receive data.
Fix: the AWS/ECS service-metric publisher in the ecs crate (dimension set ClusterName+ServiceName) and the cloudwatch alarm evaluator that must leave an undimensioned twin in INSUFFICIENT_DATA.
Traces: cloudwatch__ecs__logs.obs-ecs-2-fargate-service-metrics-dimensioned-per-service-not-per-cluster.

## Evidence
Detail: 'Error: Check timed out after 600.0s'. Expected: datapoints for AWS/ECS CPUUtilization/MemoryUtilization with ClusterName+ServiceName (Percent) and an alarm on that pair leaving INSUFFICIENT_DATA, while the ClusterName-only alarm stays INSUFFICIENT_DATA. Actual: nothing is published and no alarm changes state inside the cap.

## Steps to Reproduce
Replay cloudwatch__ecs__logs.obs-ecs-2-fargate-service-metrics-dimensioned-per-service-not-per-cluster: create a cluster with containerInsights disabled, run a Fargate service, put one alarm on ClusterName+ServiceName and one on ClusterName only, and poll both alarm states and the metric series.
