Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__sqs.events-pattern-language-array-field-matches-if-any-element-matches
Issue-Key: events__sqs.events-pattern-language-array-field-matches-if-any-element-matches|_drain_all#1|Timeout|with ThreadPoolExecutor(max_workers=max(1, len(queues))) as pool:
Issue-Commit: b4a108dc5aaf17dfb3354052018e461db9c4a3a2

# events__sqs: EventBridge array-element pattern matching is unobservable — a rule whose pattern matches an array field delivers nothing to its SQS target and the check hangs to the 60s cap

> _Note: this issue was identified against the code base at commit `b4a108dc5aaf17dfb3354052018e461db9c4a3a2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutEvents

## Description
One root cause. NOTE: the open issue 20261029T182228Z-events-sqs-eventbridge-puttargets-accepts-conflicting-invalid-input-forms-the.md describes a related inert-SQS-delivery cause and even names this check in its trace list, but the replay did NOT bind this check to it (this run's failure key is the 60s check cap, not that issue's `_drain_all#1|Timeout` key), so the check is unowned in this sweep and is filed here. It is an open issue, not a fixed one, so nothing is being reopened.

Cause 1 of 1: a rule whose event pattern selects on an array-valued event field never delivers the matching events to its SQS target, and neither does the source-only control rule, so EventBridge's array-element matching semantics cannot be exercised at all.
AWS: EventBridge matches an array-valued event field if ANY element of the array matches the pattern — for a plain value list, for a {"numeric": [">", N]} content filter and for a {"prefix": "..."} content filter. A nested array is flattened (so a pattern value ["b"] matches the event field [["b"]]), a scalar field matches a single-element value list, and an empty array or an array with no matching element matches nothing. The same verdict must be returned by TestEventPattern and observed in live delivery — Smithy: TestEventPattern input EventPattern (required, @length min 0 max 4096) + Event (required), output Result:Boolean, errors InvalidEventPatternException (HTTP 400) / InternalException (HTTP 500); the matching semantics themselves are AWS-documented event-pattern behavior and are not expressed in the model. PutRule/PutTargets/PutEvents all succeed in the recording (PutTargets returns FailedEntryCount=0, PutEvents returns no FailedEntries), so the divergence is purely in matching/delivery.
Emulator: the check ends "Check timed out after 60.0s". Across up to 16 PutEvents rounds on a freshly created custom bus, with three rules (patterns {"detail":{"xs":["b"]}}, {"detail":{"xs":[{"numeric":[">",10]}]}}, {"detail":{"xs":[{"prefix":"ab"}]}}, each scoped to the run's own source and detail-type) and one source-only control rule, each with its own SQS queue carrying an events.amazonaws.com sqs:SendMessage Allow scoped by aws:SourceArn to the rule ARN, no queue ever reached the per-rule warm barrier, so the harness never got to the confirmed round and was cut off at the cap. Nothing is delivered — not even to the control rule whose pattern constrains only source and detail-type.
Why: a client that routes on an array field (the common shape for tag lists, resource lists, status lists and any repeated detail member) gets no events at all, with no error anywhere: PutEvents reports success, the rule and target exist, and the queue stays empty. Both surfaces of the matcher are unusable — a caller cannot even fall back to TestEventPattern to predict routing, because the check could never reach the point of comparing the two.
Fix: the EventBridge rule-matching engine's array handling (any-element semantics, nested-array flattening, scalar-vs-single-element-list, empty array matching nothing, per-element numeric and prefix content filters) and the SQS target invoker that delivers a matched event to a queue on a custom bus; TestEventPattern must return the same verdict as live delivery.
Traces: events__sqs.events-pattern-language-array-field-matches-if-any-element-matches

## Evidence
Failing check: events__sqs.events-pattern-language-array-field-matches-if-any-element-matches — verdict [mismatch], "Error: Check timed out after 60.0s".

Expected (real AWS verdicts, probed with TestEventPattern and confirmed by live delivery when the check was authored), for events {one-of-three: {"xs":["a","b","c"]}, none-of-two: {"xs":["a","c"]}, scalar: {"xs":"b"}, nested-array: {"xs":[["b"]]}, empty: {"xs":[]}, one-number-over: {"xs":[1,20]}, no-number-over: {"xs":[1,2]}, one-prefixed: {"xs":["zz","abz"]}}:
  rule "value"   ({"detail":{"xs":["b"]}})                       -> delivers {one-of-three, scalar, nested-array}
  rule "numeric" ({"detail":{"xs":[{"numeric":[">",10]}]}})      -> delivers {one-number-over}
  rule "prefix"  ({"detail":{"xs":[{"prefix":"ab"}]}})           -> delivers {one-prefixed}
  control rule (source + detail-type only)                        -> delivers every case
and TestEventPattern returns exactly those verdicts for the same patterns and events.

Actual: no queue — including the source-only control queue — ever received the cases of a single PutEvents round, so the check's differential barrier never warmed and the run was cut at the 60-second cap. The assertions that would have discriminated the matcher ("an array with ONE matching element was not delivered: ...", "an empty array matched: ...", "TestEventPattern and live delivery disagree ...") were never reached because no delivery happened at all.

All control-plane calls on the path succeeded: CreateEventBus, PutRule, SetQueueAttributes, PutTargets (asserted FailedEntryCount == 0) and PutEvents (no entry carried ErrorCode).

## Steps to Reproduce
1. CreateEventBus with a fresh custom bus name.
2. CreateQueue four times; for each queue SetQueueAttributes with a Policy allowing Principal {"Service":"events.amazonaws.com"} Action sqs:SendMessage on the queue ARN, Condition ArnEquals aws:SourceArn = the rule ARN created in step 3.
3. PutRule on that bus four times, scoped to one source and detail-type: (a) control, pattern {"source":[S],"detail-type":[DT]}; (b) {"source":[S],"detail-type":[DT],"detail":{"xs":["b"]}}; (c) ..."detail":{"xs":[{"numeric":[">",10]}]}; (d) ..."detail":{"xs":[{"prefix":"ab"}]}. PutTargets each rule with its queue (FailedEntryCount comes back 0).
4. TestEventPattern each pattern against each of the eight events above — compare with the AWS verdicts listed in Evidence.
5. PutEvents one batch carrying all eight cases on that bus (every entry is accepted).
6. ReceiveMessage on each queue. Observed: every queue stays empty, including the control queue, so the run burns its rounds and ends "Check timed out after 60.0s". Expected: the control queue receives all eight cases and each pattern queue receives exactly its match set, agreeing with TestEventPattern.
