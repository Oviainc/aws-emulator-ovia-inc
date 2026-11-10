Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: stepfunctions.choice-variable-comparators-and-type-predicates stepfunctions.choice-variable-comparators-never-coerce-across-json-types-rejected stepfunctions.jsonpath-assign-reads-task-catch-map-and-choice-rule stepfunctions.jsonpath-variables-read-across-fields-and-types stepfunctions.sfn-parallel-branch-task-local-catch-handles-failure stepfunctions.task-credentials-role-arn-from-variable-resolved-before-assume stepfunctions.start-execution-sync-heartbeatseconds-over-child-wait-fails-parent stepfunctions.activity-heartbeat-timeout-catcher-order-decides-winner stepfunctions.catch-matches-256-char-errorequals-against-activity-sendtaskfailure stepfunctions.catch-timeout-empty-cause stepfunctions.deep-sfn-2-sfn-jsonpath-timeout-caught-by-jsonata-catch stepfunctions.jsonata-task-error-two-levels-deep-propagates-through-map-and-parallel stepfunctions.create-state-machine-accepts-nonexistent-activity-arn-execution-fails-states-runtime stepfunctions.map-maxconcurrency-one-succeeds-at-3000-serial-iterations stepfunctions.redrive-execution-at-history-limit-rejected stepfunctions.map-maxconcurrency-zero-over-many-items-plateaus-at-an-internal-cap stepfunctions.map-maxconcurrency-runtime-overflow-and-negative-path-fail-with-states-runtime stepfunctions.sfn-reassigning-or-leaving-map-scope-frees-variable-space stepfunctions.sfn-variable-size-limit-exceeded-per-variable-state-and-execution stepfunctions.redrive-execution-resumes-from-timed-out-status stepfunctions.express-sync-machine-timeoutseconds-shorter-than-wait-yields-timed-out stepfunctions.retry-fractional-backoffrate-rounds-delay-down stepfunctions.retry-separate-retriers-keep-independent-counters-and-backoff
Issue-Shape-Id: com.amazonaws.sfn#AWSStepFunctions
Issue-Key: stepfunctions.choice-variable-comparators-and-type-predicates|_assert_matched#3|AssertionError|assert desc["status"] == "SUCCEEDED", (
Issue-Key: stepfunctions.choice-variable-comparators-never-coerce-across-json-types-rejected|_assert_matched#3|AssertionError|assert desc["status"] == "SUCCEEDED", (
Issue-Key: stepfunctions.jsonpath-assign-reads-task-catch-map-and-choice-rule|run#4|AssertionError|assert desc["status"] == "SUCCEEDED", (
Issue-Key: stepfunctions.jsonpath-variables-read-across-fields-and-types|run#6|AssertionError|assert desc["status"] == "SUCCEEDED", (
Issue-Key: stepfunctions.sfn-parallel-branch-task-local-catch-handles-failure|run#9|AssertionError|assert desc["status"] == "SUCCEEDED", (
Issue-Key: stepfunctions.task-credentials-role-arn-from-variable-resolved-before-assume|run#8|AssertionError|assert good["status"] == "SUCCEEDED", (
Issue-Key: stepfunctions.start-execution-sync-heartbeatseconds-over-child-wait-fails-parent|_start_until_child#7|AssertionError|raise AssertionError(
Issue-Key: stepfunctions.activity-heartbeat-timeout-catcher-order-decides-winner|_poll_activity_task#7|AssertionError|raise AssertionError(
Issue-Key: stepfunctions.catch-matches-256-char-errorequals-against-activity-sendtaskfailure|_get_activity_task_token#8|AssertionError|raise AssertionError(f"no activity task token arrived on {activity_arn} within {timeout_s}s")
Issue-Key: stepfunctions.catch-timeout-empty-cause|_poll_activity_task#6|AssertionError|raise AssertionError(
Issue-Key: stepfunctions.deep-sfn-2-sfn-jsonpath-timeout-caught-by-jsonata-catch|_claim_task#5|AssertionError|raise AssertionError(f"no task claimed from {activity_arn!r} within {timeout_s}s")
Issue-Key: stepfunctions.jsonata-task-error-two-levels-deep-propagates-through-map-and-parallel|_poll_activity_task#8|AssertionError|raise AssertionError(f"no activity task arrived on {activity_arn!r} within {timeout_s}s")
Issue-Key: stepfunctions.create-state-machine-accepts-nonexistent-activity-arn-execution-fails-states-runtime|_poll_terminal#9|AssertionError|raise AssertionError(f"execution {exec_arn} not terminal within {timeout_s}s: {last!r}")
Issue-Key: stepfunctions.map-maxconcurrency-one-succeeds-at-3000-serial-iterations|run#8|AssertionError|assert result["status"] == "SUCCEEDED", (
Issue-Key: stepfunctions.redrive-execution-at-history-limit-rejected|run#14|AssertionError|assert (history_result.get("error"), history_result.get("cause")) == ("States.Runtime", _HISTORY_CAUSE), (
Issue-Key: stepfunctions.map-maxconcurrency-zero-over-many-items-plateaus-at-an-internal-cap|run#5|AssertionError|assert few_count == _FEW_ITEMS, (
Issue-Key: stepfunctions.map-maxconcurrency-runtime-overflow-and-negative-path-fail-with-states-runtime|_assert_states_runtime#7|AssertionError|assert desc["status"] == "FAILED", f"{label}: expected FAILED, got {desc['status']}: {desc!r}"
Issue-Key: stepfunctions.sfn-reassigning-or-leaving-map-scope-frees-variable-space|run#5|AssertionError|assert distinct_resp["error"] == "States.DataLimitExceeded", f"unexpected error: {distinct_resp!r}"
Issue-Key: stepfunctions.sfn-variable-size-limit-exceeded-per-variable-state-and-execution|run#4|AssertionError|assert ok_resp["status"] == "SUCCEEDED", (
Issue-Key: stepfunctions.redrive-execution-resumes-from-timed-out-status|_wait_for_status#7|AssertionError|raise AssertionError(f"execution {exec_arn} status not in {statuses} within {timeout_s}s: {last!r}")
Issue-Key: stepfunctions.express-sync-machine-timeoutseconds-shorter-than-wait-yields-timed-out|run#9|AssertionError|assert timed_out.get("error") == "States.Timeout", (
Issue-Key: stepfunctions.retry-fractional-backoffrate-rounds-delay-down|run#18|AssertionError|assert _WAIT_2_FLOOR_S <= waits[1] < _WAIT_2_UNROUNDED_S, (
Issue-Key: stepfunctions.retry-separate-retriers-keep-independent-counters-and-backoff|run#18|AssertionError|assert expected <= observed < expected + slack, (
Issue-Commit: 4d67debdc5b6a4a4b59f91b6ecdc4b307fee9666

