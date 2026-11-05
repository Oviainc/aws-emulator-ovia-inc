Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs__events__sqs.events-workload-state-change-ecs-exit-code-rules-route-each-stopped-task-and-not-delivered-to-the-other
Issue-Key: ecs__events__sqs.events-workload-state-change-ecs-exit-code-rules-route-each-stopped-task-and-not-delivered-to-the-other|_poll_until#19|Timeout|time.sleep(interval_s)
Issue-Commit: 68ec0c9cae77cfbc133f0186a4c923306bef5402

# ecs__events__sqs: a stopped Fargate task publishes no ECS Task State Change event, so exit-code-scoped EventBridge rules never receive (or withhold) anything

> _Note: this issue was identified against the code base at commit `68ec0c9cae77cfbc133f0186a4c923306bef5402`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunTask

## Description
One net-new root cause of service `ecs__events__sqs`, grounded in the failing behavioral check in trace_ids. The replay report marked it neither ALREADY FILED nor OWNED. The open issues 20261029T182228Z-ecs-events-sqs-ecs-emits-no-task-state-change-event-so.md and 20261104T062603Z-ecs-events-sqs-ecs-never-publishes-deployment-or.md describe the same missing ECS->EventBridge event stream for this service token but do not carry this check's replay key; they are named here so a fixer can close them together. This is not a refusal, so it does not reopen a resolved issue.

=== Cause 1 of 1: ECS publishes no `ECS Task State Change` event for a task's RUNNING and STOPPED transitions ===
AWS: the ECS->EventBridge event stream is not in the Smithy model (recorded behavior only; AWS reference: 'ECS task state change events' in the Amazon ECS Developer Guide). Each Fargate task transition publishes an event on the default bus with `source` = `aws.ecs` and `detail-type` = `ECS Task State Change`, whose `detail` is the task document: `clusterArn`, `taskArn`, `lastStatus`, `version`, `stopCode`, `stoppedReason` and `containers[]` with each container's `exitCode`. The RUNNING event carries a LOWER `detail.version` than the STOPPED event for the same task and no `exitCode`; the STOPPED event's `exitCode`/`stopCode`/`stoppedReason` equal what `DescribeTasks` reports for that task. Rules whose EventPattern scopes `detail.containers.exitCode` therefore each receive only the stopped task matching their code and withhold the other.
Emulator: no `ECS Task State Change` event is ever delivered. The check runs two Fargate tasks (exiting 7 and 0) with one rule per exit code targeting its own queue, then blocks reading those queues; nothing arrives, so it makes no progress and is killed at the harness ceiling: `Check timed out after 60.0s`. Neither the positive routing nor the negative (not-delivered-to-the-other-rule) half is reachable, and the `detail.version` ordering and `DescribeTasks` agreement are likewise unverifiable.
Why: the ECS task lifecycle is normally observed by subscribing to these events, not by polling. A client that reacts to task completion — retry-on-nonzero-exit, fan-out on success, alerting on stopCode — never fires at all against the emulator, and a client that scopes a rule by `exitCode` cannot tell a correctly-withheld event from an event stream that does not exist.
Fix: the ECS task state machine must publish an `ECS Task State Change` event to the default EventBridge bus on every task transition (at least RUNNING and STOPPED), with `source`/`detail-type` as above and a `detail` rendered from the same task state `DescribeTasks` serves — including `containers[].exitCode`, `stopCode`, `stoppedReason` and a monotonically increasing `detail.version`.
Traces: ecs__events__sqs.events-workload-state-change-ecs-exit-code-rules-route-each-stopped-task-and-not-delivered-to-the-other.

## Evidence
ecs__events__sqs.events-workload-state-change-ecs-exit-code-rules-route-each-stopped-task-and-not-delivered-to-the-other [mismatch]:
  expected: for two Fargate tasks in one cluster exiting 7 and 0, each STOPPED event (source `aws.ecs`, detail-type `ECS Task State Change`, detail.clusterArn = the cluster ARN) reaches only the rule whose EventPattern matches its `detail.containers.exitCode`, with exitCode/stopCode/stoppedReason equal to DescribeTasks; each RUNNING event has a lower detail.version and no exitCode.
  actual:   no event is delivered to either rule's queue; the check spends its whole window in ReceiveMessage and the harness reports `Check timed out after 60.0s`.

Not reached and therefore unverified by this replay (same cause): the detail.version ordering between RUNNING and STOPPED, and the agreement of detail exitCode/stopCode/stoppedReason with DescribeTasks.

## Steps to Reproduce
Boot the emulator and replay ecs__events__sqs.events-workload-state-change-ecs-exit-code-rules-route-each-stopped-task-and-not-delivered-to-the-other from suite simparity-behavioral-aws-v1.

By hand:
1. ecs.CreateCluster; ecs.RegisterTaskDefinition (requiresCompatibilities FARGATE, networkMode awsvpc, cpu 256, memory 512, one container running `sh -c 'sleep 5; exit <code>'`).
2. sqs.CreateQueue twice and allow events.amazonaws.com sqs:SendMessage on each.
3. events.PutRule with EventPattern {"source":["aws.ecs"],"detail-type":["ECS Task State Change"],"detail":{"clusterArn":["<cluster ARN>"],"containers":{"exitCode":[7]}}} and a second rule with exitCode 0; events.PutTargets pointing each at its own queue.
4. ecs.RunTask twice, once per task definition/command exiting 7 and 0; wait for both tasks to reach STOPPED (ecs.DescribeTasks).
5. sqs.ReceiveMessage on both queues. AWS: each queue holds exactly the STOPPED event for its own exit code, with detail exitCode/stopCode/stoppedReason equal to DescribeTasks. Emulator: both queues stay empty.
