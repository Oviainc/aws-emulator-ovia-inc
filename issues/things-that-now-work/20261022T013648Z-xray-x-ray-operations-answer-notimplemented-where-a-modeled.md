Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: xray.start-trace-retrieval-invalid-time-range-rejected xray.start-trace-retrieval-malformed-time-format-fails xray.start-trace-retrieval-exceeds-rate-throttled xray.start-trace-retrieval-throttled-when-rate-exceeded xray.get-insight-unknown-id-fails xray.get-service-graph-invalid-time-range-fails xray.concurrent-create-sampling-rule-duplicate-name xray.update-group-conflicting-name-rejected xray.get-trace-graph-unknown-trace-fails xray.trace-graph-derives-per-trace-topology-and-rejects-unknown-trace-id
Issue-Shape-Id: com.amazonaws.xray#AWSXRay
Issue-Key: xray.start-trace-retrieval-invalid-time-range-rejected|run#7|AssertionError|assert code == "InvalidRequestException", f"unexpected error code: {code}"
Issue-Key: xray.start-trace-retrieval-malformed-time-format-fails|run#8|AssertionError|assert code == "InvalidRequestException", (
Issue-Key: xray.start-trace-retrieval-exceeds-rate-throttled|run#9|AssertionError|assert throttled, (
Issue-Key: xray.start-trace-retrieval-throttled-when-rate-exceeded|run#9|AssertionError|assert control_code in (None, "ResourceNotFoundException"), (
Issue-Key: xray.get-insight-unknown-id-fails|run#3|AssertionError|assert code == "InvalidRequestException", f"expected InvalidRequestException, got {code}"
Issue-Key: xray.get-service-graph-invalid-time-range-fails|run#4|AssertionError|assert code == "InvalidRequestException", f"expected InvalidRequestException, got {code}"
Issue-Key: xray.concurrent-create-sampling-rule-duplicate-name|run#8|AssertionError|assert successes == 1, f"expected exactly 1 winner, got {successes} (codes={codes})"
Issue-Key: xray.update-group-conflicting-name-rejected|run#9|AssertionError|assert code == "InvalidRequestException", f"expected InvalidRequestException, got {code!r}"
Issue-Key: xray.get-trace-graph-unknown-trace-fails|_poll_until#8|Timeout|time.sleep(interval_s)
Issue-Key: xray.trace-graph-derives-per-trace-topology-and-rejects-unknown-trace-id|_poll_until#13|Timeout|time.sleep(interval_s)
Issue-Commit: 5b247f22dea7573e9fb6c4376c776b3a345a666f

# X-Ray operations answer NotImplemented where a modeled error or response is required (StartTraceRetrieval, GetInsight, GetServiceGraph, CreateSamplingRule, UpdateGroup, GetTraceGraph)

