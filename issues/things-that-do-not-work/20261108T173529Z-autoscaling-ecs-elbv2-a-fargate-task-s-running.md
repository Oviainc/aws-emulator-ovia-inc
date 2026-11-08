Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__ecs__elbv2.ecs-task-state-change-event-delivered-to-eventbridge-rule
Issue-Key: autoscaling__ecs__elbv2.ecs-task-state-change-event-delivered-to-eventbridge-rule|_collect_task_events#27|Timeout|sqs.delete_message(QueueUrl=queue_url, ReceiptHandle=msg["ReceiptHandle"])
Issue-Commit: 0fba5a4732f94b5ef6351efe96de38b77f8e6c14

# autoscaling__ecs__elbv2: a Fargate task's RUNNING/STOPPED transitions are never delivered as ECS Task State Change events to an EventBridge rule target (1 cause)

> _Note: this issue was identified against the code base at commit `0fba5a4732f94b5ef6351efe96de38b77f8e6c14`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunTask

## Description
One root cause, grounded in a failing check the suite lists under `### autoscaling__ecs__elbv2` that is marked neither ALREADY FILED nor OWNED.

Cause 1 of 1: ECS publishes no `ECS Task State Change` events for a task's lifecycle transitions, so an EventBridge rule matching source `aws.ecs` with an SQS target receives nothing and the check is cut off at the harness budget.
  AWS: a task started with RunTask emits an EventBridge event per lifecycle transition with `source: "aws.ecs"`, `detail-type: "ECS Task State Change"` and a `detail` carrying at least taskArn, clusterArn, lastStatus and desiredStatus; a task whose only essential container runs a short command therefore emits a RUNNING event and later a STOPPED event (the STOPPED one with detail.desiredStatus == STOPPED), in chronological order by the envelope's `time`. A rule whose pattern is {source: [aws.ecs], detail-type: [ECS Task State Change], detail: {clusterArn: [<cluster arn>]}} with an SQS queue target (queue policy allowing events.amazonaws.com sqs:SendMessage) delivers both envelopes to the queue, and DescribeTasks agrees with the delivered event (lastStatus STOPPED). Recorded behavior only for the delivery timing: against AWS both envelopes arrive well inside the collection window. The fixed parts of each envelope are source/detail-type and the detail member names; the ARNs, ids and timestamps in it are per-run values taken from the request and from ECS state.
  Emulator: nothing is delivered — the check polls ReceiveMessage for the RUNNING and STOPPED envelopes and is killed by the harness after 180.0s (`Error: Check timed out after 180.0s`) before either assertion (`no RUNNING event delivered` / `no STOPPED event delivered`) can even be reported. No ECS Task State Change envelope ever reaches the rule's target (and the task's own RUNNING->STOPPED progression is not observable either).
  Why: every event-driven integration built on ECS task lifecycle events — alarms, dead-letter wiring, task-completion handlers, deployment watchers — never fires in the emulator, so a client that waits for an event waits forever instead of receiving the envelope AWS sends, and an eventing test that passes against AWS hangs here.
  Fix: ECS's event emission for task state transitions (the task state machine must publish an `aws.ecs` / `ECS Task State Change` event per transition into the EventBridge bus) and the EventBridge rule-matching/target-delivery path to SQS that carries it. Related open issues 20261104T062603Z-ecs-events-sqs-ecs-never-publishes-deployment-or.md and 20261104T060239Z-cloudwatch-ecr-ecs-fargate-tasks-never-leave-running.md record the same family of defect for OTHER check ids and do not carry this check's replay key.
  Traces: autoscaling__ecs__elbv2.ecs-task-state-change-event-delivered-to-eventbridge-rule.

## Evidence
Expected: the SQS queue targeted by the rule receives two envelopes with source `aws.ecs`, detail-type `ECS Task State Change` and detail.taskArn == the launched task — one with detail.lastStatus RUNNING, a later one with detail.lastStatus STOPPED and detail.desiredStatus STOPPED and detail.clusterArn == the cluster's ARN; DescribeTasks then reports lastStatus STOPPED. Actual: `Error: Check timed out after 180.0s` — ReceiveMessage returns no matching envelope for the whole window, so neither the RUNNING nor the STOPPED event is ever delivered.

## Steps to Reproduce
ecs create-cluster; ecs register-task-definition (FARGATE, awsvpc, one essential container running `sleep 20`, with an execution role); sqs create-queue and set a queue policy allowing events.amazonaws.com sqs:SendMessage for the rule ARN; events put-rule --event-pattern '{"source":["aws.ecs"],"detail-type":["ECS Task State Change"],"detail":{"clusterArn":["<cluster arn>"]}}'; events put-targets with the queue ARN; ecs run-task --launch-type FARGATE in a public subnet; poll sqs receive-message for envelopes whose detail.taskArn is the launched task -> none arrive. Replay autoscaling__ecs__elbv2.ecs-task-state-change-event-delivered-to-eventbridge-rule.
