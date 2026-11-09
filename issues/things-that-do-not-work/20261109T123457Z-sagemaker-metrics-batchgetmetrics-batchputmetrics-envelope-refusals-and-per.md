Issue-Source: trace-sweep-divergence
Issue-Trace: sagemaker-metrics-batch-get-metrics-envelope-rejections
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sagemaker-metrics-batch-get-metrics-envelope-rejections sagemaker-metrics-batch-get-validation-envelope sagemaker-metrics-batch-put-metrics-read-back-by-iteration sagemaker-metrics-batch-put-metrics-structural-rejections sagemaker-metrics-batch-put-validation-envelope
Issue-Shape-Id: com.amazonaws.sagemakermetrics#SageMakerMetricsService
Issue-Key: sagemaker-metrics|BatchGetMetrics|5xx->4xx|$.message
Issue-Key: sagemaker-metrics|BatchGetMetrics|5xx->4xx|$headers.x-amzn-errortype
Issue-Key: sagemaker-metrics|BatchGetMetrics|5xx->4xx|status
Issue-Key: sagemaker-metrics|BatchGetMetrics|2xx->4xx|$.MetricQueryResults
Issue-Key: sagemaker-metrics|BatchGetMetrics|2xx->4xx|status
Issue-Key: sagemaker-metrics|BatchGetMetrics|2xx->2xx|$.MetricQueryResults[*].Message
Issue-Key: sagemaker-metrics|BatchGetMetrics|2xx->2xx|$.MetricQueryResults[*].MetricValues
Issue-Key: sagemaker-metrics|BatchGetMetrics|2xx->2xx|$.MetricQueryResults[*].Status
Issue-Key: sagemaker-metrics|BatchGetMetrics|2xx->2xx|$.MetricQueryResults[*].XAxisValues
Issue-Key: sagemaker-metrics|BatchPutMetrics|2xx->2xx|$
Issue-Key: sagemaker-metrics|BatchPutMetrics|4xx->4xx|$.message
Issue-Key: sagemaker-metrics|BatchPutMetrics|5xx->4xx|$.message
Issue-Key: sagemaker-metrics|BatchPutMetrics|5xx->4xx|$headers.x-amzn-errortype
Issue-Key: sagemaker-metrics|BatchPutMetrics|5xx->4xx|status
Issue-Key: sagemaker-metrics|BatchPutMetrics|4xx->4xx|$.Message
Issue-Key: sagemaker-metrics|BatchPutMetrics|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: sagemaker-metrics|BatchPutMetrics|2xx->4xx|$.Errors
Issue-Key: sagemaker-metrics|BatchPutMetrics|2xx->4xx|status
Issue-Key: sagemaker-metrics|BatchPutMetrics|2xx->2xx|$.Errors[*].Code
Issue-Commit: e2ad54ee7b8ee1b2faa65791f8cb738708e5249b

# sagemaker-metrics: BatchGetMetrics/BatchPutMetrics envelope refusals and per-item result reporting diverge from AWS (10 causes)

