Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__ecr__ecs__logs.ecs-service-cpu-memory-metrics-published-to-cloudwatch cloudwatch__ecr__ecs__logs.cloudwatch-logs-metric-filter-creates-alarm-from-ecs-logs cloudwatch__ecr__ecs__logs.deploy-ecs-fargate-stack-with-ecr-and-log-group
Issue-Key: cloudwatch__ecr__ecs__logs.ecs-service-cpu-memory-metrics-published-to-cloudwatch|run#23|AssertionError|assert cpu_points, (
Issue-Key: cloudwatch__ecr__ecs__logs.cloudwatch-logs-metric-filter-creates-alarm-from-ecs-logs|run#29|AssertionError|assert points, "metric filter never produced a datapoint for the matched ECS log line"
Issue-Key: cloudwatch__ecr__ecs__logs.deploy-ecs-fargate-stack-with-ecr-and-log-group|_poll_until#21|Timeout|time.sleep(interval_s)
Issue-Commit: 5252be52e0cc9f5d73bee5aa197523afb271707f

# cloudwatch__ecr__ecs__logs: 3 net-new causes — no AWS/ECS service metrics, a Logs metric filter publishes no datapoint for a matched log line, and the CFN ECS-Fargate+ECR+log-group deploy never converges

> _Note: this issue was identified against the code base at commit `5252be52e0cc9f5d73bee5aa197523afb271707f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMetricStatistics

## Description
Three root causes in the ECS/Logs→CloudWatch observability path. All three are in the cloudwatch__ecr__ecs__logs service and must each be closed.

Cause 1 of 3: no AWS/ECS CPUUtilization datapoints are published for a running ECS service.
AWS: while a service has running tasks, AWS/ECS receives CPUUtilization (and MemoryUtilization) datapoints dimensioned by ClusterName and ServiceName, so a GetMetricStatistics over the elapsed window returns datapoints. Recorded behavior only.
Emulator: the AWS/ECS namespace stays empty for the whole polling window — no CPUUtilization datapoints are ever published for the running service.
Why: utilisation-based scaling, dashboards and alarms on a deployed ECS service are blind.
Fix: the ECS service/task runtime's CloudWatch metric publication (AWS/ECS namespace).
Traces: cloudwatch__ecr__ecs__logs.ecs-service-cpu-memory-metrics-published-to-cloudwatch.

Cause 2 of 3: a CloudWatch Logs metric filter produces no metric datapoint for a log line that matches its pattern.
AWS: PutMetricFilter installs a filter on a log group; every ingested log event matching the filter pattern publishes the filter's metric transformation into the named namespace/metric, so an alarm over it fires. Recorded behavior only (the transformation semantics are the service's, not in Smithy).
Emulator: after the ECS task writes a log line that matches the filter, the filter's metric never receives a datapoint, so the alarm built from it never changes state.
Why: log-derived metrics and every alarm built on them (error-rate alarms, keyword alarms) never fire, so a client cannot detect the condition it instrumented.
Fix: the CloudWatch Logs metric-filter evaluation on log ingestion (the PutLogEvents path that must apply filters and publish the transformed metric).
Traces: cloudwatch__ecr__ecs__logs.cloudwatch-logs-metric-filter-creates-alarm-from-ecs-logs.

Cause 3 of 3: a CloudFormation-deployed ECS Fargate stack with an ECR image and a log group never reaches the state a client can observe — the check burns its whole 180 s budget.
AWS: the stack reaches CREATE_COMPLETE, the service's task starts and writes awslogs events to the declared log group, and each of those is observable well inside the window.
Emulator: the check times out after 180.0 s without reaching a verdict — the awaited condition (the deployed service's task settling and its log stream appearing) never happens.
Why: a client deploying this very ordinary stack shape cannot tell whether its workload ran; every wait-for-stable/wait-for-logs loop burns its full timeout.
Fix: the ECS service/task state machine and the awslogs log-delivery path reached from a CloudFormation-provisioned stack.
Traces: cloudwatch__ecr__ecs__logs.deploy-ecs-fargate-stack-with-ecr-and-log-group.

## Evidence
1) ecs-service-cpu-memory-metrics-published-to-cloudwatch — AssertionError: no CPUUtilization datapoints published for the running ECS service
2) cloudwatch-logs-metric-filter-creates-alarm-from-ecs-logs — AssertionError: metric filter never produced a datapoint for the matched ECS log line
3) deploy-ecs-fargate-stack-with-ecr-and-log-group — Error: Check timed out after 180.0s

Expected: datapoints in AWS/ECS for the running service, a datapoint for the metric filter's transformation after a matching log line, and a deployed Fargate stack whose task settles and delivers awslogs events inside the window.

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. Re-replay the checks in trace_ids (service cloudwatch__ecr__ecs__logs).
3. Or: (1) run a Fargate service and poll GetMetricStatistics for AWS/ECS CPUUtilization; (2) PutMetricFilter on the task's log group, emit a matching log line, poll the filter's metric; (3) CreateStack with an ECS Fargate service + ECR image + log group and wait for the service's log stream.
