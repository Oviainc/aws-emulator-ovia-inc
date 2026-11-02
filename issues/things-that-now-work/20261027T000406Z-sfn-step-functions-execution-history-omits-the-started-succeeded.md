Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: stepfunctions.deep-sfn-2-sfn-jsonata-context-retry-count-increments-per-attempt stepfunctions.sfn-retry-exhaustion-routes-to-catch-and-the-success-state-is-never-entered stepfunctions.deep-sfn-language-fail-error-path-cause-path-reach-execution stepfunctions.history-event-chain-pairing-and-reverse-order stepfunctions.map-maxconcurrency-one-serialises-iterations-against-an-unbounded-twin stepfunctions.deep-sfn-1-sfn-sync-lambda-function-timeout-is-sandbox-timedout-not-states-timeout stepfunctions.deep-sfn-1-sfn-sync-lambda-unhandled-exception-error-and-json-cause stepfunctions.parallel-result-ordering-and-sibling-branch-abort
Issue-Shape-Id: com.amazonaws.sfn#AWSStepFunctions
Issue-Key: stepfunctions.deep-sfn-2-sfn-jsonata-context-retry-count-increments-per-attempt|run#17|AssertionError|assert len(starts) == _MAX_ATTEMPTS + 1, (
Issue-Key: stepfunctions.sfn-retry-exhaustion-routes-to-catch-and-the-success-state-is-never-entered|run#12|AssertionError|assert started == _MAX_ATTEMPTS + 1, (
Issue-Key: stepfunctions.deep-sfn-language-fail-error-path-cause-path-reach-execution|run#5|AssertionError|assert len(par_failed) == 1, (
Issue-Key: stepfunctions.history-event-chain-pairing-and-reverse-order|idx_of_type#17|AssertionError|assert t in types, f"history is missing a {t} event; types were {types}"
Issue-Key: stepfunctions.map-maxconcurrency-one-serialises-iterations-against-an-unbounded-twin|_iteration_start_timestamps#16|AssertionError|assert starts, (
Issue-Key: stepfunctions.deep-sfn-1-sfn-sync-lambda-function-timeout-is-sandbox-timedout-not-states-timeout|run#9|AssertionError|assert len(task_failed) == 1, f"expected exactly one TaskFailed: {types!r}"
Issue-Key: stepfunctions.deep-sfn-1-sfn-sync-lambda-unhandled-exception-error-and-json-cause|run#10|AssertionError|assert len(task_failed) == 1, f"expected exactly one TaskFailed: {types!r}"
Issue-Key: stepfunctions.parallel-result-ordering-and-sibling-branch-abort|run#15|AssertionError|assert "SlowDone" not in entered, (
Issue-Commit: 6402e991ee1ffb1f37749ece0148ecf3009a760a

# Step Functions execution history omits the Started/Succeeded/Failed events for Parallel, Map and Task states, and a failing Parallel branch never aborts its in-flight siblings (4 causes)

