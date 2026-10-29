Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__sqs.events-delivery-and-failure-conflicting-input-and-input-path-rejected events__sqs.events-delivery-and-failure-unquoted-plain-text-template-rejected events__sqs.put-targets-upsert-with-invalid-input-rejected-and-live-target-keeps-delivering events__sqs.events-delivery-and-failure-deleted-queue-target-dead-letters-no-resource-while-sibling-delivers events__sqs.events-delivery-and-failure-input-transformer-reserved-variables-resolve-to-rule-and-event events__sqs.events-delivery-and-failure-input-transformer-substitution-forms-render-exactly events__sqs.bus-to-bus-forwarding-happens-once-and-preserves-the-envelope events__sqs.custom-bus-fanout-is-at-least-once-and-order-independent events__sqs.eventbridge-input-transformer-missing-path-not-delivered events__sqs.events-delivery-and-failure-object-in-quoted-template-string-dead-letters-as-invalid-json events__sqs.events-pattern-language-array-field-matches-if-any-element-matches events__sqs.events-pattern-language-nested-object-pattern-matches-the-leaf-path events__sqs.replay-redelivers-each-archived-event-exactly-once-and-never-the-out-of-window-one
Issue-Key: events__sqs.events-delivery-and-failure-conflicting-input-and-input-path-rejected|run#14|AssertionError|assert code == "ValidationException", (
Issue-Key: events__sqs.events-delivery-and-failure-unquoted-plain-text-template-rejected|run#11|AssertionError|assert code == "ValidationException", (
Issue-Key: events__sqs.put-targets-upsert-with-invalid-input-rejected-and-live-target-keeps-delivering|_error#9|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: events__sqs.events-delivery-and-failure-deleted-queue-target-dead-letters-no-resource-while-sibling-delivers|run#25|AssertionError|assert attrs.get("ERROR_CODE") == "NO_RESOURCE", f"expected ERROR_CODE=NO_RESOURCE, got {attrs!r}"
Issue-Key: events__sqs.events-delivery-and-failure-input-transformer-reserved-variables-resolve-to-rule-and-event|run#20|AssertionError|assert "detail" not in tx["ev"], (
Issue-Key: events__sqs.events-delivery-and-failure-input-transformer-substitution-forms-render-exactly|run#17|AssertionError|assert body == _EXPECTED_JSON, (
Issue-Key: events__sqs.bus-to-bus-forwarding-happens-once-and-preserves-the-envelope|_drain#46|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=wait)
Issue-Key: events__sqs.custom-bus-fanout-is-at-least-once-and-order-independent|run#16|Timeout|observed = {rule: [i for f in fs for i in f.result()] for rule, fs in futures.items()}
Issue-Key: events__sqs.eventbridge-input-transformer-missing-path-not-delivered|_collect#14|Timeout|resp = sqs.receive_message(
Issue-Key: events__sqs.events-delivery-and-failure-object-in-quoted-template-string-dead-letters-as-invalid-json|_drain#14|Timeout|resp = sqs.receive_message(
Issue-Key: events__sqs.events-pattern-language-array-field-matches-if-any-element-matches|_drain_all#1|Timeout|return {k: f.result() for k, f in futs.items()}
Issue-Key: events__sqs.events-pattern-language-nested-object-pattern-matches-the-leaf-path|_drain_all#1|Timeout|return {k: f.result() for k, f in futs.items()}
Issue-Key: events__sqs.replay-redelivers-each-archived-event-exactly-once-and-never-the-out-of-window-one|_drain#44|Timeout|resp = sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Commit: b1cd28f1ca277d6a032df605f83af5017e934043

# EventBridge: PutTargets accepts conflicting/invalid Input forms, the DLQ ERROR_CODE leaks the downstream SDK code, InputTransformer reserved variables and missing paths render wrong, and SQS delivery is inert (7 causes)

> _Note: this issue was identified against the code base at commit `b1cd28f1ca277d6a032df605f83af5017e934043`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutTargets

## Description
Seven root causes. 20261029T014025Z-events-sqs-an-explicit-deny-in-an-event.md and 20261025T002357Z-events-eventbridge-refusal-codes-and-messages-diverge-from-aws.md own other checks in this service; none of the checks below were bound to them.

Cause 1 of 7: PutTargets accepts a target that sets both Input and InputPath.
AWS: Input, InputPath and InputTransformer are mutually exclusive on a target. Supplying Input together with InputPath is refused with HTTP 400 ValidationException naming the conflict; the Smithy model for PutTargets lists no such error, so this is a service-common ValidationException.
Emulator: the call is accepted and raises nothing. "AssertionError: Input together with InputPath must be refused with ValidationException, got None".
Why: a target is created with a configuration AWS rejects; the caller learns nothing until the delivery payload is wrong in AWS.
Fix: cross-member validation in the EventBridge PutTargets handler.
Traces: events__sqs.events-delivery-and-failure-conflicting-input-and-input-path-rejected

