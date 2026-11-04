Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs__events__sqs.ecs-deployment-state-change-events-deliver-in-progress-then-failed ecs__events__sqs.events-workload-state-change-ecs-exit-code-rules-route-each-stopped-task-and-not-delivered-to-the-other ecs__events__sqs.xsvc-ecs-1-unpullable-image-task-configuration-failure-not-delivered-to-sibling
Issue-Key: ecs__events__sqs.ecs-deployment-state-change-events-deliver-in-progress-then-failed|_settle_rule#11|Timeout|time.sleep(interval_s)
Issue-Key: ecs__events__sqs.events-workload-state-change-ecs-exit-code-rules-route-each-stopped-task-and-not-delivered-to-the-other|_poll_until#19|Timeout|last = probe()
Issue-Key: ecs__events__sqs.xsvc-ecs-1-unpullable-image-task-configuration-failure-not-delivered-to-sibling|_receive#28|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=wait_s)
Issue-Commit: b64c3d45927e2833db57dde0eaeae2a33a5b497f

# ecs__events__sqs: ECS never publishes deployment- or task-state-change events to EventBridge, so no rule ever routes them to SQS (1 cause)

> _Note: this issue was identified against the code base at commit `b64c3d45927e2833db57dde0eaeae2a33a5b497f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutRule

## Description
Cause 1 of 1: ECS emits no lifecycle events onto the EventBridge bus.

AWS: ECS publishes 'ECS Task State Change' and 'ECS Deployment State Change' events (source aws.ecs) for every task and service-deployment transition. A rule matching those events delivers them to its SQS target; a rule whose pattern (e.g. a detail exit-code filter) does not match receives nothing, which is how a client proves routing is selective.

Emulator: nothing is ever published, so the positive deliveries never arrive and the negative (not-delivered) controls cannot be distinguished from the positive ones either — every one of these checks polls until the 60s budget expires.

Why: event-driven consumers of ECS (alerting on a failed deployment, reacting to a stopped task's exit code, fan-out to several queues by pattern) cannot be exercised at all, and a client cannot tell a correctly-scoped rule from a broken one.

Fix: the ECS task/deployment lifecycle must publish the documented event envelopes (source aws.ecs, the detail-type above, detail carrying the task/deployment state) into the EventBridge bus, from which the existing rule matching can route them.

Traces: the three failing checks in trace_ids. The open issue 20261029T182228Z-ecs-events-sqs-ecs-emits-no-task-state-change-event-so.md records a related symptom for a different replay key (the placement-failure case) and does not carry these three.

## Evidence
All three checks end with `Error: Check timed out after 60.0s`. Expected (AWS): the SQS target receives one message per matching event (an 'ECS Deployment State Change' pair IN_PROGRESS then FAILED; one 'ECS Task State Change' per stopped task routed by its exit code), and the deliberately non-matching rule receives none.

## Steps to Reproduce
1. CreateQueue; PutRule with an event pattern on {source: ['aws.ecs'], detail-type: ['ECS Deployment State Change']} and the queue as target.
2. CreateService and UpdateService it to a failing revision; poll ReceiveMessage -> empty.
3. Repeat with 'ECS Task State Change' and two rules whose detail filters select different exit codes; stop tasks with those exit codes -> nothing is delivered to either queue.
