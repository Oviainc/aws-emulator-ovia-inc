Issue-Severity: LOW
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: xray.cancel-trace-retrieval-unknown-token-fails xray.get-retrieved-traces-graph-unknown-token-fails xray.list-retrieved-traces-unknown-token-fails xray.list-retrieved-traces-status-progresses-over-time
Issue-Shape-Id: com.amazonaws.xray#AWSXRay
Issue-Key: xray.cancel-trace-retrieval-unknown-token-fails|run#21|AssertionError|assert error.get("Message") == (
Issue-Key: xray.get-retrieved-traces-graph-unknown-token-fails|run#17|AssertionError|assert message == (
Issue-Key: xray.list-retrieved-traces-unknown-token-fails|run#21|AssertionError|assert message == (
Issue-Key: xray.list-retrieved-traces-status-progresses-over-time|run#17|AssertionError|assert message == f"Invalid execution id {UNKNOWN_TOKEN} passed: Invalid Execution Id", (
Issue-Commit: f6760c7a32941ed55445a6cfba7b22a816e41414

# X-Ray unknown-RetrievalToken refusal message appends a re-templated detail instead of the service's fixed 'Invalid Execution Id' text (CancelTraceRetrieval, ListRetrievedTraces, GetRetrievedTracesGraph)

> _Note: this issue was identified against the code base at commit `f6760c7a32941ed55445a6cfba7b22a816e41414`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListRetrievedTraces

## Description
REOPENS 20261025T032230Z-xray-x-ray-refusal-messages-do-not-match-the.md: its fix left the call refused with a message that still diverges from AWS's. That issue's Cause 6 named exactly this refusal and is marked fixed; the fix corrected the leading clause ('Invalid retrieval token ...' -> 'Invalid execution id ...' on the ListRetrievedTraces path) but the trailing fixed text of the same sentence is now wrong, so the same four checks still fail on the same Message. This is not described as a new defect: it is the same message defect, still open.

Cause 1 of 1: the detail clause after 'passed: ' is re-templated from the request instead of being the service's fixed text.
AWS: a RetrievalToken that this service never issued is refused with error code InvalidRequestException at HTTP 400 (Smithy: services/service-com-amazonaws-xray-awsxray/smithy-model/com-amazonaws-xray-awsxray.json — CancelTraceRetrieval, ListRetrievedTraces and GetRetrievedTracesGraph each model InvalidRequestException with httpError 400, member Message; RetrievalToken is @required, @length 0..1020). The Message is a template with exactly ONE variable, the RetrievalToken taken from the request, followed by a FIXED detail string:
  - ListRetrievedTraces / GetRetrievedTracesGraph: 'Invalid execution id <RetrievalToken> passed: Invalid Execution Id'
  - CancelTraceRetrieval: 'Invalid retrieval token <RetrievalToken> passed: Invalid Execution Id'
