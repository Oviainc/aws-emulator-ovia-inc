Issue-Source: trace-sweep-divergence
Issue-Trace: pi-error-invalid-arguments
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: pi-error-invalid-arguments pi-error-invalid-time-range
Issue-Shape-Id: com.amazonaws.pi#PerformanceInsightsv20180227
Issue-Key: pi|DescribeDimensionKeys|4xx->4xx|$.__type
Issue-Key: pi|DescribeDimensionKeys|4xx->4xx|$.message
Issue-Key: pi|ListAvailableResourceDimensions|4xx->4xx|$.__type
Issue-Key: pi|ListAvailableResourceDimensions|4xx->4xx|$.message
Issue-Key: pi|GetResourceMetadata|4xx->4xx|$.__type
Issue-Key: pi|GetResourceMetadata|4xx->4xx|$.message
Issue-Key: pi|GetResourceMetrics|4xx->4xx|$.Message
Issue-Key: pi|CreatePerformanceAnalysisReport|4xx->4xx|$.Message
Issue-Commit: adba6aa61f0ba2f78a6b2d7c8f4b487f05884eaf

# pi input-constraint and time-range rejections carry the wrong error type and message rule

> _Note: this issue was identified against the code base at commit `adba6aa61f0ba2f78a6b2d7c8f4b487f05884eaf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDimensionKeys

## Description
Performance Insights (pi) rejects malformed requests under the wrong error identity. AWS runs protocol-level constraint validation AHEAD of the operation handler and reports a violation of a modeled @range / @length / @pattern constraint as ValidationException, whose Message is built to a fixed framework template: "Validation error(s) detected: Value at '<member>' failed to satisfy constraints." — the member is named, the offending VALUE is not echoed, and no Smithy constraint text is quoted. The emulator instead routes every such violation through the service's own InvalidArgumentException and builds a message by quoting the raw Smithy constraint. Separately, the two cross-member start/end-time rejections return the right status and code but a different fixed message template. This bundle carries 5 causes.

Model grounding: the pi Smithy model declares exactly three common errors for every operation — InternalServiceError (500), InvalidArgumentException (400) and NotAuthorizedException (400). ValidationException is NOT in the model, so for causes 1-3 the ValidationException type and its message template are RECORDED BEHAVIOR ONLY: the AWS service's protocol/validation layer emits it before the modeled operation errors are ever reachable. The recordings establish it independently on three different operations with three different constraint kinds, which is what makes it a rule rather than one recording's value.

Cause 1 of 5 — DescribeDimensionKeys with MaxResults above its modeled range (fixture pi-error-invalid-arguments, step task#1).
AWS: 400 with $.__type = ValidationException and $.message = "Validation error(s) detected: Value at 'maxResults' failed to satisfy constraints." Smithy: DescribeDimensionKeys.MaxResults targets MaxResults with @range min 0 max 25; recorded behavior only for the ValidationException type (not in the pi model).
Emulator: 400 (status matches) with $.__type = InvalidArgumentException and $.message = "Value '999' at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 25".
Why: the wire error code is what SDK error matching, retry classification and client-side error handling branch on. A client written against AWS matches ValidationException and falls through on InvalidArgumentException, so a plain client-side input bug surfaces as an unrecognized/unhandled error. The emulator also echoes the rejected value and the raw Smithy constraint sentence, which AWS does not — error-text assertions and log scrubbing built on AWS's template break.
Fix: the shared constraint-validation stage of the request walk (deserialize -> validate -> handle) that the generated pi service declaration drives — emit ValidationException with the "Validation error(s) detected: Value at '<member>' failed to satisfy constraints." template for modeled-constraint violations, instead of lowering them into the service's InvalidArgumentException.

Cause 2 of 5 — ListAvailableResourceDimensions with a Metrics list longer than its modeled length bound (fixture pi-error-invalid-arguments, step task#2).
AWS: 400 with $.__type = ValidationException and $.message = "Validation error(s) detected: Value at 'metrics' failed to satisfy constraints." Smithy: ListAvailableResourceDimensions.Metrics is @required, targets DimensionsMetricList with @length min 1 max 5; recorded behavior only for the ValidationException type.
Emulator: 400 with $.__type = InvalidArgumentException and $.message = "Value '6 items' at 'metrics' failed to satisfy constraint: Member must have length less than or equal to 5".
Why: same as cause 1 — wrong wire error code defeats client error matching; this confirms the defect is in the shared validation layer and not in one operation, since a @length violation on a list member takes the same wrong path as a @range violation on an integer.
Fix: same shared constraint-validation stage.

Cause 3 of 5 — GetResourceMetadata with an Identifier violating its modeled pattern (fixture pi-error-invalid-arguments, step task#3).
AWS: 400 with $.__type = ValidationException and $.message = "Validation error(s) detected: Value at 'identifier' failed to satisfy constraints." Smithy: GetResourceMetadata.Identifier is @required, targets IdentifierString with @length min 0 max 256 and @pattern ^[a-zA-Z0-9-]+$; recorded behavior only for the ValidationException type.
Emulator: 400 with $.__type = InvalidArgumentException and $.message = "Value 'not a valid id!' at 'identifier' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-zA-Z0-9-]+$".
Why: wrong wire error code as above, plus the emulator reflects the raw caller-supplied identifier AND the service's internal regex back to the caller. AWS's template deliberately exposes neither, so a client that logs or surfaces the error message leaks request content the real service suppresses.
Fix: same shared constraint-validation stage; the @pattern branch must use the same template and must not quote the regex.

Cause 4 of 5 — GetResourceMetrics with StartTime not earlier than EndTime (fixture pi-error-invalid-time-range, step task#4).
AWS: 400 with $.__type = InvalidArgumentException (status and code both MATCH) and Message 'Specify a start time that is before or equal to the end time.' Smithy: GetResourceMetrics.StartTime/EndTime are @required ISOTimestamp; StartTime's documentation states "The value for StartTime must be earlier than the value for EndTime." — the model fixes the RULE but states no message, so the message is recorded behavior only. Because a cross-member ordering check is not a single-member @range/@length/@pattern, it is correctly the service's own InvalidArgumentException and not the validation layer's ValidationException.
Emulator: 400, $.__type = InvalidArgumentException, Message 'The value for StartTime must be earlier than the value for EndTime.'
Why: this is a message-template divergence only — status and error code are correct, so an SDK-matching client is unaffected. It is filed because the template is a fixed, member-independent service-wide constant (nothing in it comes from the request), recorded identically for the same condition on two different operations (see cause 5), rather than an environment-specific value; a client or test asserting on PI's documented rejection text sees a different sentence. Lowest-severity cause in this bundle.
Fix: GetResourceMetrics' handler — the hand-written ordering-rejection string.

Cause 5 of 5 — CreatePerformanceAnalysisReport with StartTime not earlier than EndTime (fixture pi-error-invalid-time-range, step task#5).
AWS: 400 with $.__type = InvalidArgumentException (matching) and Message 'Specify a start time that is before or equal to the end time.' Smithy: CreatePerformanceAnalysisReport.StartTime is @required ISOTimestamp and EndTime optional; same cross-member ordering rule, message is recorded behavior only.
Emulator: 400, $.__type = InvalidArgumentException, Message 'The value for StartTime must be earlier than the value for EndTime.'
Why: the identical recorded AWS sentence on a second, unrelated operation is what establishes that PI emits ONE service-wide template for this condition while the emulator emits a per-handler one. Message-only, same low impact as cause 4.
Fix: CreatePerformanceAnalysisReport's handler — the same hand-written string; both handlers should share one template.

Severity is HIGH as the bundle maximum: causes 1-3 change the wire error CODE, causes 4-5 are LOW message-only causes travelling with them. No open pi issue covers any of these; the only existing pi issue is its bootstrap issue, which is already resolved. state_reset_failures was 0 for this replay.

Traces:
- pi-error-invalid-arguments#task#1 — DescribeDimensionKeys, __type ValidationException -> InvalidArgumentException
- pi-error-invalid-arguments#task#2 — ListAvailableResourceDimensions, __type ValidationException -> InvalidArgumentException
- pi-error-invalid-arguments#task#3 — GetResourceMetadata, __type ValidationException -> InvalidArgumentException
- pi-error-invalid-time-range#task#4 — GetResourceMetrics, Message template mismatch
- pi-error-invalid-time-range#task#5 — CreatePerformanceAnalysisReport, Message template mismatch
Code: services/service-com-amazonaws-pi-performanceinsightsv20180227/src/ops/get_resource_metrics.rs lines 208-216 (the StartTime/EndTime ordering rejection and its message) and lines 364-369 (the invalid_argument helper every rejection in the crate funnels through); services/service-com-amazonaws-pi-performanceinsightsv20180227/src/lib.rs line 21 (the generated service declaration that drives the deserialize -> validate -> handle walk where constraint violations are classified); services/service-com-amazonaws-pi-performanceinsightsv20180227/smithy-model/com-amazonaws-pi-performanceinsightsv20180227.json (MaxResults @range, DimensionsMetricList @length, IdentifierString @pattern, and the three-error common set with no ValidationException).

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator (emulator_up=true, state_reset_failures=0):

1) pi-error-invalid-arguments#task#1 DescribeDimensionKeys — expected_status 400, actual_status 400
   detail: $.__type: value_mismatch (expected 'ValidationException', got 'InvalidArgumentException'); $.message: value_mismatch (expected "Validation error(s) detected: Value at 'maxResults' failed to satisfy constraints.", got "Value '999' at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 25")

2) pi-error-invalid-arguments#task#2 ListAvailableResourceDimensions — expected_status 400, actual_status 400
   detail: $.__type: value_mismatch (expected 'ValidationException', got 'InvalidArgumentException'); $.message: value_mismatch (expected "Validation error(s) detected: Value at 'metrics' failed to satisfy constraints.", got "Value '6 items' at 'metrics' failed to satisfy constraint: Member must have length less than or equal to 5")

3) pi-error-invalid-arguments#task#3 GetResourceMetadata — expected_status 400, actual_status 400
   detail: $.__type: value_mismatch (expected 'ValidationException', got 'InvalidArgumentException'); $.message: value_mismatch (expected "Validation error(s) detected: Value at 'identifier' failed to satisfy constraints.", got "Value 'not a valid id!' at 'identifier' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-zA-Z0-9-]+$")

4) pi-error-invalid-time-range#task#4 GetResourceMetrics — expected_status 400, actual_status 400
   detail: $.Message: value_mismatch (expected 'Specify a start time that is before or equal to the end time.', got 'The value for StartTime must be earlier than the value for EndTime.')

5) pi-error-invalid-time-range#task#5 CreatePerformanceAnalysisReport — expected_status 400, actual_status 400
   detail: $.Message: value_mismatch (expected 'Specify a start time that is before or equal to the end time.', got 'The value for StartTime must be earlier than the value for EndTime.')

Requirements asserted: for causes 1-3, HTTP 400 with wire error code ValidationException and the framework message template "Validation error(s) detected: Value at '<member>' failed to satisfy constraints." (member-named, value not echoed, constraint text not quoted) — the recorded strings above are instances of that template. For causes 4-5, the status (400) and code (InvalidArgumentException) already match; the asserted rule is the single service-wide ordering-rejection template, recorded identically on two operations, and the quoted sentence is the recorded example of it.

## Steps to Reproduce
1. git clone the mockforge-aws-emulator repo (branch mainline) and boot the emulator built from it.
2. Replay the simparity-wire-aws-v1 wire corpus.
3. Cause 1: fixture pi-error-invalid-arguments, step task#1 — DescribeDimensionKeys with MaxResults=999 (modeled @range 0..25). Emulator: __type=InvalidArgumentException with the value echoed and the Smithy constraint quoted; AWS: __type=ValidationException, message "Validation error(s) detected: Value at 'maxResults' failed to satisfy constraints."
4. Cause 2: same fixture, step task#2 — ListAvailableResourceDimensions with 6 entries in Metrics (modeled @length 1..5). Same wrong __type.
5. Cause 3: same fixture, step task#3 — GetResourceMetadata with Identifier='not a valid id!' (modeled @pattern ^[a-zA-Z0-9-]+$). Same wrong __type, and the emulator reflects the identifier and the regex.
6. Cause 4: fixture pi-error-invalid-time-range, step task#4 — GetResourceMetrics with StartTime >= EndTime. Status 400 and InvalidArgumentException match; Message template differs.
7. Cause 5: same fixture, step task#5 — CreatePerformanceAnalysisReport with StartTime >= EndTime. Same Message template divergence.
8. Confirm a fix by re-replaying pi-error-invalid-arguments#task#1, pi-error-invalid-arguments#task#2, pi-error-invalid-arguments#task#3, pi-error-invalid-time-range#task#4 and pi-error-invalid-time-range#task#5 — all five must pass.
