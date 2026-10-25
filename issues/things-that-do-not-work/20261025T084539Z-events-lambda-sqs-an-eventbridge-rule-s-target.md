Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__lambda__sqs.eventbridge-rule-disabled-state-blocks-delivery events__lambda__sqs.events-delivery-and-failure-event-age-limit-dead-letters-without-further-attempts
Issue-Key: events__lambda__sqs.eventbridge-rule-disabled-state-blocks-delivery|_drain_queue#18|Timeout|resp = sqs.receive_message(
Issue-Key: events__lambda__sqs.events-delivery-and-failure-event-age-limit-dead-letters-without-further-attempts|_poll_until#31|Timeout|time.sleep(interval_s)
Issue-Commit: 8b92040b06acdac381093d8b397331a442bd94ad

# events__lambda__sqs: an EventBridge rule's target is never invoked, so neither the DISABLED-state negative nor MaximumEventAgeInSeconds dead-lettering can be observed (2 causes)

> _Note: this issue was identified against the code base at commit `8b92040b06acdac381093d8b397331a442bd94ad`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutEvents

## Description
Two EventBridge delivery behaviours in the events__lambda__sqs suite are unobservable because no target invocation ever completes. The open issue 20261025T002357Z-events-lambda-sqs-eventbridge-lambda-targets-and-sqs-event-source-mappings.md owns three other checks in this suite; the replay report does not mark either check below as OWNED or ALREADY FILED, and neither the DISABLED-state negative nor the event-age retry condition is named there.

Cause 1 of 2: a rule's State is not observable because no delivery happens on either side of the control.
AWS: recorded behavior only (rule State is modelled, its delivery effect is not). A rule whose State is DISABLED invokes no target for a matching PutEvents; re-enabling the same rule and resending the same event DOES invoke the target. The pair is what makes DISABLED attributable: the positive control must deliver.
Emulator: the check hangs to the 60 s harness cap — the positive control never delivers, so the emulator produces the same "nothing arrived" for an ENABLED rule as for a DISABLED one. A consumer cannot distinguish "suppressed because disabled" from "never delivered at all".
Why: rule enable/disable is the standard kill switch. An emulator in which neither state delivers reports a passing kill switch while the underlying delivery is broken, so a client validating its runbook learns nothing.
Fix: the eventbridge rule-target dispatcher (the Lambda target branch) and the rule State gate in front of it.
Traces: events__lambda__sqs.eventbridge-rule-disabled-state-blocks-delivery.

Cause 2 of 2: MaximumEventAgeInSeconds never ends a retry sequence, because no attempt is ever made.
AWS: recorded behavior only (RetryPolicy is inert in the PutTargets response; its effect is observable only at the endpoint and in the DLQ). With RetryPolicy {MaximumEventAgeInSeconds: 60, MaximumRetryAttempts: 185} against an endpoint that always answers a retryable HTTP 500, EventBridge retries until the event is older than 60 s, then dead-letters it and makes NO further attempt; the DLQ message's attributes name the AGE condition as the one that ended it (EXHAUSTED_RETRY_CONDITION) together with RETRY_ATTEMPTS.
Emulator: the check hangs to the 60 s harness cap with the endpoint recording no attempt and the DLQ receiving no message — neither the retry sequence nor its age-based termination exists.
Why: a client relying on MaximumEventAgeInSeconds to bound how long a failing event is retried, and on the DLQ attributes to tell age-expiry apart from attempt-exhaustion, gets no retries, no dead letter and no attributes.
Fix: the eventbridge target retry engine — per-attempt dispatch, the age/attempt exhaustion conditions, and the dead-letter message attributes it must stamp.
Traces: events__lambda__sqs.events-delivery-and-failure-event-age-limit-dead-letters-without-further-attempts.

## Evidence
events__lambda__sqs.eventbridge-rule-disabled-state-blocks-delivery [mismatch]: `Check timed out after 60.0s`. The check sets the rule DISABLED, puts a matching event, asserts no target invocation, then re-ENABLES the rule and resends; the positive control never produces an invocation, so the check cannot complete. Expected: no invocation while DISABLED, and an invocation after re-enabling. Actual: no invocation in either state.

events__lambda__sqs.events-delivery-and-failure-event-age-limit-dead-letters-without-further-attempts [mismatch]: `Check timed out after 60.0s`. Target RetryPolicy is {MaximumEventAgeInSeconds: 60, MaximumRetryAttempts: 185} with an endpoint that always answers HTTP 500 and records every hit. Expected: repeated attempts recorded at the endpoint, then one DLQ message whose attributes report the AGE exhaustion condition and the RETRY_ATTEMPTS count, and no attempt after that. Actual: zero recorded attempts and an empty DLQ.

## Steps to Reproduce
Cause 1:
1. Create an SQS result queue, a Lambda role allowing sqs:SendMessage, and a Lambda function that forwards its event to the queue.
2. create_event_bus; put_rule(..., State='DISABLED', EventPattern=<matching>); put_targets with the function ARN; add the lambda permission for events.amazonaws.com.
3. put_events a matching event -> assert the queue stays empty.
4. enable_rule and put_events the same event again -> the queue should receive the forwarded event; observe that it does not, and the check hits the 60 s cap.

Cause 2:
1. Create a hits queue and a DLQ (with a policy allowing events.amazonaws.com to send).
2. Create a Lambda function URL endpoint that records each request to the hits queue and always answers HTTP 500; create a connection + api destination pointing at it.
3. create_event_bus; put_rule; put_targets with the destination ARN, DeadLetterConfig=<DLQ arn> and RetryPolicy={'MaximumEventAgeInSeconds': 60, 'MaximumRetryAttempts': 185}.
4. put_events a matching event; poll the hits queue and the DLQ -> expect several hits then one dead letter whose attributes name the AGE condition; observe no hits and no dead letter.

Replay both check ids in trace_ids to confirm.
