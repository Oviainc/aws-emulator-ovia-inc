Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: xray.annotation-value-types-are-indexed-by-type-not-stringified xray.service-and-edge-filter-keywords-select-by-submitted-topology
Issue-Shape-Id: com.amazonaws.xray#AWSXRay
Issue-Key: xray|GetTraceSummaries|InvalidRequestException
Issue-Commit: c4171394ecd9e8acdff90bddf637ffaae1174b81

# xray: GetTraceSummaries filter-expression grammar rejects two documented forms — the unary annotation[key] bracket predicate and the ! negation operator (2 causes)

> _Note: this issue was identified against the code base at commit `c4171394ecd9e8acdff90bddf637ffaae1174b81`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetTraceSummaries

## Description
Two net-new root causes in the X-Ray filter-expression parser used by GetTraceSummaries. Both are refusals: an expression real AWS parses and evaluates is rejected outright with InvalidRequestException, so the whole query never runs.

Cause 1 of 2: the unary bracket annotation predicate `annotation[<key>]` (and its negation `!annotation[<key>]`) is not accepted by the parser.
AWS: Smithy models GetTraceSummaries.FilterExpression as a plain string (com.amazonaws.xray#GetTraceSummariesRequest member FilterExpression, target FilterExpression) and the grammar itself is documented, not modelled — recorded behavior/doc only. The X-Ray console-filters documentation (https://docs.aws.amazon.com/xray/latest/devguide/xray-console-filters.html, "Example – annotation is set" / "Example – annotation is not set") defines `annotation[key]` as a unary predicate meaning "this annotation is set" and `!annotation[key]` as "is not set", and both compose with AND against other predicates. AWS parses such an expression and returns 200 with the matching TraceSummaries.
Emulator: the request is refused 400 InvalidRequestException with message `Error Location: line 1, position 54 : Invalid input symbol: '['`. The expression submitted was the scoping predicate `annotation.probe_run = "<marker>"` AND-composed with `annotation[<key>]`; position 54 is exactly the `[` of the bracket form, so the parser accepts the dotted comparison form and has no production for the bracket form at all. Fixed text of the emulator message is `Error Location: line 1, position ` / ` : Invalid input symbol: `; the line/position and the quoted symbol are derived from the request.
Why: a client that filters traces on "annotation present / absent" — the documented way to partition a corpus by an optional annotation — gets a hard 400 instead of a result set, and cannot distinguish an unsupported grammar from a malformed query of its own.
Fix: the FilterExpression parser/grammar in the xray crate (the GetTraceSummaries handler's filter-expression module), which must add the unary bracket annotation production and its `!` form, plus evaluation against the indexed annotation keys.
Traces: xray.annotation-value-types-are-indexed-by-type-not-stringified.

Cause 2 of 2: the `!` negation operator in front of a complex keyword is not accepted.
AWS: recorded behavior/doc only — the same console-filters page documents `!` as the negation operator and `service(name)` / `edge(source, destination)` as complex keywords, and the composition `<predicate> AND !service("<name>")` is valid and returns the traces that do NOT contain that node.
Emulator: refused 400 InvalidRequestException `Error Location: line 1, position 44 : Invalid input symbol: '!s'`. The preceding legs of the same check, which use bare `service(...)` and `edge(...)`, were accepted — position 44 is the `!` that immediately precedes `service(` — so the keywords themselves parse and only the negation prefix is unrecognised.
Why: every negative topology query ("traces through root R that did NOT call service S") is refused with a 400, so a client cannot express exclusion at all and a dashboard built on it reports an error rather than an empty-or-partial result.
Fix: the same FilterExpression parser in the xray crate — the unary `!` production must apply to complex keywords (service/edge) and to the annotation predicates, not only to whatever subset currently parses.
Traces: xray.service-and-edge-filter-keywords-select-by-submitted-topology.

Neither cause is covered by any open issue: the open xray filter/graph issue 20261025T043615Z-xray-3-net-new-behavioral-root-causes-getservicegraph.md concerns GetServiceGraph content and ResponseTimeRoot derivation, not expression parsing.

## Evidence
Cause 1 — xray.annotation-value-types-are-indexed-by-type-not-stringified:
  expected: HTTP 200 with TraceSummaries for FilterExpression `annotation.probe_run = "<marker>" AND annotation[<key>]`
  actual:   botocore.errorfactory.InvalidRequestException: An error occurred (InvalidRequestException) when calling the GetTraceSummaries operation: Error Location: line 1, position 54 : Invalid input symbol: '['

Cause 2 — xray.service-and-edge-filter-keywords-select-by-submitted-topology:
  expected: HTTP 200 with TraceSummaries for FilterExpression `annotation.probe_run = "<marker>" AND !service("<name>")`
  actual:   botocore.errorfactory.InvalidRequestException: An error occurred (InvalidRequestException) when calling the GetTraceSummaries operation: Error Location: line 1, position 44 : Invalid input symbol: '!s'
  (the bare service(...) and edge(...) legs earlier in the same check were accepted, isolating the '!' prefix)

## Steps to Reproduce
1. PutTraceSegments with a segment document carrying annotations, e.g. {"probe_run": "<marker>", "probe_only_high": "<marker>"}, and a second segment with a remote-namespace subsegment naming a downstream service.
2. Cause 1: GetTraceSummaries(StartTime=..., EndTime=..., Sampling=False, FilterExpression='annotation.probe_run = "<marker>" AND annotation[probe_only_high]') -> emulator answers 400 InvalidRequestException "Invalid input symbol: '['"; AWS answers 200. Same for '!annotation[probe_only_high]'.
3. Cause 2: GetTraceSummaries(..., FilterExpression='annotation.probe_run = "<marker>" AND !service("<callee>")') -> emulator answers 400 InvalidRequestException "Invalid input symbol: '!s'"; AWS answers 200 with the non-matching traces excluded.
4. Replay: xray.annotation-value-types-are-indexed-by-type-not-stringified, xray.service-and-edge-filter-keywords-select-by-submitted-topology.
