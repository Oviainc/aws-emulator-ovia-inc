Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: stepfunctions.choice-state-exited-output-and-map-choice-history-count stepfunctions.redrive-of-jsonata-parallel-evaluation-failure-fails-again stepfunctions.deep-sfn-3-parallel-retry-reruns-completed-sibling stepfunctions.pass-context-object-shape-excludes-retry-count-and-task stepfunctions.get-execution-history-page-is-byte-bounded-not-just-count-bounded stepfunctions.fail-state-null-error-uncaught-by-named-parallel-catch stepfunctions.deep-sfn-language-choice-operand-path-null-vs-absent-rejected stepfunctions.choice-timestampgreaterthanpath-non-string-target-rejected stepfunctions.jsonata-fail-and-succeed-evaluate-expressions-undefined-rejected stepfunctions.sfn-jsonata-undefined-result-query-evaluation-error stepfunctions.jsonata-task-arguments-evaluating-to-non-object-fails-with-states-runtime stepfunctions.sfn-jsonata-evaluation-failed-location-truncated-over-256-chars stepfunctions.choice-jsonata-rule-assign-scoped-inside-map-caught-by-catch stepfunctions.standard-parent-sync-runs-express-child-to-completion stepfunctions.log-sfn-sync-nonexistent-child-rejected stepfunctions.log-sfn-sync-child-failure-cause-redacts-input-but-keeps-failed-status stepfunctions.sync-task-output-is-full-describeexecution-shape stepfunctions.deep-sfn-2-sfn-jsonata-state-error-caught-by-jsonpath-catch stepfunctions.conc-distributed-map-zero-tolerance-run-fails-and-keeps-completed-items
Issue-Shape-Id: com.amazonaws.sfn#AWSStepFunctions
Issue-Key: stepfunctions.choice-state-exited-output-and-map-choice-history-count|run#10|AssertionError|assert types == [
Issue-Key: stepfunctions.redrive-of-jsonata-parallel-evaluation-failure-fails-again|run#17|AssertionError|assert types == [
Issue-Key: stepfunctions.deep-sfn-3-parallel-retry-reruns-completed-sibling|run#7|AssertionError|assert desc["status"] == "SUCCEEDED", (
Issue-Key: stepfunctions.pass-context-object-shape-excludes-retry-count-and-task|run#21|AssertionError|assert "RetryCount" not in state, (
Issue-Key: stepfunctions.get-execution-history-page-is-byte-bounded-not-just-count-bounded|run#16|AssertionError|assert large_page.get("nextToken") is not None, (
Issue-Key: stepfunctions.fail-state-null-error-uncaught-by-named-parallel-catch|run#4|AssertionError|assert null_result.get("error") is None, (
Issue-Key: stepfunctions.deep-sfn-language-choice-operand-path-null-vs-absent-rejected|run#7|AssertionError|assert null_run["status"] == "FAILED", (
Issue-Key: stepfunctions.choice-timestampgreaterthanpath-non-string-target-rejected|run#8|AssertionError|assert bad["status"] == "FAILED", (
Issue-Key: stepfunctions.jsonata-fail-and-succeed-evaluate-expressions-undefined-rejected|run#16|AssertionError|assert succeed_bad["status"] == "FAILED", (
Issue-Key: stepfunctions.sfn-jsonata-undefined-result-query-evaluation-error|run#4|AssertionError|assert desc["status"] == "FAILED", (
Issue-Key: stepfunctions.jsonata-task-arguments-evaluating-to-non-object-fails-with-states-runtime|run#9|AssertionError|assert exe["status"] == "FAILED", f"a string Arguments must FAIL, got {exe['status']!r}"
Issue-Key: stepfunctions.sfn-jsonata-evaluation-failed-location-truncated-over-256-chars|run#9|AssertionError|assert at_limit["status"] == "FAILED", f"at-limit execution ended {at_limit['status']!r}"
Issue-Key: stepfunctions.choice-jsonata-rule-assign-scoped-inside-map-caught-by-catch|run#9|AssertionError|assert out is not None and out.get("caught") is True, (
Issue-Key: stepfunctions.standard-parent-sync-runs-express-child-to-completion|run#8|AssertionError|assert parent["status"] == "SUCCEEDED", (
Issue-Key: stepfunctions.log-sfn-sync-nonexistent-child-rejected|run#21|AssertionError|assert len(failed) == 1 and failed[0]["taskFailedEventDetails"]["resource"] == _SYNC_RESOURCE, (
Issue-Key: stepfunctions.log-sfn-sync-child-failure-cause-redacts-input-but-keeps-failed-status|run#19|AssertionError|assert "Input" not in cause and "InputDetails" not in cause, (
Issue-Key: stepfunctions.sync-task-output-is-full-describeexecution-shape|run#22|AssertionError|assert task_result.get("InputDetails", {}).get("Included") is True, (
Issue-Key: stepfunctions.deep-sfn-2-sfn-jsonata-state-error-caught-by-jsonpath-catch|run#9|AttributeError|assert out is not None and out.get("Error") == "States.QueryEvaluationError", (
Issue-Related: 20261107T233020Z-stepfunctions-3-net-new-causes-an-unresolvable-choice.md
Issue-Commit: 4d67debdc5b6a4a4b59f91b6ecdc4b307fee9666

