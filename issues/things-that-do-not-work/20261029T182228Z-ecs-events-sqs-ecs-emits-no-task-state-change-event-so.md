Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs__events__sqs.a-runtask-that-failed-placement-emits-no-task-state-change-event
Issue-Key: ecs__events__sqs.a-runtask-that-failed-placement-emits-no-task-state-change-event|_drain#24|Timeout|resp = sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Commit: b1cd28f1ca277d6a032df605f83af5017e934043

# ECS emits no Task State Change event, so a RunTask that failed placement cannot be distinguished from a bus that publishes nothing

> _Note: this issue was identified against the code base at commit `b1cd28f1ca277d6a032df605f83af5017e934043`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunTask

## Description
Cause 1 of 1: no "ECS Task State Change" event reaches EventBridge, so neither the positive control nor the asserted absence can be observed.
AWS: ECS publishes on the default bus source "aws.ecs", detail-type "ECS Task State Change" with detail carrying clusterArn, taskArn, lastStatus, desiredStatus, group and containers, as a task moves PROVISIONING→PENDING→RUNNING→STOPPED. A RunTask whose placement fails returns a populated `failures` list and creates no task, so no Task State Change event is emitted for it, while a successfully placed sibling task does emit one. recorded behavior only for the exact detail body.
Emulator: the rule's SQS target receives nothing for either task; the check ends "Check timed out after 60.0s", so the differential the check is built on collapses.
Why: any client reacting to task lifecycle through EventBridge (scale, alarm, clean up) sees nothing; and a placement failure looks the same as a successful placement because both produce silence.
Fix: the ECS task state machine must publish ECS Task State Change events to the EventBridge bus on every lastStatus transition, and RunTask must not emit one for a task it never placed.
Traces: ecs__events__sqs.a-runtask-that-failed-placement-emits-no-task-state-change-event

## Evidence
"Error: Check timed out after 60.0s" — the SQS target behind a rule on {"source":["aws.ecs"],"detail-type":["ECS Task State Change"]} is empty after both a placeable RunTask (positive control) and an unplaceable one. Expected: at least one envelope for the placed task with detail.taskArn and detail.lastStatus, and none for the unplaceable one.

## Steps to Reproduce
PutRule on {"source":["aws.ecs"],"detail-type":["ECS Task State Change"]} with an SQS target, RunTask once with satisfiable placement constraints and once with constraints no instance satisfies, and poll the queue. Nothing arrives for either.
