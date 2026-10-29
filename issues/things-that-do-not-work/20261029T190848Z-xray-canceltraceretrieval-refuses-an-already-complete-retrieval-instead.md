Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: xray.cancel-trace-retrieval-already-completed-fails xray.e2e-trace-retrieval-full-flow xray.start-trace-retrieval-malformed-time-format-fails xray.e2e-sampling-rule-drives-target-allocation
Issue-Shape-Id: com.amazonaws.xray#AWSXRay
Issue-Key: xray|CancelTraceRetrieval|InvalidRequestException
Issue-Key: xray.start-trace-retrieval-malformed-time-format-fails|run#10|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: xray.e2e-sampling-rule-drives-target-allocation|run#22|AssertionError|assert row["RequestCount"] == 500
Issue-Commit: c4171394ecd9e8acdff90bddf637ffaae1174b81

# xray: CancelTraceRetrieval refuses an already-COMPLETE retrieval instead of no-op 200, StartTraceRetrieval renders the wrong out-of-window refusal, and GetSamplingStatisticSummaries misreports reported RequestCount (3 causes)

> _Note: this issue was identified against the code base at commit `c4171394ecd9e8acdff90bddf637ffaae1174b81`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CancelTraceRetrieval

## Description
Three net-new root causes across the Trace Retrieval and sampling data planes.

Cause 1 of 3: CancelTraceRetrieval on a retrieval that has already reached a terminal COMPLETE state is refused instead of succeeding as a no-op.
AWS: Smithy models CancelTraceRetrieval with input RetrievalToken (required, @length max 1020), an EMPTY output (CancelTraceRetrievalResult), http POST /CancelTraceRetrieval code 200, and errors InvalidRequestException (400), ResourceNotFoundException (404), ThrottledException (429). Nothing in the model makes terminal state an error condition; the recorded behavior is HTTP 200 with an empty body, and the job's recorded status stays COMPLETE (a subsequent ListRetrievedTraces still reports RetrievalStatus=COMPLETE). Cancelling a retrieval that is still ACTIVE is the distinct case that genuinely transitions it to CANCELLED.
Emulator: 400 InvalidRequestException with message `the retrieval for RetrievalToken \`<token>\` is COMPLETE, so there is no ongoing retrieval to cancel` — fixed text `the retrieval for RetrievalToken ` ... ` is COMPLETE, so there is no ongoing retrieval to cancel`, with the token taken from the request and the state word from the emulator's stored job. AWS raises no error here at all, so there is no AWS wording to match: the requirement is 200 and an empty body.
Why: the documented idempotent-cancel path is unusable. Every client that cancels defensively on a shutdown or in a cleanup handler — not knowing whether the job already finished — gets a 400 it must special-case, and an end-to-end retrieval flow aborts at its last step even though the retrieval succeeded.
Fix: the CancelTraceRetrieval handler in the xray crate — terminal retrieval states (COMPLETE and the other terminal states) must return the empty 200 result and leave the stored status untouched, with the refusal reserved for an unknown/invalid token. Related but distinct open issues 20261028T150031Z-xray-canceltraceretrieval-renders-one-refusal-message-for-two.md and 20261029T103712Z-xray-canceltraceretrieval-refuses-a-case-mutated-retrievaltoken-with.md concern BAD-token classification, not a valid token in a terminal state; neither covers this.
Traces: xray.cancel-trace-retrieval-already-completed-fails, xray.e2e-trace-retrieval-full-flow.

Cause 2 of 3: StartTraceRetrieval refuses an out-of-range time window with the wrong message (and the wrong rule).
AWS: Smithy models StartTraceRetrieval input TraceIds (required, @length max 100), StartTime and EndTime (required timestamps) and error InvalidRequestException (400). For a window whose instants lie outside the range any retrieval could cover — here the classic epoch-seconds-scaled-by-1000 mistake, an anchor of 200000000000 — AWS answers 400 InvalidRequestException with message `Invalid input parameters for starting trace retrieval.End time should not be before the service was generally available.` That message is entirely fixed text: it echoes no value from the request.
Emulator: 400 InvalidRequestException with `Input parameters invalid for request. StartTime (200000000000) and EndTime (200003600000) must be within 86400 seconds of the present, in seconds of epoch time` — different fixed text, and it interpolates the two request timestamps and a 86400-second window bound that AWS's message does not state. The status and code match; the message does not, and the stated rule ("within 86400 seconds of the present") is not the rule AWS applies to this input class.
Why: a client matching on the refusal text to tell "your timestamps are mis-scaled" from an unrelated validation failure never matches, and the emulator asserts a recency bound AWS does not apply to the general-availability floor case.
Fix: the StartTraceRetrieval handler's time-window validation in the xray crate — emit AWS's fixed message for the out-of-range/pre-GA window class.
Traces: xray.start-trace-retrieval-malformed-time-format-fails.