# stepfunctions: 10 net-new causes — history event sequences, Parallel retry, context object, Choice/JSONata failure semantics and startExecution.sync shapes

> _Note: this issue was identified against the code base at commit `4d67debdc5b6a4a4b59f91b6ecdc4b307fee9666`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetExecutionHistory

## Description
Ten independent root causes in execution-history recording, retry semantics, Choice/JSONata failure decisions and the nested-execution (.sync) task's shapes. Each must be closed separately.

Cause 1 of 10: recorded history event sequences diverge from AWS.
AWS: a Map over 2 items followed by a Choice records a specific 18-event sequence, with MapIterationStarted/MapIterationSucceeded per item and the Map's own entered/started/succeeded/exited events in AWS's order; a redriven execution records ExecutionRedriven followed by the re-run state's full entered/started/failed events and EvaluationFailed before ExecutionFailed in AWS's order.
Emulator: emits 18 events whose order/content does not match (`MapStateSucceeded` before `MapStateExited`, no per-iteration entered/exited pairing as AWS records it), and for the redrive case the shape `[... 'ExecutionFailed', 'ExecutionRedriven', 'ParallelStateStarted', 'ParallelStateFailed', 'EvaluationFailed', 'ExecutionFailed']` with no ParallelStateEntered on the re-run.
Why: any consumer that replays or audits history (CloudWatch, a redrive UI, a client stepping events) sees a sequence AWS never produces.
Fix: event emission order in the Map/Choice/Parallel state handlers and in the redrive path of the history recorder.
Traces: stepfunctions.choice-state-exited-output-and-map-choice-history-count, stepfunctions.redrive-of-jsonata-parallel-evaluation-failure-fails-again

Cause 2 of 10: a retried Parallel re-runs its branches and never succeeds.
AWS: a Parallel with a Retrier re-enters on the second attempt — ParallelStateEntered is recorded again, the transient failure does not recur, and the execution SUCCEEDS; a completed sibling branch is not re-run.
Emulator: the execution ends FAILED with the original error after three identical `ParallelStateStarted … ParallelStateFailed` cycles and no ParallelStateEntered between them.
Why: retry on Parallel is unusable, and re-running completed branches duplicates their side effects.
Fix: the Retry path of the Parallel state handler (re-entry and per-branch state).
Traces: stepfunctions.deep-sfn-3-parallel-retry-reruns-completed-sibling

Cause 3 of 10: the context object carries State.RetryCount for a state that was never retried.
AWS: `$$.State` on a state that has not been retried omits RetryCount (and omits Task), so the context object shape excludes it.
Emulator: `$$.State` carries `RetryCount: 0`.
Why: a client reading the context object sees a member AWS does not supply, and JSONata/JSONPath over `$$` yields a different document.
Fix: the context-object builder.
Traces: stepfunctions.pass-context-object-shape-excludes-retry-count-and-task

Cause 4 of 10: GetExecutionHistory pages are not byte-bounded.
AWS: a page of history is bounded by bytes as well as by count — a 62-event history of large-payload events returns a nextToken.
Emulator: returns all 62 events with no nextToken.
Why: a client paging a large history gets an over-sized single response and never exercises its pagination path.
Fix: the page-size accounting in the GetExecutionHistory handler.
Traces: stepfunctions.get-execution-history-page-is-byte-bounded-not-just-count-bounded

Cause 5 of 10: a Fail state with Error set to null is turned into States.Runtime.
AWS: an uncaught Fail whose Error is null surfaces a null execution `error`, and is NOT matched by a Catch naming a specific error.
Emulator: the execution reports error `States.Runtime`.
Why: the null error AWS surfaces becomes a real error name, which can be caught by rules that must not match it.
Fix: the Fail state handler's error propagation.
Traces: stepfunctions.fail-state-null-error-uncaught-by-named-parallel-catch

