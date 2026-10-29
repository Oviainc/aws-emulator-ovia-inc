Issue-Source: trace-sweep-divergence
Issue-Trace: sfn-execution-included-data-and-history-paging
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sfn-execution-included-data-and-history-paging sfn-history-default-page-and-order-bound-token states-execution-failure-and-redrive states-execution-lifecycle-success-and-abort states-standard-execution-lifecycle stepfunctions-standard-execution-start-monitor-stop
Issue-Shape-Id: com.amazonaws.sfn#AWSStepFunctions
Issue-Key: states|GetExecutionHistory|2xx->2xx|$.events[*].__type
Issue-Key: states|GetExecutionHistory|2xx->2xx|$.events[*].executionStartedEventDetails.roleArn
Issue-Key: states|GetExecutionHistory|2xx->2xx|$.events[*].previousEventId
Issue-Key: states|GetExecutionHistory|2xx->2xx|$.events[*].stateEnteredEventDetails.input
Issue-Key: states|GetExecutionHistory|4xx->2xx|$.__type
Issue-Key: states|GetExecutionHistory|4xx->2xx|$.message
Issue-Key: states|GetExecutionHistory|4xx->2xx|status
Issue-Key: states|GetExecutionHistory|2xx->2xx|$.events[id=1].__type
Issue-Key: states|GetExecutionHistory|2xx->2xx|$.events[id=1].executionStartedEventDetails.roleArn
Issue-Key: states|GetExecutionHistory|2xx->2xx|$.events[id=2].__type
Issue-Key: states|GetExecutionHistory|2xx->2xx|$.events[id=2].previousEventId
Issue-Key: states|GetExecutionHistory|2xx->2xx|$.events[id=3].__type
Issue-Key: states|GetExecutionHistory|2xx->2xx|$.events[id=4].__type
Issue-Key: states|GetExecutionHistory|2xx->2xx|$.events[id=5].__type
Issue-Key: states|GetExecutionHistory|2xx->2xx|$.events[id=5].executionFailedEventDetails
Issue-Key: states|GetExecutionHistory|2xx->2xx|$.events[id=5].type
Issue-Key: states|GetExecutionHistory|2xx->2xx|$.events[id=2].stateEnteredEventDetails.input
Issue-Key: states|GetExecutionHistory|2xx->2xx|$.events[id=3].stateExitedEventDetails.output
Issue-Commit: a9006feb831fdd789c02a15722f93d2f2e52852a

# Step Functions execution history is serialized without the HistoryEvent __type and misreports roleArn, previousEventId, the terminal failure event and an invalid nextToken (10 causes)

