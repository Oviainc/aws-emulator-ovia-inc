Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__lambda__sns__sqs.eventbridge-lambda-rule-matching events__lambda__sns__sqs.eventbridge-lambda-missing-resource-policy events__lambda__sns__sqs.sns-lambda-notification events__lambda__sns__sqs.sqs-lambda-esm-delivers events__lambda__sns__sqs.sqs-lambda-esm-missing-permission
Issue-Key: events__lambda__sns__sqs.eventbridge-lambda-rule-matching|_poll_until#22|Timeout|time.sleep(interval_s)
Issue-Key: events__lambda__sns__sqs.eventbridge-lambda-missing-resource-policy|_poll_until#23|Timeout|time.sleep(interval_s)
Issue-Key: events__lambda__sns__sqs.sns-lambda-notification|_poll_until#14|Timeout|time.sleep(interval_s)
Issue-Key: events__lambda__sns__sqs.sqs-lambda-esm-delivers|_poll_until#11|Timeout|time.sleep(interval_s)
Issue-Key: events__lambda__sns__sqs.sqs-lambda-esm-missing-permission|run#17|AssertionError|assert depth >= 1, (
Issue-Commit: b1cd28f1ca277d6a032df605f83af5017e934043

# events/lambda/sns/sqs: EventBridge and SNS never invoke Lambda, and an SQS event source mapping consumes messages while its role has no receive permission (3 causes)

> _Note: this issue was identified against the code base at commit `b1cd28f1ca277d6a032df605f83af5017e934043`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSourceMapping

## Description
Three root causes. 20261026T070928Z-events-lambda-sns-sqs-a-putevents-event-matching.md and 20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md own other checks in this service; the checks below were not bound to them.

Cause 1 of 3: an EventBridge rule never invokes its Lambda target, with or without the function's resource-policy grant.
AWS: a matching PutEvents invokes the Lambda target once when the function policy allows lambda:InvokeFunction for events.amazonaws.com, and not at all when it does not.
Emulator: the function is never invoked in either state; both checks end "Check timed out after 60.0s", so the negative leg is ungraded.
Why: the pattern is unusable and the authorization difference is unobservable.
Fix: the EventBridge Lambda-target invoker and its resource-policy evaluation.
Traces: events__lambda__sns__sqs.eventbridge-lambda-rule-matching, events__lambda__sns__sqs.eventbridge-lambda-missing-resource-policy

Cause 2 of 3: an SNS topic with a Lambda subscription never invokes the function on Publish.
AWS: Publish to a topic with a confirmed lambda-protocol subscription invokes the function once with the SNS event payload — Records[0].Sns carrying TopicArn, Message, MessageId, Subject and Timestamp.
Emulator: the function is never invoked; the check ends "Check timed out after 60.0s".
Why: SNS fan-out to Lambda does nothing.
Fix: the SNS Lambda-subscription dispatcher (payload construction and invocation).
Traces: events__lambda__sns__sqs.sns-lambda-notification

Cause 3 of 3: an SQS event source mapping deletes messages from the queue without invoking the function, and without the role permission that would authorize it.
AWS: an ESM polls with the function's execution role; if the role lacks sqs:ReceiveMessage and sqs:DeleteMessage, no message is received and none is deleted, so the queue depth is unchanged and the message stays visible after its visibility timeout. With the permissions present, the function is invoked and only then is the batch deleted.
Emulator: in the permitted case the function is never invoked ("Check timed out after 60.0s"); in the unpermitted case the message disappears from the queue regardless. "AssertionError: message vanished from the subject queue while its ESM had no sqs:ReceiveMessage/DeleteMessage permission (depth=0, elapsed=5.0s of 15.0s window calibrated from a 2.0s control)" — the requirement is that the depth stays at 1; the elapsed times are per-run values.
Why: this silently destroys data. A client relying on a scoped-down ESM role to leave a queue untouched finds the messages gone, and the function that was supposed to process them never ran, so the payload is lost with no record anywhere.
Fix: the Lambda event-source-mapping poller must authorize its receive and delete calls with the execution role, and must only delete a batch after a successful invocation.
Traces: events__lambda__sns__sqs.sqs-lambda-esm-delivers, events__lambda__sns__sqs.sqs-lambda-esm-missing-permission

## Evidence
Causes 1 and 2: "Error: Check timed out after 60.0s" with the Lambda never invoked after a matching PutEvents or an SNS Publish.
Cause 3: "AssertionError: message vanished from the subject queue while its ESM had no sqs:ReceiveMessage/DeleteMessage permission (depth=0, elapsed=5.0s of 15.0s window calibrated from a 2.0s control)" — expected the queue depth to stay at 1 and the function never to be invoked; and in the permitted variant "Error: Check timed out after 60.0s" with the function never invoked.

## Steps to Reproduce
(1) PutRule + PutTargets with a Lambda target (with and without AddPermission) and PutEvents — never invoked. (2) Subscribe a Lambda to a topic and Publish — never invoked. (3) CreateEventSourceMapping whose function execution role has no sqs:ReceiveMessage/DeleteMessage, SendMessage one message, then read ApproximateNumberOfMessages — it drops to 0 and the function never ran.
