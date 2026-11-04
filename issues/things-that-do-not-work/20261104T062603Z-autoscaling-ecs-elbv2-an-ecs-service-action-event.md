Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__ecs__elbv2.ecs-service-action-event-triggers-target-lambda
Issue-Key: autoscaling__ecs__elbv2.ecs-service-action-event-triggers-target-lambda|_find_log_matches#27|Timeout|return []
Issue-Commit: b64c3d45927e2833db57dde0eaeae2a33a5b497f

# autoscaling__ecs__elbv2: an ECS service action event never reaches its EventBridge Lambda target (1 cause)

> _Note: this issue was identified against the code base at commit `b64c3d45927e2833db57dde0eaeae2a33a5b497f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutRule

## Description
Cause 1 of 1: ECS service action events are never published to the EventBridge bus, so a rule targeting a Lambda is never invoked.

AWS: ECS emits 'ECS Service Action' events (source aws.ecs) for service-level actions such as a service steady-state or a capacity/placement event; a rule whose pattern matches invokes its target (here a Lambda), and the invocation is observable from the function's output.

Emulator: the check polls until the 60s budget expires — no service-action event is produced, so the rule never fires and the target is never invoked.

Why: a client that reacts to ECS service actions (alerting on a service that cannot place tasks, auto-remediation) sees nothing, and cannot distinguish "the event did not match my pattern" from "the event was never emitted".

Fix: the ECS service scheduler must publish the documented service-action event envelope (source aws.ecs, detail-type 'ECS Service Action') into the EventBridge bus; the existing rule matching and target dispatch then carry it to the Lambda.

Traces: autoscaling__ecs__elbv2.ecs-service-action-event-triggers-target-lambda. The open issue 20261104T042502Z-autoscaling-ecs-elbv2-6-net-new-root-causes.md covers other causes in this suite and does not carry this replay key.

## Evidence
Expected (AWS): the Lambda target is invoked once with an 'ECS Service Action' event body from source aws.ecs.
Actual (emulator): `Error: Check timed out after 60.0s` — the target is never invoked.

## Steps to Reproduce
1. Create a Lambda and PutRule with an event pattern {source: ['aws.ecs'], detail-type: ['ECS Service Action']}, PutTargets the function.
2. Create an ECS service and drive a service action (e.g. let it reach steady state / fail placement).
3. Poll the function's log output/invocation count -> never invoked.
