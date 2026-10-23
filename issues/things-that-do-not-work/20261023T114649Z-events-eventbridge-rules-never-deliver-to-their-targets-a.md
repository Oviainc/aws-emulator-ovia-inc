Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__lambda__sqs.eventbridge-rule-sends-to-sqs-target events__lambda__sqs.eventbridge-to-sqs-message-body-is-full-event events__lambda__sqs.eventbridge-event-envelope-format-to-lambda events__lambda__sqs.eventbridge-sqs-target-arn-must-match-queue-arn events__lambda__sqs.eventbridge-pattern-filtering-excludes-non-matching events__lambda__sqs.eventbridge-rule-event-pattern-content-filtering-sqs events__lambda__sqs.eventbridge-lambda-sourcearn-condition-arn-resolution
Issue-Key: events__lambda__sqs.eventbridge-rule-sends-to-sqs-target|_reemit_until_delivered#14|AssertionError|raise AssertionError(
Issue-Key: events__lambda__sqs.eventbridge-to-sqs-message-body-is-full-event|_reemit_until_delivered#11|AssertionError|raise AssertionError(
Issue-Key: events__lambda__sqs.eventbridge-event-envelope-format-to-lambda|_reemit_until_delivered#13|AssertionError|raise AssertionError(
Issue-Key: events__lambda__sqs.eventbridge-sqs-target-arn-must-match-queue-arn|_reemit_until_delivered#17|AssertionError|raise AssertionError(
Issue-Key: events__lambda__sqs.eventbridge-pattern-filtering-excludes-non-matching|_reemit_until_delivered#12|AssertionError|raise AssertionError(
Issue-Key: events__lambda__sqs.eventbridge-rule-event-pattern-content-filtering-sqs|run#8|AssertionError|assert len(messages) == 1, f"expected exactly 1 matching delivery, got {len(messages)}"
Issue-Key: events__lambda__sqs.eventbridge-lambda-sourcearn-condition-arn-resolution|_reemit_until_delivered#17|AssertionError|raise AssertionError(
Issue-Commit: 048778663a26e363156c6db724999c4fb229af36

# EventBridge rules never deliver to their targets: a matching PutEvents reaches no SQS queue or Lambda target

> _Note: this issue was identified against the code base at commit `048778663a26e363156c6db724999c4fb229af36`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutEvents

## Description
Cause 1 of 1: PutEvents accepts an event and reports success, but the rule-matching/target-dispatch path never delivers it, so no target ever receives anything.

AWS: PutEvents puts the event on the bus and returns FailedEntryCount 0 with an EventId per entry. EventBridge then evaluates every ENABLED rule on that bus against the event; for each matching rule it delivers to every target - an SQS target receives a message whose Body is the full event envelope (version, id, detail-type, source, account, time, region, resources, detail), and a Lambda target is invoked with that envelope as its payload. Delivery is asynchronous but observable within seconds. Smithy (eventbridge): PutEvents/PutRule/PutTargets model the request and response; the delivery semantics and the SQS body being the full event envelope are recorded behavior from the checks.

Emulator: PutEvents returns success and the rule and targets read back correctly, but nothing is ever delivered. The checks poll the SQS target and report "event never delivered to https://<host>/<account>/<queue> after 8 attempts of 6.0s each" (48s of polling) and "expected exactly 1 matching delivery, got 0"; the Lambda-target variants hit the same absence of delivery.

Why: EventBridge is inert. Every event-driven flow built on it silently does nothing while the control plane reports a healthy rule with targets, which is the worst possible failure mode: a positive test hangs and times out, and a negative test (pattern mismatch, disabled rule, missing queue policy, wrong principal) passes for the wrong reason because non-delivery is indistinguishable from correct suppression.

Fix: the PutEvents handler / bus dispatch path - after accepting an entry, match it against the bus's rules and enqueue a delivery per target, honouring the rule State, the event pattern, the input transformer, and the target's resource-policy / queue-policy check and DLQ configuration.

Traces: see trace_ids. The positive delivery checks (eventbridge-rule-sends-to-sqs-target, eventbridge-to-sqs-message-body-is-full-event, eventbridge-event-envelope-format-to-lambda) are the ones that pin the defect; the pattern-filtering and policy checks in the same group fail on their positive control for the same reason.

## Evidence
Expected: after PutEvents with a detail-type matching an ENABLED rule whose target is an SQS queue with an EventBridge-allowing queue policy, ReceiveMessage returns one message whose Body is the full event envelope.
Actual: ReceiveMessage returns nothing. Replay: "event never delivered to https://127.0.0.1:44157/000000000000/sim2real-xsvc-q-3655c6a20731 after 8 attempts of 6.0s each"; "expected exactly 1 matching delivery, got 0". PutEvents itself answered success and PutRule/PutTargets read back the rule and target.

## Steps to Reproduce
1. CreateQueue and set a queue policy allowing events.amazonaws.com to sqs:SendMessage.
2. PutRule with EventPattern {"source":["sim2real"]} and State ENABLED; PutTargets with the queue ARN.
3. PutEvents with Source 'sim2real' and a Detail body.
4. Poll ReceiveMessage on the queue -> no message arrives (polled 8 x 6s).
