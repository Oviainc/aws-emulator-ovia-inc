Issue-Source: trace-sweep-divergence
Issue-Trace: events-ebecs-trigger-pattern-test
Issue-Step: task#12
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: events-ebecs-trigger-pattern-test
Issue-Shape-Id: com.amazonaws.eventbridge#AWSEvents
Issue-Key: events|TestEventPattern|2xx->2xx|$.Result
Issue-Key: events|TestEventPattern|4xx->4xx|$.message
Issue-Commit: e0cc4f83c3608c610536f43d6e35ff223081a6f4

# events: the event-pattern engine misses a match and re-words the invalid-pattern reason (2 causes)

> _Note: this issue was identified against the code base at commit `e0cc4f83c3608c610536f43d6e35ff223081a6f4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TestEventPattern

## Description
Two net-new wire root causes in the events service, both in the event-pattern engine reached through TestEventPattern — one in matching, one in the parser's diagnostic. Counts: 2 causes.

Cause 1 of 2: TestEventPattern reports no match for a pattern/event pair AWS matches.
AWS: TestEventPattern evaluates the EventPattern against the supplied Event and answers HTTP 200 with $.Result = true when the pattern matches. For the recorded pair AWS answers true. Smithy (com.amazonaws.eventbridge): TestEventPattern input {EventPattern @required @length(0,4096), Event @required}, output Result boolean with default false, errors InternalException / InvalidEventPatternException — the model says nothing about which patterns match, so the matching semantics are recorded behavior only.
Emulator: HTTP 200 with $.Result = false. Hypothesis: the surrounding steps of the same fixture exercise content filters on a nested detail member (the adjacent steps submit a prefix matcher at detail.group), so the matcher most likely does not implement the content filter used at this step and falls through to the member's modeled default of false rather than failing loudly.
Why: a Result of false is indistinguishable from a legitimately non-matching pattern, so a client that uses TestEventPattern to validate a rule's pattern before PutRule concludes its pattern is wrong and 'fixes' a correct pattern; the same matcher governs rule delivery, so every rule built on that filter form silently never fires.
Fix: the event-pattern matcher in the events crate (the comparison/content-filter evaluation shared by TestEventPattern and rule matching) — implement the filter form and return a parse/validation error rather than a silent false for a form it cannot evaluate.
Traces: events-ebecs-trigger-pattern-test#task#12.

Cause 2 of 2: the InvalidEventPatternException reason for a non-string prefix matcher is re-worded.
AWS: a pattern whose prefix matcher is given a non-string value is refused with HTTP 400 and $.message = 'Event pattern is not valid. Reason: prefix match pattern must be a string\n at [Source: (String)"<the submitted pattern>"; line: <n>, column: <c>]'. The fixed text is the frame 'Event pattern is not valid. Reason: ' plus the per-form reason 'prefix match pattern must be a string' — with no member path in it — and the variable parts are the caller's own pattern string echoed verbatim and its line/column offset. Smithy: TestEventPattern models InvalidEventPatternException (client, 400); the reason strings are recorded behavior only.
Emulator: the status, error class and the trailing '[Source: (String)"..."; line: 1, column: 35]' suffix are right, but the reason is replaced by an emulator-authored one that prefixes the member path and enumerates the accepted forms: 'detail.group: prefix takes a string, or {"equals-ignore-case": "<string>"}'.
Why: the message is the only machine-readable detail of this error class (the code is the same for every malformed pattern), so tooling that classifies pattern defects by reason, and any recorded-response comparison, fails on a reason AWS does not produce.
Fix: the event-pattern parser's diagnostic construction in the events crate — emit AWS's fixed per-form reason text, keeping the existing Source/line/column suffix and leaving the member path out of it.
Traces: events-ebecs-trigger-pattern-test#task#13 (cluster also covers task#14 of the same fixture).

## Evidence
Cause 1 — TestEventPattern, HTTP 200 both sides. Expected $.Result true; actual false.
Cause 2 — TestEventPattern, HTTP 400 both sides. Expected $.message 'Event pattern is not valid. Reason: prefix match pattern must be a string\n at [Source: (String)"{"detail": {"group": [{"prefix": 5}]}}"; line: 1, column: 35]'; actual 'Event pattern is not valid. Reason: detail.group: prefix takes a string, or {"equals-ignore-case": "<string>"}\n at [Source: (String)"{"detail": {"group": [{"prefix": 5}]}}"; line: 1, column: 35]' (2 steps).

## Steps to Reproduce
Replay the corpus: replay_wire_corpus() against the emulator built from this checkout.
Cause 1: call TestEventPattern with the fixture's pattern/event pair and read $.Result (events-ebecs-trigger-pattern-test#task#12).
Cause 2: call TestEventPattern with a pattern giving prefix a numeric value (e.g. a prefix matcher on a nested detail member whose value is 5) and read $.message (events-ebecs-trigger-pattern-test#task#13).
