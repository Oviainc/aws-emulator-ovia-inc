Issue-Severity: LOW
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: xray.get-service-graph-invalid-time-range-fails xray.get-trace-summaries-end-before-start-fails xray.start-trace-retrieval-unknown-trace-ids-rejected xray.get-trace-summaries-malformed-filter-expression-rejected xray.group-filter-expression-on-metadata-field-rejected xray.update-group-invalid-filter-expression-preserves-prior xray.batch-get-traces-invalid-format-fails xray.put-trace-segments-invalid-trace-id-reports-segment-id xray.batch-get-traces-five-id-limit-boundary xray.create-sampling-rule-malformed-resource-arn-rejected xray.list-retrieved-traces-unknown-token-fails xray.list-retrieved-traces-status-progresses-over-time xray.get-insight-impact-graph-unknown-id-fails
Issue-Shape-Id: com.amazonaws.xray#AWSXRay
Issue-Key: xray.get-service-graph-invalid-time-range-fails|run#5|AssertionError|assert message == "End time cannot be earlier than start time", (
Issue-Key: xray.get-trace-summaries-end-before-start-fails|run#7|AssertionError|assert message == "End time cannot be earlier than start time", (
Issue-Key: xray.start-trace-retrieval-unknown-trace-ids-rejected|run#12|AssertionError|assert message_other == expected_message, (
Issue-Key: xray.get-trace-summaries-malformed-filter-expression-rejected|run#6|AssertionError|assert unbalanced_message == (
Issue-Key: xray|CreateGroup|InvalidRequestException
Issue-Key: xray.batch-get-traces-invalid-format-fails|run#8|AssertionError|assert error.get("Message") == "Invalid trace id not-a-trace-id: incorrect id format", (
Issue-Key: xray.put-trace-segments-invalid-trace-id-reports-segment-id|run#22|AssertionError|assert message == f"Invalid trace id {bad_trace_id}: incorrect id format", (
Issue-Key: xray|BatchGetTraces|InvalidRequestException
Issue-Key: xray|CreateSamplingRule|InvalidRequestException
Issue-Key: xray.list-retrieved-traces-unknown-token-fails|run#21|AssertionError|assert message == (
Issue-Key: xray.list-retrieved-traces-status-progresses-over-time|run#17|AssertionError|assert message == f"Invalid execution id {UNKNOWN_TOKEN} passed: Invalid Execution Id", (
Issue-Key: xray.get-insight-impact-graph-unknown-id-fails|run#12|AssertionError|assert malformed_message == (
Issue-Commit: 01142ac518ef08b9ccb64fede7482782162b2994

# X-Ray refusal messages do not match the service's fixed/templated texts: inverted time range, filter-expression parser location, trace-id format, id-count limit, ResourceARN syntax, retrieval token and insightId pattern (7 causes)

> _Note: this issue was identified against the code base at commit `01142ac518ef08b9ccb64fede7482782162b2994`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetTraceSummaries

## Description
Seven root causes, all of the same KIND: the emulator refuses the right request with the right HTTP status and the right error code, but composes its own sentence where X-Ray's message is produced by a known rule — a fixed service text for the refusal class, or a template filled in from the request. In each cause below the status and code already match; ONLY the message diverges, which is why this is LOW. It is filed because these messages are the rule, not a recorded example: each is either a fixed service string the corpus observes identically across several operations, or a template whose one variable is the offending request value (so a client or test asserting on it is comparing a contract, not an incidental string). Where the rule is not knowable the divergence is NOT filed: PutEncryptionConfig's enum-violation message lists the enum as [NONE, KMS], which is the Smithy model's own member order, so that difference is left alone.

