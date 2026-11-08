Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events.log-group-target-dead-letters-events-timed-too-old-or-too-far-ahead-while-a-current-event-is-written
Issue-Shape-Id: com.amazonaws.eventbridge#AWSEvents
Issue-Key: events.log-group-target-dead-letters-events-timed-too-old-or-too-far-ahead-while-a-current-event-is-written|run#22|AssertionError|assert old["attrs"] == {"ERROR_CODE": "INVALID_PARAMETER", "ERROR_MESSAGE": want_msg,
Issue-Commit: 2ede6488b773bac2f5ae3900456fa42c3facff99

# events: a log-group target's dead letter carries a non-AWS ERROR_CODE and ERROR_MESSAGE for an out-of-window event

> _Note: this issue was identified against the code base at commit `2ede6488b773bac2f5ae3900456fa42c3facff99`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutTargets

## Description
Cause 1 of 1: when an EventBridge rule target is a CloudWatch Logs log group and the event cannot be written because its Time falls outside the window the group accepts, the event IS dead-lettered (the delivery path and the DeadLetterConfig wiring work), but the dead-letter message attributes that classify the failure are the emulator's own vocabulary instead of EventBridge's. Failing check: events.log-group-target-dead-letters-events-timed-too-old-or-too-far-ahead-while-a-current-event-is-written.

AWS: Smithy models the target plumbing only — PutTargets Target.DeadLetterConfig.Arn ("The ARN of the SQS queue specified as the target for the dead-letter queue", ResourceArn @length 1..1600) and Target.RetryPolicy{MaximumRetryAttempts @range 0..185, MaximumEventAgeInSeconds @range 60..86400}; the dead-letter message-attribute vocabulary itself is recorded behavior only. The recording shows that for an event whose Time is far in the past (so CloudWatch Logs refuses the PutLogEvents), EventBridge dead-letters it with exactly four message attributes: ERROR_CODE=INVALID_PARAMETER, ERROR_MESSAGE="Could not complete putLogEvents call for <logGroupName>.", RULE_ARN=<the rule's ARN> and TARGET_ARN=<the log group's ARN>. In that message, "Could not complete putLogEvents call for " and the trailing "." are fixed text, and <logGroupName> is the name of the log group named by the target ARN in the request — never a literal from one run. The same batch's future-timed entry is dead-lettered with ERROR_CODE=EVENTS_IN_BATCH_REQUEST_REJECTED and the same ERROR_MESSAGE, plus EXHAUSTED_RETRY_CONDITION=MaximumRetryAttempts and RETRY_ATTEMPTS=<the target's configured MaximumRetryAttempts, as a decimal string>; the current-time entry of the batch is not dead-lettered and is written to the group with the event's Time as the log timestamp. The replay stopped at the first (past-timed) assertion, so the past-timed leg is the observed divergence and the future-timed/current-time attribute sets are the rest of the same requirement.

Emulator: the dead letter arrives with RULE_ARN and TARGET_ARN correct, but ERROR_CODE="InvalidParameterException" (a Smithy exception name, not the dead-letter error code INVALID_PARAMETER) and ERROR_MESSAGE="The log event's timestamp is outside the window the log group accepts, so no log event was written." — prose the service never emits, with no reference to putLogEvents or the log group name.

Why: a consumer draining an EventBridge dead-letter queue classifies failures by switching on the ERROR_CODE attribute and surfaces ERROR_MESSAGE to operators. With "InvalidParameterException" in that slot, every rule that branches on INVALID_PARAMETER / EVENTS_IN_BATCH_REQUEST_REJECTED falls through to its unknown-failure path, and the message tells the operator nothing about which log group refused the write, so an out-of-window timestamp is indistinguishable from any other parameter fault.

Fix: the EventBridge target-delivery failure path for a CloudWatch Logs log-group target — the component that builds the dead-letter SQS message attributes (it must map a refused PutLogEvents to the dead-letter ERROR_CODE vocabulary and render ERROR_MESSAGE from the target's log group name) rather than forwarding the downstream exception's shape name and its own prose.

Traces: events.log-group-target-dead-letters-events-timed-too-old-or-too-far-ahead-while-a-current-event-is-written.

## Evidence
Expected (AWS) dead-letter message attributes for the 15-day-old entry: {"ERROR_CODE": "INVALID_PARAMETER", "ERROR_MESSAGE": "Could not complete putLogEvents call for <logGroupName>.", "RULE_ARN": <rule ARN>, "TARGET_ARN": <log group ARN>}.
Actual (emulator): {"ERROR_CODE": "InvalidParameterException", "ERROR_MESSAGE": "The log event's timestamp is outside the window the log group accepts, so no log event was written.", "RULE_ARN": <rule ARN, correct>, "TARGET_ARN": <log group ARN, correct>}.
Replay detail: AssertionError: the 15-day-old event's dead letter carries {'ERROR_CODE': 'InvalidParameterException', 'ERROR_MESSAGE': "The log event's timestamp is outside the window the log group accepts, so no log event was written.", 'TARGET_ARN': True, 'RULE_ARN': True} (TARGET_ARN/RULE_ARN shown as True = they matched).
Still unverified behind that first assertion, same cause: the future-timed entry must carry ERROR_CODE=EVENTS_IN_BATCH_REQUEST_REJECTED, the same ERROR_MESSAGE, EXHAUSTED_RETRY_CONDITION=MaximumRetryAttempts and RETRY_ATTEMPTS=<configured MaximumRetryAttempts>; the current-time entry must not be dead-lettered.

## Steps to Reproduce
1. CreateLogGroup <group>; PutResourcePolicy on that group admitting events.amazonaws.com for logs:CreateLogStream/logs:PutLogEvents with Condition ArnEquals aws:SourceArn = the rule ARN.
2. CreateQueue <dlq> with a policy admitting events.amazonaws.com sqs:SendMessage.
3. CreateEventBus <bus>; PutRule on <bus> with EventPattern {"source":[<source>]}.
4. PutTargets with Arn = the log group ARN, DeadLetterConfig.Arn = <dlq> ARN, RetryPolicy {MaximumRetryAttempts: 0, MaximumEventAgeInSeconds: 60}.
5. PutEvents one batch of three entries on <bus> with the same Source: Time = now, Time = now - 15 days, Time = now + 3 hours.
6. ReceiveMessage on <dlq> with MessageAttributeNames=["All"] and inspect the attributes of the entry whose body id is the past-timed event's EventId: ERROR_CODE is "InvalidParameterException" instead of "INVALID_PARAMETER" and ERROR_MESSAGE is the emulator's own prose instead of "Could not complete putLogEvents call for <group>.".
