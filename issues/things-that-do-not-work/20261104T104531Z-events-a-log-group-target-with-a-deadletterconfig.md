Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events.log-group-target-dead-letters-events-timed-too-old-or-too-far-ahead-while-a-current-event-is-written
Issue-Shape-Id: com.amazonaws.eventbridge#AWSEvents
Issue-Key: events.log-group-target-dead-letters-events-timed-too-old-or-too-far-ahead-while-a-current-event-is-written|_poll_written#11|Timeout|time.sleep(3)
Issue-Commit: a57dea262f41459244daf18c6d5b4cdf00e6a648

# events: a log-group target with a DeadLetterConfig neither writes the in-window event nor dead-letters the out-of-window ones

> _Note: this issue was identified against the code base at commit `a57dea262f41459244daf18c6d5b4cdf00e6a648`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutTargets

## Description
One net-new root cause. It is the same missing CloudWatch Logs target kind reported for service events__logs in the companion issue filed in this sweep, but it is grounded in a check the suite lists under `### events` and it adds a requirement that issue does not cover: the dead-letter classification of entries whose Time falls outside the PutLogEvents window. No open issue covers it; the closest EventBridge delivery issues (e.g. 20261029T182228Z-events-eventbridge-does-not-authorize-sqs-targets-against-the.md) concern SQS targets.

