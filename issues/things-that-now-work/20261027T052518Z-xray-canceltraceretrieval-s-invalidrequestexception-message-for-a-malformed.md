Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: xray.start-trace-retrieval-unknown-destination-not-found
Issue-Shape-Id: com.amazonaws.xray#AWSXRay
Issue-Key: xray.start-trace-retrieval-unknown-destination-not-found|run#15|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Commit: 214529776f1e8d2a1a58052998eb185e42274954

# xray: CancelTraceRetrieval's InvalidRequestException message for a malformed RetrievalToken omits the rejected token's inner clause

> _Note: this issue was identified against the code base at commit `214529776f1e8d2a1a58052998eb185e42274954`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CancelTraceRetrieval

## Description
One net-new root cause.

Cause 1 of 1: the message X-Ray returns when `CancelTraceRetrieval` rejects a structurally-invalid `RetrievalToken` is truncated — the emulator emits a fixed trailing clause instead of AWS's `Invalid execution id provided: <token>`. Failing check: xray.start-trace-retrieval-unknown-destination-not-found.

AWS: `CancelTraceRetrieval` takes a single required member `RetrievalToken` (Smithy: CancelTraceRetrieval.RetrievalToken, target `RetrievalToken`, @required, @length min 0 max 1020; POST /CancelTraceRetrieval; modeled errors InvalidRequestException (HTTP 400, defaulted), ResourceNotFoundException (404), ThrottledException (429)). The token is integrity-checked rather than looked up, so a one-character mutation of a genuine token — same length, same base64url alphabet — is rejected as a malformed parameter with `InvalidRequestException` (HTTP 400). The model documents the error class but not its message text, so the message is recorded behavior only: AWS words it as two clauses, an outer one naming the parameter and the rejected value and an inner one from the execution-id decoder that names the same value again. Its fixed text is `Invalid retrieval token ` … ` passed: Invalid execution id provided: `; its variable parts are the rejected token value, substituted TWICE — once after `Invalid retrieval token ` and once after `Invalid execution id provided: ` — taken verbatim from the request's `RetrievalToken`. Nothing else in the message varies.

Emulator: the status (400) and the error code (`InvalidRequestException`) are correct, but the message stops after the outer clause and appends a static, differently-cased stub. For a rejected token T the emulator returns `Invalid retrieval token T passed: Invalid Execution Id` where AWS returns `Invalid retrieval token T passed: Invalid execution id provided: T`. Two divergences in one string: the inner clause is title-cased (`Invalid Execution Id`) rather than AWS's sentence-cased `Invalid execution id provided:`, and the second substitution of the offending token is missing entirely. Replay output for this check: `AssertionError: expected message 'Invalid retrieval token 9777320e1fcd5a05A6690fbd0286fd1e passed: Invalid execution id provided: 9777320e1fcd5a05A6690fbd0286fd1e', got 'Invalid retrieval token 9777320e1fcd5a05A6690fbd0286fd1e passed: Invalid Execution Id'` (that hex string is the per-run mutated token, a value from the request, not part of the template). Everything else the check asserts about this path holds: the real retrieval is left uncancelled and the genuine token is still accepted, so only the message text diverges.

Why: a client that reads the message rather than only the code gets the wrong information. The second token echo is the part that tells an operator or a log-scraping tool WHICH token failed the integrity check when a batch of cancels is issued concurrently — with the emulator's message, every malformed-token rejection in the account renders to the identical trailing clause and the offending value is unrecoverable from the inner clause. Tests, retry wrappers and support tooling that match on AWS's wording (`Invalid execution id provided`) never match, so they misclassify an integrity failure as an unknown error class and fall into a generic retry or surface the raw string to a user.

Fix: belongs in the shared message builder the operation's validation calls before the token is resolved, not in the handler's control flow — the handler's early-return branch is already correct in choosing `InvalidRequestException`/400 for a token that is not of the issued form. The builder must render BOTH clauses and substitute the request's token into each, and must use AWS's sentence casing for the inner clause. Any sibling operation that validates a `RetrievalToken` through the same builder (e.g. `ListRetrievedTraces`, `GetRetrievedTracesGraph`) inherits the same wording and should be re-checked once the builder is corrected, since the first clause's parameter label is passed in by the caller.