# stepfunctions: 10 net-new causes — Assign/Catch/Credentials/HeartbeatSeconds uninterpreted, activity tasks never dispatched, and emulator-internal caps fail executions AWS completes

> _Note: this issue was identified against the code base at commit `4d67debdc5b6a4a4b59f91b6ecdc4b307fee9666`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExecution

## Description
Ten independent root causes in the interpreter's runtime: definition members it refuses to interpret, a whole task type (Activity) that never dispatches, and emulator-internal limits that surface as execution failures. All ten announce themselves to clients as States.Runtime failures or as executions that never reach a terminal state.

Cause 1 of 10: the JSONPath-language `Assign` member (variables) is not interpreted.
AWS: in the JSONPath language a state may carry `Assign`; its values are evaluated after the state's result is computed and bound as execution variables readable as `$var` in later states' paths, Choice rules, Catch, Map and ItemSelector fields. A machine using Assign runs normally.
Emulator: every execution over such a machine ends FAILED error States.Runtime cause `state '<Name>' carries the member 'Assign', which this emulator does not interpret, so nothing was run for it`.
Why: no JSONPath state machine that uses variables can run at all — a positive control as simple as assigning `$v = 10` and comparing it with NumericEqualsPath fails.
Fix: implement `Assign` in the JSONPath branch of the state interpreter plus the variable store it reads.
Traces: stepfunctions.choice-variable-comparators-and-type-predicates, stepfunctions.choice-variable-comparators-never-coerce-across-json-types-rejected, stepfunctions.jsonpath-assign-reads-task-catch-map-and-choice-rule, stepfunctions.jsonpath-variables-read-across-fields-and-types

Cause 2 of 10: a Task-level `Catch` inside a Parallel branch is not interpreted.
AWS: a Task inside a Parallel branch may declare its own Catch; that Catch absorbs the task's failure, the branch completes, and the Parallel (declaring no Catch of its own) SUCCEEDS.
Emulator: the execution ends FAILED States.Runtime cause `state 'AlwaysTimesOut' carries the member 'Catch', which this emulator does not interpret, so nothing was run for it`.
Why: error handling inside Parallel branches is unusable; a failure a client has handled kills the execution.
Fix: Catch handling for states nested inside Parallel branches in the interpreter.
Traces: stepfunctions.sfn-parallel-branch-task-local-catch-handles-failure

