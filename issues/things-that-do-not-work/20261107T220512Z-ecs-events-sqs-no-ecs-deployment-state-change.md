Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs__events__sqs.ecs-deployment-state-change-events-deliver-in-progress-then-failed
Issue-Key: ecs__events__sqs.ecs-deployment-state-change-events-deliver-in-progress-then-failed|_drain_for#19|Timeout|resp = sqs.receive_message(
Issue-Commit: ae442358c61414b2bbb5a7e8dd80b7eec77accf8

# ecs__events__sqs: no "ECS Deployment State Change" event is ever published, so an EventBridge rule targeting SQS receives neither SERVICE_DEPLOYMENT_IN_PROGRESS nor SERVICE_DEPLOYMENT_FAILED

> _Note: this issue was identified against the code base at commit `ae442358c61414b2bbb5a7e8dd80b7eec77accf8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateService

## Description
Cause 1 of 1: the ECS service-deployment lifecycle publishes no events onto the EventBridge bus.
  AWS: every service-deployment transition publishes an event with source `aws.ecs` and detail-type `ECS Deployment State Change` whose detail carries eventType (INFO/ERROR), eventName (SERVICE_DEPLOYMENT_IN_PROGRESS, SERVICE_DEPLOYMENT_COMPLETED, SERVICE_DEPLOYMENT_FAILED), the deploymentId and the service ARN; a rule matching {source:["aws.ecs"], detail-type:["ECS Deployment State Change"]} delivers each one to its SQS target, so a healthy deployment yields IN_PROGRESS then COMPLETED and a deployment failed by the circuit breaker yields SERVICE_DEPLOYMENT_FAILED naming the failed deploymentId (documented: AmazonECS/latest/developerguide/ecs_service_deployment_events.html). Recorded behavior only for the timing.
  Emulator: nothing is ever delivered — the check polls ReceiveMessage until the harness budget expires (`Error: Check timed out after 180.0s`), so neither the positive (healthy) leg nor the failed leg can be observed, and the negative control cannot be distinguished from the positive one.
  Why: event-driven consumers of ECS deployments (alerting on SERVICE_DEPLOYMENT_FAILED, gating a pipeline on SERVICE_DEPLOYMENT_COMPLETED) cannot be exercised at all; a client sees an empty queue and cannot tell a correctly-scoped rule from a broken one.
  Fix: the ECS service-deployment state machine must emit the documented event envelope (source aws.ecs, detail-type "ECS Deployment State Change", detail with eventType/eventName/deploymentId/service ARN) into the EventBridge bus, from which the existing rule matching and SQS target delivery can route it.
  Note: the open 20261104T062603Z-ecs-events-sqs-ecs-never-publishes-deployment-or.md records this same cause and even lists this check id, but under a different replay key (a 60s Timeout inside _settle_rule); this run fails with a 180s harness timeout, so the check is not owned by it — close them together.
  Traces: ecs__events__sqs.ecs-deployment-state-change-events-deliver-in-progress-then-failed (suite heading `### ecs__events__sqs`).

## Evidence
Expected (AWS): the SQS target of a rule on {source:["aws.ecs"], detail-type:["ECS Deployment State Change"]} receives an INFO event with detail.eventName=SERVICE_DEPLOYMENT_IN_PROGRESS followed by SERVICE_DEPLOYMENT_COMPLETED for the healthy deployment, and an ERROR event with detail.eventName=SERVICE_DEPLOYMENT_FAILED naming the failed deploymentId for the circuit-breaker-failed deployment.
Actual (emulator): `Error: Check timed out after 180.0s` — ReceiveMessage returns no message for any leg; no aws.ecs deployment event is ever published.

## Steps to Reproduce
1. sqs create-queue; events put-rule with event pattern {"source":["aws.ecs"],"detail-type":["ECS Deployment State Change"]} and events put-targets with the queue.
2. ecs create-cluster, register-task-definition, create-service with deploymentCircuitBreaker enabled; wait for steady state.
3. ecs update-service to a revision that cannot start (unresolvable image tag) so the circuit breaker fails the deployment.
4. Poll sqs receive-message -> no message arrives for either the IN_PROGRESS/COMPLETED pair or the FAILED event.
Replay ecs__events__sqs.ecs-deployment-state-change-events-deliver-in-progress-then-failed.
