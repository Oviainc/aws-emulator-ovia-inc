Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: xray.start-trace-retrieval-unknown-destination-not-found
Issue-Shape-Id: com.amazonaws.xray#AWSXRay
Issue-Key: xray.start-trace-retrieval-unknown-destination-not-found|run#15|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Commit: af767dd3eb1ad507e674c736ee4881cbe95a30bf

# xray: CancelTraceRetrieval renders ONE refusal message for TWO distinct bad-token classes — an integrity-failing well-formed RetrievalToken must end 'Invalid execution id provided: <token>', not the fixed clause 'Invalid Execution Id'

> _Note: this issue was identified against the code base at commit `af767dd3eb1ad507e674c736ee4881cbe95a30bf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CancelTraceRetrieval

## Description
REOPENS 20261027T052518Z-xray-canceltraceretrieval-s-invalidrequestexception-message-for-a-malformed.md: its fix left the call refused. This is NOT a new defect — that issue was filed against exactly this check (xray.start-trace-retrieval-unknown-destination-not-found) and marked fixed, and the same message divergence is back, byte for byte. The call is refused with the right status and the right code; only the message text diverges, and it diverges in exactly the direction that issue described.

ONE root cause (Cause 1 of 1). The xray service is bootstrapped and answering (105/125 xray checks pass in this run), so this is a fidelity defect, not a bootstrap gap. The failing check is marked neither ALREADY FILED nor OWNED in the replay report.

Cause 1 of 1: CancelTraceRetrieval collapses two distinct refusal classes onto a single message template, so the integrity-failure class gets the unissued-token class's text. Failing check: xray.start-trace-retrieval-unknown-destination-not-found.

AWS: Smithy (services/service-com-amazonaws-xray-awsxray/smithy-model/com-amazonaws-xray-awsxray.json) models CancelTraceRetrieval as POST /CancelTraceRetrieval, input member RetrievalToken (target RetrievalToken, @required, @length min 0 max 1020), output CancelTraceRetrievalResult (empty, 200), modeled errors InvalidRequestException (HTTP 400, defaulted), ResourceNotFoundException (404), ThrottledException (429). The token is integrity-checked, not looked up, so a token of the issued form whose check digit/signature does not verify is a malformed parameter: HTTP 400, code InvalidRequestException. The model does not state message text, so the text is recorded behavior only, and the recordings show AWS words TWO different messages for TWO different bad-token classes on this same operation:
  (a) a string that is not of the issued token form at all (e.g. an arbitrary caller-made string) -> `Invalid retrieval token <RetrievalToken> passed: Invalid Execution Id`. Fixed text: `Invalid retrieval token `, ` passed: Invalid Execution Id` (title-cased, no second echo). Variable part: the request's RetrievalToken, substituted once.
  (b) a string that IS of the issued token form — same length, same base64url alphabet — but fails the integrity check (a one-character mutation of a genuine token) -> `Invalid retrieval token <RetrievalToken> passed: Invalid execution id provided: <RetrievalToken>`. Fixed text: `Invalid retrieval token `, ` passed: Invalid execution id provided: ` (sentence-cased). Variable part: the request's RetrievalToken, substituted TWICE — the inner clause comes from the execution-id decoder, which names the value it could not decode. Nothing else varies; the hex-looking token in any one replay is a per-run request value filling the slot, not part of the template.
Class (a) is the behavior the open issue 20261028T090715Z-xray-unknown-retrievaltoken-refusals-append-invalid-execution-id.md describes, and the checks asserting it currently PASS. This issue is class (b) only. RELATED, NOT COVERING: 20261028T090715Z covers the unissued/non-token-form wording for CancelTraceRetrieval, ListRetrievedTraces and GetRetrievedTracesGraph and never names the integrity-failure class; satisfying it alone is what re-broke this check.

Emulator: HTTP 400 and code InvalidRequestException are correct, and the leading clause `Invalid retrieval token <token> passed: ` is correct, but the emulator has only ONE bad-token branch and one message builder, so class (b) is rendered with class (a)'s text. For a rejected token T the emulator returns `Invalid retrieval token T passed: Invalid Execution Id` where AWS returns `Invalid retrieval token T passed: Invalid execution id provided: T`. Two divergences in one string: the inner clause is title-cased instead of sentence-cased with the `provided:` label, and the second substitution of the offending token is missing entirely. Everything else the check asserts on this path holds — the refused cancel leaves the real retrieval uncancelled (ListRetrievedTraces still reports a non-CANCELLED RetrievalStatus) and the genuine token is still accepted — so the defect is scoped to the message of the integrity-failure class.

Why: the message is part of the contract clients match on. The second token echo is the only part of the message that identifies WHICH token failed the integrity check; with the emulator's text every integrity failure in the account renders to an identical trailing clause and the offending value is unrecoverable, so an operator or log-scraping tool triaging a batch of concurrent cancels cannot tell them apart. SDK wrappers, retry logic and test suites that match AWS's wording (`Invalid execution id provided`) never match and misclassify an integrity failure as an unknown error class. Because the code is correct, the mismatch is silent until a consumer compares text. The regression risk is symmetric: a fix that simply swaps the single template back breaks the class-(a) checks that pass today.

Fix: the operation's validation and its shared refusal-message builder, not its control flow — the classification (InvalidRequestException/400) is already right. The single form gate `crate::state::is_retrieval_token_form()` must be split so the handler can tell a string that is not of the issued form (class a) from one that is of the issued form but fails the integrity check (class b), and `crate::state::malformed_retrieval_token_message()` must render the per-class trailing clause: the fixed `Invalid Execution Id` for (a), and `Invalid execution id provided: ` plus a second substitution of the request's token for (b). The sibling operations that validate a RetrievalToken through the same builder (ListRetrievedTraces, GetRetrievedTracesGraph) must keep their currently-correct class-(a) text.

Traces: xray.start-trace-retrieval-unknown-destination-not-found. Handler branch and message construction: services/service-com-amazonaws-xray-awsxray/src/ops/cancel_trace_retrieval.rs lines 161-176, which gates on `crate::state::is_retrieval_token_form(&input.retrieval_token)` and returns `crate::state::malformed_retrieval_token_message("retrieval token", &input.retrieval_token)`; the builder and the form predicate both live in services/service-com-amazonaws-xray-awsxray/src/state.rs.

## Evidence
Check: xray.start-trace-retrieval-unknown-destination-not-found — verdict mismatch.

The check enables Transaction Search (logs.put_resource_policy granting xray.amazonaws.com logs:PutLogEvents on aws/spans and /aws/application-signals/data, then update_trace_segment_destination(Destination='CloudWatchLogs') polled to ACTIVE), ingests one segment via PutTraceSegments, obtains a genuine RetrievalToken from StartTraceRetrieval, flips the token's MIDDLE character within the base64url alphabet (length and charset preserved, so only the integrity check can reject it) and calls CancelTraceRetrieval with it.

HTTP status: MATCHES (400).
Error code: MATCHES — expected InvalidRequestException, got InvalidRequestException.
Error message: DIVERGES. With T = the mutated token taken from the request:
  expected (AWS): "Invalid retrieval token T passed: Invalid execution id provided: T"
  actual (emulator): "Invalid retrieval token T passed: Invalid Execution Id"

Verbatim replay assertion (T was cb2b39aa6be0f0c7A92dfa82d42cbfbb on this run — a per-run request value, not part of the requirement):
  File "check_xray_start_trace_retrieval_unknown_destination_not_found.py", line 51, in run
    assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
  AssertionError: expected message 'Invalid retrieval token cb2b39aa6be0f0c7A92dfa82d42cbfbb passed: Invalid execution id provided: cb2b39aa6be0f0c7A92dfa82d42cbfbb', got 'Invalid retrieval token cb2b39aa6be0f0c7A92dfa82d42cbfbb passed: Invalid Execution Id'

Required message, fixed text vs variable parts (integrity-failure class):
  fixed:    "Invalid retrieval token " ... " passed: Invalid execution id provided: "
  variable: the request's RetrievalToken value, substituted twice.

Contrast that currently PASSES and must not regress (unissued/non-token-form class, per open issue 20261028T090715Z): CancelTraceRetrieval with a string that is not of the issued form answers 400 / InvalidRequestException / "Invalid retrieval token <token> passed: Invalid Execution Id", with the token substituted once. xray.cancel-trace-retrieval-unknown-token-fails, xray.list-retrieved-traces-unknown-token-fails and xray.get-retrieved-traces-graph-unknown-token-fails are all passing in this run, so the single shared template cannot simply be swapped.

Unaffected on this path (scoping the defect to the message): after the refused cancel, list_retrieved_traces(RetrievalToken=genuine, TraceFormat='XRAY')['RetrievalStatus'] is not 'CANCELLED', and cancel_trace_retrieval(RetrievalToken=genuine) still succeeds.

History: filed before as 20261027T052518Z-xray-canceltraceretrieval-s-invalidrequestexception-message-for-a-malformed.md (status fixed, Issue-Trace-Ids: xray.start-trace-retrieval-unknown-destination-not-found) with the identical expected/actual strings. The divergence is present again at the current commit.

## Steps to Reproduce
1. git_clone, then run_behavioral_suite (it builds and boots the emulator); confirm bootstrap_signal is false and xray answers (105/125 xray checks pass).
2. Enable Transaction Search so StartTraceRetrieval will accept a trace id:
   a. logs.put_resource_policy(policyName='sim2real-xray-transaction-search-policy', policyDocument=<Allow principal Service xray.amazonaws.com, Action logs:PutLogEvents, Resource arn:aws:logs:<region>:<account>:log-group:aws/spans:* and .../aws/application-signals/data:*>).
   b. xray.update_trace_segment_destination(Destination='CloudWatchLogs'), then poll xray.get_trace_segment_destination() until Status == 'ACTIVE'.
3. Ingest a segment: xray.put_trace_segments(TraceSegmentDocuments=[json.dumps({'name':'sim2real-probe-seed','id':<16 hex>,'trace_id':'1-<8 hex epoch>-<24 hex>','start_time': now-5,'end_time': now})]).
4. token = xray.start_trace_retrieval(TraceIds=[<that trace_id>], StartTime=now-10min, EndTime=now)['RetrievalToken'] (retry while it answers ResourceNotFoundException).
5. Mutate exactly ONE character in place, preserving length and the base64url alphabet: mid = len(token)//2; mutated = token[:mid] + <any other base64url char> + token[mid+1:].
6. Call xray.cancel_trace_retrieval(RetrievalToken=mutated) and capture the ClientError.
7. Observe Code == 'InvalidRequestException' (correct, HTTP 400) but Message == 'Invalid retrieval token <mutated> passed: Invalid Execution Id'. AWS returns 'Invalid retrieval token <mutated> passed: Invalid execution id provided: <mutated>' — the inner clause sentence-cased with the 'provided:' label and the rejected token echoed a second time.
8. Confirm the rest of the path is unaffected: xray.list_retrieved_traces(RetrievalToken=token, TraceFormat='XRAY')['RetrievalStatus'] != 'CANCELLED', and xray.cancel_trace_retrieval(RetrievalToken=token) still succeeds.
9. Regression guard for the OTHER bad-token class: xray.cancel_trace_retrieval(RetrievalToken='sim2real-behav-not-a-real-token') must still answer 400 / InvalidRequestException / 'Invalid retrieval token sim2real-behav-not-a-real-token passed: Invalid Execution Id' (token substituted once), and ListRetrievedTraces / GetRetrievedTracesGraph must keep their 'Invalid execution id <token> passed: Invalid Execution Id' wording.
10. Re-replay xray.start-trace-retrieval-unknown-destination-not-found to confirm the fix.
