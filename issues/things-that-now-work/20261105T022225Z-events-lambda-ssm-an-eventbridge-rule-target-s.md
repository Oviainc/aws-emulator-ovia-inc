Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__lambda__ssm.eventbridge-dlq-captures-failed-lambda-invocation
Issue-Key: events__lambda__ssm.eventbridge-dlq-captures-failed-lambda-invocation|publish_then_poll_success#31|Timeout|resp = sqs.receive_message(
Issue-Commit: 3ae1c5f9ad044bf0dd64ac25eb19f35c30d1d713

# events__lambda__ssm: an EventBridge rule target's dead-letter queue never receives a failed Lambda invocation, so the check never reaches a verdict

> _Note: this issue was identified against the code base at commit `3ae1c5f9ad044bf0dd64ac25eb19f35c30d1d713`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutTargets

## Description
One root cause. The failing check is not marked OWNED or ALREADY FILED in the replay report.

AWS: recorded behavior only. A rule target configured with DeadLetterConfig (an SQS queue ARN) has its failed deliveries dead-lettered: when EventBridge cannot deliver a matched event to its Lambda target (the invocation fails, or the target is not invocable), the event is written to the configured queue after retries, carrying the EventBridge message attributes that name the failure (RULE_ARN, TARGET_ARN, ERROR_CODE, ERROR_MESSAGE) alongside the original event body. A client therefore observes the dead-lettered event on the queue within seconds of the retries being exhausted.
Emulator: nothing is ever written to the target's dead-letter queue. The check publishes a matching event and polls the DLQ, and is abandoned with "Check timed out after 60.0s" — no verdict is reached. This is consistent with EventBridge having no delivery path to Lambda targets in this emulator at all (see the sibling finding filed for events__lambda__sqs), so there is no failed delivery to dead-letter in the first place.
Why: a client that relies on a DLQ to detect undelivered events gets permanent silence. Events that AWS would surface as dead-lettered, with the error code and message explaining why, simply vanish — the failure mode EventBridge dead-lettering exists to make visible becomes invisible.
Fix: the EventBridge target delivery path — it must attempt the target invocation, apply the target's RetryPolicy, and on exhaustion publish the event plus the RULE_ARN/TARGET_ARN/ERROR_CODE/ERROR_MESSAGE attributes to the target's DeadLetterConfig queue. An open issue covers EventBridge never invoking Lambda targets in this scenario group (20261029T182228Z-events-lambda-ssm-eventbridge-never-invokes-lambda-targets.md); it does not carry this check's replay key, and the dead-letter leg is a distinct observable.
Traces: events__lambda__ssm.eventbridge-dlq-captures-failed-lambda-invocation.

## Evidence
Expected: after a matching PutEvents whose Lambda target cannot be invoked successfully, the rule target's DeadLetterConfig SQS queue receives the event with EventBridge's failure message attributes.
Actual (replay): `Error: Check timed out after 60.0s` — the DLQ poll never returns a message, so the check is abandoned without a verdict.

## Steps to Reproduce
1. Create a Lambda function that always raises, an SQS queue for dead letters, and an EventBridge rule with an event pattern.
2. PutTargets with the function ARN as the target and DeadLetterConfig.Arn set to the queue ARN; grant events.amazonaws.com lambda:InvokeFunction on the function and sqs:SendMessage on the queue.
3. PutEvents with an entry matching the pattern.
4. Poll SQS ReceiveMessage on the dead-letter queue — nothing ever arrives.
