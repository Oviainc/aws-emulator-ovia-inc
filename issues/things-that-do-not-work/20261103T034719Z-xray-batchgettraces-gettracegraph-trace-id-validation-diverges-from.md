Issue-Source: trace-sweep-divergence
Issue-Trace: xray-trace-id-format-mistakes
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: xray-trace-id-format-mistakes
Issue-Shape-Id: com.amazonaws.xray#AWSXRay
Issue-Key: xray|BatchGetTraces|4xx->4xx|$.message
Issue-Key: xray|BatchGetTraces|4xx->2xx|$.Message
Issue-Key: xray|BatchGetTraces|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: xray|BatchGetTraces|4xx->2xx|status
Issue-Key: xray|BatchGetTraces|4xx->4xx|$.Message
Issue-Key: xray|GetTraceGraph|4xx->4xx|$.Message
Issue-Key: xray|GetTraceGraph|4xx->2xx|$.Message
Issue-Key: xray|GetTraceGraph|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: xray|GetTraceGraph|4xx->2xx|status
Issue-Commit: 80501f644d2883f30c53d819479800a81248de03

# xray BatchGetTraces/GetTraceGraph trace-id validation diverges from AWS (5 causes)

> _Note: this issue was identified against the code base at commit `80501f644d2883f30c53d819479800a81248de03`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchGetTraces

## Description
FIVE distinct net-new root causes in the TraceIds input validation shared by BatchGetTraces (POST /Traces) and GetTraceGraph (POST /TraceGraph). Two are requests AWS refuses that the emulator answers 200; three are refusals where the status is right and the message diverges. Smithy model consulted: services/service-com-amazonaws-xray-awsxray/smithy-model/com-amazonaws-xray-awsxray.json — BatchGetTraces and GetTraceGraph both take a required TraceIds (TraceIdList, member TraceId, @length min 1 max 35) and an optional NextToken, and both declare InvalidRequestException (400, member Message) and ThrottledException (429); there is no ValidationException shape in this model. No open issue names any of these five clusters, and none was marked stub_path or owned_by in the replay report.

Cause 1 of 5: the TraceIds length-constraint message reports the wrong member path and only the violated bound.
AWS: for a trace id longer than the modelled 35 characters the refusal is 400 carrying body $.message (lowercase — the framework validation envelope) = "1 validation error detected: Value '<the whole TraceIds list, rendered [A, B]>' at 'traceIds' failed to satisfy constraint: Member must satisfy constraint: [Member must have length less than or equal to 35, Member must have length greater than or equal to 1]". Fixed text: "1 validation error detected: Value '", "' at '", "' failed to satisfy constraint: Member must satisfy constraint: [Member must have length less than or equal to ", ", Member must have length greater than or equal to ", "]". Variable parts: the rendered LIST value from the request, the member path of the LIST ('traceIds'), and the two bounds from the Smithy @length trait. AWS reports the collection, not the offending element, and always renders both bounds.
Emulator: reports the single element and only the violated bound — "1 validation error detected: Value '<element>' at 'traceIds.1.member' failed to satisfy constraint: Member must have length less than or equal to 35". Status (400) and body key are right; the value, the member path and the constraint clause all diverge.
Why: a client or test asserting AWS's wording never matches, and anything parsing the member path out of the message to point at the bad input is handed an indexed path AWS never emits.
Fix: the collection-constraint renderer in the xray crate's shared input validation — render the list value at the list's own member path and emit both bounds for a member @length violation.
Traces: xray-trace-id-format-mistakes#task#1.

Cause 2 of 5: BatchGetTraces accepts a TraceIds list containing duplicates.
AWS: recorded behavior (the model puts no @uniqueItems on TraceIdList, so this is a server-side rule): a TraceIds list carrying the same id twice is refused 400 with $.Message = "Containing duplicate trace ids" — entirely fixed text, nothing from the request — plus the x-amzn-errortype response header.
Emulator: HTTP 200 with a normal batch body, no Message and no x-amzn-errortype.
Why: a client that accumulates ids from several sources and de-duplicates only because AWS forces it to sees the duplicate batch succeed against the emulator and fail in production; and the Traces/UnprocessedTraceIds accounting for a duplicated id is undefined behavior it is being invited to depend on.
Fix: the BatchGetTraces handler's input validation in the xray crate — refuse a duplicated id before any lookup.
Traces: xray-trace-id-format-mistakes#task#4.

Cause 3 of 5: BatchGetTraces invents NextToken validation where AWS refuses pagination outright.
AWS: NextToken is modelled on BatchGetTracesRequest (and the operation is marked paginated), but recorded behavior is that ANY NextToken is refused 400 with $.Message = "NextToken is currently not supported" — entirely fixed text, the submitted token is not echoed. The modelled pagination is simply not available on this operation.
Emulator: refuses 400 but with a token-provenance message that echoes the token and the request's ids — "NextToken `<token>` was not issued by this service for these TraceIds" — implying a working token scheme whose tokens were simply not recognised.
Why: the two messages mean opposite things. AWS's text tells a client to stop paginating this call; the emulator's tells it its token handling is buggy, so a client retries or re-derives tokens and loops instead of giving up, and a client that successfully obtained a token from the emulator would build a paging loop AWS rejects at the first hop.
Fix: the BatchGetTraces handler in the xray crate — refuse any supplied NextToken with InvalidRequestException and AWS's fixed text, and never issue one.
Traces: xray-trace-id-format-mistakes#task#5.

