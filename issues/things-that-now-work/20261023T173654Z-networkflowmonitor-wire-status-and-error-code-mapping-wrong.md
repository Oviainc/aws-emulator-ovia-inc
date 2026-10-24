Issue-Source: trace-sweep-divergence
Issue-Trace: networkflowmonitor-error-paths
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: networkflowmonitor-error-paths networkflowmonitor-tag-and-untag-resource-round-trip
Issue-Shape-Id: com.amazonaws.networkflowmonitor#NetworkFlowMonitor
Issue-Key: networkflowmonitor|GetScope|4xx->4xx|$.message
Issue-Key: networkflowmonitor|GetScope|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: networkflowmonitor|GetScope|4xx->4xx|status
Issue-Key: networkflowmonitor|CreateMonitor|4xx->4xx|$.message
Issue-Key: networkflowmonitor|CreateMonitor|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: networkflowmonitor|CreateMonitor|4xx->4xx|status
Issue-Key: networkflowmonitor|TagResource|2xx->2xx|status
Issue-Key: networkflowmonitor|UntagResource|2xx->2xx|status
Issue-Commit: a91eb31cf888327ed8ad916c03309e3893f84f70

# NetworkFlowMonitor wire status and error-code mapping wrong for scope validation, missing-scope CreateMonitor and tagging

> _Note: this issue was identified against the code base at commit `a91eb31cf888327ed8ad916c03309e3893f84f70`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetScope

## Description
Four distinct root causes in service networkflowmonitor, all replayed from the wire corpus simparity-wire-aws-v1. In each one the emulator answers with the WRONG HTTP STATUS (and in two of them the wrong error code too), so the divergence is on the wire contract, not on message text. None of these were marked stub_path or owned_by in the replay report. Model citations are from services/service-com-amazonaws-networkflowmonitor-networkflowmonitor/smithy-model/com-amazonaws-networkflowmonitor-networkflowmonitor.json. NOTE: the two open networkflowmonitor issues (20261022T213607Z-networkflowmonitor-control-plane-updatescope-answers-501-updatemonitor-skips.md and 20261023T132415Z-networkflowmonitor-query-monitor-plane-stopquery-rejects-terminal-queries.md) are behavioral-corpus issues; neither covers any of the four status/code faults below and neither carries a wire replay key.

=== Cause 1 of 4: GetScope treats a syntactically invalid scope id as not-found (404 ResourceNotFoundException) instead of invalid (400 ValidationException) ===
AWS: Smithy: GetScope is GET /scopes/{scopeId}, input member scopeId is @httpLabel @required targeting ScopeId (@length min 1); its modeled errors include ValidationException (httpError 400) and ResourceNotFoundException (httpError 404). Recorded behavior: a scopeId that is not a well-formed scope identifier (AWS scope ids are UUIDs; the fixture passes an identifier of another form) is refused as a REQUEST-SHAPE error, HTTP 400 with x-amzn-errortype ValidationException, message example 'The provided Scope ARN is not valid' — i.e. format validation runs BEFORE any existence lookup, so the service never reaches the 404 path.
Emulator: the identifier is passed straight to the scope lookup and the miss is reported as absence: HTTP 404 with x-amzn-errortype ResourceNotFoundException, message example 'scope sc-tor-b66da349 was not found'.
Why: a client cannot distinguish a malformed scope id (its own bug, never retryable, fix the request) from a scope that genuinely does not exist (a race or a deleted scope, which a reconciling client retries or recreates). Both the status class and the SDK exception type a caller catches are wrong, so AccessDenied/Validation/NotFound branching and any 4xx-retry policy keyed on the error type behave differently against the emulator.
Emulator component to fix: GetScope's input validation in its handler (src/ops/get_scope.rs) — validate the scopeId's form and raise the modeled ValidationException (400) before consulting scope state in src/state.rs.
Traces: networkflowmonitor-error-paths#task#3.

=== Cause 2 of 4: CreateMonitor with a scopeArn that names no scope answers 400 ValidationException instead of 403 AccessDeniedException ===
AWS: Smithy: CreateMonitor is POST /monitors with @required scopeArn (target Arn, @length 20..2048, pattern ^arn:.*$). Its modeled error set is AccessDeniedException (403), ConflictException (409), InternalServerException (500), ServiceQuotaExceededException (402), ThrottlingException (429), ValidationException (400) — it deliberately does NOT model ResourceNotFoundException. Recorded behavior: a well-formed scope ARN that names no scope the caller can see is refused with HTTP 403 and x-amzn-errortype AccessDeniedException, message example "scope supplied doesn't exist". Hypothesis: AWS collapses unknown-scope into authorization failure precisely because the operation models no not-found error, so an unauthorizable scope reference cannot be reported as 400/404.
Emulator: the same request is refused with HTTP 400 and x-amzn-errortype ValidationException, message example 'scopeArn: "arn:aws:networkflowmonitor:<region>:<account>:scope/<scopeId>" does not name a scope'.
Why: the status class itself is wrong (4xx client-input vs 403 authorization). A client that treats 403 as 'ask the operator for permissions / use a different scope' and 400 as 'my request body is malformed' takes the opposite branch, and SDK error handlers catching AccessDeniedException never fire.
Emulator component to fix: CreateMonitor's handler and its scope resolution (src/ops/create_monitor.rs) — when the referenced scopeArn resolves to no scope, raise the modeled AccessDeniedException (403) rather than a ValidationException.
Traces: networkflowmonitor-error-paths#task#4.

