Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events.put-events-target-without-resource-policy-blocks-delivery events.queue-policy-deny-and-missing-allow-both-dead-letter-as-no-resource-without-retries events.sqs-target-encrypted-with-a-key-events-cannot-use-dead-letters-until-the-key-policy-grants-it events.disabled-rule-with-a-dead-letter-configured-target-drops-the-event-instead-of-dead-lettering-it events.input-transformer-plain-text-template-does-not-escape-strings-and-dead-letters-invalid-json
Issue-Shape-Id: com.amazonaws.eventbridge#AWSEvents
Issue-Key: events.put-events-target-without-resource-policy-blocks-delivery|run#20|AssertionError|assert not resp.get("Messages"), (
Issue-Key: events.queue-policy-deny-and-missing-allow-both-dead-letter-as-no-resource-without-retries|_receive#25|Timeout|for m in sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10, WaitTimeSeconds=5,
Issue-Key: events.sqs-target-encrypted-with-a-key-events-cannot-use-dead-letters-until-the-key-policy-grants-it|_receive#22|Timeout|for m in sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10, WaitTimeSeconds=5,
Issue-Key: events.disabled-rule-with-a-dead-letter-configured-target-drops-the-event-instead-of-dead-lettering-it|_receive#21|Timeout|for m in sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10, WaitTimeSeconds=5,
Issue-Key: events.input-transformer-plain-text-template-does-not-escape-strings-and-dead-letters-invalid-json|_drain#28|Timeout|resp = sqs.receive_message(
Issue-Commit: b1cd28f1ca277d6a032df605f83af5017e934043

# EventBridge does not authorize SQS targets against the queue resource policy or KMS key policy, never dead-letters, and plain-text InputTemplate handling is unobservable (3 causes)

> _Note: this issue was identified against the code base at commit `b1cd28f1ca277d6a032df605f83af5017e934043`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutTargets

## Description
Three root causes in the EventBridge service itself. Several adjacent causes are already filed (20261025T002357Z-events-eventbridge-performs-no-authorization-event-bus-resource-policies.md, 20261027T052336Z-events-4-net-new-behavioral-root-causes-api.md, 20261029T014025Z-events-3-net-new-behavioral-root-causes-an.md); none of the checks below were bound to them.

Cause 1 of 3: an SQS target with no resource-policy grant (or with an explicit Deny) is delivered to instead of being refused and dead-lettered.
AWS: EventBridge calls sqs:SendMessage on the target queue as the service principal events.amazonaws.com. If the queue policy has no matching Allow, or has an explicit Deny that matches, the call is refused; the envelope is not enqueued, and with a DeadLetterConfig on the target the envelope lands in the DLQ with message attributes RULE_ARN, TARGET_ARN, ERROR_CODE=NO_RESOURCE-class and ERROR_MESSAGE, with no retries (an authorization refusal is not retryable).
Emulator: the target queue receives the envelope. "AssertionError: event was delivered to an SQS target lacking a resource policy granting EventBridge access". In the deny/missing-allow and KMS-encrypted variants nothing is dead-lettered either and the check ends "Check timed out after 60.0s".
Why: the emulator fabricates a delivery AWS refuses. A client validating least privilege, or relying on a Deny as a kill switch, gets the opposite result from AWS; and the DLQ that would have told it so stays empty.
Fix: resource-policy (and, for an encrypted queue, KMS key-policy) authorization in the EventBridge SQS-target invoker, and its dead-letter path with the four documented message attributes.
Traces: events.put-events-target-without-resource-policy-blocks-delivery, events.queue-policy-deny-and-missing-allow-both-dead-letter-as-no-resource-without-retries, events.sqs-target-encrypted-with-a-key-events-cannot-use-dead-letters-until-the-key-policy-grants-it

Cause 2 of 3: a DISABLED rule's dead-letter behaviour cannot be observed because its target pipeline is inert.
AWS: a rule in State=DISABLED does not match events at all, so no invocation is attempted and nothing is dead-lettered even when the target carries a DeadLetterConfig; re-enabling the rule makes the next matching event deliver normally. The check establishes the positive (enabled) control first.
Emulator: the check ends "Check timed out after 60.0s" — the enabled control never delivers, so the disabled leg proves nothing.
Why: the rule State switch cannot be validated; a client cannot tell "disabled, correctly silent" from "enabled but broken".
Fix: the EventBridge rule dispatcher must deliver for ENABLED rules (see cause 1) and must not enter the dead-letter path for DISABLED rules.
Traces: events.disabled-rule-with-a-dead-letter-configured-target-drops-the-event-instead-of-dead-lettering-it

Cause 3 of 3: the plain-text InputTemplate path — no string escaping, and an invalid-JSON rendering dead-lettered — cannot be observed.
AWS: an InputTransformer InputTemplate that is not valid JSON is treated as plain text: substituted values are inserted verbatim with no JSON escaping, and if the rendered result is not valid JSON for a target that requires JSON, the invocation fails and the envelope is dead-lettered.
Emulator: the check ends "Check timed out after 60.0s" — neither the rendered delivery nor the dead-letter arrives.
Why: an InputTransformer misconfiguration silently produces nothing instead of a dead-letter a client can alarm on.
Fix: the EventBridge InputTransformer renderer (plain-text vs JSON template discrimination and escaping rules) and the dead-letter path for a rendering a target rejects.
Traces: events.input-transformer-plain-text-template-does-not-escape-strings-and-dead-letters-invalid-json

## Evidence
Cause 1: "AssertionError: event was delivered to an SQS target lacking a resource policy granting EventBridge access" — expected zero messages on the target queue plus one DLQ message carrying RULE_ARN/TARGET_ARN/ERROR_CODE/ERROR_MESSAGE and no retries; the deny/missing-allow and KMS variants end "Error: Check timed out after 60.0s" with an empty DLQ.
Cause 2 and 3: "Error: Check timed out after 60.0s" with neither the control delivery nor the dead-letter observed.

## Steps to Reproduce
(1) Create an SQS queue with no queue policy (then a variant with an explicit Deny for events.amazonaws.com, then a variant encrypted with a CMK whose key policy does not grant events), PutRule + PutTargets with a DeadLetterConfig, PutEvents a matching entry, read the target queue and the DLQ. The target queue receives the envelope / the DLQ is empty. (2) Create a rule with a DLQ-configured target, confirm delivery while ENABLED, DisableRule, PutEvents, read the DLQ. (3) PutTargets with an InputTransformer whose InputTemplate is a bare sentence, PutEvents, read the target and the DLQ.