Traces: xray.start-trace-retrieval-unknown-destination-not-found. Handler early-return and message construction: services/service-com-amazonaws-xray-awsxray/src/ops/cancel_trace_retrieval.rs lines 161-176, which calls `crate::state::malformed_retrieval_token_message("retrieval token", &input.retrieval_token)` — the builder in services/service-com-amazonaws-xray-awsxray/src/state.rs is where the truncated text is produced.

## Evidence
Check: xray.start-trace-retrieval-unknown-destination-not-found (verdict: mismatch).

The check enables Transaction Search (logs resource policy + trace-segment destination pinned to CloudWatchLogs), ingests a segment, obtains a genuine RetrievalToken from StartTraceRetrieval, flips the token's middle character within the base64url alphabet (length and charset preserved, so only the integrity check can reject it) and calls CancelTraceRetrieval with it.

Error code: MATCHES. Expected `InvalidRequestException`, got `InvalidRequestException` (HTTP 400).

Error message: DIVERGES.
  Expected (AWS), with T = the mutated token from the request:
    "Invalid retrieval token T passed: Invalid execution id provided: T"
  Actual (emulator):
    "Invalid retrieval token T passed: Invalid Execution Id"

Verbatim replay assertion (T happened to be 9777320e1fcd5a05A6690fbd0286fd1e on this run; that value is a per-run variable part, not a requirement):
  File "check_xray_start_trace_retrieval_unknown_destination_not_found.py", line 51, in run
    assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
  AssertionError: expected message 'Invalid retrieval token 9777320e1fcd5a05A6690fbd0286fd1e passed: Invalid execution id provided: 9777320e1fcd5a05A6690fbd0286fd1e', got 'Invalid retrieval token 9777320e1fcd5a05A6690fbd0286fd1e passed: Invalid Execution Id'

Fixed text vs variable parts of the required message:
  fixed:    "Invalid retrieval token ", " passed: Invalid execution id provided: "
  variable: the request's RetrievalToken value, substituted twice.

What the emulator gets right on this path (so the defect is scoped to the message): the mutated cancel does NOT move the real retrieval to CANCELLED (ListRetrievedTraces still reports a non-CANCELLED RetrievalStatus), and the genuine token is still accepted by a subsequent CancelTraceRetrieval.

Emulator source backing the diagnosis: services/service-com-amazonaws-xray-awsxray/src/ops/cancel_trace_retrieval.rs:166-176 returns InvalidRequestException with `crate::state::malformed_retrieval_token_message("retrieval token", &input.retrieval_token)` — the token-form check and the error classification are right; the builder's rendered text is not.

## Steps to Reproduce
1. Boot the emulator and point an xray client at it.
2. Enable Transaction Search so StartTraceRetrieval will accept a trace id:
   a. logs.put_resource_policy(policyName="sim2real-xray-transaction-search-policy", policyDocument=<allow xray.amazonaws.com logs:PutLogEvents on aws/spans and /aws/application-signals/data>)
   b. xray.update_trace_segment_destination(Destination="CloudWatchLogs") and poll xray.get_trace_segment_destination() until Status == "ACTIVE".
3. Ingest a segment: xray.put_trace_segments(TraceSegmentDocuments=[json.dumps({"name": "sim2real-probe-seed", "id": <16 hex>, "trace_id": "1-<8 hex epoch>-<24 hex>", "start_time": now-5, "end_time": now})]).
4. token = xray.start_trace_retrieval(TraceIds=[<that trace_id>], StartTime=now-10min, EndTime=now)["RetrievalToken"] (retry while it answers ResourceNotFoundException).
5. Mutate exactly one character of the token in place, keeping length and the base64url alphabet: mid = len(token)//2; mutated = token[:mid] + <any other base64url char> + token[mid+1:].
6. Call xray.cancel_trace_retrieval(RetrievalToken=mutated) and capture the ClientError.
7. Observe: Code == "InvalidRequestException" (correct, HTTP 400) but Message == "Invalid retrieval token <mutated> passed: Invalid Execution Id".
   AWS returns "Invalid retrieval token <mutated> passed: Invalid execution id provided: <mutated>" — the inner clause must be sentence-cased and must echo the rejected token a second time.
8. Confirm the rest of the path is unaffected: xray.list_retrieved_traces(RetrievalToken=token, TraceFormat="XRAY")["RetrievalStatus"] is not "CANCELLED", and xray.cancel_trace_retrieval(RetrievalToken=token) still succeeds.
