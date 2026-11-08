Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__ecs__elbv2.eventbridge-rule-without-target-permission-no-invocation
Issue-Key: autoscaling__ecs__elbv2.eventbridge-rule-without-target-permission-no-invocation|run#27|AssertionError|assert delivered_with_permission is not None, (
Issue-Commit: 8c5329059e752b9fa65691305eadc8c02e100ead

# ECS Task State Change events are never delivered to an EventBridge rule's SQS target even after the queue policy grants events.amazonaws.com SendMessage, so the negative-delivery check passes vacuously

> _Note: this issue was identified against the code base at commit `8c5329059e752b9fa65691305eadc8c02e100ead`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunTask

## Description
Cause 1 of 1: the emulator never publishes/delivers an ECS Task State Change event to an EventBridge rule whose target is an SQS queue, even once that queue's resource policy explicitly allows events.amazonaws.com to sqs:SendMessage. The 'no permission -> no delivery' half of the check therefore passes for the wrong reason (nothing is ever delivered), and the positive control that proves the pipeline works fails.

AWS: running and stopping a task in an ECS cluster publishes an EventBridge event with source 'aws.ecs', detail-type 'ECS Task State Change' and detail.clusterArn/detail.taskArn/detail.lastStatus; an ENABLED rule whose EventPattern matches {source: [aws.ecs], detail-type: [ECS Task State Change], detail: {clusterArn: [<cluster>]}} with an SQS target delivers the event body to that queue, and the delivered detail.taskArn is the ARN of the task that actually ran. Target-side authorization is evaluated at DELIVERY time, not at PutTargets time: PutTargets succeeds against a queue with no policy, delivery is then silently dropped, and after SetQueueAttributes adds a statement allowing Principal {Service: events.amazonaws.com} to sqs:SendMessage on that queue ARN, the NEXT event is delivered. Recorded behavior only — the Smithy models describe PutRule/PutTargets/RunTask shapes, not the delivery or its authorization.

Emulator: with the policy absent no message arrives (which the check accepts), and after the allow statement is added and a second task is run and stopped, polling the queue for 90s still yields no message: 'AssertionError: event was still not delivered after granting the SQS queue policy -- positive control failed'. No ECS Task State Change event reaches the SQS target in either phase, so the emulator delivers nothing rather than delivering the wrong thing.

Why: a client cannot build anything on ECS task-state eventing — no event-driven task bookkeeping, no 'task stopped' alarm, no asynchronous completion notification works. Worse, every negative-delivery assertion in this area becomes vacuous: a platform that publishes nothing is indistinguishable from one that correctly withholds an event for a missing permission, so a least-privilege test proves nothing and a missing-permission regression would go unnoticed.

Fix: the ECS task state machine's EventBridge publisher (emitting ECS Task State Change on RunTask/StopTask transitions) and the EventBridge rule-matching/target-delivery path into SQS, including its delivery-time evaluation of the target queue's resource policy. Not the PutRule/PutTargets handlers, which already accept the wiring.

Related open issue that lists this check id but under a different replay key (its key for this check is a `_poll_for_message#21` Timeout inside sqs.receive_message; this replay reaches the positive-control assertion at run#126 instead, so the suite did not mark it owned): 20261104T042502Z-autoscaling-ecs-elbv2-6-net-new-root-causes.md, Cause 1.

Traces: autoscaling__ecs__elbv2.eventbridge-rule-without-target-permission-no-invocation.

## Evidence
Behavioral replay of simparity-behavioral-aws-v1 against the emulator built from this checkout (50/414 passed; the emulator is up — ECS CreateCluster/RegisterTaskDefinition/RunTask, SQS CreateQueue/SetQueueAttributes and EventBridge PutRule/PutTargets in this check all succeeded).

[mismatch] autoscaling__ecs__elbv2.eventbridge-rule-without-target-permission-no-invocation:
  File "check_autoscaling__ecs__elbv2_eventbridge_rule_without_target_permission_no_invocation.py", line 126, in run
    assert delivered_with_permission is not None, (
  AssertionError: event was still not delivered after granting the SQS queue policy -- positive control failed

Expected (recording): phase 1 — PutTargets on a queue with no resource policy, run+stop a Fargate task, poll the queue 60s, no message (delivery dropped for lack of permission). Phase 2 — SetQueueAttributes(Policy={Statement:[{Effect:Allow, Principal:{Service:'events.amazonaws.com'}, Action:'sqs:SendMessage', Resource:<queueArn>}]}), run+stop another task, poll 90s, and receive a message whose body parses to an event with source 'aws.ecs', detail-type 'ECS Task State Change' and detail.taskArn equal to the ARN returned by that RunTask.

Actual: phase 2 yields no message within 90s (delivered_with_permission is None), so the taskArn assertion is never reached. Phase 1's 'no delivery' outcome is therefore not evidence of authorization being enforced. (Queue URL/ARN, cluster ARN, task ARN and the poll budgets are per-run values; the requirement is that the task-state-change event is published and delivered once the target policy allows it.)

## Steps to Reproduce
1. Boot the emulator. Create an ECS cluster and register an awsvpc/FARGATE task definition (cpu 256, memory 512, one container running `sleep 20`) with an execution role trusting ecs-tasks.amazonaws.com.
2. Create an SQS queue and read its QueueArn via GetQueueAttributes. Do NOT set a queue policy yet.
3. events PutRule(Name=<rule>, State='ENABLED', EventPattern={"source":["aws.ecs"],"detail-type":["ECS Task State Change"],"detail":{"clusterArn":[<clusterArn>]}}), then PutTargets(Rule=<rule>, Targets=[{Id:'1', Arn:<queueArn>}]) — expect PutTargets to succeed.
4. RunTask(launchType='FARGATE', networkConfiguration=awsvpcConfiguration{subnet, sg, assignPublicIp ENABLED}), let it stop, and poll ReceiveMessage for 60s: expect no message (no target permission).
5. SetQueueAttributes(Policy allowing Principal {Service: events.amazonaws.com} Action sqs:SendMessage on <queueArn>). RunTask again, let it stop, and poll ReceiveMessage for 90s.
6. Expect a message whose JSON body has source 'aws.ecs', detail-type 'ECS Task State Change' and detail.taskArn == the second RunTask's taskArn; observe that no message ever arrives.
7. Re-replay autoscaling__ecs__elbv2.eventbridge-rule-without-target-permission-no-invocation to confirm a fix.