Fixed text, per operation: 'Invalid execution id ' / 'Invalid retrieval token ' as the leading clause, and ' passed: Invalid Execution Id' as the trailing clause — note the detail is the capitalised phrase 'Invalid Execution Id' and interpolates NOTHING. Variable part: the RetrievalToken value from the request, and nothing else. The model documents no message text, so the texts above are recorded behavior (the corpus asserts them identically across three independent operations, which is why they are a rule and not one recording's value).
Emulator: the status (400) and the code (InvalidRequestException) are correct on all three operations, and the leading clause is now correct per operation, but the trailing detail is replaced by a second, request-interpolated sentence: 'Invalid execution id <token> passed: Invalid execution id provided: <token>' (ListRetrievedTraces, GetRetrievedTracesGraph) and 'Invalid retrieval token <token> passed: Invalid execution id provided: <token>' (CancelTraceRetrieval). The token is therefore echoed twice and the fixed phrase 'Invalid Execution Id' never appears.
Why: this message is the only machine-readable discriminator between the several InvalidRequestException causes of these operations (bad TraceFormat, un-issued NextToken, token-form violation, retrieval-no-longer-ongoing). A client or parity suite that matches the service's sentence to classify 'this retrieval token was never issued' and re-issue StartTraceRetrieval does not recognise the emulator's text, and the duplicated interpolation means the emulator's sentence grows with the token rather than ending in the service's fixed phrase.
Fix: the shared refusal-message builder crate::state::malformed_retrieval_token_message(label, token) must render '<label-cased leading clause> <token> passed: Invalid Execution Id' with the detail as fixed text; it is called from services/service-com-amazonaws-xray-awsxray/src/ops/cancel_trace_retrieval.rs:169 (label 'retrieval token'), services/service-com-amazonaws-xray-awsxray/src/ops/list_retrieved_traces.rs:216 (label 'execution id') and the same token-form guard in services/service-com-amazonaws-xray-awsxray/src/ops/get_retrieved_traces_graph.rs. Fix it in that one builder in services/service-com-amazonaws-xray-awsxray/src/state.rs so the three handlers cannot disagree.
Traces: xray.cancel-trace-retrieval-unknown-token-fails, xray.get-retrieved-traces-graph-unknown-token-fails, xray.list-retrieved-traces-unknown-token-fails, xray.list-retrieved-traces-status-progresses-over-time.

## Evidence
All four checks reach the refusal leg with the correct code and status and fail ONLY on the Message.

xray.cancel-trace-retrieval-unknown-token-fails [mismatch] — AssertionError: the refusal must name the rejected token, got {'Message': 'Invalid retrieval token sim2real-behav-not-a-real-token passed: Invalid execution id provided: sim2real-behav-not-a-real-token', 'Code': 'InvalidRequestException'}; expected Message 'Invalid retrieval token sim2real-behav-not-a-real-token passed: Invalid Execution Id'. The check first proves the operation is reachable: it cancels a real, just-issued token and observes RetrievalStatus CANCELLED.

xray.get-retrieved-traces-graph-unknown-token-fails [mismatch] — AssertionError: unexpected message: 'Invalid execution id sim2real-behav-not-a-real-token passed: Invalid execution id provided: sim2real-behav-not-a-real-token'; expected 'Invalid execution id sim2real-behav-not-a-real-token passed: Invalid Execution Id'. Code InvalidRequestException as expected; the positive control (a genuinely issued token) was accepted first.

xray.list-retrieved-traces-unknown-token-fails [mismatch] — AssertionError: unexpected message: 'Invalid execution id sim2real-behav-not-a-real-token passed: Invalid execution id provided: sim2real-behav-not-a-real-token'; same expectation; code InvalidRequestException as expected.

xray.list-retrieved-traces-status-progresses-over-time [mismatch] — AssertionError: MESSAGE='Invalid execution id sim2real-behav-unknown-retrieval-token-0000000000 passed: Invalid execution id provided: sim2real-behav-unknown-retrieval-token-0000000000'; expected 'Invalid execution id sim2real-behav-unknown-retrieval-token-0000000000 passed: Invalid Execution Id'. The status-progression legs of this check passed (the retrieval reached COMPLETE), so only the message leg diverges.

In every case the token shown is the one the request carried (the only variable of the template); the divergence is the fixed trailing text, which must read 'Invalid Execution Id'.

## Steps to Reproduce
1. git_clone, then run_behavioral_suite (it builds and boots the emulator); use a us-west-2 xray client.
2. Enable the trace-segment destination the checks use (UpdateTraceSegmentDestination -> CloudWatchLogs) and PutTraceSegments one well-formed segment, then StartTraceRetrieval for that trace id to obtain a real RetrievalToken (positive control: the three operations below accept it).
3. Call xray.list_retrieved_traces(RetrievalToken='sim2real-behav-not-a-real-token', TraceFormat='XRAY'). Confirm HTTP 400 / InvalidRequestException, then compare the Message: observed 'Invalid execution id sim2real-behav-not-a-real-token passed: Invalid execution id provided: sim2real-behav-not-a-real-token', expected 'Invalid execution id sim2real-behav-not-a-real-token passed: Invalid Execution Id'.
4. Call xray.get_retrieved_traces_graph(RetrievalToken='sim2real-behav-not-a-real-token') — same expected/observed pair.
5. Call xray.cancel_trace_retrieval(RetrievalToken='sim2real-behav-not-a-real-token') — observed 'Invalid retrieval token <token> passed: Invalid execution id provided: <token>', expected 'Invalid retrieval token <token> passed: Invalid Execution Id'.
6. Re-replay the four check ids in trace_ids to confirm the fix.