Cause 2 of 7: PutTargets accepts an InputTransformer InputTemplate that is a bare, unquoted sentence.
AWS: an InputTemplate must be valid JSON or a quoted string; a bare sentence is refused with HTTP 400 ValidationException.
Emulator: accepted, no error. "AssertionError: a bare-sentence InputTemplate must be refused with ValidationException, got None".
Why: the misconfiguration surfaces only at delivery time in AWS, as a failed invocation rather than a create-time error.
Fix: InputTemplate syntax validation in the PutTargets handler.
Traces: events__sqs.events-delivery-and-failure-unquoted-plain-text-template-rejected

Cause 3 of 7: a PutTargets upsert carrying an invalid Input is accepted, overwriting a working target.
AWS: the upsert is refused (HTTP 400 ValidationException / a populated FailedEntries entry) and the previously configured, working target is left intact and keeps delivering.
Emulator: the call returns HTTP 200 with FailedEntryCount=0 and FailedEntries=[]. "AssertionError: expected the call to be refused, but it succeeded: {'FailedEntryCount': 0, 'FailedEntries': [], ...}".
Why: a bad deploy silently replaces a live target with a broken one instead of being rejected, so delivery stops with no error anywhere.
Fix: Input validation in the PutTargets handler, with the existing target preserved on rejection.
Traces: events__sqs.put-targets-upsert-with-invalid-input-rejected-and-live-target-keeps-delivering

Cause 4 of 7: the dead-letter message's ERROR_CODE is the downstream SDK error code and ERROR_MESSAGE is a Rust debug dump.
AWS: when a target's resource no longer exists, the dead-lettered envelope carries the message attribute ERROR_CODE = "NO_RESOURCE" and an ERROR_MESSAGE describing the failure, alongside RULE_ARN and TARGET_ARN. ERROR_CODE is drawn from EventBridge's own fixed vocabulary, not from the target service.
Emulator: ERROR_CODE is "QueueDoesNotExist" and ERROR_MESSAGE is the raw SDK error Debug rendering — "service error: unhandled error (QueueDoesNotExist): Error { code: \"QueueDoesNotExist\", ... ServiceError(ServiceError { source: Unhandled(Unhandled { source: ErrorMetadata { ... } }), raw: Response { status: StatusCode(400), ... } }))". The requirement is the fixed token NO_RESOURCE and a human-readable message; RULE_ARN and TARGET_ARN are values from the rule and target.
Why: a consumer that switches on ERROR_CODE (the documented way to classify a dead-letter) never matches NO_RESOURCE, and the internal Rust type names and HTTP frames in ERROR_MESSAGE leak emulator internals into a customer-visible payload.
Fix: the EventBridge dead-letter path must map target-invocation failures onto its own ERROR_CODE vocabulary and render ERROR_MESSAGE as prose instead of forwarding the SDK error's Debug output.
Traces: events__sqs.events-delivery-and-failure-deleted-queue-target-dead-letters-no-resource-while-sibling-delivers

Cause 5 of 7: the InputTransformer reserved variable <aws.events.event> includes the `detail` field.
AWS: <aws.events.event> resolves to the event envelope WITHOUT the detail field (detail is reached through <aws.events.event.json> or an explicit InputPath); <aws.events.rule-arn> and the other reserved variables resolve to the rule and event metadata.
Emulator: the rendering contains detail — got {'version': '0', 'id': ..., 'detail-type': 'Reserved', 'source': ..., 'account': ..., 'time': ..., 'region': ..., 'resources': [], 'detail': {'nonce': ...}} where the detail key must be absent. The member names are the requirement; the id, nonce and time are per-run values.
Why: a target receives a payload with an extra field, so strict consumers (schema validation, typed deserialisation) reject it, and the documented way to pass the envelope without the detail is unavailable.
Fix: the EventBridge InputTransformer's reserved-variable resolution.
Traces: events__sqs.events-delivery-and-failure-input-transformer-reserved-variables-resolve-to-rule-and-event

