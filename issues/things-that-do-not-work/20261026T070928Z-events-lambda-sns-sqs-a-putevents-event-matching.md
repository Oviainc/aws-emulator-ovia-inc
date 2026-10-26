Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__lambda__sns__sqs.eventbridge-fanout-lambda-sns-sqs
Issue-Key: events__lambda__sns__sqs.eventbridge-fanout-lambda-sns-sqs|_poll_until#35|Timeout|time.sleep(interval_s)
Issue-Commit: 6eb5dfdd8fc0f77b54a569b75c45f405ceb2f53c

# events__lambda__sns__sqs: a PutEvents event matching an EventBridge rule reaches NONE of the rule's three targets (Lambda, SQS, SNS), so the fan-out never happens

> _Note: this issue was identified against the code base at commit `6eb5dfdd8fc0f77b54a569b75c45f405ceb2f53c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutEvents

## Description
One root cause (Cause 1 of 1).

Attribution note: this check id's leading token is the composite 'events__lambda__sns__sqs'. The per-service bootstrap gate cannot match that composite literally, but every constituent service is stood up in this checkout (events -> service-com-amazonaws-eventbridge-awsevents / service-com-amazonaws-cloudwatchevents-awsevents, lambda -> service-com-amazonaws-lambda-awsgirapiservice, sns -> service-com-amazonaws-sns-amazonsimplenotificationservice, sqs -> service-com-amazonaws-sqs-amazonsqs are all in the bootstrapped set; none appears in pending), so the verdict is FILE. The operation whose effect is missing is the EventBridge one.

Related OPEN issues that do NOT cover this failure (the suite marked this check neither 'OWNED by' nor 'ALREADY FILED'): 20261025T002357Z-events-lambda-sns-eventbridge-sns-targets-never-publish-and-an-sns.md reports the SNS-target leg for the events__lambda__sns group, and 20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md reports Lambda Invoke refusing real invocations. Neither carries this check's replay key, and this check fails on ALL THREE target kinds at once — including the pure SQS target, which involves neither SNS nor Lambda — so closing either of those alone will not make it pass.

Cause 1 of 1: a rule with three targets receives no delivery of a matching event.
AWS: recorded behavior only for the fan-out effect (PutEvents' response shape only reports per-entry acceptance: FailedEntryCount and Entries[].EventId, so the request/response pair cannot show it). Expected: one PutRule with EventPattern {"source": ["test.fanout"], "detail-type": ["Broadcast"]} and one PutTargets carrying three targets — a Lambda function ARN, an SQS queue ARN and an SNS topic ARN — fans a single matching PutEvents entry out to all three: the Lambda function is invoked (observable here by the item it writes to DynamoDB), the SQS queue receives the event envelope, and the SNS topic publishes it to its own subscribed SQS queue. Resource permissions for all three were granted in the same run (lambda:InvokeFunction for events.amazonaws.com with the rule ARN as SourceArn, an SQS queue policy allowing events.amazonaws.com:SendMessage, and a topic policy allowing events.amazonaws.com:Publish).
Emulator: every control-plane call (PutRule, PutTargets, AddPermission, SetQueueAttributes, SetTopicAttributes, Subscribe) succeeds and PutEvents is accepted, but no target ever observes the event. The check re-publishes the same entry every 5s and polls all three sinks in the same loop; its predicate (Lambda's DynamoDB item present AND the direct queue non-empty AND the via-SNS queue non-empty) is never satisfied, and the harness kills the check at its 60s bound ("Check timed out after 60.0s"; the check's own poll deadline is 120s). No call raises.
Why: EventBridge is unusable as an event router. A client can declare rules and targets, gets 200s and a FailedEntryCount of 0 from PutEvents, and nothing downstream ever runs — every event-driven integration (fan-out, notification, async processing) silently does nothing, which is indistinguishable from an event that legitimately did not match.
Fix: the rule-target dispatch path behind PutEvents in the EventBridge service (services/service-com-amazonaws-eventbridge-awsevents/src/ops/put_events.rs and the rule/target state it reads, written by put_rule.rs / put_targets.rs) — a matched entry must be delivered to each configured target kind: Lambda Invoke for a function ARN, SQS SendMessage for a queue ARN, SNS Publish for a topic ARN.
Traces: events__lambda__sns__sqs.eventbridge-fanout-lambda-sns-sqs.

## Evidence
Suite simparity-behavioral-ga-prod-v1: events__lambda__sns__sqs 0/1 passed, 1 failed, and that check carries no 'OWNED by' / 'ALREADY FILED' marker.

Failing check, verbatim detail: "Error: Check timed out after 60.0s".

Shape of the run (from the replayed check): _create_table / _create_queue x2 / _create_topic / _create_lambda, PutRule with EventPattern {"source": ["test.fanout"], "detail-type": ["Broadcast"]}, lambda AddPermission(Principal='events.amazonaws.com', SourceArn=<rule arn>), an SQS queue policy and an SNS topic policy both allowing events.amazonaws.com, Subscribe(Protocol='sqs') for the via-SNS queue, then PutTargets with Ids toLambda/toSqs/toSns. The probe loop then calls put_events(Entries=[{Source: 'test.fanout', DetailType: 'Broadcast', Detail: '{"id": "fanout-1"}'}]) every 5s and drains both queues plus dynamodb.get_item for the Lambda's marker.

Expected: the probe predicate holds (Lambda marker item present, direct queue non-empty, via-SNS queue non-empty) and the three closing assertions pass. Actual: the predicate never holds and the check is killed at the 60s harness bound with no error from any API call — i.e. all three sinks stayed empty. The three assertions the check would otherwise report are "fan-out event never reached the Lambda target", "fan-out event never reached the SQS target" and "fan-out event never reached the SNS target".

## Steps to Reproduce
1. Boot the emulator; point botocore events/lambda/sns/sqs/dynamodb/iam clients at it.
2. Create a DynamoDB table, two SQS queues, an SNS topic and a Lambda function whose handler writes one item to the table.
3. events.put_rule(Name=<rule>, EventPattern='{"source": ["test.fanout"], "detail-type": ["Broadcast"]}').
4. Grant the three target permissions: lambda.add_permission(Action='lambda:InvokeFunction', Principal='events.amazonaws.com', SourceArn=arn:aws:events:<region>:<account>:rule/<rule>); an SQS queue policy allowing events.amazonaws.com:SendMessage on the direct queue; an SNS topic policy allowing events.amazonaws.com:Publish; and sns.subscribe(Protocol='sqs') the second queue to the topic.
5. events.put_targets(Rule=<rule>, Targets=[{'Id':'toLambda','Arn':<fn arn>},{'Id':'toSqs','Arn':<direct queue arn>},{'Id':'toSns','Arn':<topic arn>}]).
6. events.put_events(Entries=[{'Source':'test.fanout','DetailType':'Broadcast','Detail':'{"id": "fanout-1"}'}]) and poll: dynamodb.get_item for the Lambda marker, sqs.receive_message on the direct queue, sqs.receive_message on the via-SNS queue. Expect all three to be satisfied; observe all three empty indefinitely (the check is killed at 60s).
7. Full grounding: replay events__lambda__sns__sqs.eventbridge-fanout-lambda-sns-sqs.