=== Cause 3 of 4: TagResource returns HTTP 200 instead of 204 for its empty output ===
AWS: Smithy: TagResource is POST /tags/{resourceArn}, @idempotent, output shape TagResourceOutput has NO members; the @http trait carries no explicit code (the tool reports code 200 with codeDefaulted=true), so the model does not pin the success status and the recording is the authority. Recorded behavior only: a successful tag application answers HTTP 204 with no body.
Emulator: the same successful call answers HTTP 200 (empty body).
Why: the success status is part of the wire contract. Clients, proxies and test harnesses that assert 204 / 'no content' on a bodiless success — or that treat 200 as implying a parseable entity — disagree with the emulator on every tag write, and any recorded-response comparison or contract test fails even though the tagging itself worked.
Emulator component to fix: the response construction/serializer for TagResource (src/ops/tag_resource.rs) — emit 204 No Content for the empty output shape.
Traces: networkflowmonitor-tag-and-untag-resource-round-trip#task#4.

=== Cause 4 of 4: UntagResource returns HTTP 200 instead of 204 for its empty output ===
AWS: Smithy: UntagResource is DELETE /tags/{resourceArn} with tagKeys bound to the tagKeys query string, @idempotent, output shape UntagResourceOutput has NO members and the @http code is defaulted (not pinned by the model). Recorded behavior only: a successful untag answers HTTP 204 with no body.
Emulator: answers HTTP 200 (empty body). Same defect as cause 3 on the sibling operation, so the fault is in how this service renders bodiless success responses rather than in one handler.
Why: as cause 3 — the success status differs on every tag removal, so status-asserting clients and contract tests diverge.
Emulator component to fix: the response construction/serializer for UntagResource (src/ops/untag_resource.rs), ideally the shared empty-output response helper used by both tagging operations.
Traces: networkflowmonitor-tag-and-untag-resource-round-trip#task#6.

## Evidence
Replay of trace set simparity-wire-aws-v1: 14 steps, 6 passed, 8 mismatched (score 42.9%); emulator_up=true, bootstrap_signal=false, state_reset_failures=0, so these are genuine per-step divergences against a serving emulator.

Cause 1 — fixture networkflowmonitor-error-paths, step task#3, operation GetScope
  divergence_key: service=networkflowmonitor|GetScope|status=400->404|keys=$.message,$headers.x-amzn-errortype,status
  expected: HTTP 400, x-amzn-errortype: ValidationException, message 'The provided Scope ARN is not valid'
  actual:   HTTP 404, x-amzn-errortype: ResourceNotFoundException, message 'scope sc-tor-b66da349 was not found'
  (The recorded message also carries SDK-appended 'Service/Status Code/Request ID/SDK Attempt Count' text, which is client-side decoration, not a service requirement.)

Cause 2 — fixture networkflowmonitor-error-paths, step task#4, operation CreateMonitor
  divergence_key: service=networkflowmonitor|CreateMonitor|status=403->400|keys=$.message,$headers.x-amzn-errortype,status
  expected: HTTP 403, x-amzn-errortype: AccessDeniedException, message "scope supplied doesn't exist"
  actual:   HTTP 400, x-amzn-errortype: ValidationException, message 'scopeArn: "arn:aws:networkflowmonitor:us-west-2:000000000000:scope/sc-tor-b66da349" does not name a scope'

Cause 3 — fixture networkflowmonitor-tag-and-untag-resource-round-trip, step task#4, operation TagResource
  divergence_key: service=networkflowmonitor|TagResource|status=204->200|keys=status
  detail: status: expected 204, got 200 (body empty in both; the ONLY divergence is the status)

Cause 4 — fixture networkflowmonitor-tag-and-untag-resource-round-trip, step task#6, operation UntagResource
  divergence_key: service=networkflowmonitor|UntagResource|status=204->200|keys=status
  detail: status: expected 204, got 200 (body empty in both; the ONLY divergence is the status)

## Steps to Reproduce
Boot the emulator and point a boto3 `networkflowmonitor` client at it.

1. Cause 1: call get_scope(scopeId="sc-tor-b66da349") — any identifier that is not a well-formed scope id. Observe HTTP 404 / ResourceNotFoundException; AWS answers HTTP 400 / ValidationException because the identifier fails format validation before any lookup.
2. Cause 2: call create_monitor(monitorName=<fresh name>, localResources=[{"type": "AWS::Region", "identifier": <region>}], scopeArn="arn:aws:networkflowmonitor:<region>:<account>:scope/sc-tor-b66da349") with a well-formed ARN naming a scope that does not exist. Observe HTTP 400 / ValidationException; AWS answers HTTP 403 / AccessDeniedException (CreateMonitor models no ResourceNotFoundException at all).
3. Cause 3: CreateScope (one ACCOUNT target in the caller's account/Region), wait for SUCCEEDED, then call tag_resource(resourceArn=<scopeArn>, tags={"k": "v"}) and inspect ResponseMetadata.HTTPStatusCode — observe 200 where AWS returns 204 with no body.
4. Cause 4: on the same ARN call untag_resource(resourceArn=<scopeArn>, tagKeys=["k"]) and inspect ResponseMetadata.HTTPStatusCode — observe 200 where AWS returns 204 with no body.
5. Re-replay to confirm the fix: fixture networkflowmonitor-error-paths steps task#3 and task#4, and fixture networkflowmonitor-tag-and-untag-resource-round-trip steps task#4 and task#6.
