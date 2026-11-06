Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__lambda__stepfunctions.refarch-sfn-nonexistent-role-accepted-at-create-rejected-on-data-plane
Issue-Key: events__lambda__stepfunctions.refarch-sfn-nonexistent-role-accepted-at-create-rejected-on-data-plane|run#26|AssertionError|assert control["status"] == "SUCCEEDED", (
Issue-Commit: 5678b2d659e23eeff4b2f39316abc675f4291395

# events__lambda__stepfunctions: the state-machine interpreter abandons executions after a 2-second interpretation budget

> _Note: this issue was identified against the code base at commit `5678b2d659e23eeff4b2f39316abc675f4291395`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExecution

## Description
One root cause behind 1 failing check.

Cause 1 of 1: a standard-workflow execution is stopped by an emulator-internal interpretation budget instead of running to its terminal state, so even the positive control of a role-validation scenario cannot succeed.
AWS: StartExecution on a valid standard state machine runs the definition to a terminal status; a Task/Choice/Wait poller loop that eventually reaches a Succeed state ends `SUCCEEDED`. An execution is only cut short by the machine's own `TimeoutSeconds` (then status `TIMED_OUT`, not `FAILED`) or by StopExecution (`ABORTED`). A `States.Runtime` failure means a runtime error in the definition, not an interpreter budget. Model: StartExecution/DescribeExecution and the States error names are modelled in services/service-com-amazonaws-sfn-awsstepfunctions.
Emulator: the execution ends `FAILED` with cause `the execution was interpreted for longer than the 2 seconds this emulator interprets one definition for, so it was stopped` — an emulator-specific limit with an emulator-specific message, surfaced as a FAILED execution. Because the control machine (which differs only in its roleArn) cannot succeed, the check's real subject — a nonexistent role being accepted at CreateStateMachine and refused on the data plane — is unreachable.
Why: no multi-step workflow can complete, so a client sees FAILED/`States.Runtime` for definitions AWS runs successfully; `TimeoutSeconds` semantics (TIMED_OUT) are replaced by a wrong terminal status; and any check or application that depends on a workflow finishing is blocked.
Fix: the Step Functions interpreter's execution scheduler — an execution must be driven to its terminal state and bounded only by the definition's own TimeoutSeconds, not by a fixed wall-clock interpretation budget. The open issue 20261102T091831Z-events-lambda-stepfunctions-every-task-state-invoking-lambda.md owns the sibling executions of this service; it does not carry this check's replay key.
Traces: events__lambda__stepfunctions.refarch-sfn-nonexistent-role-accepted-at-create-rejected-on-data-plane.

## Evidence
Expected (AWS): the control state machine (identical but for its roleArn) runs to `SUCCEEDED`; the machine with the nonexistent role is created successfully and fails only on the data plane with an access/role error.

Actual (emulator): AssertionError: the control machine differs only in its roleArn and must succeed; got 'FAILED' cause='the execution was interpreted for longer than the 2 seconds this emulator interprets one definition for, so it was stopped'.

## Steps to Reproduce
1. CreateStateMachine (STANDARD) with a Task -> Choice -> Wait poller loop that reaches a Succeed state after a few iterations.
2. StartExecution and poll DescribeExecution.
3. Expected: SUCCEEDED. Observed: FAILED with cause naming a 2-second interpretation budget.