Cause 1 of 7: an inverted time range is refused with a computed sentence instead of the service's fixed text, on three operations.
AWS: the fixed message for EndTime before StartTime is 'End time cannot be earlier than start time' (GetServiceGraph, GetTraceSummaries) and, on StartTraceRetrieval, 'Input parameters invalid for request. End time cannot be earlier than start time'; code InvalidRequestException, HTTP 400. The rule is known because the corpus observes the SAME fixed phrase for the same refusal class across three independent operations, with no request value interpolated.
Emulator: code and status are right, the text is a per-call computation: 'EndTime (1792890624.596426) must be at or after StartTime (1792897824.596426), in seconds of epoch time' (GetServiceGraph, StartTraceRetrieval) and 'EndTime precedes StartTime, so the requested [StartTime, EndTime) time range is inverted' (GetTraceSummaries) — and the emulator's text even embeds raw epoch floats, which differ per run.
Why: a client (or a parity test) that matches the documented sentence to classify the refusal sees an unrecognised message, and the emulator's variant is not even stable across runs because it interpolates timestamps.
Fix: a single shared time-range validator used by these handlers (services/service-com-amazonaws-xray-awsxray/src/ops/get_service_graph.rs, get_trace_summaries.rs, start_trace_retrieval.rs) emitting the service's fixed text.
Traces: xray.get-service-graph-invalid-time-range-fails, xray.get-trace-summaries-end-before-start-fails, xray.start-trace-retrieval-unknown-trace-ids-rejected.

