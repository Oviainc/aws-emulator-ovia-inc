Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events.test-event-pattern-cidr-matcher-boundaries events.test-event-pattern-envelope-required-fields-and-parseable-time events.test-event-pattern-match-and-mismatch events.test-event-pattern-exists-treats-every-object-leaf-and-the-empty-array-as-absent events.test-event-pattern-malformed-operator-rejection events.test-event-pattern-numeric-rejects-non-numeric-json-values-and-degenerate-ranges events.events-create-archive-with-an-invalid-event-pattern-rejected
Issue-Shape-Id: com.amazonaws.eventbridge#AWSEvents
Issue-Key: events.test-event-pattern-cidr-matcher-boundaries|_client_error#4|AssertionError|raise AssertionError(
Issue-Key: events.test-event-pattern-envelope-required-fields-and-parseable-time|run#6|AssertionError|assert code == "ValidationException", f"an envelope without {field!r} gave {code!r}"
Issue-Key: events.test-event-pattern-match-and-mismatch|run#11|AssertionError|assert code == "ValidationException", (
Issue-Key: events.test-event-pattern-exists-treats-every-object-leaf-and-the-empty-array-as-absent|_expect#12|AssertionError|assert got is want, (
Issue-Key: events.test-event-pattern-malformed-operator-rejection|_expect_pattern_invalid#2|AssertionError|assert message == expected, (
Issue-Key: events.test-event-pattern-numeric-rejects-non-numeric-json-values-and-degenerate-ranges|run#9|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: events.events-create-archive-with-an-invalid-event-pattern-rejected|run#13|AssertionError|assert err == expected, (
Issue-Commit: 8b92040b06acdac381093d8b397331a442bd94ad

# EventBridge pattern engine and pattern-validation refusals diverge: a full-width cidr mask is accepted, an invalid Event envelope gets the wrong error class, exists:true misses a null leaf, and InvalidEventPatternException Reasons are rewritten (4 causes)

> _Note: this issue was identified against the code base at commit `8b92040b06acdac381093d8b397331a442bd94ad`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TestEventPattern

## Description
TestEventPattern now answers (it is no longer a stub), but its matcher and its pattern-validation refusals diverge from AWS in four independent ways. NOTE: the open issue 20261025T031335Z-events-eventbridge-testeventpattern-is-an-unimplemented-scaffold-stub-so.md describes TestEventPattern as a stub; that is not what these replays see — the operation responds, so that issue does not cover any of the causes below. The refusal-message issue 20261025T002357Z-events-eventbridge-refusal-codes-and-messages-diverge-from-aws.md does not name the pattern-validator Reason strings either.

Cause 1 of 4: a `cidr` prefix whose mask is the full address width is accepted instead of refused.
AWS: Smithy models TestEventPattern with required {EventPattern @length 0..4096, Event} and exactly two errors, InternalException (500) and InvalidEventPatternException (400). Real EventBridge refuses a `cidr` operand with a single-host mask — IPv4 /32 and IPv6 /128 — with InvalidEventPatternException; the recorded Reasons are `IPv4 mask bits must be < 32` and `IPv6 mask bits must be < 128` (measured, cited by the check as examples). The requirement is the 400 + InvalidEventPatternException refusal; the mask-width rule is that a `cidr` prefix must be strictly narrower than the address width.
Emulator: HTTP 200 with {"Result": false} for {"detail": {"sourceIPAddress": [{"cidr": "203.0.113.0/32"}]}} — the pattern is accepted and quietly evaluated to a non-match.
Why: a caller that validates a pattern with TestEventPattern before putting it on a rule gets a green light for a pattern AWS will reject at PutRule, and the same over-permissive matcher will silently never match in the emulator while erroring in production.
Fix: the `cidr` matcher's operand validation in the eventbridge pattern engine (pattern compile/validate path), which must reject rather than evaluate.
Traces: events.test-event-pattern-cidr-matcher-boundaries.

