Issue-Source: trace-sweep-divergence
Issue-Trace: xray-trace-summaries-hourly-pagination
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: xray-trace-summaries-hourly-pagination
Issue-Shape-Id: com.amazonaws.xray#AWSXRay
Issue-Key: xray|GetTraceSummaries|4xx->4xx|$.Message
Issue-Commit: 60bf370f5bec400ea538d88fdddbc2779d51adc3

# xray GetTraceSummaries rejects an unissued NextToken with the wrong InvalidRequestException message

> _Note: this issue was identified against the code base at commit `60bf370f5bec400ea538d88fdddbc2779d51adc3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetTraceSummaries

## Description
ONE net-new root cause on the GetTraceSummaries pagination-token error path: the emulator rejects an unissued/garbage NextToken with the right HTTP status (400) and the right error type (InvalidRequestException), but the error MESSAGE is the emulator's own invented wording rather than AWS's.

Smithy model consulted: services/service-com-amazonaws-xray-awsxray/smithy-model/com-amazonaws-xray-awsxray.json. GetTraceSummaries is @paginated (inputToken NextToken / outputToken NextToken / items TraceSummaries), its NextToken input member is documented as "Specify the pagination token returned by a previous request to retrieve the next page of results", and its only modelled 4xx client error is InvalidRequestException (httpError 400, doc: "The request is missing required parameters or has invalid parameters") with a single Message member. No open issue names this cluster and the replay report marked it neither stub_path nor owned_by. The sibling open issue 20261103T034719Z-xray-gettracesummaries-hourly-paging-missing-and-getservicegraph-echoes.md concerns the SUCCESS paging envelope (ApproximateTime / NextToken emission), not the rejection message for a bad token, so it does not cover this; 20261103T034719Z-xray-batchgettraces-gettracegraph-trace-id-validation-diverges-from.md covers the analogous BatchGetTraces wording, a different operation and handler.

Cause 1 of 1: the InvalidRequestException message for an uninterpretable pagination token does not use AWS's wording.
AWS: for a NextToken value it did not issue (or cannot decode), X-Ray answers HTTP 400 with x-amzn-errortype InvalidRequestException and a Message whose FIXED text is "Invalid token " followed by exactly one VARIABLE part — the offending token string taken verbatim from the request's NextToken member. In this step the request sent NextToken="bogus-token" and the recorded message is "Invalid token bogus-token"; the token substring is request data, the leading "Invalid token " is the requirement. The message does not name the service, does not quote or backtick the token, and does not explain provenance.
Emulator: answers 400/InvalidRequestException (both correct) with Message "NextToken `bogus-token` was not issued by this service" — different fixed text ("... was not issued by this service"), and it wraps the request's token in backticks.
Why: the status and error code match, so a client never falls back; it reads the message. Callers and SDK/CLI users that branch on, log-match or assert the documented "Invalid token <token>" text (and tests/runbooks written against the real service) see an unrecognised string and either mis-classify a bad cursor as an unrelated service-side failure or fail to extract the offending token from the message, which is the only place the token is reported back.
Fix: the GetTraceSummaries handler's NextToken validation in the xray crate — where the token is decoded and rejected, emit the InvalidRequestException message as the fixed prefix "Invalid token " plus the request's NextToken value unquoted, instead of the emulator's "was not issued by this service" phrasing.
Traces: xray-trace-summaries-hourly-pagination#task#3.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true, bootstrap_signal=false, steps_total=123, steps_passed=93, divergence_count=19). Exactly ONE cluster was net-new for service=xray (no stub_path, no owned_by); the other 18 are owned by open issues or blocked by an unimplemented operation.

Cluster divergence_key service=xray|GetTraceSummaries|status=400->400|keys=$.Message, verdict=mismatch, affected_count=1:
  fixture xray-trace-summaries-hourly-pagination, step task#3, GetTraceSummaries, expected_status 400, actual_status 400
  $.Message: value_mismatch
    expected: "Invalid token bogus-token"
    actual:   "NextToken `bogus-token` was not issued by this service"

Status (400) and error type (InvalidRequestException, the only modelled 400 for this operation) agree; only the message text diverges. Breakdown of the expected message: fixed text "Invalid token "; variable part "bogus-token", which is the NextToken value this step's request supplied (not a per-run value — any rejected token is echoed in that position).

## Steps to Reproduce
Prerequisite: git_clone(), then replay_wire_corpus() (it builds and boots the emulator from the checkout).

1. POST /TraceSummaries with the required StartTime/EndTime window and NextToken set to a value this emulator never issued (the fixture uses "bogus-token").
2. Expect HTTP 400 with x-amzn-errortype InvalidRequestException and body {"Message": "Invalid token <the NextToken value sent>"} — e.g. "Invalid token bogus-token". Currently the body is {"Message": "NextToken `<token>` was not issued by this service"}.
3. Repeat with a different bogus token to confirm the variable part is the request's token verbatim and unquoted, and that the fixed prefix "Invalid token " is unchanged.
4. Re-replay xray-trace-summaries-hourly-pagination#task#3 to confirm the cause is closed.