> _Note: this issue was identified against the code base at commit `5b247f22dea7573e9fb6c4376c776b3a345a666f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartTraceRetrieval

## Description
Six root causes, each one X-Ray operation that answers the emulator's non-AWS `NotImplemented` error where the model requires a response or a modeled error. These failing checks were NOT marked ALREADY FILED / OWNED by the replay report, so they are filed here; where a standing scaffold-stub issue exists for the same operation it is named below, and the behavioural requirement stated here (which modeled error path must exist) is what these checks grade beyond bare implementation. Service crate: services/service-com-amazonaws-xray-awsxray.

Common shape: the emulator answers error code `NotImplemented` with the message "operation <Op> is scaffolded but not implemented in this emulator". `NotImplemented` is not an error AWS's X-Ray model declares for any operation, so no client can handle it: botocore surfaces it as a generic ClientError and every error-typed catch block misses.

Cause 1 of 6: StartTraceRetrieval.
AWS: Smithy models StartTraceRetrieval with required TraceIds (TraceIdListForRetrieval, @length 0..100), required StartTime and StartTime/EndTime timestamps, output RetrievalToken, and errors InvalidRequestException (client, HTTP 400), ResourceNotFoundException (404) and ThrottledException (HTTP 429). An inverted or implausible time range (e.g. a millisecond-scale value landing thousands of years in the future) is refused with InvalidRequestException, and a concurrent burst beyond the per-account rate yields at least one ThrottledException (429). Emulator: every call answers NotImplemented. Why: neither the validation nor the throttling contract exists, and a client cannot even tell a bad request from an unavailable operation. Fix: implement the StartTraceRetrieval handler including its time-range validation and rate/throttle path. Traces: xray.start-trace-retrieval-invalid-time-range-rejected, xray.start-trace-retrieval-malformed-time-format-fails, xray.start-trace-retrieval-exceeds-rate-throttled, xray.start-trace-retrieval-throttled-when-rate-exceeded.

Cause 2 of 6: GetInsight.
AWS: Smithy models GetInsight.InsightId with a UUID @pattern and errors InvalidRequestException (400) / ThrottledException (429); an InsightId that matches the pattern but names no insight is refused with InvalidRequestException at HTTP 400. Emulator: answers NotImplemented. Why: a client probing for an insight gets an unhandleable error instead of the modeled refusal. Fix: implement the GetInsight handler and its unknown-id refusal. Traces: xray.get-insight-unknown-id-fails.

Cause 3 of 6: GetServiceGraph time-range validation.
AWS: GetServiceGraph refuses an invalid [StartTime, EndTime) window with InvalidRequestException at HTTP 400. Emulator: answers NotImplemented. A scaffold-stub issue for GetServiceGraph is already filed from the checkout (named by the report as blocking 5 checks); this check additionally requires the operation's invalid-time-range refusal, which that issue does not state. Why: a client with a reversed window is told the operation does not exist. Fix: implement GetServiceGraph including its time-range validation. Traces: xray.get-service-graph-invalid-time-range-fails.

Cause 4 of 6: CreateSamplingRule duplicate-name race.
AWS: Smithy models CreateSamplingRule with errors InvalidRequestException (400), RuleLimitExceededException (400) and ThrottledException (429). When N concurrent calls create the same RuleName, exactly one succeeds and the losers are refused — there is always exactly one winner. Emulator: all five concurrent calls answered NotImplemented, so there was no winner at all. A scaffold-stub issue for CreateSamplingRule is already filed (report: blocks 25 checks); the single-winner serialization this check grades is an additional requirement. Why: a client relying on create-wins-once for idempotent rule provisioning gets neither a success nor a collision error. Fix: implement CreateSamplingRule with a name-collision check under the state lock. Traces: xray.concurrent-create-sampling-rule-duplicate-name.

Cause 5 of 6: UpdateGroup conflicting-name refusal.
AWS: Smithy models UpdateGroup (GroupName @length 1..32, GroupARN @length 1..400) with errors InvalidRequestException (400) / ThrottledException (429); an update that names a group selector in conflict is refused with InvalidRequestException at 400. Emulator: answers NotImplemented. A scaffold-stub issue for UpdateGroup is already filed (report: blocks 1 check); this check requires the conflicting-name refusal specifically. Why: the group mutation surface is unusable and the refusal is unhandleable. Fix: implement the UpdateGroup handler and its selector-conflict refusal. Traces: xray.update-group-conflicting-name-rejected.

Cause 6 of 6: GetTraceGraph.
AWS: GetTraceGraph returns the per-trace topology in `Services` for a stored trace, and refuses a trace id that is not indexed with InvalidRequestException (HTTP 400) and the fixed message "Trace id not found" — unlike BatchGetTraces, which returns an empty list. Recorded behavior for the message; Smithy: GetTraceGraph models InvalidRequestException (400) / ThrottledException (429). Emulator: answers NotImplemented, so both checks' positive controls (a just-submitted trace's graph populating) never pass and the checks run to the harness deadline ("Check timed out after 60.0s") rather than reporting a code. A scaffold-stub issue for GetTraceGraph is already filed (report: blocks 1 check); these two checks additionally require the populated positive path and the not-found refusal. Fix: implement the GetTraceGraph handler, deriving the graph from stored segments, plus its unknown-trace refusal. Traces: xray.get-trace-graph-unknown-trace-fails, xray.trace-graph-derives-per-trace-topology-and-rejects-unknown-trace-id.

## Evidence
Cause 1: `AssertionError: unexpected error code: NotImplemented`; `expected InvalidRequestException for a millisecond-scale time value landing thousands of years in the future, got 'NotImplemented'`; `expected at least one ThrottledException from a burst of concurrent StartTraceRetrieval calls, saw error codes: {'NotImplemented'}`; `unexpected control error code: 'NotImplemented'`.
Cause 2: `AssertionError: expected InvalidRequestException, got NotImplemented`.
Cause 3: `AssertionError: expected InvalidRequestException, got NotImplemented`.
Cause 4: `AssertionError: expected exactly 1 winner, got 0 (codes=['NotImplemented', 'NotImplemented', 'NotImplemented', 'NotImplemented', 'NotImplemented'])`.
Cause 5: `AssertionError: expected InvalidRequestException, got 'NotImplemented'`.
Cause 6: both checks end in `Check timed out after 60.0s`; their helper swallows the GetTraceGraph ClientError and returns [] (`_trace_graph_or_empty`), so the positive-control poll for the just-submitted trace's graph never satisfies and the asserted refusal `InvalidRequestException` / 'Trace id not found' is never reached.
In every case the emulator body is `An error occurred (NotImplemented) when calling the <Op> operation: operation <Op> is scaffolded but not implemented in this emulator`.

## Steps to Reproduce
1. Boot the emulator and run the behavioral suite (run_behavioral_suite).
2. Cause 1: StartTraceRetrieval with EndTime < StartTime, then with a millisecond-scale epoch value, then fire a concurrent burst — inspect the error codes.
3. Cause 2: GetInsight with a well-formed UUID that names no insight.
4. Cause 3: GetServiceGraph with an invalid [StartTime, EndTime) window.
5. Cause 4: fire five concurrent CreateSamplingRule calls using one RuleName — count winners.
6. Cause 5: UpdateGroup with a conflicting group selector.
7. Cause 6: PutTraceSegments one segment, then GetTraceGraph for that trace id (expect its service in Services), then GetTraceGraph for an unused trace id (expect 400 InvalidRequestException / 'Trace id not found').
8. Re-replay the check ids in trace_ids to confirm.