Cause 2 of 4: an invalid/incomplete Event envelope is answered InvalidEventPatternException instead of ValidationException.
AWS: the `Event` member is the event envelope, not the pattern; recorded behavior only (the Smithy model lists only InvalidEventPatternException and InternalException for TestEventPattern, so the envelope-validation class is not modelled). Both audited checks record that AWS answers error code `ValidationException` when the supplied Event omits a required envelope field (id, detail-type, source, account, time, region) or carries an unparseable `time`. The requirement is the error CODE: ValidationException for a bad Event, InvalidEventPatternException only for a bad EventPattern.
Emulator: error code `InvalidEventPatternException` for an envelope missing `id` (and for an incomplete envelope generally).
Why: the two codes mean different things to a client — one says "fix your pattern", the other "fix your event". A client retry/branch on InvalidEventPatternException will blame the wrong input, and a pattern that is in fact valid is reported invalid.
Fix: TestEventPattern's input validation in the eventbridge crate must validate the Event envelope separately from the EventPattern and classify envelope failures as ValidationException.
Traces: events.test-event-pattern-envelope-required-fields-and-parseable-time, events.test-event-pattern-match-and-mismatch.

Cause 3 of 4: `exists: true` does not match a JSON null leaf.
AWS: recorded behavior only. The audited check records that `exists: true` matches a leaf whose value is JSON `null` (null is PRESENT), while a leaf whose value is a JSON object is absent unconditionally and an EMPTY array is absent. So pattern {"detail": {"items": [{"exists": true}]}} against an event whose detail is {"items": null} must answer Result=true.
Emulator: HTTP 200 with Result=false for exactly that pair.
Why: a rule using `exists` to detect a present-but-null field never fires in the emulator but fires in AWS, so a consumer's routing is inverted for null-valued fields.
Fix: the `exists` operator's absence rule in the eventbridge pattern engine — null must be treated as present, distinct from {} and [].
Traces: events.test-event-pattern-exists-treats-every-object-leaf-and-the-empty-array-as-absent.

Cause 4 of 4: InvalidEventPatternException Reason text is rewritten and the source-location suffix is dropped.
AWS: the refusal code is InvalidEventPatternException (400) and the message is built from the request: `Event pattern is not valid. Reason: <reason>` followed by `\n at [Source: (String)"<the pattern as submitted>"; line: <n>, column: <c>]`. Because the suffix is derived from the submitted pattern, the rule — reason sentence plus a source location naming the submitted pattern text and the offending line/column — is the requirement; the exact recorded strings below are examples. The reason sentences themselves are AWS's own vocabulary, e.g. `Bottom must be less than top` for a degenerate numeric range and `Unrecognized match type <op>` for an unknown operator.
Emulator: the code is right but the message is truncated and, for the numeric case, reworded into the emulator's own vocabulary: `Event pattern is not valid. Reason: detail.amount: numeric names a range whose bottom is not below its top` instead of `Event pattern is not valid. Reason: Bottom must be less than top\n at [Source: (String)"..."; line: 1, column: 55]`; and `Event pattern is not valid. Reason: "source" must be an object or an array` with no ` at [Source: ...]` suffix. The same truncation appears on CreateArchive's pattern validation.
Why: tooling that surfaces or parses the validator's source location to point a user at the offending character of their pattern gets nothing, and a reason sentence that does not match AWS's breaks any consumer keying on it.
Fix: the shared event-pattern validator's error construction in the eventbridge crate (used by TestEventPattern, PutRule, CreateArchive and UpdateArchive) must emit AWS's reason sentence and append the source-location suffix derived from the submitted pattern.
Traces: events.test-event-pattern-malformed-operator-rejection, events.test-event-pattern-numeric-rejects-non-numeric-json-values-and-degenerate-ranges, events.events-create-archive-with-an-invalid-event-pattern-rejected.