> _Note: this issue was identified against the code base at commit `a9006feb831fdd789c02a15722f93d2f2e52852a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetExecutionHistory

## Description
This issue bundles TEN net-new wire divergence clusters on the Step Functions execution-history read path. They are driven by five underlying faults in the GetExecutionHistory serializer, the history recorder and the page-token validator; every cluster is a separate replay unit and all ten must be closed before submitting. Re-replay every fixture#step listed in trace_ids.

Shared fault A — the HistoryEvent structure is serialized with no `__type` discriminator. AWS: recorded behavior only (the Smithy model does not describe awsJson1_0 type discriminators) — every element of GetExecutionHistoryOutput.events is serialized with the member `__type` set to the fixed string `com.amazonaws.swf.base.model#HistoryEvent`. The emulator omits it on every event of every page, while it DOES emit `__type` for other sfn structures (e.g. `com.amazonaws.swf.base.model#CloudWatchEventsExecutionDataDetails` on DescribeExecution), so the serializer is inconsistent with itself. Why: a client that dispatches on the discriminator (hand-written awsJson1_0 decoders, the console-style viewers and any polyglot deserializer that round-trips the raw JSON) cannot identify the event shape, and a byte-comparison against a recorded AWS response never matches.
Shared fault B — executionStartedEventDetails.roleArn is never emitted. AWS: recorded behavior — the ExecutionStarted event (event id 1) carries executionStartedEventDetails.roleArn holding the IAM role ARN the execution assumed, i.e. the roleArn stored on the state machine. The emulator omits the member. Why: the role an execution ran under is only auditable from the history; a compliance reader sees nothing.
Shared fault C — previousEventId of the first state event is 1 instead of 0. AWS: recorded behavior — event id 1 (ExecutionStarted) has no predecessor and the event that follows it reports previousEventId 0, the sentinel meaning "no previous event"; the emulator stamps 1 (the ExecutionStarted event's own id). Why: a client walking the history backwards by previousEventId never terminates at the sentinel and mis-renders the first edge of the graph.
Shared fault D — execution input/output echoed into history details is re-serialized compactly. AWS: recorded behavior — the execution input supplied to StartExecution is echoed back BYTE-FOR-BYTE in stateEnteredEventDetails.input and stateExitedEventDetails.output, including the caller's whitespace. The emulator parses the payload and re-emits it with separators stripped. Why: any caller that hashes, signs or diffs the echoed payload against what it sent sees a mismatch.
Shared fault E — see Cause 5 (page-token validation) and Cause 7 (terminal Fail-state event), described in place.

=== Cause 1 of 10: history of an execution read with includeExecutionData — no __type, no roleArn, previousEventId 1, payload re-serialized ===
Faults A+B+C+D on one page. Fix: GetExecutionHistory's serializer (emit `__type` per event), the history recorder's ExecutionStarted details (carry the state machine's roleArn), the previousEventId computation and the raw-payload echo in the execution state.
Traces: sfn-execution-included-data-and-history-paging#task#6.

=== Cause 2 of 10: subsequent history pages carry no __type ===
Fault A on the paged reads. Fix: the same serializer. Traces: sfn-execution-included-data-and-history-paging#task#7 (cluster also covers #task#8 and #task#9).

=== Cause 3 of 10: default-page history read (100 events) carries no __type, no roleArn and previousEventId 1 ===
Faults A+B+C across a full default page. Fix: as Cause 1. Traces: sfn-history-default-page-and-order-bound-token#task#7 (also #task#8).

=== Cause 4 of 10: reverse-order / second-page history read carries no __type ===
Fault A. Fix: as Cause 2. Traces: sfn-history-default-page-and-order-bound-token#task#9.

=== Cause 5 of 10: a nextToken this service never issued is accepted and answered 200 instead of refused InvalidToken ===
AWS: Smithy models GetExecutionHistory input {executionArn (required), maxResults (0..1000), reverseOrder, nextToken (@length 1..1024), includeExecutionData} and declares the error InvalidToken (400). Recorded: a nextToken that is well-formed by length but was not minted by this service is refused with HTTP 400, `__type` = `com.amazonaws.swf.service.v2.model#InvalidToken` and the message `Invalid Token` (fixed text, no variable part).
Emulator: HTTP 200 with a page of events and no error at all — the token is silently ignored.
Why: a paginator that corrupts, truncates or replays a token from another execution silently re-reads page one instead of failing, so a caller can loop forever or process the same events twice and never learn its cursor was invalid.
Fix: GetExecutionHistory's handler/validation — validate and bind the page token (to this execution and this sort order) and raise InvalidToken with AWS's fixed message when it does not resolve.
Traces: sfn-history-default-page-and-order-bound-token#task#10 (also #task#12).

=== Cause 6 of 10: last history page carries no __type ===
Fault A. Fix: as Cause 2. Traces: sfn-history-default-page-and-order-bound-token#task#11.

=== Cause 7 of 10: a Fail state terminates the history with FailStateEntered instead of an ExecutionFailed event carrying executionFailedEventDetails ===
AWS: recorded behavior — when an execution ends through a Fail state, the LAST history event has `type` = `ExecutionFailed` and carries `executionFailedEventDetails` with `error` and `cause` taken from the Fail state's Error/Cause (here error=CustomError, cause="Something went wrong" — those two values come from the definition under test, not from the requirement). The emulator ends the history with `type` = `FailStateEntered` and no executionFailedEventDetails member at all, so the execution's terminal error is not recoverable from the history. Faults A+B+C apply to the same page.
Why: the terminal ExecutionFailed event is the documented place to read why an execution failed; a client auditing failures (or counting them by event type) finds no ExecutionFailed event at all and no error/cause.
Fix: the interpreter's Fail-state handling and the history recorder — record the ExecutionFailed terminal event with executionFailedEventDetails{error, cause} after the state-entered event.
Traces: states-execution-failure-and-redrive#task#11.

=== Cause 8 of 10: success/abort lifecycle history — no __type, no roleArn, previousEventId 1, payload re-serialized ===
Faults A+B+C+D. Fix: as Cause 1. Traces: states-execution-lifecycle-success-and-abort#task#7.

=== Cause 9 of 10: standard execution history — no __type, no roleArn, previousEventId 1, and both the entered input and the exited output re-serialized ===
Faults A+B+C+D (D on stateExitedEventDetails.output as well as stateEnteredEventDetails.input). Fix: as Cause 1. Traces: states-standard-execution-lifecycle#task#6.

=== Cause 10 of 10: start/monitor/stop history — no __type and no roleArn ===
Faults A+B. Fix: as Cause 1. Traces: stepfunctions-standard-execution-start-monitor-stop#task#5.

## Evidence
Replay of trace set simparity-wire-aws-v1; all clusters verdict=mismatch, service=states, operation GetExecutionHistory.

Cause 1 — sfn-execution-included-data-and-history-paging#task#6 (status 200 both sides): `$.events[0].__type: missing (expected 'com.amazonaws.swf.base.model#HistoryEvent', got None)`; `$.events[0].executionStartedEventDetails.roleArn: missing (expected 'arn:aws:iam::<account>:role/<role name>', got None)`; `$.events[1].__type: missing`; `$.events[1].previousEventId: value_mismatch (expected 0, got 1)`; `$.events[1].stateEnteredEventDetails.input: value_mismatch (expected '{"order": "o-1"}', got '{"order":"o-1"}')`.
Cause 2 — sfn-execution-included-data-and-history-paging#task#7: `$.events[0].__type: missing`, `$.events[1].__type: missing` (cluster also #task#8, #task#9).
Cause 3 — sfn-history-default-page-and-order-bound-token#task#7: `__type` missing on events[0]..events[99]; `executionStartedEventDetails.roleArn: missing`; `$.events[1].previousEventId: expected 0, got 1` (also #task#8).
Cause 4 — sfn-history-default-page-and-order-bound-token#task#9: `__type` missing on events[0]..events[21].
Cause 5 — sfn-history-default-page-and-order-bound-token#task#10: `status: expected 400, got 200`; `$.__type: missing (expected 'com.amazonaws.swf.service.v2.model#InvalidToken', got None)`; `$.message: missing (expected 'Invalid Token', got None)` (also #task#12).
Cause 6 — sfn-history-default-page-and-order-bound-token#task#11: `$.events[0].__type: missing`.
Cause 7 — states-execution-failure-and-redrive#task#11: `$.events[id=5].type: value_mismatch (expected 'ExecutionFailed', got 'FailStateEntered')`; `$.events[id=5].executionFailedEventDetails: missing (expected {'cause': 'Something went wrong', 'error': 'CustomError'}, got None)`; plus `__type` missing on ids 1-5, `executionStartedEventDetails.roleArn: missing`, `$.events[id=2].previousEventId: expected 0, got 1`.
Cause 8 — states-execution-lifecycle-success-and-abort#task#7: `__type` missing on ids 1-4; roleArn missing; `previousEventId: expected 0, got 1`; `stateEnteredEventDetails.input: expected '{"a": 1}', got '{"a":1}'`.
Cause 9 — states-standard-execution-lifecycle#task#6: `__type` missing on ids 1-4; roleArn missing; `previousEventId: expected 0, got 1`; `stateEnteredEventDetails.input: expected '{"probe": true}', got '{"probe":true}'`; `stateExitedEventDetails.output: expected '{"probe": true}', got '{"probe":true}'`.
Cause 10 — stepfunctions-standard-execution-start-monitor-stop#task#5: `$.events[id=1].__type: missing`; `$.events[id=1].executionStartedEventDetails.roleArn: missing`.

The role ARNs, execution names, payload values and event counts quoted are this run's generated values; the requirements are the presence and identity of `__type`, the presence of roleArn, previousEventId==0 for the event following ExecutionStarted, the byte-identical echo of the caller's payload, the ExecutionFailed terminal event with its details, and the InvalidToken refusal.

## Steps to Reproduce
Faults A-D (Causes 1-4, 6, 8-10): 1) iam:CreateRole trusted by states.amazonaws.com; 2) sfn:CreateStateMachine(type=STANDARD) with a short Pass/Wait chain; 3) sfn:StartExecution(input='{"a": 1}') — note the space after the colon; 4) poll sfn:DescribeExecution until SUCCEEDED; 5) sfn:GetExecutionHistory(executionArn=<exec>, includeExecutionData=true) and also with maxResults=2 + nextToken paging and reverseOrder=true. Expect: every events[] element carries `__type` == "com.amazonaws.swf.base.model#HistoryEvent"; events[0] (ExecutionStarted) carries executionStartedEventDetails.roleArn == the state machine's roleArn; the next event's previousEventId == 0; stateEnteredEventDetails.input and stateExitedEventDetails.output echo '{"a": 1}' byte-for-byte. Observed: no `__type`, no roleArn, previousEventId 1, payload re-serialized as '{"a":1}'.
Cause 5: call sfn:GetExecutionHistory(executionArn=<exec>, nextToken="<any 1..1024-char string this service never issued>"). Expect HTTP 400, __type "com.amazonaws.swf.service.v2.model#InvalidToken", message "Invalid Token"; observed HTTP 200 with a page of events.
Cause 7: 1) create a STANDARD state machine whose StartAt chain ends in a Fail state {Error: "CustomError", Cause: "Something went wrong"}; 2) StartExecution and wait for FAILED; 3) sfn:GetExecutionHistory. Expect the last event type == "ExecutionFailed" with executionFailedEventDetails {error: "CustomError", cause: "Something went wrong"}; observed the history ends at "FailStateEntered" with no executionFailedEventDetails.
Replay: the fixture#step tokens in Issue-Trace/Issue-Step and trace_ids.
