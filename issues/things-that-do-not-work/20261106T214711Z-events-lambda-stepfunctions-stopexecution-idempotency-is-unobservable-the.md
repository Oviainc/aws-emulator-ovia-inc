Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__lambda__stepfunctions.refarch-sfn-stop-execution-on-terminal-execution-is-idempotent
Issue-Key: events__lambda__stepfunctions.refarch-sfn-stop-execution-on-terminal-execution-is-idempotent|run#8|AssertionError|assert before["status"] == "SUCCEEDED", (
Issue-Commit: e20658567def5986c911ca4169de4ecb96db84bd

# events__lambda__stepfunctions: StopExecution idempotency is unobservable — the poller execution is abandoned by the 2-second interpreter budget and ends FAILED States.Runtime instead of SUCCEEDED (1 cause)

> _Note: this issue was identified against the code base at commit `e20658567def5986c911ca4169de4ecb96db84bd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StopExecution

## Description
One net-new root cause behind 1 failing check.

Cause 1 of 1: the execution that must reach a terminal SUCCEEDED before StopExecution is called is instead abandoned by an emulator-internal wall-clock interpretation budget, so the subject behaviour — StopExecution on an already-terminal execution being a no-op — can never be exercised.

AWS: StartExecution on a valid STANDARD state machine runs the definition to a terminal state and DescribeExecution reports `status=SUCCEEDED`; StopExecution against an execution that is already terminal returns HTTP 200 and changes nothing (status, output and stopDate unchanged, no new history event). Smithy: StartExecution / DescribeExecution (`status: ExecutionStatus`, `error: SensitiveError`, `cause: SensitiveCause`) in services/service-com-amazonaws-sfn-awsstepfunctions. AWS cuts an execution short only through the definition's own `TimeoutSeconds` (status TIMED_OUT, error `States.Timeout`), StopExecution (ABORTED) or a documented quota; `States.Runtime` means a runtime fault in the definition, and no AWS limit is expressed as seconds of interpretation.

Emulator: the execution ends `status='FAILED'` with `cause='the execution was interpreted for longer than the 2 seconds this emulator interprets one definition for, so it was stopped'` — emulator-authored fixed text whose `2 seconds` is the emulator's hard-coded budget, not a value from the request or definition. The check asserts on the pre-stop status and never reaches the StopExecution calls.

Why: a client relying on AWS's behavior sees a wrong terminal status and a non-AWS error/cause for definitions AWS completes, so no workflow finishes; and because nothing reaches a SUCCEEDED terminal state, the idempotency contract of StopExecution against terminal executions is untestable and could be silently wrong as well.

Fix: the Step Functions interpreter's execution scheduler in the sfn service crate — bound an execution only by its definition's `TimeoutSeconds` and AWS's documented quotas, surfacing AWS's status/error name when a bound is reached, and never by a fixed wall-clock interpretation budget. The already-open issue 20261106T151148Z-events-lambda-stepfunctions-the-state-machine-interpreter-abandons.md describes this same budget for a sibling check of this service but does not carry this check's replay key; 20261102T091831Z-events-lambda-stepfunctions-every-task-state-invoking-lambda.md owns the Lambda-dispatch causes of this suite.

Traces: events__lambda__stepfunctions.refarch-sfn-stop-execution-on-terminal-execution-is-idempotent.

## Evidence
Expected (AWS): DescribeExecution before the stop reads `status="SUCCEEDED"`; StopExecution then returns 200 and leaves status, output and stopDate untouched, and a second StopExecution does the same.

Actual (emulator): AssertionError: the execution must reach SUCCEEDED before it is stopped; got 'FAILED' cause='the execution was interpreted for longer than the 2 seconds this emulator interprets one definition for, so it was stopped'.

Fixed text vs values: the whole cause is emulator-invented; `2 seconds` is the emulator's internal budget, absent from the request and the definition. AWS returns SUCCEEDED here, or `status=TIMED_OUT` / `error=States.Timeout` for a real definition timeout.

## Steps to Reproduce
1. Create a Lambda submit function and a check function, and a STANDARD state machine whose Task -> Choice -> Wait poller loop reaches a Succeed state.
2. StartExecution and poll DescribeExecution to a terminal status.
3. Expected: SUCCEEDED. Observed: FAILED with cause naming a 2-second interpretation budget.
4. StopExecution on the terminal execution (twice) can therefore never be exercised.