## Evidence
Cause 1 — events.test-event-pattern-cidr-matcher-boundaries [mismatch]:
AssertionError: TestEventPattern accepted {"detail": {"sourceIPAddress": [{"cidr": "203.0.113.0/32"}]}} and returned {'Result': False, 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}; AWS refuses it
Expected: HTTP 400 InvalidEventPatternException (recorded example Reason: `IPv4 mask bits must be < 32`). Actual: HTTP 200 {"Result": false}.

Cause 2 — events.test-event-pattern-envelope-required-fields-and-parseable-time [mismatch]:
AssertionError: an envelope without 'id' gave 'InvalidEventPatternException'
events.test-event-pattern-match-and-mismatch [mismatch]:
AssertionError: expected ValidationException for an incomplete event envelope, got InvalidEventPatternException
Expected error code: ValidationException. Actual: InvalidEventPatternException.

Cause 3 — events.test-event-pattern-exists-treats-every-object-leaf-and-the-empty-array-as-absent [mismatch]:
AssertionError: exists:true must match a null leaf -- null is present, unlike {} or []: expected Result=True, got False
pattern = {"detail": {"items": [{"exists": true}]}}
event   = {... "detail": {"items": null}}

Cause 4 — events.test-event-pattern-malformed-operator-rejection [mismatch]:
AssertionError: TestEventPattern refused a leaf value not wrapped in an array with message 'Event pattern is not valid. Reason: "source" must be an object or an array', expected 'Event pattern is not valid. Reason: "source" must be an object or an array\n at [Source: (String)"{"source": "sim2real.orders"}"; line: 1, column: 13]'
events.test-event-pattern-numeric-rejects-non-numeric-json-values-and-degenerate-ranges [mismatch]:
AssertionError: expected message 'Event pattern is not valid. Reason: Bottom must be less than top\n at [Source: (String)"{"detail": {"amount": [{"numeric": [">", 100, "<", 50]}]}}"; line: 1, column: 55]', got 'Event pattern is not valid. Reason: detail.amount: numeric names a range whose bottom is not below its top'
events.events-create-archive-with-an-invalid-event-pattern-rejected [mismatch]:
CreateArchive with EventPattern {'source': [...], 'detail': {'n': [{'nosuchop': [1]}]}} settled on ('InvalidEventPatternException', 'Event pattern is not valid. Reason: Unrecognized match type nosuchop'), expected ('InvalidEventPatternException', 'Event pattern is not valid. Reason: Unrecognized match type nosuchop\n at [Source: (String)"..."; line: 1, column: 79]')

## Steps to Reproduce
1. Boot the emulator; create an events client.
2. Cause 1: events.test_event_pattern(EventPattern='{"detail":{"sourceIPAddress":[{"cidr":"203.0.113.0/32"}]}}', Event=<valid envelope>) -> observe HTTP 200 {"Result": false}; AWS answers 400 InvalidEventPatternException. Repeat with an IPv6 /128 prefix.
3. Cause 2: events.test_event_pattern(EventPattern='{"source":["s"]}', Event=<envelope with "id" removed>) -> observe InvalidEventPatternException; AWS answers ValidationException. Repeat for each of detail-type, source, account, time, region and for an unparseable "time".
4. Cause 3: events.test_event_pattern(EventPattern='{"detail":{"items":[{"exists":true}]}}', Event='{...,"detail":{"items":null}}') -> observe Result=false; AWS answers Result=true.
5. Cause 4: events.test_event_pattern(EventPattern='{"source": "sim2real.orders"}', Event=<valid envelope>) and events.test_event_pattern(EventPattern='{"detail":{"amount":[{"numeric":[">",100,"<",50]}]}}', ...) -> observe the Reason text and the absent ` at [Source: (String)"..."; line: N, column: C]` suffix; repeat through events.create_archive(EventPattern=...).
Replay the seven check ids in trace_ids to confirm.