Cause 4 of 5: GetTraceGraph's malformed-trace-id refusal message diverges from AWS.
AWS: recorded behavior — a trace id that is not in X-Ray's id format is refused 400 with $.Message = "Invalid traceId. ErrorCode: InvalidId", entirely fixed text; the offending id is NOT interpolated.
Emulator: "Invalid trace id <the submitted id>: incorrect id format" — different wording, and it echoes the request's value where AWS does not.
Why: the message is the only discriminator among this operation's 400s (the code is shared), so a client branching on "bad id format" versus "bad window" or "bad token" never matches; AWS's trailing "ErrorCode: InvalidId" in particular is what callers key on.
Fix: the trace-id format guard on the GetTraceGraph handler in the xray crate.
Traces: xray-trace-id-format-mistakes#task#8.

Cause 5 of 5: GetTraceGraph accepts an empty TraceIds list.
AWS: TraceIds is required and TraceIdList carries no list-level @length, so the non-empty rule is server-side. Recorded behavior: an empty list is refused 400 with $.Message = "Zero trace ids provided" — entirely fixed text — plus the x-amzn-errortype header.
Emulator: HTTP 200 with a graph body (no Message, no x-amzn-errortype), i.e. an empty request is reported as a successful empty graph.
Why: an empty id list is almost always an upstream bug (a filter that matched nothing being fed straight into the graph call). AWS surfaces it; the emulator turns it into a silent empty success, so the defect is invisible until production.
Fix: the GetTraceGraph handler's input validation in the xray crate — refuse an empty TraceIds with InvalidRequestException and AWS's fixed text.
Traces: xray-trace-id-format-mistakes#task#9.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true, bootstrap_signal=false): 5 net-new divergence clusters, service=xray, fixture xray-trace-id-format-mistakes, none carrying stub_path or owned_by.

Cause 1 — xray-trace-id-format-mistakes#task#1, BatchGetTraces (status 400 on both sides):
  $.message expected: "1 validation error detected: Value '[Root=1-67f0a1b2-352d9bbbafb679ec28c2cbb4]' at 'traceIds' failed to satisfy constraint: Member must satisfy constraint: [Member must have length less than or equal to 35, Member must have length greater than or equal to 1]"
  $.message actual:   "1 validation error detected: Value 'Root=1-67f0a1b2-352d9bbbafb679ec28c2cbb4' at 'traceIds.1.member' failed to satisfy constraint: Member must have length less than or equal to 35"
  (the id text and the list rendering are the request's value; 'traceIds' and the two bounds 35/1 are the requirement)

Cause 2 — xray-trace-id-format-mistakes#task#4, BatchGetTraces:
  status: expected 400, got 200
  $.Message: expected 'Containing duplicate trace ids', got None
  $headers.x-amzn-errortype: expected (shape-asserted), got None

Cause 3 — xray-trace-id-format-mistakes#task#5, BatchGetTraces (status 400 on both sides):
  $.Message: expected 'NextToken is currently not supported', got 'NextToken `bogus` was not issued by this service for these TraceIds' (`bogus` being the token the step submitted)

Cause 4 — xray-trace-id-format-mistakes#task#8, GetTraceGraph (status 400 on both sides):
  $.Message: expected 'Invalid traceId. ErrorCode: InvalidId', got 'Invalid trace id 1-abc: incorrect id format' ('1-abc' being the id the step submitted)

Cause 5 — xray-trace-id-format-mistakes#task#9, GetTraceGraph:
  status: expected 400, got 200
  $.Message: expected 'Zero trace ids provided', got None
  $headers.x-amzn-errortype: expected (shape-asserted), got None

## Steps to Reproduce
Prerequisite: git_clone, then replay_wire_corpus (it builds and boots the emulator from the checkout).

Cause 1: POST /Traces with TraceIds holding one id longer than 35 characters -> the validation message must render the whole list at 'traceIds' and both @length bounds; currently it names 'traceIds.1.member' and one bound. Replay xray-trace-id-format-mistakes#task#1.
Cause 2: POST /Traces with the same trace id twice -> must be 400 InvalidRequestException, Message 'Containing duplicate trace ids', with an x-amzn-errortype header; currently 200. Replay xray-trace-id-format-mistakes#task#4.
Cause 3: POST /Traces with any NextToken -> must be 400 with Message 'NextToken is currently not supported'; currently a token-provenance message. Replay xray-trace-id-format-mistakes#task#5.
Cause 4: POST /TraceGraph with a trace id that is not in X-Ray id format -> Message must be exactly 'Invalid traceId. ErrorCode: InvalidId'; currently a different, value-interpolating message. Replay xray-trace-id-format-mistakes#task#8.
Cause 5: POST /TraceGraph with TraceIds: [] -> must be 400 with Message 'Zero trace ids provided'; currently 200 with a graph body. Replay xray-trace-id-format-mistakes#task#9.

Re-replay all five bindings (issue_trace/issue_step plus trace_ids) to confirm every cause is closed.
