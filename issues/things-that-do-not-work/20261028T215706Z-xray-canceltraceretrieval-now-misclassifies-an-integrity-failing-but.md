Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: xray.start-trace-retrieval-unknown-destination-not-found
Issue-Shape-Id: com.amazonaws.xray#AWSXRay
Issue-Key: xray.start-trace-retrieval-unknown-destination-not-found|run#13|AssertionError|assert code == "InvalidRequestException", f"unexpected error code: {code}"
Issue-Commit: 6249b35117da18b492ce466a823561b75d5aa9f1

# xray: CancelTraceRetrieval now MISCLASSIFIES an integrity-failing but well-formed RetrievalToken as ResourceNotFoundException (404) instead of InvalidRequestException (400) — the token is looked up instead of integrity-checked

> _Note: this issue was identified against the code base at commit `6249b35117da18b492ce466a823561b75d5aa9f1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CancelTraceRetrieval

## Description
REOPENS 20261028T090715Z-xray-unknown-retrievaltoken-refusals-append-invalid-execution-id.md: its fix left the call refused. That issue is marked fixed and it changed exactly this refusal path on CancelTraceRetrieval; the operation is still refusing this request, and it now refuses it with the WRONG error class, so this is a regression of that fix's classification rather than a brand-new surface. The divergence has moved up the check: the previously filed defect on this same check was message-only (see the OPEN issue 20261028T150031Z-xray-canceltraceretrieval-renders-one-refusal-message-for-two.md, which asserts the status 400 and the code InvalidRequestException both MATCH and only the message text diverges). At the current commit the code itself diverges, so that open issue no longer covers this failure — the replay report marks this check neither ALREADY FILED nor OWNED, and its previous replay key (the message assertion, run#15) is not the key that fails now (the error-code assertion, check line 46). RELATED, NOT COVERING: 20261028T150031Z (message wording of the same class) and 20261022T045628Z-xray-x-ray-account-config-surfaces-are-overly-permissive.md (other account-config refusals).

ONE root cause (Cause 1 of 1). xray is bootstrapped and answering (106/125 xray checks pass in this run, bootstrap_signal false), so this is a fidelity defect, not a bootstrap gap.

Cause 1 of 1: CancelTraceRetrieval resolves a well-formed RetrievalToken by STORE LOOKUP only, with no integrity check, so a token that has the issued form but does not verify is reported as a retrieval that does not exist (404 ResourceNotFoundException) instead of as a malformed parameter (400 InvalidRequestException). Failing check: xray.start-trace-retrieval-unknown-destination-not-found.

AWS: Smithy (services/service-com-amazonaws-xray-awsxray/smithy-model/com-amazonaws-xray-awsxray.json) models CancelTraceRetrieval as POST /CancelTraceRetrieval, input member RetrievalToken (target RetrievalToken, @required, @length min 0 max 1020), output CancelTraceRetrievalResult (empty, HTTP 200), modeled errors InvalidRequestException (HTTP 400, defaulted) 'The request is missing required parameters or has invalid parameters', ResourceNotFoundException (HTTP 404) 'The resource was not found...', ThrottledException (HTTP 429). The model does not state which class a non-verifying token falls in, so the classification is recorded behavior only: the recording shows that the RetrievalToken is INTEGRITY-CHECKED, not looked up — a string of the issued token form (same length, same base64url alphabet) whose embedded execution id does not decode is rejected as an invalid parameter with HTTP 400 / InvalidRequestException and the message 'Invalid retrieval token <RetrievalToken> passed: Invalid execution id provided: <RetrievalToken>'. Fixed text of that message: 'Invalid retrieval token ' and ' passed: Invalid execution id provided: '; the only variable part is the request's RetrievalToken value, substituted TWICE (the inner clause is emitted by the execution-id decoder, naming the value it could not decode). The 404 ResourceNotFoundException class is reserved for a token that DOES verify but names no live retrieval. Hypothesis: AWS encodes the execution id inside the token with a self-check, so a mutated token fails to decode before any job lookup happens.

Emulator: the handler judges the token's FORM and then goes straight to the store. services/service-com-amazonaws-xray-awsxray/src/ops/cancel_trace_retrieval.rs gates on crate::state::is_retrieval_token_form(&input.retrieval_token) (lines 161-176) — a one-character, same-alphabet, same-length mutation of a genuine token passes that gate — and then calls store.cancel_trace_retrieval(...) (lines 178-204), whose Err(None) arm answers ResourceNotFoundException (HTTP 404) with 'RetrievalToken `<token>` was not issued by this service, or its retrieval has timed out'. So the replay gets code ResourceNotFoundException where AWS answers InvalidRequestException; the status diverges with it (404 vs 400) and the message is the not-found text rather than the integrity-failure text. There is no third branch for 'of the issued form but does not verify'.

Why: a client cannot tell a bad request from a missing resource. 400 InvalidRequestException is non-retryable and means 'fix the parameter'; 404 ResourceNotFoundException means 'the job is gone' and is what SDK wrappers, console flows and cleanup helpers treat as benign/idempotent — the check's own helpers swallow ResourceNotFoundException. A corrupted or truncated token therefore reads as a retrieval that already finished and vanished, so callers silently stop cancelling real jobs, stop surfacing the typo to the user, and never see the offending token echoed. Every assertion downstream of the code (the exact message, and that the real job stays uncancelled) is unreachable while the class is wrong.

Fix: the operation's validation in the CancelTraceRetrieval handler plus the shared token predicate it calls — the store lookup is correct for the tokens that reach it. The form gate must be split into (a) 'not of the issued form', (b) 'of the issued form but fails the integrity check' — both HTTP 400 / InvalidRequestException, with the per-class message text — and only (c) a verifying token may reach the store and be eligible for ResourceNotFoundException. The predicate and the refusal-message builder are shared with the sibling token operations (ListRetrievedTraces, GetRetrievedTracesGraph), whose currently-passing class-(a) wording must not regress.

Traces: xray.start-trace-retrieval-unknown-destination-not-found. Code citations: services/service-com-amazonaws-xray-awsxray/src/ops/cancel_trace_retrieval.rs lines 161-176 (form gate) and 178-204 (store resolution, Err(None) -> ResourceNotFoundException); crate::state::is_retrieval_token_form and crate::state::malformed_retrieval_token_message in services/service-com-amazonaws-xray-awsxray/src/state.rs.

## Evidence
Check: xray.start-trace-retrieval-unknown-destination-not-found — verdict mismatch.

The check enables Transaction Search (logs.put_resource_policy granting xray.amazonaws.com logs:PutLogEvents on aws/spans and /aws/application-signals/data, then update_trace_segment_destination(Destination='CloudWatchLogs') polled to ACTIVE), ingests one segment with PutTraceSegments, obtains a GENUINE RetrievalToken from StartTraceRetrieval, then flips the token's MIDDLE character within the base64url alphabet (length and charset preserved, so only an integrity check can reject it) and calls CancelTraceRetrieval with it.

Error code: DIVERGES — expected InvalidRequestException, got ResourceNotFoundException.
HTTP status: diverges with it — AWS 400, emulator 404.
Error message: also diverges (unreachable in this run, the code assertion fires first). With T = the mutated token taken from the request, AWS: "Invalid retrieval token T passed: Invalid execution id provided: T" (fixed text 'Invalid retrieval token ' + ' passed: Invalid execution id provided: ', T substituted twice). Emulator renders its not-found text instead: "RetrievalToken `T` was not issued by this service, or its retrieval has timed out" (cancel_trace_retrieval.rs lines 192-203).

Verbatim replay assertion:
  File "run.py", line 189, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
  File "check_xray_start_trace_retrieval_unknown_destination_not_found.py", line 46, in run
    assert code == "InvalidRequestException", f"unexpected error code: {code}"
  AssertionError: unexpected error code: ResourceNotFoundException

That the check reached line 46 at all shows the rest of the setup path now works: PutResourcePolicy on logs, the destination switch to CloudWatchLogs/ACTIVE, PutTraceSegments and StartTraceRetrieval all succeeded and a genuine RetrievalToken was issued. The defect is confined to how CancelTraceRetrieval classifies a token that has the issued form but does not verify.

History on this same check: 20261027T052518Z-xray-canceltraceretrieval-s-invalidrequestexception-message-for-a-malformed.md (fixed) and 20261028T090715Z-xray-unknown-retrievaltoken-refusals-append-invalid-execution-id.md (fixed) both reshaped this refusal; the open 20261028T150031Z-xray-canceltraceretrieval-renders-one-refusal-message-for-two.md records that at its commit the code was InvalidRequestException and only the message diverged. At this commit the code is ResourceNotFoundException, i.e. the classification itself regressed, so the open issue's replay key (the message assertion) is not the one failing now.

Regression guard that must keep passing: CancelTraceRetrieval with a string that is not of the issued form (e.g. 'sim2real-behav-not-a-real-token') must stay 400 / InvalidRequestException, and ListRetrievedTraces / GetRetrievedTracesGraph must keep their current class-(a) wording — xray.cancel-trace-retrieval-unknown-token-fails, xray.list-retrieved-traces-unknown-token-fails and xray.get-retrieved-traces-graph-unknown-token-fails pass in this run.

## Steps to Reproduce
1. git_clone, then run_behavioral_suite (it builds and boots the emulator); confirm bootstrap_signal is false and xray answers (106/125 xray checks pass).
2. Enable Transaction Search so StartTraceRetrieval will accept a trace id:
   a. logs.put_resource_policy(policyName='sim2real-xray-transaction-search-policy', policyDocument=<Allow principal Service xray.amazonaws.com, Action logs:PutLogEvents, Resource arn:aws:logs:<region>:<account>:log-group:aws/spans:* and .../aws/application-signals/data:*>).
   b. xray.update_trace_segment_destination(Destination='CloudWatchLogs'), then poll xray.get_trace_segment_destination() until Status == 'ACTIVE'.
3. Ingest one segment: xray.put_trace_segments(TraceSegmentDocuments=[json.dumps({'name':'sim2real-probe-seed','id':<16 hex>,'trace_id':'1-<8 hex epoch>-<24 hex>','start_time': now-5, 'end_time': now})]).
4. token = xray.start_trace_retrieval(TraceIds=[<that trace_id>], StartTime=now-10min, EndTime=now)['RetrievalToken'] (retry while it answers ResourceNotFoundException).
5. Mutate exactly ONE character in place, preserving length and the base64url alphabet: mid = len(token)//2; mutated = token[:mid] + <any other base64url char> + token[mid+1:].
6. Call xray.cancel_trace_retrieval(RetrievalToken=mutated) and capture the ClientError.
7. Observe Code == 'ResourceNotFoundException' (HTTP 404) with the message 'RetrievalToken `<mutated>` was not issued by this service, or its retrieval has timed out'. AWS answers HTTP 400 / Code 'InvalidRequestException' with 'Invalid retrieval token <mutated> passed: Invalid execution id provided: <mutated>' — the token is integrity-checked before any lookup, so a non-verifying token of the issued form is an invalid parameter, never a missing resource.
8. Confirm the rest of the path is unaffected once the class is right: xray.list_retrieved_traces(RetrievalToken=token, TraceFormat='XRAY')['RetrievalStatus'] != 'CANCELLED', and xray.cancel_trace_retrieval(RetrievalToken=token) still succeeds.
9. Regression guard: xray.cancel_trace_retrieval(RetrievalToken='sim2real-behav-not-a-real-token') must still answer 400 / InvalidRequestException with the token substituted once, and ListRetrievedTraces / GetRetrievedTracesGraph must keep their current unknown-token wording.
10. Re-replay xray.start-trace-retrieval-unknown-destination-not-found to confirm the fix.