Cause 3 of 10: the Task `Credentials` member is not interpreted.
AWS: a Task may carry `Credentials.RoleArn` (literal, `.$` path, or an intrinsic over a variable); Step Functions resolves it, assumes that role, and makes the integration call under it.
Emulator: FAILED States.Runtime cause `state 'T' carries the member 'Credentials', which this emulator does not interpret, so nothing was run for it`.
Why: cross-account / per-task role tasks cannot run. (The open issue 20261109T210043Z-lambda-stepfunctions-8-net-new-causes-step-functions-ignores-task.md covers the lambda__stepfunctions checks of this member; it does not cover this stepfunctions-service check, which is filed here.)
Fix: Credentials resolution + AssumeRole in the Task handler.
Traces: stepfunctions.task-credentials-role-arn-from-variable-resolved-before-assume

Cause 4 of 10: `HeartbeatSeconds` on a states:startExecution.sync task is not interpreted.
AWS: HeartbeatSeconds on a .sync task is accepted and governs heartbeat expiry; the child execution is still started.
Emulator: the parent fails BEFORE starting its child, States.Runtime cause `state 'CallChild' carries the member 'HeartbeatSeconds', which this emulator does not interpret, so nothing was run for it`.
Why: any nested-execution task carrying a heartbeat never starts its child.
Fix: HeartbeatSeconds handling in the .sync nested-execution task handler.
Traces: stepfunctions.start-execution-sync-heartbeatseconds-over-child-wait-fails-parent

Cause 5 of 10: activity tasks are never handed to a GetActivityTask poller.
AWS: when an execution reaches a Task whose Resource is an activity ARN, the task is queued and the next GetActivityTask on that ARN returns a taskToken and input; a Task naming an activity ARN that does not exist fails the execution States.Runtime.
Emulator: GetActivityTask returns an empty response forever (bodies of `{}`, content-length 2) — no token is ever handed out within 20–120 s — and an execution on a nonexistent activity ARN stays RUNNING indefinitely instead of failing.
Why: the entire activity/worker programming model is non-functional, and executions hang with no terminal state.
Fix: activity task dispatch (queueing a scheduled activity task and serving it from GetActivityTask) plus the unknown-activity failure path in the Task handler.
Traces: stepfunctions.activity-heartbeat-timeout-catcher-order-decides-winner, stepfunctions.catch-matches-256-char-errorequals-against-activity-sendtaskfailure, stepfunctions.catch-timeout-empty-cause, stepfunctions.deep-sfn-2-sfn-jsonpath-timeout-caught-by-jsonata-catch, stepfunctions.jsonata-task-error-two-levels-deep-propagates-through-map-and-parallel, stepfunctions.create-state-machine-accepts-nonexistent-activity-arn-execution-fails-states-runtime

Cause 6 of 10: an emulator-internal 1000-state-transition cap fails executions AWS runs to completion.
AWS: there is no 1000-transition ceiling on a Standard execution; a serial Map over 3000 items succeeds, and an execution driven past the history-event limit fails with the documented history-limit failure, not a transition cap.
Emulator: FAILED States.Runtime cause `the execution made more than the 1000 state transitions this emulator runs, so it was stopped`.
Why: ordinary long workflows fail, and the documented history-limit failure can never be observed because the cap fires first.
Fix: remove/raise the transition cap in the execution driver and implement the history-event limit failure instead.
Traces: stepfunctions.map-maxconcurrency-one-succeeds-at-3000-serial-iterations, stepfunctions.redrive-execution-at-history-limit-rejected

Cause 7 of 10: Map MaxConcurrency is effectively ignored — iterations run one at a time and out-of-range runtime values are not rejected.
AWS: `MaxConcurrency: 0` means unbounded, so 10 items run concurrently (observed in-flight 10); a MaxConcurrency/MaxConcurrencyPath value that overflows the allowed range fails the execution States.Runtime at runtime.
Emulator: with MaxConcurrency 0 over 10 items the plateaued in-flight count is 1 (fully serial), and an overflowing literal MaxConcurrency on an inline Map SUCCEEDS instead of failing.
Why: concurrency semantics and the runtime validation of this field are both absent, so timing-dependent workflows behave differently and invalid values pass silently.
Fix: the Map state's concurrency scheduler and its runtime MaxConcurrency range check.
Traces: stepfunctions.map-maxconcurrency-zero-over-many-items-plateaus-at-an-internal-cap, stepfunctions.map-maxconcurrency-runtime-overflow-and-negative-path-fail-with-states-runtime

Cause 8 of 10: the JSONata function `$pad` is refused as unboundable.
AWS: `$pad` is a supported JSONata function; `$pad('', 262142, 'x')` evaluates, so a variable exactly at the 262144-byte cap is assignable and leaving a Map scope frees that space.
Emulator: the execution ends FAILED error States.QueryEvaluationError cause `'$pad('', 262142, 'x')' uses $pad, whose cost this emulator cannot bound, so it was not evaluated`.
Why: a legal JSONata expression is refused, and the variable-size behaviour it exercises can never be reached.
Fix: the JSONata function table/allow-list in the query evaluator.
Traces: stepfunctions.sfn-reassigning-or-leaving-map-scope-frees-variable-space, stepfunctions.sfn-variable-size-limit-exceeded-per-variable-state-and-execution

