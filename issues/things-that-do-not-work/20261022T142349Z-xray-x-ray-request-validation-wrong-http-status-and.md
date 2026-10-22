Issue-Source: trace-sweep-divergence
Issue-Trace: xray-get-insight-summaries-window
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: xray-get-insight-summaries-window xray-insights-investigation xray-indexing-rule-configuration xray-long-term-trace-retrieval
Issue-Shape-Id: com.amazonaws.xray#AWSXRay
Issue-Key: xray|GetInsightSummaries|4xx->2xx|$.Message
Issue-Key: xray|GetInsightSummaries|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: xray|GetInsightSummaries|4xx->2xx|status
Issue-Key: xray|GetTimeSeriesServiceStatistics|2xx->4xx|$.ContainsOldGroupVersions
Issue-Key: xray|GetTimeSeriesServiceStatistics|2xx->4xx|$.TimeSeriesServiceStatistics
Issue-Key: xray|GetTimeSeriesServiceStatistics|2xx->4xx|status
Issue-Key: xray|UpdateIndexingRule|4xx->4xx|$.Message
Issue-Key: xray|UpdateIndexingRule|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: xray|UpdateIndexingRule|4xx->4xx|status
Issue-Key: xray|ListRetrievedTraces|4xx->4xx|$.Message
Issue-Key: xray|ListRetrievedTraces|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: xray|ListRetrievedTraces|4xx->4xx|status
Issue-Key: xray|GetRetrievedTracesGraph|4xx->4xx|$.Message
Issue-Key: xray|GetRetrievedTracesGraph|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: xray|GetRetrievedTracesGraph|4xx->4xx|status
Issue-Key: xray|CancelTraceRetrieval|4xx->4xx|$.Message
Issue-Key: xray|CancelTraceRetrieval|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: xray|CancelTraceRetrieval|4xx->4xx|status
Issue-Key: xray|GetInsightSummaries|4xx->4xx|$.message
Issue-Commit: c73ebf67a23d923b166243bec836d38257c16c68

# X-Ray request validation: wrong HTTP status and error type on invalid requests, and a valid query refused

> _Note: this issue was identified against the code base at commit `c73ebf67a23d923b166243bec836d38257c16c68`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetInsightSummaries

## Description
Five distinct root causes in which an X-Ray operation of services/service-com-amazonaws-xray-awsxray answers the wrong HTTP status and/or the wrong modeled error type, all from wire fixture replay (simparity-wire-aws-v1, state_reset_failures=0). None is marked ALREADY FILED or OWNED in the replay report; the open issue 20261022T045628Z-xray-x-ray-account-config-surfaces-are-overly-permissive.md concerns other operations and carries none of these replay keys.

Cause 1 of 5: GetInsightSummaries accepts a request that names neither GroupName nor GroupARN.
AWS: insight summaries are always scoped to a group, so a request carrying only the time window is refused with InvalidRequestException at HTTP 400 (x-amzn-errortype: InvalidRequestException), message example "Provide either GroupName or GroupARN" — the rule is that exactly one group selector is required. Smithy: GetInsightSummaries input has GroupARN (@length 1..400) and GroupName (@length 1..32), neither @required individually, StartTime/EndTime @required, and models InvalidRequestException (client, 400) / ThrottledException (429).
Emulator: answers HTTP 200 with an insight-summaries result and no Message or x-amzn-errortype header.
Why: a client that omits the selector (or computes it to empty) gets a successful, account-wide-looking answer instead of the rejection, so a scoping bug in the caller is never surfaced and the result set it acts on is meaningless.
Fix: the GetInsightSummaries handler's input validation — require exactly one of GroupName/GroupARN before querying.
Traces: xray-get-insight-summaries-window#task#3.