> _Note: this issue was identified against the code base at commit `e2ad54ee7b8ee1b2faa65791f8cb738708e5249b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchGetMetrics,BatchPutMetrics

## Description
Replay of the wire corpus against the sagemaker-metrics data plane scored 6/26 steps (23.1%). All 20 diverging steps cluster into the TEN root causes below, all in the two bound operations' refusal envelope and per-item result reporting. The shared fault: this service answers structurally odd input with the framework's generic Smithy constraint refusal (HTTP 400 ValidationException, lowercase `message`, '1 validation error detected: ...'), where AWS either accepts the request and reports the outcome in-band (200, per-query Status / per-item Errors) or refuses it with its OWN status, body member and wording — and when it does report in-band, the emulator's result/error shapes and codes are not AWS's. Each cause must be closed before submitting.

Related open issue (does NOT cover these; it is the behavioral corpus and carries none of these wire replay keys): 20261022T212853Z-sagemaker-metrics-batchgetmetrics-skips-per-query-axis-period.md. Its Cause 1 overlaps Causes 3-4 below and its Cause 2 overlaps Cause 10 below; fixing those closes both views, but these wire steps must be re-replayed.

Cause 1 of 10: BatchGetMetrics with a null/absent MetricQueries is refused 400 ValidationException instead of AWS's 500.
AWS: Smithy BatchGetMetricsRequest.MetricQueries is `required` (target MetricQueryList, @length 1..100); the operation models NO errors, so no refusal class is modeled for it — for the status this is recorded behavior only. The recording shows AWS answering HTTP 500 for a request whose `MetricQueries` member is JSON null, and the recorded body carries NO lowercase `message` member at all (the fixture asserts the shape of x-amzn-errortype, not a code, so the error code is not a requirement here; the status and the absence of a lowercase `message` are).
Emulator: HTTP 400 with x-amzn-errortype: ValidationException and body {"message": "1 validation error detected: Value null at 'metricQueries' failed to satisfy constraint: Member must not be null"}. That text is the framework's generic constraint-walk sentence (fixed text "<n> validation error(s) detected: Value null at '<memberPath>' failed to satisfy constraint: Member must not be null", variable parts the count and the lowerCamel member path taken from the request) — AWS does not word this class that way for this service.
Why: a client that distinguishes a retryable server fault (5xx) from a permanent client fault (4xx) inverts its decision: against AWS a null MetricQueries is retried/escalated as a server-side failure, against the emulator it is dropped as a client error. Negative-path handling for this envelope can never be exercised.
Fix: the sagemaker-metrics request pipeline's refusal wording/status selection (crate::service::EmulatedService::override_refusal, the one seam that already re-words this service's boundary refusals) — a BatchGetMetrics request with a null required MetricQueries must leave the framework's 400 ValidationException path and answer 500 with no lowercase `message` member.
Traces: sagemaker-metrics-batch-get-metrics-envelope-rejections#task#3; also sagemaker-metrics-batch-get-validation-envelope#task#4.

Cause 2 of 10: BatchGetMetrics refuses 400 a request AWS accepts, so the per-query result list is never produced.
AWS: recorded behavior only (BatchGetMetrics models no errors; its per-query channel is MetricQueryResult.Status, enum MetricQueryResultStatus {Complete, Truncated, InternalError, ValidationError}). The recording shows AWS answering HTTP 200 with MetricQueryResults = [{Status: 'Complete', XAxisValues: [], MetricValues: [], Message absent}] — i.e. the envelope is accepted and the query is answered in-band with empty value lists, rather than the whole call being refused.
Emulator: HTTP 400 and no MetricQueryResults member at all — the framework's constraint walk refuses the envelope before crate::ops::batch_get_metrics::handle runs, so no query is evaluated.
Why: AWS's contract for this operation is that a query's fate is reported positionally in a 200 response and one query's problem never fails its siblings. A client that reads MetricQueryResults[i].Status gets a transport-level exception instead of a result list, so no reader of this operation works against the emulator for this input class.
Fix: BatchGetMetrics' request validation/admission path — narrow the pre-handler constraint refusal to only what AWS actually refuses at the envelope, and let the handler answer the query in-band (Status=Complete with empty XAxisValues/MetricValues when nothing matched, which the handler already does).
Traces: sagemaker-metrics-batch-get-metrics-envelope-rejections#task#4; also #task#5 and sagemaker-metrics-batch-get-validation-envelope#task#3.

Cause 3 of 10: a MetricQuery whose Period is IterationNumber but whose XAxisType is not is reported Complete instead of ValidationError, and the failing result carries empty lists instead of none.
AWS: Smithy MetricQuery.Period is enum Period {OneMinute, FiveMinute, OneHour, IterationNumber} and MetricQuery.XAxisType is enum XAxisType {IterationNumber, Timestamp}; MetricQueryResult.Status is required (MetricQueryResultStatus) and Message is Message (@length 0..2048). The model does not state which pairings are legal, so the rule is recorded behavior only: an iteration-keyed Period paired with a non-iteration axis resolves, inside a 200 response, to that query's Status='ValidationError' with Message='Period must be IterationNumber when XAxisType is IterationNumber' — all fixed text, no request value interpolated — and with MetricValues and XAxisValues ABSENT from that result (not empty arrays).
Emulator: Status='Complete', Message absent, MetricValues=[] and XAxisValues=[] — the query is answered as if well formed. crate::ops::batch_get_metrics::evaluate only rejects enum values outside the model, a nameless ARN and Start>End; it never cross-checks Period against XAxisType, and `failed()` (same file) builds empty Vecs for the two list members, which the required-member serializer always emits.
Why: Status is the only channel AWS offers for a bad query (the call still returns 200). A client validating a dashboard/query configuration sees success for a pairing AWS rejects and plots values real AWS never returns; and a client that tests `MetricValues is None` to detect a failed result sees an empty list instead.
Fix: BatchGetMetrics' per-query evaluation (crate::ops::batch_get_metrics::evaluate) plus the MetricQueryResult serialization of a failing result — cross-validate Period against XAxisType per query, emit Status=ValidationError with the fixed sentence, and omit MetricValues/XAxisValues on a failing result while sibling queries are still computed normally.
Traces: sagemaker-metrics-batch-get-metrics-envelope-rejections#task#6.

Cause 4 of 10: the mirror pairing — XAxisType=IterationNumber with a non-iteration Period — is likewise reported Complete, on a non-first query position.
AWS: recorded behavior only, same model members as Cause 3. MetricQueryResults[1].Status='ValidationError' with Message='XAxisType must be IterationNumber when Period is IterationNumber' (all fixed text), MetricValues and XAxisValues absent, while the sibling query at index 0 is still answered normally in the same 200 response.
Emulator: MetricQueryResults[1].Status='Complete', Message absent, MetricValues=[] and XAxisValues=[].
Why: same as Cause 3, and it additionally shows the gap is positional — a bad query anywhere in the list is silently promoted to success, so a batch of mixed-validity queries cannot be told apart.
Fix: same component as Cause 3 (crate::ops::batch_get_metrics::evaluate); this direction of the mismatch needs its own sentence, so both messages must be produced, selected by which member is the iteration-keyed one.
Traces: sagemaker-metrics-batch-get-validation-envelope#task#2.

Cause 5 of 10: a fully successful BatchPutMetrics answers an EMPTY body instead of {"Errors": []}.
AWS: Smithy BatchPutMetricsResponse.Errors targets BatchPutMetricsErrorList (@length 1..10). Despite that length bound, the recording shows AWS serializing the member as an EMPTY list on a call in which nothing failed: body {"Errors": []} with HTTP 200 — so the member's presence is not conditional on a failure.
Emulator: HTTP 200 with body {} — the member is omitted. crate::ops::batch_put_metrics (handler tail, `errors: (!errors.is_empty()).then_some(errors)`) deliberately drops it, reasoning from the 1..10 bound; the recording contradicts that reasoning.
Why: a client that reads response['Errors'] unconditionally — the natural reading of a partial-success batch API — raises a KeyError/NPE against the emulator on the success path, the single most common path. Length-of-Errors assertions in client tests cannot run at all.
Fix: BatchPutMetrics' output construction/serializer — always emit Errors, as [] when every data point was ingested.
Traces: sagemaker-metrics-batch-put-metrics-read-back-by-iteration#task#2; also sagemaker-metrics-batch-put-metrics-structural-rejections#task#1 and sagemaker-metrics-batch-put-validation-envelope#task#1.

Cause 6 of 10: BatchPutMetrics with a null/absent TrialComponentName is refused with the generic constraint sentence instead of AWS's 'Invalid resource.'
AWS: Smithy BatchPutMetricsRequest.TrialComponentName is `required` (ExperimentEntityName, @length 1..120, @pattern '^[a-z0-9](-*[a-z0-9]){0,119}$'). The operation models no errors, so the wording is recorded behavior only: AWS answers HTTP 400 with body message exactly 'Invalid resource.' — a constant, entirely fixed text with NO member path, submitted value or pattern quoted back (the same shape of constant this service already uses for a malformed ARN, 'Invalid SageMaker ARN').
Emulator: HTTP 400 with message "1 validation error detected: Value null at 'trialComponentName' failed to satisfy constraint: Member must not be null" — the framework's generic sentence, whose variable parts are the violation count and the lowerCamel member path from the request.
Why: the error message is part of the contract. A client or test that matches AWS's 'Invalid resource.' never matches, and conversely the emulator leaks an internal member path and constraint vocabulary that AWS does not expose for this service, so message-driven branching diverges in both directions.
Fix: the sagemaker-metrics refusal wording seam (crate::service::EmulatedService::override_refusal, which already owns INVALID_RESOURCE_ARN_MESSAGE) — word a missing/invalid TrialComponentName refusal as the constant 'Invalid resource.' at HTTP 400.
Traces: sagemaker-metrics-batch-put-metrics-structural-rejections#task#2; also sagemaker-metrics-batch-put-validation-envelope#task#3.

Cause 7 of 10: BatchPutMetrics with a null/absent MetricData is refused 400 ValidationException instead of AWS's 500 'Unhandled internal failure'.
AWS: Smithy BatchPutMetricsRequest.MetricData is `required` (RawMetricDataList, @length 1..10); no errors are modeled, so this is recorded behavior only: AWS answers HTTP 500 with body message exactly 'Unhandled internal failure' — a constant, all fixed text, nothing from the request interpolated (and the fixture asserts only the shape of x-amzn-errortype, so the code is not the requirement; the 500 and that sentence are).
Emulator: HTTP 400, x-amzn-errortype: ValidationException, message "1 validation error detected: Value null at 'metricData' failed to satisfy constraint: Member must not be null".
Why: both the retry class and the text are wrong. A caller that omits MetricData sees, against AWS, a server-side 5xx it must retry or escalate; against the emulator a permanent 4xx client error it will drop. SDK retry policies behave oppositely on the two.
Fix: the same refusal seam as Causes 1 and 6 — select HTTP 500 and the constant 'Unhandled internal failure' for a BatchPutMetrics request with a null required MetricData, instead of falling through to the framework's constraint refusal.
Traces: sagemaker-metrics-batch-put-metrics-structural-rejections#task#3.

Cause 8 of 10: a JSON-type-mismatched member is refused in the wrong error envelope — AWS's capital-M `Message` and its 'is not an expected Json type' sentence are never produced.
AWS: recorded behavior only (restJson1; the operation models no errors). When a member arrives with a JSON type the shape cannot accept (here a bare string where RawMetricDataList is modeled), AWS answers HTTP 400 with the body member spelled `Message` (capital M, unlike the lowercase `message` it uses for the refusals above) carrying '<offending value> is not an expected Json type' — fixed text ' is not an expected Json type', variable part the offending JSON value as submitted (the recording's 'STRING_VALUE' is that request's value filling the template, not part of it) — and with an x-amzn-errortype whose shape is NOT the emulator's 'ValidationException'.
Emulator: HTTP 400 with NO `Message` member at all (it emits lowercase `message` with the generic constraint sentence) and x-amzn-errortype: ValidationException.
Why: deserialization failures are a distinct class for this service, with a distinct body member name and a distinct error type. A client that reads error['Message'] (as AWS's own SDK error shaping does for this class) finds nothing, and anything branching on the error type mis-files the failure as a constraint violation.
Fix: BatchPutMetrics' request deserialization error path and the service's error serializer — raise type-mismatch failures as their own class, serialized with the capital-M `Message` member and the '<value> is not an expected Json type' sentence, not through the constraint-violation ValidationException envelope.
Traces: sagemaker-metrics-batch-put-metrics-structural-rejections#task#4.

Cause 9 of 10: BatchPutMetrics refuses 400 a batch AWS accepts, so the partial-success Errors channel is never reached (the largest cluster: 5 steps).
AWS: recorded behavior only. Smithy models BatchPutMetricsError.Code as enum PutMetricsErrorCode {METRIC_LIMIT_EXCEEDED, INTERNAL_ERROR, VALIDATION_ERROR, CONFLICT_ERROR} with MetricIndex identifying the offending item — i.e. AWS's contract for a questionable data point is a per-index entry inside a 200 response, not an envelope refusal. The recordings show HTTP 200 with Errors = [] for these batches: the request is accepted and every point ingested.
Emulator: HTTP 400 with no Errors member — the pre-handler constraint walk refuses the whole batch, so crate::ops::batch_put_metrics never runs and nothing is stored.
Why: this is the core of a batch-ingestion API. A client writing a mixed batch gets a transport exception instead of a 200 with per-index verdicts, loses the points AWS would have accepted, and can never exercise MetricIndex-based retry/dead-letter logic. It also silently diverges state: the metrics AWS stored are absent from the emulator, so every later BatchGetMetrics read-back for them is wrong too.
Fix: BatchPutMetrics' request validation/admission path — restrict envelope-level refusal to what AWS actually refuses, and route the remaining per-point problems into BatchPutMetricsError entries (Code + MetricIndex) in a 200 response while sibling points ingest.
Traces: sagemaker-metrics-batch-put-metrics-structural-rejections#task#5; also #task#6, #task#7, sagemaker-metrics-batch-put-validation-envelope#task#4 and #task#5.

Cause 10 of 10: a data point AWS rejects per-index as INTERNAL_ERROR is reported VALIDATION_ERROR.
AWS: Smithy RawMetricData.Value is a required Double and BatchPutMetricsError.Code is enum PutMetricsErrorCode {METRIC_LIMIT_EXCEEDED, INTERNAL_ERROR, VALIDATION_ERROR, CONFLICT_ERROR}. The model does not say which code this class of point draws, so the selection is recorded behavior only: the point is rejected per-index inside a 200 response with Code='INTERNAL_ERROR' at its MetricIndex, while sibling valid points in the same batch still ingest.
Emulator: Errors[0].Code='VALIDATION_ERROR'. crate::ops::batch_put_metrics (the per-point loop, the `!raw.value.is_finite()` branch) pushes CODE_VALIDATION_ERROR.
Why: the code is the client's retry decision. INTERNAL_ERROR is retried, VALIDATION_ERROR is treated as a permanent client fault and dropped — so the emulator inverts the retry/dead-letter behavior for this class of point, and client retry logic cannot be exercised faithfully.
Fix: BatchPutMetrics' per-point classification (crate::ops::batch_put_metrics handler loop) — classify a non-finite Value as INTERNAL_ERROR when building the BatchPutMetricsError, keeping MetricIndex and the partial-success behavior for siblings.
Traces: sagemaker-metrics-batch-put-validation-envelope#task#2.

## Evidence
Wire replay of trace set simparity-wire-aws-v1 against the booted emulator: 5 fixtures replayed, 26 steps, 6 passed, 20 mismatched, 0 errored, 0 blocked — score 23.1%. 10 divergence clusters, all service=sagemaker-metrics, none carrying stub_path or owned_by. Emulator was up (/__admin/health 200, data listener accepting), so bootstrap_signal is false.

Cause 1 — sagemaker-metrics-batch-get-metrics-envelope-rejections#task#3 (BatchGetMetrics), affected 2:
  status: expected 500, got 400
  $.message: unexpected_present (expected None, got "1 validation error detected: Value null at 'metricQueries' failed to satisfy constraint: Member must not be null")
  $headers.x-amzn-errortype: shape_mismatch (expected '<shaped-not-asserted>', got 'ValidationException')

Cause 2 — sagemaker-metrics-batch-get-metrics-envelope-rejections#task#4 (BatchGetMetrics), affected 3:
  status: expected 200, got 400
  $.MetricQueryResults: missing (expected [{'Message': None, 'MetricValues': [], 'Status': 'Complete', 'XAxisValues': []}], got None)

Cause 3 — sagemaker-metrics-batch-get-metrics-envelope-rejections#task#6 (BatchGetMetrics), status 200 both:
  $.MetricQueryResults[0].Message: missing (expected 'Period must be IterationNumber when XAxisType is IterationNumber', got None)
  $.MetricQueryResults[0].Status: value_mismatch (expected 'ValidationError', got 'Complete')
  $.MetricQueryResults[0].MetricValues: type_mismatch (expected None, got [])
  $.MetricQueryResults[0].XAxisValues: type_mismatch (expected None, got [])

Cause 4 — sagemaker-metrics-batch-get-validation-envelope#task#2 (BatchGetMetrics), status 200 both:
  $.MetricQueryResults[1].Message: missing (expected 'XAxisType must be IterationNumber when Period is IterationNumber', got None)
  $.MetricQueryResults[1].Status: value_mismatch (expected 'ValidationError', got 'Complete')
  $.MetricQueryResults[1].MetricValues: type_mismatch (expected None, got [])
  $.MetricQueryResults[1].XAxisValues: type_mismatch (expected None, got [])

Cause 5 — sagemaker-metrics-batch-put-metrics-read-back-by-iteration#task#2 (BatchPutMetrics), affected 3, status 200 both:
  $: empty_body (expected {'Errors': []}, got {})

Cause 6 — sagemaker-metrics-batch-put-metrics-structural-rejections#task#2 (BatchPutMetrics), affected 2, status 400 both:
  $.message: value_mismatch (expected 'Invalid resource.', got "1 validation error detected: Value null at 'trialComponentName' failed to satisfy constraint: Member must not be null")

Cause 7 — sagemaker-metrics-batch-put-metrics-structural-rejections#task#3 (BatchPutMetrics):
  status: expected 500, got 400
  $.message: value_mismatch (expected 'Unhandled internal failure', got "1 validation error detected: Value null at 'metricData' failed to satisfy constraint: Member must not be null")
  $headers.x-amzn-errortype: shape_mismatch (expected '<shaped-not-asserted>', got 'ValidationException')

Cause 8 — sagemaker-metrics-batch-put-metrics-structural-rejections#task#4 (BatchPutMetrics), status 400 both:
  $.Message: missing (expected 'STRING_VALUE is not an expected Json type', got None)
  $headers.x-amzn-errortype: shape_mismatch (expected '<shaped-not-asserted>', got 'ValidationException')

Cause 9 — sagemaker-metrics-batch-put-metrics-structural-rejections#task#5 (BatchPutMetrics), affected 5:
  status: expected 200, got 400
  $.Errors: missing (expected [], got None)

Cause 10 — sagemaker-metrics-batch-put-validation-envelope#task#2 (BatchPutMetrics), status 200 both:
  $.Errors[0].Code: value_mismatch (expected 'INTERNAL_ERROR', got 'VALIDATION_ERROR')

Supporting code sites in the checkout: services/service-com-amazonaws-sagemakermetrics-sagemakermetricsservice/src/ops/batch_put_metrics.rs lines 227-231 (Errors omitted when empty — Cause 5) and line 184 (non-finite Value -> CODE_VALIDATION_ERROR — Cause 10); .../src/ops/batch_get_metrics.rs lines 306-313 (`failed()` builds empty XAxisValues/MetricValues) and 324-375 (evaluate() never cross-checks Period against XAxisType — Causes 3-4); .../src/ops/batch_get_metrics.rs lines 34-40 document that boundary refusals are worded in crate::service::EmulatedService::override_refusal, the seam Causes 1, 6 and 7 belong to.

## Steps to Reproduce
Boot the emulator from this checkout and replay the bound wire corpus (trace set simparity-wire-aws-v1), or drive the endpoint directly:

1) Cause 1: POST /BatchGetMetrics with body {"MetricQueries": null}. Emulator: 400, x-amzn-errortype ValidationException, lowercase `message` with the generic constraint sentence. AWS: 500, no lowercase `message` member. (fixture sagemaker-metrics-batch-get-metrics-envelope-rejections, step task#3)

2) Cause 2: replay step task#4 of sagemaker-metrics-batch-get-metrics-envelope-rejections. Emulator: 400 with no MetricQueryResults. AWS: 200 with MetricQueryResults=[{Status:'Complete', XAxisValues:[], MetricValues:[]}].

3) Causes 3 and 4: POST /BatchGetMetrics with a MetricQuery pairing Period='IterationNumber' with XAxisType='Timestamp' (Cause 3), and in a second request/position the mirror pairing XAxisType='IterationNumber' with a non-iteration Period (Cause 4), alongside a well-formed sibling query. Inspect the 200 body: emulator reports Status='Complete' with MetricValues=[]/XAxisValues=[] and no Message; AWS reports Status='ValidationError' with the matching 'Period must be IterationNumber when XAxisType is IterationNumber' / 'XAxisType must be IterationNumber when Period is IterationNumber' sentence and OMITS MetricValues/XAxisValues, while the sibling stays 'Complete'. (steps task#6 and sagemaker-metrics-batch-get-validation-envelope#task#2)

4) Cause 5: PUT /BatchPutMetrics with one valid data point for a trial component. Emulator: 200 with body {}. AWS: 200 with body {"Errors": []}. (sagemaker-metrics-batch-put-metrics-read-back-by-iteration#task#2)

5) Cause 6: PUT /BatchPutMetrics with {"TrialComponentName": null, "MetricData": [ ...valid point... ]}. Emulator message: the generic "1 validation error detected: Value null at 'trialComponentName' ..."; AWS message: exactly 'Invalid resource.' at 400.

6) Cause 7: PUT /BatchPutMetrics with {"TrialComponentName": "<valid name>", "MetricData": null}. Emulator: 400 + generic constraint message. AWS: 500 + 'Unhandled internal failure'.

7) Cause 8: PUT /BatchPutMetrics with MetricData set to a bare JSON string instead of a list. Emulator: 400, lowercase `message`, x-amzn-errortype ValidationException, no `Message` member. AWS: 400 with capital-M `Message` = '<that submitted value> is not an expected Json type' and a different error type.

8) Cause 9: replay step task#5 (and task#6, task#7) of sagemaker-metrics-batch-put-metrics-structural-rejections and task#4/task#5 of sagemaker-metrics-batch-put-validation-envelope. Emulator: 400 with no Errors member and nothing stored. AWS: 200 with Errors=[] and the points ingested (verify with a follow-up BatchGetMetrics read-back).

9) Cause 10: PUT /BatchPutMetrics with a batch whose point carries a non-finite Value (NaN) plus a valid sibling. Inspect Errors[0].Code: emulator 'VALIDATION_ERROR', AWS 'INTERNAL_ERROR', with MetricIndex naming the offending item and the sibling still ingested. (sagemaker-metrics-batch-put-validation-envelope#task#2)