Cause 2 of 7: filter-expression parser refusals do not carry X-Ray's 'Error Location' form.
AWS: a malformed FilterExpression is refused InvalidRequestException/400 with the parser's own template 'Error Location: line <L>, position <P> : Invalid input symbol: '<symbol>'' — e.g. "Error Location: line 1, position 15 : Invalid input symbol: '((service(\"x\")'" for an unbalanced expression, "Error Location: line 1, position 1 : Invalid input symbol: 'met'" for `metadata.probe_key` (metadata is not a token in the language at all, so it fails at position 1), and "Error Location: line 1, position 9 : Invalid input symbol: '\"unclosed'" for an unterminated string in UpdateGroup. The rule is known: the message is a template built from the request, whose variables are the line/position of the failure and the offending symbol.
Emulator: the right code and status with prose that reports no location: 'Invalid filter expression: expected a closing `)`, found the end of the expression'; 'Invalid filter expression: `metadata.probe_key` is not an indexed field'; 'Invalid filter expression: unterminated string literal, expected a closing "'. The `metadata` case also mis-classifies the failure as an unknown indexed FIELD rather than an unknown input symbol.
Why: the line/position pair is the actionable part of the refusal — it is what an editor, a console or an agent uses to point at the character that broke an expression; without it a client cannot locate the error, and the 'not an indexed field' wording tells the client the grammar accepts `metadata.*` as a field when it does not.
Fix: report the failure location and offending symbol from the filter-expression parser and render the service's template, in the shared filter-expression module used by GetTraceSummaries, CreateGroup and UpdateGroup.
Traces: xray.get-trace-summaries-malformed-filter-expression-rejected, xray.group-filter-expression-on-metadata-field-rejected, xray.update-group-invalid-filter-expression-preserves-prior.

Cause 3 of 7: the malformed-trace-id refusal does not use the service's template.
AWS: a trace id that does not match 1-<8 hex>-<24 hex> is refused on the READ path with InvalidRequestException/400 and the message 'Invalid trace id <the id>: incorrect id format' (recorded real-AWS behavior, us-west-2 2026-09-07, for both a non-id string and a well-formed-prefix id with only 20 trailing hex). The rule is known: a template naming the offending value and the format rule.
Emulator: right code and status, different text: 'TraceIds carries `not-a-trace-id`, which is not a trace id of the form 1-<8 hex>-<24 hex>'.
Why: a client matching the service's refusal text to distinguish 'this id is malformed' from the other InvalidRequestException causes of BatchGetTraces does not recognise it.
Fix: emit the service template from the trace-id validator shared by services/service-com-amazonaws-xray-awsxray/src/ops/batch_get_traces.rs (and the same validation on the write path).
Traces: xray.batch-get-traces-invalid-format-fails, xray.put-trace-segments-invalid-trace-id-reports-segment-id (fails on the read-path refusal leg).

Cause 4 of 7: BatchGetTraces' id-count refusal does not name the limit the way the service does.
AWS: more than 5 TraceIds in one BatchGetTraces call is refused InvalidRequestException/400 with the fixed message 'Exceeding maximum query size: 5' (the limit is a contract limit absent from the Smithy model — TraceIdList carries no @length — so the refusal message is the only place it is stated).
Emulator: right code and status, text 'TraceIds carries 6 ids; at most 5 are accepted per call'.
Why: the fixed phrase is the only machine-readable statement of this limit; a client that keys on it to shrink its batch and retry does not match.
Fix: the id-count check in services/service-com-amazonaws-xray-awsxray/src/ops/batch_get_traces.rs.
Traces: xray.batch-get-traces-five-id-limit-boundary.

Cause 5 of 7: CreateSamplingRule's ResourceARN syntax refusal does not name the syntax rule AWS names.
AWS: CreateSamplingRule with SamplingRule.ResourceARN='not-an-arn' is refused InvalidRequestException/400 with 'Invalid resource ARN: not-an-arn. Malformed ARN - doesn't start with 'arn:'' (real-AWS verified us-west-2 2026-09-11; note AWS ACCEPTS concrete non-wildcard ARNs and even the empty string, so the boundary being reported is ARN syntax). The rule is known: a template naming the offending value plus the specific syntax rule broken.
Emulator: right code and status, text 'ResourceARN must be an ARN or `*`, not `not-an-arn`' — which additionally states a rule AWS does not enforce (that only an ARN or `*` is allowed).
Why: the message is what tells a client WHY the value was rejected; the emulator's wording implies a wildcard-or-ARN rule, so a client reading it concludes correctly-accepted values (like a concrete ARN or an empty string) would also be refused.
Fix: the ResourceARN validator in services/service-com-amazonaws-xray-awsxray/src/ops/create_sampling_rule.rs.
Traces: xray.create-sampling-rule-malformed-resource-arn-rejected.

Cause 6 of 7: the unknown-retrieval-token refusal says 'Invalid retrieval token' where the service's template says 'Invalid execution id'.
AWS: ListRetrievedTraces with a RetrievalToken that was never issued is refused InvalidRequestException/400 with 'Invalid execution id <token> passed: Invalid Execution Id'. The rule is known: a fixed template with the request's token interpolated.
Emulator: right code and status, one word different: 'Invalid retrieval token <token> passed: Invalid Execution Id'.
Why: the message is asserted verbatim by clients and parity suites keying on this refusal class; the rest of the template matches exactly, so the single-token divergence is a plain defect in the string rather than a different diagnosis.
Fix: the token-lookup refusal in services/service-com-amazonaws-xray-awsxray/src/ops/list_retrieved_traces.rs.
Traces: xray.list-retrieved-traces-unknown-token-fails, xray.list-retrieved-traces-status-progresses-over-time (fails at the same unknown-token leg).

Cause 7 of 7: the insightId pattern-violation ValidationException quotes an anchored pattern.
AWS: a pattern-invalid InsightId is refused with the standard validation message '1 validation error detected: Value '<value>' at 'insightId' failed to satisfy constraint: Member must satisfy regular expression pattern: [0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[1-5][0-9a-fA-F]{3}-[89abAB][0-9a-fA-F]{3}-[0-9a-fA-F]{12}' — the modeled pattern rendered WITHOUT its ^ and $ anchors. The rule is known: this message is built from the request (member name and offending value) and from the model's pattern trait.
Emulator: the same message but quoting the pattern with the anchors included ('^...$'), so the rendered constraint differs from the one AWS reports.
Why: a client that echoes or re-applies the reported pattern (for client-side pre-validation, or a message comparison) gets a different regular expression than AWS reports for the same member.
Fix: strip the leading ^ / trailing $ when rendering a pattern trait into the ValidationException message, in the shared constraint-violation message builder used before the xray handlers run.
Traces: xray.get-insight-impact-graph-unknown-id-fails.

## Evidence
Cause 1: xray.get-service-graph-invalid-time-range-fails — code InvalidRequestException (as expected), AssertionError: unexpected message: 'EndTime (1792890624.596426) must be at or after StartTime (1792897824.596426), in seconds of epoch time'; expected 'End time cannot be earlier than start time'. xray.get-trace-summaries-end-before-start-fails — got 'EndTime precedes StartTime, so the requested [StartTime, EndTime) time range is inverted'; same expectation. xray.start-trace-retrieval-unknown-trace-ids-rejected — AssertionError: expected message 'Input parameters invalid for request. End time cannot be earlier than start time', got 'EndTime (1792894075.691024) must be at or after StartTime (1792897675.691024), in seconds of epoch time'.
Cause 2: xray.get-trace-summaries-malformed-filter-expression-rejected — expected "Error Location: line 1, position 15 : Invalid input symbol: '((service(\"x\")'", got 'Invalid filter expression: expected a closing `)`, found the end of the expression'. xray.group-filter-expression-on-metadata-field-rejected — expected "Error Location: line 1, position 1 : Invalid input symbol: 'met'", got 'Invalid filter expression: `metadata.probe_key` is not an indexed field'. xray.update-group-invalid-filter-expression-preserves-prior — expected 'Error Location: line 1, position 9 : Invalid input symbol: '\"unclosed'', got 'Invalid filter expression: unterminated string literal, expected a closing "'.
Cause 3: xray.batch-get-traces-invalid-format-fails — got {'Message': 'TraceIds carries `not-a-trace-id`, which is not a trace id of the form 1-<8 hex>-<24 hex>', 'Code': 'InvalidRequestException'}; expected Message 'Invalid trace id not-a-trace-id: incorrect id format'. xray.put-trace-segments-invalid-trace-id-reports-segment-id — got 'TraceIds carries `1-6add71cc-3c58f4ad1a56877a3a77`, which is not a trace id of the form 1-<8 hex>-<24 hex>'; expected 'Invalid trace id 1-6add71cc-3c58f4ad1a56877a3a77: incorrect id format'.
Cause 4: xray.batch-get-traces-five-id-limit-boundary — code InvalidRequestException and HTTP 400 as expected; AssertionError: rejection must name the id-count limit, got 'TraceIds carries 6 ids; at most 5 are accepted per call'; expected 'Exceeding maximum query size: 5'.
Cause 5: xray.create-sampling-rule-malformed-resource-arn-rejected — got 'ResourceARN must be an ARN or `*`, not `not-an-arn`'; expected "Invalid resource ARN: not-an-arn. Malformed ARN - doesn't start with 'arn:'".
Cause 6: xray.list-retrieved-traces-unknown-token-fails — got 'Invalid retrieval token sim2real-behav-not-a-real-token passed: Invalid Execution Id'; expected 'Invalid execution id sim2real-behav-not-a-real-token passed: Invalid Execution Id'. xray.list-retrieved-traces-status-progresses-over-time — got 'Invalid retrieval token sim2real-behav-unknown-retrieval-token-0000000000 passed: Invalid Execution Id' against the same expectation.
Cause 7: xray.get-insight-impact-graph-unknown-id-fails — code ValidationException as expected; message quoted the pattern as '^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[1-5][0-9a-fA-F]{3}-[89abAB][0-9a-fA-F]{3}-[0-9a-fA-F]{12}$' where AWS reports the same pattern unanchored.

## Steps to Reproduce
1. Boot the emulator (run_behavioral_suite builds and boots it); use a us-west-2 xray client. In every step below, confirm the HTTP status and error code already match and compare only the Message.
2. Cause 1: xray.get_service_graph(StartTime=now, EndTime=now-7200); xray.get_trace_summaries(StartTime=now, EndTime=now-7200); xray.start_trace_retrieval(TraceIds=[<well-formed id>], StartTime=now, EndTime=now-3600).
3. Cause 2: xray.get_trace_summaries(StartTime=..., EndTime=..., FilterExpression='((service("x")'); xray.create_group(GroupName=..., FilterExpression='metadata.probe_key = "v"'); xray.update_group(GroupName=<existing>, FilterExpression='service("unclosed') .
4. Cause 3: xray.batch_get_traces(TraceIds=["not-a-trace-id"]) and xray.batch_get_traces(TraceIds=["1-<8hex>-<20hex>"]).
5. Cause 4: xray.batch_get_traces(TraceIds=[six well-formed ids]).
6. Cause 5: xray.create_sampling_rule(SamplingRule={... "ResourceARN": "not-an-arn" ...}).
7. Cause 6: xray.list_retrieved_traces(RetrievalToken="sim2real-behav-not-a-real-token", TraceFormat="XRAY").
8. Cause 7: xray.get_insight_impact_graph(InsightId="11111111-2222-3333-4444-555555555555", StartTime=..., EndTime=...).
9. Re-replay the thirteen check ids in trace_ids to confirm the fixes.
