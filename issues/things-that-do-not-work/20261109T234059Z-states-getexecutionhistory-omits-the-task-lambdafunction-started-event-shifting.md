Issue-Source: trace-sweep-divergence
Issue-Trace: sfn-failing-lambda-task-execution-diagnosis
Issue-Step: task#10
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sfn-failing-lambda-task-execution-diagnosis sfn-legacy-lambda-arn-task-failure-events
Issue-Shape-Id: com.amazonaws.sfn#AWSStepFunctions
Issue-Key: states|GetExecutionHistory|2xx->2xx|$.events[*]
Issue-Key: states|GetExecutionHistory|2xx->2xx|$.events[*].taskFailedEventDetails
Issue-Key: states|GetExecutionHistory|2xx->2xx|$.events[*].taskStartedEventDetails
Issue-Key: states|GetExecutionHistory|2xx->2xx|$.events[*].type
Issue-Key: states|GetExecutionHistory|2xx->2xx|$.nextToken
Issue-Key: states|GetExecutionHistory|2xx->2xx|$.events[*].executionRedrivenEventDetails
Issue-Key: states|GetExecutionHistory|2xx->2xx|$.events[*].previousEventId
Issue-Key: states|GetExecutionHistory|2xx->2xx|$.events[*].taskScheduledEventDetails
Issue-Related: 20261105T115709Z-states-retry-catch-and-legacy-lambda-task-modelling-diverge.md
Issue-Commit: 0a7cae03cb1d4210d38997f5682a54b52a42bce2

# GetExecutionHistory omits the Task/LambdaFunction Started event, shifting every later event id (4 causes)