Cause 6 of 10: Choice rules silently fall through where AWS fails the execution.
AWS: a Choice operand path that resolves to an explicit null, and a TimestampGreaterThanPath whose target resolves to a number, each FAIL the execution (States.Runtime); they do not fall through to Default.
Emulator: both SUCCEED, taking the Default branch (`output='"fell-back"'`).
Why: a type error in a Choice rule is swallowed and the workflow takes a path AWS never takes — a wrong answer rather than an error.
Fix: operand type/null validation in the Choice rule evaluator.
Traces: stepfunctions.deep-sfn-language-choice-operand-path-null-vs-absent-rejected, stepfunctions.choice-timestampgreaterthanpath-non-string-target-rejected

Cause 7 of 10: JSONata results that are undefined, or of the wrong kind, do not fail the execution.
AWS: a JSONata `Output`/`Arguments`/result query that evaluates to undefined fails the execution States.QueryEvaluationError — including on a Succeed and a Fail state; `Arguments` evaluating to a non-object (a string) fails; and an evaluation-failure `Location` longer than 256 characters is truncated while the execution still FAILS.
Emulator: all of these SUCCEED — `status='SUCCEEDED'`, with `output='null'` for the undefined case and the at-limit location case ending SUCCEEDED.
Why: a query that produced nothing is treated as a successful null result, so a broken workflow reports success.
Fix: the undefined/type checks after JSONata evaluation in the query evaluator and the Succeed/Fail/Task argument paths.
Traces: stepfunctions.jsonata-fail-and-succeed-evaluate-expressions-undefined-rejected, stepfunctions.sfn-jsonata-undefined-result-query-evaluation-error, stepfunctions.jsonata-task-arguments-evaluating-to-non-object-fails-with-states-runtime, stepfunctions.sfn-jsonata-evaluation-failed-location-truncated-over-256-chars

Cause 8 of 10: a Map's Catch does not run when a JSONata Choice rule's Assign fails inside the Map.
AWS: the failure inside the iteration propagates to the Map state, whose Catch runs (the output records the caught branch).
Emulator: the Map completes with `{'caught': False, 'items': [...]}` — the Catch never ran.
Why: errors inside Map iterations escape the error handling declared for them, so a workflow continues on unhandled failure.
Fix: error propagation from an iteration to the Map state's Catch in the Map handler.
Traces: stepfunctions.choice-jsonata-rule-assign-scoped-inside-map-caught-by-catch

Cause 9 of 10: the states:startExecution.sync task's refusals, events and shapes diverge.
AWS: `arn:aws:states:::states:startExecution.sync:2` accepts an EXPRESS child and runs it to completion; a nonexistent child records exactly one TaskFailed event on that resource and fails with StepFunctions.StateMachineDoesNotExistException; the TaskFailed cause for a failed child is REDACTED — it carries ExecutionArn/StateMachineArn/Name/Status/Error/Cause/RedriveCount/RedriveStatus and NOT Input/InputDetails; and the DescribeExecution-shaped payload contains only modelled members — Smithy: CloudWatchEventsExecutionDataDetails has exactly one member, `included` (boolean), so `Included=True` is the whole shape.
Emulator: the parent fails `StepFunctions.InvalidArnException` `Invalid Arn: 'Resource type not valid in this context: express' ...` for an Express child; records no TaskFailed for a nonexistent child (`expected one TaskFailed on 'startExecution.sync:2': []`); leaks `Input` and `InputDetails` (including the secret marker) into the redacted cause; adds a non-modelled `__type: com.amazonaws.swf.base.model#CloudWatchEventsExecutionDataDetails` member to InputDetails; and returns a JSON array where the catcher's error output must be an object (the client crashes with `AttributeError: 'list' object has no attribute 'get'` reading `Error` off it).
Why: nested synchronous executions cannot call Express children, lose their failure event, LEAK the child's input into a cause AWS redacts, and emit members and a JSON kind no SDK expects.
Fix: the startExecution.sync task handler (child type acceptance, TaskFailed emission, cause redaction) and the DescribeExecution-shaped serializer it reuses.
Traces: stepfunctions.standard-parent-sync-runs-express-child-to-completion, stepfunctions.log-sfn-sync-nonexistent-child-rejected, stepfunctions.log-sfn-sync-child-failure-cause-redacts-input-but-keeps-failed-status, stepfunctions.sync-task-output-is-full-describeexecution-shape, stepfunctions.deep-sfn-2-sfn-jsonata-state-error-caught-by-jsonpath-catch