Cause 2 of 5: GetTimeSeriesServiceStatistics refuses a valid request with HTTP 400.
AWS: a well-formed GetTimeSeriesServiceStatistics call over a group and a valid window answers HTTP 200 with TimeSeriesServiceStatistics [] and ContainsOldGroupVersions false when no statistics exist for the window — an empty window is an empty result, not an error. Smithy: GetTimeSeriesServiceStatisticsResult.TimeSeriesServiceStatistics (list) and ContainsOldGroupVersions (boolean, default false); errors are InvalidRequestException (400) / ThrottledException (429).
Emulator: answers HTTP 400 with no TimeSeriesServiceStatistics and no ContainsOldGroupVersions for the request the recording shows AWS accepting. Hypothesis: the handler rejects the request's group/window combination (or a member it treats as mandatory) that AWS admits.
Why: the operation is unusable for the normal "no data yet" case — a dashboard or poller that charts a service's statistics gets a hard client error instead of an empty series, and cannot distinguish a genuinely bad request from an empty window.
Fix: the GetTimeSeriesServiceStatistics handler's input validation and empty-result path — accept the modeled request and serialize an empty list with ContainsOldGroupVersions false.
Traces: xray-insights-investigation#task#3.

Cause 3 of 5: UpdateIndexingRule reports a non-default rule name as ResourceNotFoundException/404 instead of InvalidRequestException/400.
AWS: only the account's default indexing rule may be updated, so UpdateIndexingRule with any other Name is refused with InvalidRequestException at HTTP 400 (x-amzn-errortype: InvalidRequestException), message example "Invalid update request as only default rule updates are allowed" — the rule is that the restriction is reported as an invalid request, not as a missing resource. Smithy: UpdateIndexingRule(Name @required, Rule @required) models both InvalidRequestException (client, 400) and ResourceNotFoundException (client, 404), so the two are distinguishable and the choice is wire-visible.
Emulator: answers HTTP 404 with x-amzn-errortype ResourceNotFoundException (message example "no indexing rule named `...` exists in this account").
Why: a client branching on the error type retries or creates a rule it believes is merely missing, when in fact no such rule can ever be created; 404-vs-400 also flips SDK retry/translation behaviour and error metrics.
Fix: the UpdateIndexingRule handler's name check — reject a non-default Name with the operation's InvalidRequestException before the store lookup.
Traces: xray-indexing-rule-configuration#task#2.

Cause 4 of 5: the long-term trace-retrieval reads short-circuit to ResourceNotFoundException/404 instead of validating the request first with InvalidRequestException/400.
AWS: ListRetrievedTraces, GetRetrievedTracesGraph and CancelTraceRetrieval validate the request before resolving the retrieval token, so a call with a missing/invalid required member or a malformed token is refused with InvalidRequestException at HTTP 400 (x-amzn-errortype: InvalidRequestException); recorded message examples "TraceFormat cannot be empty. Valid values are: XRAY, OTEL" (ListRetrievedTraces), "Invalid execution id <token> passed: Invalid Execution Id" (GetRetrievedTracesGraph) and "Invalid retrieval token <token> passed: Invalid Execution Id" (CancelTraceRetrieval). Smithy: all three model InvalidRequestException (400) and ResourceNotFoundException (404) and take RetrievalToken @required (@length 0..1020); ListRetrievedTraces also takes TraceFormat -> TraceFormatType enum.
Emulator: all three answer HTTP 404 with x-amzn-errortype ResourceNotFoundException (message example "RetrievalToken `...` was not issued by this service, or its retrieval has timed out"), i.e. the token lookup runs before — and instead of — request validation.
Why: a client cannot tell a malformed request from an expired/unknown retrieval, which is exactly the distinction the long-term retrieval workflow polls on: it treats a fixable 400 (wrong TraceFormat, bad token syntax) as a terminal "retrieval gone" and abandons the job, or retries a request that can never succeed.
Fix: input validation ordering in the retrieval read handlers (ListRetrievedTraces / GetRetrievedTracesGraph / CancelTraceRetrieval) — validate required members and token syntax, raising InvalidRequestException, before the retrieval-store lookup that raises ResourceNotFoundException.
Traces: xray-long-term-trace-retrieval#task#2 (ListRetrievedTraces), #task#3 (GetRetrievedTracesGraph), #task#4 (CancelTraceRetrieval).