> _Note: this issue was identified against the code base at commit `0a7cae03cb1d4210d38997f5682a54b52a42bce2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetExecutionHistory

## Description
GetExecutionHistory's event stream for a Task state is missing the lifecycle event that marks the task having been handed to the integration. Because HistoryEvent.id is sequential and previousEventId chains to it, dropping one event does not merely lose a record: every subsequent event is renumbered one lower, so ids, previousEventId links and page boundaries all disagree with AWS for the rest of the execution. 4 causes, all on the same event stream.

Cause 1 of 4 — the TaskStarted event is absent for an optimized Lambda Task, and the page therefore ends one event early.
AWS: Smithy: HistoryEventType enumerates TaskScheduled, TaskStarted, TaskFailed and ExecutionFailed, and HistoryEvent carries taskStartedEventDetails (TaskStartedEventDetails, with required resourceType and resource) and taskFailedEventDetails (TaskFailedEventDetails, with error/cause/resource/resourceType). HistoryEvent.id is documented "Events are numbered sequentially, starting at one" and previousEventId is "the id of the previous event". For an optimized integration Task that fails, AWS emits, in order, the state-entered event, TaskScheduled, TaskStarted (taskStartedEventDetails.resourceType = the integration's service name, .resource = the integration's action), TaskFailed, then ExecutionFailed.
Emulator: the TaskStarted event is never emitted. In its slot the emulator puts the TaskFailed event, in the TaskFailed slot the ExecutionFailed event, and the last requested event is missing entirely (the position where AWS returns ExecutionFailed with id 6/previousEventId 5 holds nothing). Neither the taskStartedEventDetails nor the taskFailedEventDetails structure is present where AWS puts it.
Why: a client that walks the history to find when a task was dispatched (latency between TaskScheduled and TaskStarted, or the resourceType/resource pair identifying which integration ran) finds no such event at all, and a client that reads a fixed-size page gets a short page whose last event is not the terminal one. Any logic keyed on event id or previousEventId — resuming a walk, correlating a redrive, rendering the execution graph — is off by one for the whole execution.
Fix: the execution interpreter's history-event recorder for Task states (the component that appends events to the execution's history) — it must append a TaskStarted event, carrying taskStartedEventDetails, between TaskScheduled and the task's outcome event, and the terminal ExecutionFailed event must be included in the page.
Traces: sfn-failing-lambda-task-execution-diagnosis#task#10 (cluster also covers sfn-failing-lambda-task-execution-diagnosis#task#12)

Cause 2 of 4 — the same omission on the legacy Lambda-ARN Task path: the LambdaFunctionStarted event is absent.
AWS: Smithy: HistoryEventType enumerates LambdaFunctionScheduled, LambdaFunctionStarted, LambdaFunctionFailed; HistoryEvent carries lambdaFunctionFailedEventDetails (error, cause). LambdaFunctionStarted has no details structure. For a Task whose Resource is a bare Lambda function ARN, AWS emits LambdaFunctionScheduled, LambdaFunctionStarted, LambdaFunctionFailed (with lambdaFunctionFailedEventDetails), then ExecutionFailed.
Emulator: LambdaFunctionStarted is never emitted; the LambdaFunctionFailed event occupies its slot with no lambdaFunctionFailedEventDetails where AWS carries it, ExecutionFailed moves into the LambdaFunctionFailed slot, and the terminal ExecutionFailed (AWS id 6, previousEventId 5) is missing from the page.
Why: the legacy Lambda task path is indistinguishable from a task that was scheduled and never started, and the same off-by-one id shift propagates to every later event.
Fix: the same history-event recorder, on the legacy Lambda-ARN Task branch — emit LambdaFunctionStarted between LambdaFunctionScheduled and the function's outcome event.
Traces: sfn-legacy-lambda-arn-task-failure-events#task#10

Cause 3 of 4 — reading the tail of the history (reverseOrder) returns the terminal event with the wrong id and previousEventId, a direct consequence of the missing Started event.
AWS: Smithy: GetExecutionHistory.input.reverseOrder (default false) and HistoryEvent.id/previousEventId as above. Reading the failed execution's history newest-first, the first event AWS returns is ExecutionFailed with id 6 and previousEventId 5.
Emulator: the first event is ExecutionFailed with id 5 and previousEventId 4 — one lower, because the stream is one event short.
Why: a client that correlates an event id obtained from one read (a log record, a prior forward page, a CloudWatch Logs delivery) with a reverse read gets a different id for the same event, and previousEventId chains that are expected to meet do not.
Fix: same recorder/state — once the Started event is recorded, the sequential ids and previousEventId links align with AWS; the reverseOrder read path itself needs no separate change.
Traces: sfn-failing-lambda-task-execution-diagnosis#task#11

Cause 4 of 4 — after a redrive, the history read shows the same missing-Started/shifted-id pattern across the redriven attempt.
AWS: Smithy: HistoryEventType includes ExecutionRedriven and HistoryEvent carries executionRedrivenEventDetails; taskScheduledEventDetails and taskStartedEventDetails bracket each attempt. Reading a redriven execution's history, AWS returns the redrive's ExecutionFailed at id 11/previousEventId 10, its TaskFailed at id 10/previousEventId 9 with taskFailedEventDetails, a TaskStarted with taskStartedEventDetails, the TaskScheduled with taskScheduledEventDetails, and the ExecutionRedriven event with executionRedrivenEventDetails.
Emulator: the TaskStarted event is again absent, so every event of the redriven attempt is returned two ids lower than AWS's (ExecutionFailed at id 9/previousEventId 8, TaskFailed at id 8/previousEventId 7), the taskStartedEventDetails member is absent, and the types at the asserted positions are shifted (the ExecutionRedriven and TaskScheduled slots hold other event types).
Why: redrive is specified in terms of the event id an attempt resumed from, so a client that reads redriveCount/redrive bookkeeping against history ids cannot line up an attempt with its events.
Fix: same recorder — the per-attempt event sequence appended on redrive must include the Started event, so attempt boundaries and ids match AWS.
Traces: sfn-failing-lambda-task-execution-diagnosis#task#19

Note on already-filed issues: the resolved issue 20261103T052008Z-states-step-functions-execution-history-omits-task-lifecycle-events.md and the resolved 20261027T000406Z-sfn-step-functions-execution-history-omits-the-started-succeeded.md addressed earlier history-event gaps, but the replay of this checkout still diverges on these four clusters and neither resolved issue covers them. The open 20261105T115709Z-states-retry-catch-and-legacy-lambda-task-modelling-diverge.md and 20261103T052008Z-states-execution-outcome-error-name-and-cause-text-diverge.md own the error-name and cause-text values of these events, not the missing event or the id shift reported here.

## Evidence
All four causes are mismatch clusters from the replay of simparity-wire-aws-v1 (trace set simparity-wire-aws-v1) against the emulator built from this checkout. Every step returned HTTP 200 as expected; the divergence is in the body.

Cause 1 — sfn-failing-lambda-task-execution-diagnosis#task#10 (cluster also covers #task#12), GetExecutionHistory, 200 -> 200:
  $.events[3].taskStartedEventDetails: expected {'resource': 'invoke', 'resourceType': 'lambda'}, got None
  $.events[3].type: expected 'TaskStarted', got 'TaskFailed'
  $.events[4].taskFailedEventDetails: expected {'cause': <not asserted>, 'error': 'RuntimeError', 'resource': 'invoke', 'resourceType': 'lambda'}, got None
  $.events[4].type: expected 'TaskFailed', got 'ExecutionFailed'
  $.events[5]: expected a HistoryEvent {'type': 'ExecutionFailed', 'id': 6, 'previousEventId': 5, 'executionFailedEventDetails': {...}}, got None

Cause 2 — sfn-legacy-lambda-arn-task-failure-events#task#10, GetExecutionHistory, 200 -> 200:
  $.events[3].type: expected 'LambdaFunctionStarted', got 'LambdaFunctionFailed'
  $.events[4].lambdaFunctionFailedEventDetails: expected {'cause': <not asserted>, 'error': 'RuntimeError'}, got None
  $.events[4].type: expected 'LambdaFunctionFailed', got 'ExecutionFailed'
  $.events[5]: expected {'type': 'ExecutionFailed', 'id': 6, 'previousEventId': 5, ...}, got None

Cause 3 — sfn-failing-lambda-task-execution-diagnosis#task#11, GetExecutionHistory, 200 -> 200:
  $.events[0]: expected {'type': 'ExecutionFailed', 'id': 6, 'previousEventId': 5, 'executionFailedEventDetails': {...}}, got the same event type with 'id': 5, 'previousEventId': 4
  (the same step also diverges on $.nextToken, filed separately as the non-opaque pagination token)

Cause 4 — sfn-failing-lambda-task-execution-diagnosis#task#19, GetExecutionHistory, 200 -> 200:
  $.events[0]: expected {'type': 'ExecutionFailed', 'id': 11, 'previousEventId': 10, ...}, got {'type': 'ExecutionFailed', 'id': 9, 'previousEventId': 8, ...}
  $.events[1]: expected {'type': 'TaskFailed', 'id': 10, 'previousEventId': 9, 'taskFailedEventDetails': {'resource': 'invoke', 'resourceType': 'lambda', ...}}, got the same type with 'id': 8, 'previousEventId': 7
  $.events[2].taskStartedEventDetails: expected {'resource': 'invoke', 'resourceType': 'lambda'}, got None; $.events[2].type diverges, and so do $.events[3].type/.previousEventId/.taskScheduledEventDetails and $.events[4].type/.executionRedrivenEventDetails

The recorded event ids, the per-run cause strings and the timestamps in these clusters are environment values; the requirement asserted here is the presence and ORDER of the Started event, the presence of the *EventDetails structure on each event, and the sequential id/previousEventId chain that follows from them.

## Steps to Reproduce
1. Build the emulator from this checkout and replay the wire corpus simparity-wire-aws-v1.
2. Fixture sfn-failing-lambda-task-execution-diagnosis, step task#10 (the representative): start an execution whose Task uses an optimized lambda:invoke Resource and fails, then call GetExecutionHistory. Compare $.events[3..5]: AWS returns TaskStarted (with taskStartedEventDetails) then TaskFailed (with taskFailedEventDetails) then ExecutionFailed; the emulator returns TaskFailed then ExecutionFailed and nothing at $.events[5].
3. Fixture sfn-legacy-lambda-arn-task-failure-events, step task#10: same with a Task whose Resource is a bare Lambda function ARN — the LambdaFunctionStarted event is absent.
4. Fixture sfn-failing-lambda-task-execution-diagnosis, step task#11: call GetExecutionHistory with reverseOrder on the same failed execution and compare $.events[0].id / previousEventId (6/5 on AWS, 5/4 on the emulator).
5. Fixture sfn-failing-lambda-task-execution-diagnosis, step task#19: redrive the failed execution, then read the history — the redriven attempt's events are returned two ids low and carry no taskStartedEventDetails.