> _Note: this issue was identified against the code base at commit `6402e991ee1ffb1f37749ece0148ecf3009a760a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetExecutionHistory

## Description
Four net-new root causes in the Step Functions execution-history recorder and the Parallel state's abort semantics. The emulator records only the `*StateEntered` / `*StateExited` pair for every compound state and never the lifecycle events AWS emits between them, so any client that reasons about attempts, iterations or task outcomes from the history is blind. The open issues 20261024T043150Z-sfn-step-functions-interpreter-does-not-run-the-workflow.md and 20261023T114649Z-sfn-step-functions-never-executes-task-states-every-execution.md describe the interpreter not running states at all; these four causes are different — the states DO run here (the Entered/Exited events and the final ExecutionSucceeded/ExecutionFailed are present), the history stream and the abort rule are what diverge. Nothing already filed covers them.

Cause 1 of 4: Parallel state emits no ParallelStateStarted / ParallelStateSucceeded / ParallelStateFailed.
AWS: Smithy HistoryEventType enum (com.amazonaws.sfn#HistoryEventType) models ParallelStateEntered, ParallelStateStarted, ParallelStateSucceeded, ParallelStateFailed, ParallelStateAborted and ParallelStateExited. AWS emits ParallelStateStarted once per attempt at the Parallel state (so a Retry of the Parallel produces one ParallelStateStarted per attempt), ParallelStateSucceeded when all branches complete, and ParallelStateFailed when a branch fails.
Emulator: only ParallelStateEntered and ParallelStateExited appear. Recorded histories: `['ExecutionStarted','ParallelStateEntered','ChoiceStateEntered','ChoiceStateExited','FailStateEntered', ... ,'ParallelStateExited','PassStateEntered','PassStateExited','ExecutionSucceeded']` — the three retry attempts are visible as repeated Choice/Fail pairs, yet ParallelStateStarted count is 0 where MaxAttempts=2 requires 3. A Parallel whose branch runs a Fail state produces `['ExecutionStarted','ParallelStateEntered','FailStateEntered','ExecutionFailed']` with no ParallelStateFailed. A fully successful Parallel produces `...'ParallelStateEntered','PassStateEntered','PassStateExited','PassStateEntered','PassStateExited','ParallelStateExited'...` with no ParallelStateSucceeded.
Why: a client cannot count Parallel attempts, cannot pair a Parallel's start with its outcome, and cannot distinguish a Parallel that failed from one that was merely exited. Retry/Catch observability built on the history breaks.
Fix: the history recorder for the Parallel state in the sfn crate's ASL interpreter — emit ParallelStateStarted on each attempt and ParallelStateSucceeded/ParallelStateFailed/ParallelStateAborted on the branch outcome, between Entered and Exited.
Traces: sfn.deep-sfn-2-sfn-jsonata-context-retry-count-increments-per-attempt (0 ParallelStateStarted, expected 3), sfn.sfn-retry-exhaustion-routes-to-catch-and-the-success-state-is-never-entered (Parallel started 0 times for MaxAttempts=2, expected 3), sfn.deep-sfn-language-fail-error-path-cause-path-reach-execution (expected one ParallelStateFailed, got none), sfn.history-event-chain-pairing-and-reverse-order (history missing ParallelStateSucceeded).

Cause 2 of 4: inline Map emits no MapStateStarted / MapIterationStarted / MapIterationSucceeded / MapStateSucceeded.
AWS: Smithy HistoryEventType models MapStateStarted, MapIterationStarted, MapIterationSucceeded, MapIterationFailed, MapIterationAborted, MapStateSucceeded, MapStateFailed. An inline Map emits one MapIterationStarted and one MapIterationSucceeded per item, each carrying the iteration index, which is how a caller observes that MaxConcurrency serialised the iterations.
Emulator: the only Map events present are MapStateEntered and MapStateExited — recorded types for an inline Map over several items were `['ExecutionStarted','ExecutionSucceeded','MapStateEntered','MapStateExited','PassStateEntered','PassStateExited','WaitStateEntered','WaitStateExited']`, i.e. one Entered/Exited pair for the whole Map and no per-iteration events at all.
Why: a client cannot observe iteration fan-out, per-item outcome, or the effect of MaxConcurrency; nothing distinguishes a Map that ran one item from one that ran a hundred.
Fix: the history recorder for the inline Map state in the sfn crate's ASL interpreter — emit MapStateStarted, then a MapIterationStarted/MapIterationSucceeded (or Failed/Aborted) pair per item carrying the iteration index, then MapStateSucceeded/MapStateFailed.
Traces: sfn.map-maxconcurrency-one-serialises-iterations-against-an-unbounded-twin.

Cause 3 of 4: a failing Task state emits no TaskScheduled / TaskStarted / TaskFailed — the execution jumps straight from TaskStateEntered to ExecutionFailed.
AWS: Smithy HistoryEventType models TaskScheduled, TaskStarted, TaskSubmitted, TaskSucceeded, TaskFailed, TaskTimedOut, TaskStartFailed, TaskSubmitFailed, TaskStateAborted, TaskStateExited. When a Task's underlying call fails, AWS records exactly one TaskFailed event carrying the error name and cause, between TaskStateEntered and the execution's own terminal event.
Emulator: the recorded history is `['ExecutionStarted','TaskStateEntered','ExecutionFailed']` — no TaskScheduled, no TaskStarted, no TaskFailed. The failure is reported only on the execution, never on the state that caused it.
Why: a caller cannot attribute a failure to the Task that produced it, cannot read the Task's error/cause from the history, and cannot tell a task-level failure (TaskFailed) from a task timeout (TaskTimedOut) or a scheduling failure (TaskStartFailed) — distinctions AWS models as separate event types and clients key retries and alarms on.
Fix: the Task state's history recorder in the sfn crate's ASL interpreter — emit TaskScheduled/TaskStarted on entry and exactly one TaskFailed (or TaskTimedOut / TaskStartFailed / TaskSubmitFailed as appropriate) carrying the error name and cause before the execution terminates.
Traces: sfn.deep-sfn-1-sfn-sync-lambda-function-timeout-is-sandbox-timedout-not-states-timeout, sfn.deep-sfn-1-sfn-sync-lambda-unhandled-exception-error-and-json-cause (both: expected exactly one TaskFailed, got types ['ExecutionStarted','TaskStateEntered','ExecutionFailed']).

Cause 4 of 4: a Parallel branch failure does not abort the in-flight sibling branches.
AWS: recorded behavior only (the ASL Parallel state's documented rule). When any branch of a Parallel fails, AWS stops the remaining branches immediately — the states after the point of abort in a surviving branch are never entered, and those branches record MapIterationAborted/ParallelStateAborted-class events rather than completing.
Emulator: the sibling branch runs to completion while the other branch fails. In the recording the surviving branch was parked in a long Wait and the state after that Wait was still entered: entered states were `['Fan','LongWait','SlowDone','Boom','Recovered']` where `SlowDone` (the state after the Wait in the surviving branch) must never be entered once `Boom` fails.
Why: side effects in the surviving branch keep firing after the Parallel has already failed — a client that relies on a Parallel failure as a fence (no further writes from any branch) sees duplicated or post-failure work.
Fix: the Parallel state's branch scheduler in the sfn crate's ASL interpreter — on the first branch failure, cancel the other branches' pending work instead of letting them run to completion.
Traces: sfn.parallel-result-ordering-and-sibling-branch-abort.

## Evidence
Cause 1 — sfn.sfn-retry-exhaustion-routes-to-catch-and-the-success-state-is-never-entered:
  expected: 3 ParallelStateStarted events (first attempt + 2 retries for MaxAttempts=2)
  actual:   0; history was ['ExecutionStarted','ParallelStateEntered','ChoiceStateEntered','ChoiceStateExited','FailStateEntered','ChoiceStateEntered','ChoiceStateExited','FailStateEntered','ChoiceStateEntered','ChoiceStateExited','FailStateEntered','ParallelStateExited','PassStateEntered','PassStateExited','ExecutionSucceeded']
Cause 1 — sfn.deep-sfn-2-sfn-jsonata-context-retry-count-increments-per-attempt:
  AssertionError: 0 ParallelStateStarted events, expected one per attempt (3)
Cause 1 — sfn.deep-sfn-language-fail-error-path-cause-path-reach-execution:
  AssertionError: expected one ParallelStateFailed event, got types ['ExecutionStarted','ParallelStateEntered','FailStateEntered','ExecutionFailed']
Cause 1 — sfn.history-event-chain-pairing-and-reverse-order:
  AssertionError: history is missing a ParallelStateSucceeded event; types were ['ExecutionStarted','PassStateEntered','PassStateExited','ChoiceStateEntered','ChoiceStateExited','WaitStateEntered','WaitStateExited','ParallelStateEntered','PassStateEntered','PassStateExited','PassStateEntered','PassStateExited','ParallelStateExited','SucceedStateEntered','SucceedStateExited','ExecutionSucceeded']

Cause 2 — sfn.map-maxconcurrency-one-serialises-iterations-against-an-unbounded-twin:
  AssertionError: history must record MapIterationStarted events for an inline Map; the types present were ['ExecutionStarted','ExecutionSucceeded','MapStateEntered','MapStateExited','PassStateEntered','PassStateExited','WaitStateEntered','WaitStateExited']

Cause 3 — sfn.deep-sfn-1-sfn-sync-lambda-unhandled-exception-error-and-json-cause and sfn.deep-sfn-1-sfn-sync-lambda-function-timeout-is-sandbox-timedout-not-states-timeout:
  AssertionError: expected exactly one TaskFailed: ['ExecutionStarted', 'TaskStateEntered', 'ExecutionFailed']

Cause 4 — sfn.parallel-result-ordering-and-sibling-branch-abort:
  AssertionError: the surviving branch must be aborted mid-Wait, so 'SlowDone' must never be entered; entered states were ['Fan','LongWait','SlowDone','Boom','Recovered']

Smithy reference (com-amazonaws-sfn-awsstepfunctions.json, HistoryEventType enum) models all of: ParallelStateStarted, ParallelStateSucceeded, ParallelStateFailed, ParallelStateAborted, MapStateStarted, MapIterationStarted, MapIterationSucceeded, MapIterationFailed, MapIterationAborted, MapStateSucceeded, TaskScheduled, TaskStarted, TaskFailed, TaskTimedOut, TaskStartFailed. None of these are ever emitted by the emulator.

## Steps to Reproduce
1. Create an execution role Step Functions may assume and a STANDARD state machine.
2. Cause 1: define a Parallel state whose branch always fails with a custom error, with Retry {ErrorEquals:[that error], MaxAttempts:2} on the Parallel and a Catch routing to a Pass. StartExecution, wait for terminal, GetExecutionHistory. Count events of type ParallelStateStarted -> 0 (AWS: 3). Also define a Parallel whose branches all succeed and look for ParallelStateSucceeded -> absent. Also define a Parallel containing a Fail state and look for ParallelStateFailed -> absent.
3. Cause 2: define an inline Map over N items, each iteration a Pass plus a Wait. StartExecution, wait for terminal, GetExecutionHistory. No MapStateStarted / MapIterationStarted / MapIterationSucceeded / MapStateSucceeded events appear; only MapStateEntered and MapStateExited.
4. Cause 3: define a single Task state whose underlying call fails (unhandled handler exception, or a handler that outruns its function timeout). StartExecution, wait for terminal, GetExecutionHistory. The history is ['ExecutionStarted','TaskStateEntered','ExecutionFailed'] — no TaskScheduled/TaskStarted/TaskFailed.
5. Cause 4: define a Parallel with two branches — branch A: a long Wait followed by a state 'SlowDone'; branch B: a short delay followed by a Fail. StartExecution, wait for terminal, then read every stateEnteredEventDetails.name from GetExecutionHistory. 'SlowDone' is present; on AWS the abort of branch A prevents it.