Cause 10 of 10: a distributed Map run with zero tolerated failures never reaches a terminal state.
AWS: a distributed Map whose ToleratedFailureCount is 0 fails the run promptly on the first failed item and keeps the already-completed items' results.
Emulator: recorded behavior only — the check ends `Check timed out after 180.0s`, i.e. the run never terminated and no verdict could be read.
Why: the execution hangs instead of failing, so a client waiting on it waits forever.
Fix: the failure-tolerance handling of the distributed Map run driver.
Traces: stepfunctions.conc-distributed-map-zero-tolerance-run-fails-and-keeps-completed-items

## Evidence
1) `a Map over 2 items followed by a Choice must produce exactly this 18-event history, got 18 events: ['ExecutionStarted','MapStateEntered','MapStateStarted','MapIterationStarted','PassStateEntered','PassStateExited','MapIterationSucceeded', ... 'MapStateSucceeded','MapStateExited','ChoiceStateEntered','ChoiceStateExited', ...]`; `unexpected redrive history shape: [... 'ExecutionFailed','ExecutionRedriven','ParallelStateStarted','ParallelStateFailed','EvaluationFailed','ExecutionFailed']`.
2) `the retried Parallel must SUCCEED on its second attempt; got 'FAILED' error='Sim2Real.Transient' cause='first attempt only' history=[... three ParallelStateStarted/ParallelStateFailed cycles ...]`.
3) `State carried a RetryCount of 0 though 'Ctx' was never retried`.
4) `a page of 62 large-payload events returned no nextToken; expected one because the full history has 62 events of this shape`.
5) `an uncaught Fail with Error:null must surface a null execution error, got 'States.Runtime'`.
6) `an operand path resolving to an explicit null must fail, got 'SUCCEEDED' output='"fell-back"'`; `a TimestampGreaterThanPath target that resolves to a number must FAIL the execution, got status=SUCCEEDED output='"fell-back"'`.
7) `an undefined Output expression must fail the execution even on a Succeed state; got 'SUCCEEDED'`; `[Output/top-level] an undefined result must FAIL the execution, got 'SUCCEEDED' output='null'`; `a string Arguments must FAIL, got 'SUCCEEDED'`; `at-limit execution ended 'SUCCEEDED'`.
8) `the Map's Catch must have run, got output {'caught': False, 'items': [...]}`.
9) `the parent ended 'FAILED': 'StepFunctions.InvalidArnException' "Invalid Arn: 'Resource type not valid in this context: express' ..."`; `expected one TaskFailed on 'startExecution.sync:2': []`; `redacted TaskFailed.cause must drop Input/InputDetails: {... 'Input': {'mode': 'fail', 'marker': 'sim2real-secret-red-...'}, 'InputDetails': {'__type': 'com.amazonaws.swf.base.model#CloudWatchEventsExecutionDataDetails', 'included': True} ...}`; `InputDetails was {'__type': ..., 'included': True}, expected Included=True`; `AttributeError: 'list' object has no attribute 'get'` on the catcher's error output.
10) `Error: Check timed out after 180.0s`.

## Steps to Reproduce
1. A Map over 2 items followed by a Choice; read GetExecutionHistory. Then redrive a JSONata Parallel evaluation failure and read the history.
2. A Parallel with a Retrier whose branch fails only on the first attempt.
3. A Pass state reading `$$.State`; inspect the context object.
4. An execution with 62 large-payload history events; call GetExecutionHistory and look for nextToken.
5. A Fail state with `Error: null` inside a Parallel with a named Catch.
6. A Choice whose operand path resolves to null, and a TimestampGreaterThanPath whose target is a number.
7. JSONata `Output` evaluating to undefined on Succeed/Fail/Pass; `Arguments` evaluating to a string; an evaluation failure whose Location exceeds 256 chars.
8. A Map whose JSONata Choice rule Assign fails, with a Catch on the Map.
9. A parent with `states:startExecution.sync:2` naming (a) an EXPRESS child, (b) a nonexistent child, (c) a child that fails with a secret in its input; inspect TaskFailed cause and InputDetails.
10. A distributed Map with ToleratedFailureCount 0 over items where one fails.