Cause 5 of 5 (LOW): the constraint-violation message does not render the rejected value for a list member.
AWS: a modeled constraint violation is reported in the standard, request-derived format "1 validation error detected: Value '<the submitted value>' at '<member>' failed to satisfy constraint: <constraint>" — the value is rendered, e.g. Value '[ACTIVE, CLOSED]' at 'states' failed to satisfy constraint: Member must have length less than or equal to 1. Smithy: GetInsightSummaries.States -> InsightStateList @length min 0 max 1. HTTP status 400 and the error code matched.
Emulator: emits the same format but substitutes a count for the value: "Value '2 items' at 'states' failed to satisfy constraint: Member must have length less than or equal to 1".
Why: the message's whole purpose is to name what was rejected; a caller (or operator reading logs) cannot see which states were submitted, and tooling that parses the standard validation format out of the message extracts a placeholder.
Fix: the shared constraint-violation message builder used by X-Ray input validation — render the submitted collection value, not its cardinality.
Traces: xray-get-insight-summaries-window#task#4.

## Evidence
Cause 1: xray-get-insight-summaries-window#task#3 GetInsightSummaries — `status: expected 400, got 200`; `$.Message: missing (expected 'Provide either GroupName or GroupARN', got None)`; `$headers.x-amzn-errortype: missing (expected '<shaped-not-asserted>', got None)`.
Cause 2: xray-insights-investigation#task#3 GetTimeSeriesServiceStatistics — `status: expected 200, got 400`; `$.ContainsOldGroupVersions: missing (expected False, got None)`; `$.TimeSeriesServiceStatistics: missing (expected [], got None)`.
Cause 3: xray-indexing-rule-configuration#task#2 UpdateIndexingRule — `status: expected 400, got 404`; `$headers.x-amzn-errortype: value_mismatch (expected 'InvalidRequestException', got 'ResourceNotFoundException')`; message example expected 'Invalid update request as only default rule updates are allowed', got 'no indexing rule named `sim2real-probe-xray-ir-1aa872ad` exists in this account'.
Cause 4: xray-long-term-trace-retrieval#task#2/#task#3/#task#4 — each `status: expected 400, got 404` with `$headers.x-amzn-errortype: value_mismatch (expected 'InvalidRequestException', got 'ResourceNotFoundException')`; expected message examples 'TraceFormat cannot be empty. Valid values are: XRAY, OTEL', 'Invalid execution id 59adda79-... passed: Invalid Execution Id', 'Invalid retrieval token 59adda79-... passed: Invalid Execution Id' — all three answered 'RetrievalToken `59adda79-...` was not issued by this service, or its retrieval has timed out'.
Cause 5: xray-get-insight-summaries-window#task#4 GetInsightSummaries, 400 vs 400 — `$.message: value_mismatch (expected "1 validation error detected: Value '[ACTIVE, CLOSED]' at 'states' failed to satisfy constraint: Member must have length less than or equal to 1", got "1 validation error detected: Value '2 items' at 'states' ...")`.

## Steps to Reproduce
1. git_clone, then replay_wire_corpus (simparity-wire-aws-v1); emulator_up is true and bootstrap_signal false.
2. Cause 1: POST /InsightSummaries with StartTime/EndTime only (no GroupName, no GroupARN) — expect 400 with x-amzn-errortype InvalidRequestException; observe 200. Fixture xray-get-insight-summaries-window step task#3.
3. Cause 2: POST /TimeSeriesServiceStatistics for an existing group over a window with no data — expect 200 with TimeSeriesServiceStatistics [] and ContainsOldGroupVersions false; observe 400. Fixture xray-insights-investigation step task#3.
4. Cause 3: UpdateIndexingRule(Name='<any non-default rule name>', Rule={...}) — expect 400 / InvalidRequestException; observe 404 / ResourceNotFoundException. Fixture xray-indexing-rule-configuration step task#2.
5. Cause 4: with a retrieval token that was never issued, call ListRetrievedTraces (omitting TraceFormat), GetRetrievedTracesGraph and CancelTraceRetrieval — expect 400 / InvalidRequestException from each; observe 404 / ResourceNotFoundException. Fixture xray-long-term-trace-retrieval steps task#2, task#3, task#4.
6. Cause 5: GetInsightSummaries with States=['ACTIVE','CLOSED'] (over the modeled max length of 1) — read the 400 response's message and confirm the rejected list value is rendered. Fixture xray-get-insight-summaries-window step task#4.
7. Re-replay the fixtures/steps in Issue-Trace/Issue-Step and trace_ids to confirm.