Cause 1 of 1: a CloudWatch Logs log-group target silently drops every event instead of writing the in-window one and dead-lettering the out-of-window ones.
AWS: Smithy (eventbridge): PutTargets.Targets[].Arn is required, and a Target may carry DeadLetterConfig and RetryPolicy{MaximumRetryAttempts, MaximumEventAgeInSeconds}; PutTargets answers FailedEntryCount=0 for an accepted log-group target. The rest is recorded behavior only. PutEvents accepts all three entries of a batch (one stamped now, one stamped 15 days ago, one stamped 3 hours in the future) with FailedEntryCount=0 and returns an EventId per entry. The current-time entry is written to the log group as one log event whose timestamp is the entry's Time in epoch milliseconds. The two entries whose Time is outside the log group's accepted PutLogEvents window are NOT written and are instead sent to the target's dead-letter queue, each as a message whose body is the event envelope (so body["id"] is that entry's EventId) and whose SQS message attributes are exactly: for the 15-day-old entry ERROR_CODE="INVALID_PARAMETER", ERROR_MESSAGE, RULE_ARN and TARGET_ARN and nothing else; for the 3-hours-ahead entry ERROR_CODE="EVENTS_IN_BATCH_REQUEST_REJECTED", the same ERROR_MESSAGE, EXHAUSTED_RETRY_CONDITION="MaximumRetryAttempts", RETRY_ATTEMPTS="0", RULE_ARN and TARGET_ARN. ERROR_MESSAGE's fixed text is `Could not complete putLogEvents call for ` followed by the target log group's NAME and a final `.`; the group name, RULE_ARN (the rule's ARN) and TARGET_ARN (the log-group ARN without the trailing :*) are values taken from the request and state, and RETRY_ATTEMPTS is the configured MaximumRetryAttempts rendered as a string. The current-time entry must NOT appear in the dead-letter queue, and the group must hold exactly that one entry of the batch.
Emulator: PutTargets accepts the target (FailedEntryCount=0) and PutEvents accepts the batch, but nothing is ever written to the log group — the check's liveness loop, which re-PutEvents a current-time event and polls FilterLogEvents for up to 180 s, never sees a single record — and nothing is ever sent to the dead-letter queue, so neither out-of-window entry is classified at all. The check is killed at the harness ceiling with `Check timed out after 60.0s`. The emulator therefore reports success for events it discards, with no log record and no dead letter.
Why: a client relying on AWS's behavior loses both halves of the contract. The in-window event it published is reported accepted but is unobservable, and the two events AWS would hand it on the DLQ — with the exact ERROR_CODE that tells it WHY (a timestamp outside the window, INVALID_PARAMETER, versus a batch rejection that exhausted MaximumRetryAttempts) — never arrive, so an out-of-window publisher looks healthy and its events are lost without any failure signal.
Fix: the EventBridge target-delivery dispatcher — the CloudWatch Logs target kind (write one log event per matched event, timestamp = the envelope time) and its failure path: validate the event Time against the PutLogEvents window, and on rejection route the envelope to the target's DeadLetterConfig queue with the ERROR_CODE/ERROR_MESSAGE/RULE_ARN/TARGET_ARN attributes, adding EXHAUSTED_RETRY_CONDITION and RETRY_ATTEMPTS only for the retried (batch-rejected) class.
Traces: events.log-group-target-dead-letters-events-timed-too-old-or-too-far-ahead-while-a-current-event-is-written.

## Evidence
Suite report, ### events: `[mismatch] events.log-group-target-dead-letters-events-timed-too-old-or-too-far-ahead-while-a-current-event-is-written` / `Error: Check timed out after 60.0s` — no ALREADY FILED / OWNED marker.

Expected: PutEvents on a batch of three entries (Time = now, now-15d, now+3h) answers FailedEntryCount=0; the dead-letter queue receives the old entry with message attributes {ERROR_CODE: "INVALID_PARAMETER", ERROR_MESSAGE: "Could not complete putLogEvents call for <logGroupName>.", RULE_ARN: <rule ARN>, TARGET_ARN: <log-group ARN>} and the future entry with {ERROR_CODE: "EVENTS_IN_BATCH_REQUEST_REJECTED", ERROR_MESSAGE: same, EXHAUSTED_RETRY_CONDITION: "MaximumRetryAttempts", RETRY_ATTEMPTS: "0", RULE_ARN, TARGET_ARN}; the current entry is absent from the DLQ and present in the log group with timestamp == int(Time.timestamp()*1000).

Actual: the log group receives nothing (the pre-assertion liveness loop `no liveness event reached the log group within 180.0s` can never be satisfied) and the DLQ receives nothing, so no ERROR_CODE is produced at all. The check is terminated by the harness at 60 s. Only the fixed text `Could not complete putLogEvents call for ` and the trailing `.` are literal in ERROR_MESSAGE; the log group name, both ARNs and "0" are values from the request/state and must not be hard-coded.

## Steps to Reproduce
1. logs.create_log_group(logGroupName=<group>); read its ARN and strip the trailing ":*".
2. sqs.create_queue(<dlq>); set a Policy allowing events.amazonaws.com sqs:SendMessage on it with aws:SourceAccount=<account>.
3. events.create_event_bus(Name=<bus>); events.put_rule(Name=<rule>, EventBusName=<bus>, EventPattern={"source":[<source>]}).
4. logs.put_resource_policy(resourceArn=<group ARN>, policyDocument=Allow events.amazonaws.com logs:CreateLogStream+logs:PutLogEvents on "<group ARN>:*" with ArnEquals aws:SourceArn=<rule ARN>).
5. events.put_targets(Rule=<rule>, EventBusName=<bus>, Targets=[{"Id":"group","Arn":<group ARN>,"DeadLetterConfig":{"Arn":<dlq ARN>},"RetryPolicy":{"MaximumRetryAttempts":0,"MaximumEventAgeInSeconds":60}}]) — FailedEntryCount=0.
6. events.put_events with three entries on <bus>, same Source, Time = now / now-15 days / now+3 hours, Detail={"m":<marker>} — FailedEntryCount=0, three EventIds returned.
7. Poll logs.filter_log_events(logGroupName=<group>) — stays empty (AWS holds exactly the current-time entry, timestamped at its Time).
8. Poll sqs.receive_message(QueueUrl=<dlq>, MessageAttributeNames=["All"]) — stays empty (AWS delivers the old and future entries with the ERROR_CODE/ERROR_MESSAGE/RULE_ARN/TARGET_ARN attributes above, and only the future one carries EXHAUSTED_RETRY_CONDITION and RETRY_ATTEMPTS).
