Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__lambda__sns__sqs.eventbridge-fanout-lambda-sns-sqs
Issue-Key: events__lambda__sns__sqs.eventbridge-fanout-lambda-sns-sqs|_poll_until#35|Timeout|time.sleep(interval_s)
Issue-Commit: 6f030629c911e30ed6652430413113629aa7dccf

# EventBridge fan-out to a Lambda and an SNS->SQS branch from one event delivers to neither branch (1 cause)

> _Note: this issue was identified against the code base at commit `6f030629c911e30ed6652430413113629aa7dccf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutEvents

## Description
Cause 1 of 1 — one PutEvents matched by rules with a Lambda target and an SNS target produces no effect on either branch.
AWS: a single event matched by two rules (or one rule with two targets) is delivered independently to each: the Lambda function is invoked with the event envelope, and the SNS topic is published to and fans out to its SQS subscriber. Each branch's failure is independent of the other's; both effects are observable for one PutEvents.
Emulator: neither branch produces its effect. The check completes CreateFunction, AddPermission, CreateTopic, SetTopicAttributes, Subscribe, SetQueueAttributes, PutRule and PutTargets (FailedEntryCount 0) and PutEvents (FailedEntryCount 0 with an EventId), then polls the function's recorded effect and the subscriber queue; neither appears and the replay cuts the check off at 60 s.
Why: fan-out is the reason to put an event bus in front of consumers at all. With every control-plane call reporting success and no delivery on any branch, a client cannot tell a partial fan-out failure (which AWS would show as one branch succeeding) from total non-delivery, and loses every event.
Fix: the EventBridge target dispatcher's per-target fan-out in services/service-com-amazonaws-eventbridge-awsevents — each matched target must be dispatched independently — together with its Lambda and SNS target paths.
Traces: events__lambda__sns__sqs.eventbridge-fanout-lambda-sns-sqs.

## Evidence
"Check timed out after 60.0s". All wiring calls returned 2xx beforehand (CreateFunction, lambda AddPermission for events.amazonaws.com, CreateTopic, SetTopicAttributes admitting events.amazonaws.com, Subscribe of the SQS queue, SetQueueAttributes admitting sns.amazonaws.com, PutRule, PutTargets with FailedEntryCount 0), and PutEvents returned FailedEntryCount 0 with an EventId. Neither the function's effect nor a message in the SNS-subscribed queue ever appears. Expected: both branch effects observable for the single published event, within the check's delivery budget.

## Steps to Reproduce
1. CreateFunction(fn) with a recorded side effect; lambda AddPermission(fn, Principal='events.amazonaws.com').
2. CreateTopic(t); SetTopicAttributes(t, Policy allowing events.amazonaws.com sns:Publish); CreateQueue(q); SetQueueAttributes(q, Policy allowing sns.amazonaws.com); Subscribe(t, Protocol='sqs', Endpoint=<q arn>).
3. PutRule(Name=r, EventPattern='{"source":["fanout.src"]}'); PutTargets(Rule=r, Targets=[{Id:'t1', Arn:<fn arn>}, {Id:'t2', Arn:<t arn>}]).
4. PutEvents one matching event -> neither the function effect nor a message in q appears.
