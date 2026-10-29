Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__lambda__ssm.eventbridge-rule-invokes-lambda-basic events__lambda__ssm.eventbridge-fanout-two-lambda-targets-write-ssm events__lambda__ssm.eventbridge-input-transformer-reshapes-event-for-lambda events__lambda__ssm.eventbridge-dlq-captures-failed-lambda-invocation events__lambda__ssm.eventbridge-reserved-aws-source-rejected-custom-delivered events__lambda__ssm.cfn-deploy-eventbridge-lambda-ssm-pipeline events__lambda__ssm.ssm-parameter-change-triggers-lambda-via-eventbridge
Issue-Key: events__lambda__ssm.eventbridge-rule-invokes-lambda-basic|_poll_until#15|Timeout|time.sleep(interval_s)
Issue-Key: events__lambda__ssm.eventbridge-fanout-two-lambda-targets-write-ssm|_poll_until#18|Timeout|time.sleep(interval_s)
Issue-Key: events__lambda__ssm.eventbridge-input-transformer-reshapes-event-for-lambda|_poll_until#15|Timeout|time.sleep(interval_s)
Issue-Key: events__lambda__ssm.eventbridge-dlq-captures-failed-lambda-invocation|publish_then_poll#16|Timeout|resp = sqs.receive_message(
Issue-Key: events__lambda__ssm.eventbridge-reserved-aws-source-rejected-custom-delivered|_poll_until#25|Timeout|time.sleep(interval_s)
Issue-Key: events__lambda__ssm.cfn-deploy-eventbridge-lambda-ssm-pipeline|_poll_until#17|Timeout|time.sleep(interval_s)
Issue-Key: events__lambda__ssm.ssm-parameter-change-triggers-lambda-via-eventbridge|_poll_until#19|Timeout|time.sleep(interval_s)
Issue-Commit: b1cd28f1ca277d6a032df605f83af5017e934043

# events/lambda/ssm: EventBridge never invokes Lambda targets and SSM publishes no Parameter Store Change events (2 causes)

> _Note: this issue was identified against the code base at commit `b1cd28f1ca277d6a032df605f83af5017e934043`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutEvents

## Description
Two root causes. 20261025T002357Z-events-lambda-ssm-eventbridge-never-invokes-a-lambda-target-so-neither.md and 20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md own other checks in this service; the checks below were not bound to them.

Cause 1 of 2: an EventBridge rule never invokes its Lambda target, so fan-out, InputTransformer rendering, the dead-letter path and the reserved-source rule are all unobservable.
AWS: a matching PutEvents invokes each Lambda target once with the envelope (or the InputTransformer rendering) as the payload; a rule with two Lambda targets invokes both; a target whose invocation fails is dead-lettered into the target's DeadLetterConfig with RULE_ARN, TARGET_ARN, ERROR_CODE and ERROR_MESSAGE message attributes. PutEvents of an entry whose Source begins with the reserved prefix "aws." is rejected for that entry (FailedEntryCount>0 with ErrorCode "NotAuthorizedForSourceException") while a custom source in the same batch is accepted and delivered.
Emulator: no Lambda target is ever invoked and nothing is dead-lettered; every check ends "Check timed out after 60.0s", so the reserved-source differential (reject the aws.* entry, deliver the custom one) cannot be observed either.
Why: the EventBridge→Lambda pattern is unusable, no failure signal exists, and a client cannot tell a correctly rejected reserved source from a bus that delivers nothing.
Fix: the EventBridge Lambda-target invoker, its InputTransformer renderer and its dead-letter path, plus the reserved-source validation in the PutEvents handler.
Traces: events__lambda__ssm.eventbridge-rule-invokes-lambda-basic, events__lambda__ssm.eventbridge-fanout-two-lambda-targets-write-ssm, events__lambda__ssm.eventbridge-input-transformer-reshapes-event-for-lambda, events__lambda__ssm.eventbridge-dlq-captures-failed-lambda-invocation, events__lambda__ssm.eventbridge-reserved-aws-source-rejected-custom-delivered, events__lambda__ssm.cfn-deploy-eventbridge-lambda-ssm-pipeline

Cause 2 of 2: SSM publishes no "Parameter Store Change" event to EventBridge.
AWS: PutParameter emits on the default bus source "aws.ssm", detail-type "Parameter Store Change", with detail carrying name, type, operation (Create/Update/Delete) and description, and the parameter ARN in the envelope's resources. recorded behavior only for the exact detail body.
Emulator: a rule scoped to that source/detail-type receives nothing after PutParameter; the check ends "Check timed out after 60.0s".
Why: configuration-change-driven automation (reload on parameter change, audit a secret rotation) never fires.
Fix: the SSM PutParameter/DeleteParameter handlers must publish the Parameter Store Change event to the EventBridge bus.
Traces: events__lambda__ssm.ssm-parameter-change-triggers-lambda-via-eventbridge

## Evidence
All seven checks end "Error: Check timed out after 60.0s": the Lambda target is never invoked after a matching PutEvents, nothing reaches the configured dead-letter queue, and a rule on {"source":["aws.ssm"],"detail-type":["Parameter Store Change"]} receives nothing after PutParameter. Expected: one invocation per target per matched event, one DLQ message carrying RULE_ARN/TARGET_ARN/ERROR_CODE/ERROR_MESSAGE on failure, and one aws.ssm envelope per parameter write.

## Steps to Reproduce
(1) PutRule + PutTargets with one and then two Lambda targets (and an InputTransformer), AddPermission for events.amazonaws.com, PutEvents a matching entry — no target is invoked. (2) PutEvents a batch with one entry whose Source is "aws.something" and one custom source — observe the per-entry result and the delivery. (3) PutRule on {"source":["aws.ssm"],"detail-type":["Parameter Store Change"]} with a target, PutParameter, and poll — nothing arrives.