Cause 3 of 3: GetSamplingStatisticSummaries does not report the statistics that were reported for a rule.
AWS: Smithy SamplingStatisticSummary is documented as "Aggregated request sampling data for a sampling rule across all services for a 10-second window" with members RuleName, Timestamp, RequestCount, BorrowCount, SampledCount (all Integer, default 0). After GetSamplingTargets is called with SamplingStatisticsDocuments reporting RequestCount=500 / SampledCount=50 for the rule, the summary row for that rule reports that reporting window's counts — RequestCount 500 and SampledCount 50 — recorded behavior for this flow.
Emulator: a row for the rule does appear (the poll for it succeeds, and the GetSamplingTargets legs — FixedRate echo and the full ReservoirQuota allocation — pass), but its RequestCount is not 500, so the per-window counts are wrong. Hypothesis: the emulator accumulates every statistics document the client reported during the run into one row (the check reports the same 500/50 document more than once while polling for ReservoirQuota) instead of keeping per-10-second-window rows, or it stores zeroes and never copies the reported counts.
Why: a sampler-health dashboard or an auto-scaling control loop reading GetSamplingStatisticSummaries sees request/sampled volumes that do not match what was reported, so reservoir utilisation computed from them is wrong.
Fix: the GetSamplingStatisticSummaries handler and the sampling-statistics state in the xray crate — record each reported statistics document into its 10-second window keyed by rule and serve that window's counts, rather than a run-total.
Traces: xray.e2e-sampling-rule-drives-target-allocation.

## Evidence
Cause 1 — xray.cancel-trace-retrieval-already-completed-fails (and identically xray.e2e-trace-retrieval-full-flow):
  expected: CancelTraceRetrieval(RetrievalToken=<completed token>) -> HTTP 200, empty body; ListRetrievedTraces still reports RetrievalStatus=COMPLETE
  actual:   botocore.errorfactory.InvalidRequestException: An error occurred (InvalidRequestException) when calling the CancelTraceRetrieval operation: the retrieval for RetrievalToken `f284b6c126f771d965d2302814eff4ee` is COMPLETE, so there is no ongoing retrieval to cancel
            (same refusal aborts the e2e flow: token `6e338ed0f160a5be3b68b6984de88c5b`)

Cause 2 — xray.start-trace-retrieval-malformed-time-format-fails:
  StartTraceRetrieval(TraceIds=[<well-formed id>], StartTime=200000000000, EndTime=200003600000)
  expected code/message: InvalidRequestException / 'Invalid input parameters for starting trace retrieval.End time should not be before the service was generally available.'
  actual message:        'Input parameters invalid for request. StartTime (200000000000) and EndTime (200003600000) must be within 86400 seconds of the present, in seconds of epoch time'

Cause 3 — xray.e2e-sampling-rule-drives-target-allocation:
  after GetSamplingTargets(SamplingStatisticsDocuments=[{RuleName, ClientID, Timestamp, RequestCount: 500, SampledCount: 50, BorrowCount: 0}])
  expected: the GetSamplingStatisticSummaries row for the rule has RequestCount == 500 (and SampledCount == 50)
  actual:   AssertionError at check line 115 `assert row["RequestCount"] == 500` — the row exists but its RequestCount differs

## Steps to Reproduce
Cause 1:
1. Enable the CloudWatchLogs trace-segment destination (Transaction Search) and PutTraceSegments one segment.
2. StartTraceRetrieval for that trace id, poll ListRetrievedTraces until RetrievalStatus == COMPLETE.
3. CancelTraceRetrieval(RetrievalToken=<that token>) -> emulator answers 400 InvalidRequestException; AWS answers 200 and leaves the status COMPLETE.

Cause 2:
4. StartTraceRetrieval(TraceIds=['1-<8 hex>-<24 hex>'], StartTime=200000000000, EndTime=200003600000) -> compare the InvalidRequestException message with AWS's fixed 'Invalid input parameters for starting trace retrieval.End time should not be before the service was generally available.'

Cause 3:
5. CreateSamplingRule(ReservoirSize=15, FixedRate=0.42, Priority=<1..9999>, all matchers '*').
6. GetSamplingTargets with one SamplingStatisticsDocument reporting RequestCount=500, SampledCount=50, BorrowCount=0 for that rule (repeat while polling for ReservoirQuota).
7. GetSamplingStatisticSummaries and read the row whose RuleName is the rule -> RequestCount must be 500 and SampledCount 50.

Replay: xray.cancel-trace-retrieval-already-completed-fails, xray.e2e-trace-retrieval-full-flow, xray.start-trace-retrieval-malformed-time-format-fails, xray.e2e-sampling-rule-drives-target-allocation.