Cause 6 of 7: an InputTransformer path that matches nothing renders JSON null instead of an empty string.
AWS: a declared InputPath variable whose JSON path is absent from the event renders as the empty string — bare, it yields "" and inside a quoted template it contributes nothing, so a template of "x<miss>y" renders "xy".
Emulator: the absent path renders null: the JSON template produced {'missq': 'xnully', 'miss': None} where AWS produces {'missq': 'xy', 'miss': ''}; the array-path variant produced {'beyond': None} where AWS produces {'beyond': ''}. The member names and the substitution rule are the requirement; the surrounding values come from the event.
Why: the literal text "null" is injected into rendered strings and null into rendered fields, so a target's typed consumer gets a wrong value or fails to deserialise — and the corruption is silent.
Fix: the EventBridge InputTransformer renderer's handling of an unresolved path (empty string, not null).
Traces: events__sqs.events-delivery-and-failure-input-transformer-substitution-forms-render-exactly

Cause 7 of 7: SQS targets receive nothing in a range of rule configurations, so bus-to-bus forwarding, custom-bus fan-out, pattern-language matching, replay and the invalid-JSON dead-letter are all unobservable.
AWS: a bus-to-bus forward delivers the envelope to the destination bus exactly once and preserves every envelope field; a custom-bus fan-out delivers to every target at least once in no guaranteed order; an array field in the event matches if ANY element matches the pattern's value list, and a nested object pattern matches along the leaf path; an InputTransformer whose required path is missing dead-letters (or is not delivered) rather than delivering a malformed payload; a template that interpolates an object into a quoted string renders invalid JSON and is dead-lettered; StartReplay redelivers each archived event in the window exactly once and never an event outside it.
Emulator: every one of these checks ends "Check timed out after 60.0s" with the target queue empty.
Why: none of EventBridge's core routing semantics — forwarding, fan-out, the pattern language, replay — can be relied on or even inspected.
Fix: the EventBridge rule-matching engine (array and nested-object pattern evaluation), the event-bus and SQS target invokers, the dead-letter path, and the archive/replay machinery.
Traces: events__sqs.bus-to-bus-forwarding-happens-once-and-preserves-the-envelope, events__sqs.custom-bus-fanout-is-at-least-once-and-order-independent, events__sqs.eventbridge-input-transformer-missing-path-not-delivered, events__sqs.events-delivery-and-failure-object-in-quoted-template-string-dead-letters-as-invalid-json, events__sqs.events-pattern-language-array-field-matches-if-any-element-matches, events__sqs.events-pattern-language-nested-object-pattern-matches-the-leaf-path, events__sqs.replay-redelivers-each-archived-event-exactly-once-and-never-the-out-of-window-one

## Evidence
Cause 1: "AssertionError: Input together with InputPath must be refused with ValidationException, got None".
Cause 2: "AssertionError: a bare-sentence InputTemplate must be refused with ValidationException, got None".
Cause 3: "AssertionError: expected the call to be refused, but it succeeded: {'FailedEntryCount': 0, 'FailedEntries': [], 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}".
Cause 4: "AssertionError: expected ERROR_CODE=NO_RESOURCE, got {'ERROR_MESSAGE': 'service error: unhandled error (QueueDoesNotExist): Error { code: \"QueueDoesNotExist\", message: \"The specified queue does not exist.\" } (ServiceError(... raw: Response { status: StatusCode(400), ... }))', 'RULE_ARN': ..., 'TARGET_ARN': ..., 'ERROR_CODE': 'QueueDoesNotExist'}".
Cause 5: "AssertionError: <aws.events.event> is documented WITHOUT the detail field, got {'version': '0', 'id': ..., 'detail-type': 'Reserved', ..., 'detail': {'nonce': ...}}".
Cause 6: "AssertionError: JSON template rendered {... 'missq': 'xnully', 'miss': None}; expected {... 'missq': 'xy', 'miss': ''}".
Cause 7: seven checks end "Error: Check timed out after 60.0s" with the SQS target empty.

## Steps to Reproduce
(1) PutTargets with both Input and InputPath on one target — accepted. (2) PutTargets with an InputTransformer whose InputTemplate is a bare sentence — accepted. (3) PutTargets a working target, then upsert the same Id with an invalid Input — accepted with FailedEntryCount=0. (4) PutTargets an SQS target with a DeadLetterConfig, delete the target queue, PutEvents, read the DLQ message attributes — ERROR_CODE is QueueDoesNotExist. (5) PutTargets with an InputTransformer using <aws.events.event>, PutEvents, read the rendering — detail is present. (6) PutTargets with an InputTransformer declaring a path absent from the event, PutEvents — the rendering contains null / the literal "null". (7) Configure bus-to-bus forwarding, a custom-bus fan-out, an array/nested-object pattern, and an archive + StartReplay — the SQS targets stay empty.
