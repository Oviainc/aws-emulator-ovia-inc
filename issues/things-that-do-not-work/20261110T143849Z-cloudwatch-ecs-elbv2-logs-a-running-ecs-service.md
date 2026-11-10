Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__ecs__elbv2__logs.ecs-service-cpu-memory-metrics-in-cloudwatch
Issue-Key: cloudwatch__ecs__elbv2__logs.ecs-service-cpu-memory-metrics-in-cloudwatch|_poll_metric_datapoints#10|Timeout|time.sleep(30)
Issue-Commit: 8ffd4f499c6d6f6b1d86ee4455070f9416ebee70

# cloudwatch__ecs__elbv2__logs: a running ECS service publishes no AWS/ECS CPUUtilization or MemoryUtilization, so the service-metrics check never reaches a verdict

> _Note: this issue was identified against the code base at commit `8ffd4f499c6d6f6b1d86ee4455070f9416ebee70`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMetricStatistics

## Description
Cause 1 of 1: no AWS/ECS service-level utilization series is ever published for a running ECS service, so a client polling CloudWatch for it waits forever.
AWS: recorded behavior only (the CloudWatch and ECS Smithy models describe the APIs, not which series ECS emits). A service with a running task publishes the AWS/ECS namespace series CPUUtilization and MemoryUtilization dimensioned by ClusterName + ServiceName at 1-minute resolution, with no Container Insights required. GetMetricStatistics / ListMetrics for those dimensions therefore return datapoints within a few minutes of the task running, and a task that busy-loops and allocates reports Average > 0. Both the namespace/metric names and the dimension pair are the requirement; the datapoint values are per-run.
Emulator: the check creates the cluster, registers a CPU/memory-burning task definition, creates the service, waits for it to run, and then polls CloudWatch for AWS/ECS CPUUtilization and MemoryUtilization with ClusterName/ServiceName dimensions. No datapoint is ever served and the check is killed by its 600 s budget — no response for those series ever arrives, so the emulator serves an empty metric where AWS serves a populated one (and, depending on which wait consumed the budget, the service may also never reach runningCount>=1).
Why: every autoscaling, dashboard and alarm client built on ECS service utilization is inert: Application Auto Scaling target tracking on ECSServiceAverageCPUUtilization never scales, alarms stay INSUFFICIENT_DATA, and capacity dashboards read empty — with no error to attribute it to.
Fix: the ECS service/task runtime's metric publication into the CloudWatch metric store (the component that should emit AWS/ECS CPUUtilization and MemoryUtilization per ClusterName+ServiceName while tasks run), plus whatever ECS state decides a task is running. Open issue 20261109T143515Z-cloudwatch-ecs-elbv2-logs-an-alb-s-dnsname-is-published-as-name.md describes the same metric gap for this service group but does not carry this check's replay key, so it is filed here and that issue is the one to close it with if they are fixed together.
Traces: cloudwatch__ecs__elbv2__logs.ecs-service-cpu-memory-metrics-in-cloudwatch.

## Evidence
Check cloudwatch__ecs__elbv2__logs.ecs-service-cpu-memory-metrics-in-cloudwatch [mismatch]:
  Error: Check timed out after 600.0s
  The body: _wait_service_running(cluster, service, desired_count=1, timeout_s=600) then _poll_metric_datapoints('AWS/ECS','CPUUtilization',[{ClusterName},{ServiceName}],timeout_s=600) and the same for MemoryUtilization, asserting non-empty datapoints with Average > 0.
  Expected: both series return datapoints for the ClusterName+ServiceName dimension pair, at least one with Average > 0.
  Actual: the poll never observes a datapoint; the check is terminated at its 600 s budget having produced no verdict.

## Steps to Reproduce
1. Create an ECS cluster (containerInsights disabled) and register a Fargate task definition whose command burns CPU and allocates memory.
2. CreateService with desiredCount=1 in a subnet with a security group, and wait until DescribeServices reports runningCount=1.
3. Call cloudwatch GetMetricStatistics for Namespace=AWS/ECS, MetricName=CPUUtilization (then MemoryUtilization), Dimensions=[{ClusterName: <cluster>}, {ServiceName: <service>}], Statistics=[Average], over the last few minutes.
4. No datapoints are ever returned (AWS returns datapoints, at least one with Average > 0).