Cause 9 of 10: machine-level TimeoutSeconds never fires.
AWS: an execution that exceeds the state machine's TimeoutSeconds moves to TIMED_OUT (and is then redrivable); on StartSyncExecution the response itself carries status/error `States.Timeout`.
Emulator: the Standard execution is still RUNNING 68 s later (redriveStatusReason `Execution is RUNNING and cannot be redriven`), and the Express synchronous response carries error None / cause None.
Why: timeouts never happen, so neither TIMED_OUT handling nor redrive-from-timeout can work.
Fix: the execution timeout timer in the execution driver and the sync-response error mapping.
Traces: stepfunctions.redrive-execution-resumes-from-timed-out-status, stepfunctions.express-sync-machine-timeoutseconds-shorter-than-wait-yields-timed-out

Cause 10 of 10: retry back-off delays are not truncated to whole seconds.
AWS: a retry delay computed as IntervalSeconds * BackoffRate^attempt is truncated to whole seconds before waiting — 5 * 1.5 = 7.5 s is observed as 7.0 s, never 7.5 s — and each Retrier keeps its own attempt counter.
Emulator: waits the unrounded 7.50 s (measured waits [5.0, 7.5] and [5.0, 5.0, 10.0, 7.5]).
Why: observed retry timing differs from AWS, which breaks tests and schedules built on the documented truncation.
Fix: the back-off computation in the Retry handler.
Traces: stepfunctions.retry-fractional-backoffrate-rounds-delay-down, stepfunctions.retry-separate-retriers-keep-independent-counters-and-backoff

## Evidence
1) `status=FAILED error='States.Runtime' cause="state 'Seed' carries the member 'Assign', which this emulator does not interpret, so nothing was run for it"` on a machine whose only use of Assign is `$v = 10`.
2) `cause="state 'AlwaysTimesOut' carries the member 'Catch', which this emulator does not interpret, so nothing was run for it"`; the Parallel must have SUCCEEDED.
3) `cause="state 'T' carries the member 'Credentials', which this emulator does not interpret, so nothing was run for it"`.
4) `parent ended 'FAILED' before starting its child: 'States.Runtime' "state 'CallChild' carries the member 'HeartbeatSeconds', which this emulator does not interpret..."`.
5) `no activity task was ever handed to a worker within 120.0s; last poll {... 'content-length': '2' ...}` (empty GetActivityTask body); and `execution ... not terminal within 40.0s: {... 'status': 'RUNNING' ...}` for a Task naming a nonexistent activity ARN.
6) `status='FAILED' error='States.Runtime' cause='the execution made more than the 1000 state transitions this emulator runs, so it was stopped'` for a MaxConcurrency=1 inline Map over 3000 items that must succeed.
7) `with only 10 items (well under the internal cap), MaxConcurrency 0 must run every one of them at once; the plateaued in-flight count was 1`; `literal overflow, INLINE: expected FAILED, got SUCCEEDED`.
8) `error='States.QueryEvaluationError' cause="'$pad('', 262142, 'x')' uses $pad, whose cost this emulator cannot bound, so it was not evaluated"`.
9) `status not in {'TIMED_OUT'} within 68s: {... 'status': 'RUNNING' ...}`; `a synchronous Express machine-level timeout reports error 'States.Timeout' on the sync response itself; got None cause=None`.
10) `wait #2 (IntervalSeconds=5 * BackoffRate=1.5^1 = 7.5s) must be observed rounded DOWN to 7.0s ...; observed 7.50s. All measured waits: [5.0, 7.5]`.

## Steps to Reproduce
1. CreateStateMachine (JSONPath) with a Pass state carrying `Assign: {"v": 10}` and a Choice using `NumericEqualsPath`/`$v`; StartExecution.
2. A Parallel whose branch Task declares its own Catch and always times out.
3. A Task with `Credentials: {"RoleArn.$": "States.Format(...)"}` naming an assumable role.
4. A `arn:aws:states:::states:startExecution.sync` Task carrying `HeartbeatSeconds`.
5. CreateActivity, CreateStateMachine with a Task on that activity ARN, StartExecution, then GetActivityTask in a loop; separately point a Task at a nonexistent activity ARN.
6. An inline Map with MaxConcurrency 1 over 3000 items.
7. An inline Map with MaxConcurrency 0 over 10 slow items; measure in-flight. Separately set an out-of-range MaxConcurrency literal.
8. A JSONata Assign using `$pad('', 262142, 'x')`.
9. A state machine with TimeoutSeconds shorter than its Wait, Standard and Express(sync).
10. A Task with Retry IntervalSeconds 5, BackoffRate 1.5, two attempts; measure the waits.
