Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__lambda__s3__stepfunctions.tenant-provisioning-workflow-writes-objects-url-and-provisioned-status
Issue-Key: dynamodb__lambda__s3__stepfunctions.tenant-provisioning-workflow-writes-objects-url-and-provisioned-status|_await_workflow_role#1|AssertionError|assert not assumed and time.monotonic() < deadline, (
Issue-Commit: 0a6b2caf8aaebe9b3d91e5e0596565c0c4125f85

# dynamodb__lambda__s3__stepfunctions: the state-machine interpreter abandons an execution after a 2-second interpretation budget, so the tenant-provisioning workflow never reaches a terminal state (1 cause)

> _Note: this issue was identified against the code base at commit `0a6b2caf8aaebe9b3d91e5e0596565c0c4125f85`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExecution

## Description
One root cause behind 1 failing check.

Cause 1 of 1: a STANDARD state machine execution is stopped by an emulator-internal wall-clock interpretation budget and reported as a FAILED execution with error `States.Runtime`, instead of being driven to its terminal state. The check's estate setup runs tenant-less 'role barrier' probe executions until the execution role has propagated; each probe comes back FAILED/States.Runtime from the budget rather than with the handler's KeyError, so the barrier never clears and the four-task provisioning workflow (S3 objects, tenant_url, PROVISIONED status, OwnerIndex query) is never exercised at all.
AWS: StartExecution on a valid STANDARD state machine runs the definition to a terminal status, and DescribeExecution.status is one of the modelled ExecutionStatus values RUNNING | SUCCEEDED | FAILED | TIMED_OUT | ABORTED | PENDING_REDRIVE (Smithy: services/service-com-amazonaws-sfn-awsstepfunctions/smithy-model/com-amazonaws-sfn-awsstepfunctions.json, StartExecution -> StartExecutionOutput, DescribeExecution -> DescribeExecutionOutput with required `status: ExecutionStatus`, optional `error: SensitiveError` and `cause: SensitiveCause`). An execution is cut short only by the definition's own TimeoutSeconds — status TIMED_OUT with error `States.Timeout` — or by StopExecution, which yields ABORTED. `States.Runtime` is reserved for a genuine runtime fault in the definition and its cause is AWS's own description of that fault, naming the state or field at issue; AWS has no interpreter-side wall-clock budget, so a definition that reaches a Succeed state ends SUCCEEDED however long interpretation took. When a Task's handler raises, the execution ends FAILED carrying the handler's error (here 'KeyError'), not a synthesized interpreter error.
Emulator: the execution terminates with status 'FAILED', error 'States.Runtime' and cause 'the execution was interpreted for longer than the 2 seconds this emulator interprets one definition for, so it was stopped'. That sentence, including the 2-second budget, is emulator-authored and has no AWS counterpart, and the status/error pair is wrong for a time limit even if AWS had one (it would be TIMED_OUT / States.Timeout against the definition's TimeoutSeconds).
Why: a client that starts any multi-step workflow and polls DescribeExecution sees FAILED/States.Runtime for definitions AWS runs to SUCCEEDED. It cannot tell a definition fault from the emulator abandoning the run, role-propagation barriers and Catch/Choice/Wait logic never converge, TimeoutSeconds semantics are replaced by a wrong terminal status, and every effect the workflow should have (DynamoDB item transitions to PROVISIONED with its timestamps, the two S3 objects, the stored tenant_url) is missing.
Fix: the Step Functions interpreter's execution scheduler/driver in the sfn service — an execution must be driven to its terminal state and bounded only by the definition's own TimeoutSeconds (reported as TIMED_OUT / States.Timeout), never by a fixed wall-clock interpretation budget, and States.Runtime must be reserved for genuine definition runtime faults with AWS's wording. The open issue 20261106T151148Z-events-lambda-stepfunctions-the-state-machine-interpreter-abandons.md describes the same interpreter budget for the events__lambda__stepfunctions service, and 20261102T111517Z-dynamodb-lambda-s3-stepfunctions-a-task-state-invoking.md owns the sibling Lambda-Task executions of this service; neither carries this check's replay key, so this check stays unfixed when those are closed.
Traces: dynamodb__lambda__s3__stepfunctions.tenant-provisioning-workflow-writes-objects-url-and-provisioned-status (role-barrier execution 1, raised from _await_workflow_role during _provisioning_estate).

## Evidence
Expected (AWS): the tenant-less barrier probe runs the definition to a terminal status decided by the definition and its handlers — the check expects FAILED with error 'KeyError' from the first Lambda task, which proves the execution role is assumable — and only the definition's TimeoutSeconds can cut an execution short, as TIMED_OUT/States.Timeout.

Actual (emulator): AssertionError: role barrier execution 1 ended 'FAILED': 'States.Runtime' 'the execution was interpreted for longer than the 2 seconds this emulator interprets one definition for, so it was stopped'. Fixed text of the diverging cause: the entire sentence 'the execution was interpreted for longer than the 2 seconds this emulator interprets one definition for, so it was stopped' is emulator-authored with no AWS counterpart; its only variable-looking part, the 2-second budget, is an emulator constant and not the definition's TimeoutSeconds. Diverging status/error pair: 'FAILED' + 'States.Runtime' where AWS reports the handler's error (FAILED + 'KeyError') or, on a genuine timeout, TIMED_OUT + 'States.Timeout'.

## Steps to Reproduce
1. Create the four provisioning Lambda functions and a STANDARD state machine chaining them with LambdaInvoke Task states (definition carries TimeoutSeconds 900), with a valid roleArn.
2. StartExecution with input {"role_barrier": true} and poll DescribeExecution to a non-RUNNING status.
3. Expected: FAILED carrying the handler's own error 'KeyError' (the tenant-less probe), proving role propagation; nothing bounded by an interpreter budget.
4. Observed: status FAILED, error 'States.Runtime', cause 'the execution was interpreted for longer than the 2 seconds this emulator interprets one definition for, so it was stopped' — the barrier never clears, and the provisioning workflow, its S3 objects and its PROVISIONED DynamoDB record are never produced.
