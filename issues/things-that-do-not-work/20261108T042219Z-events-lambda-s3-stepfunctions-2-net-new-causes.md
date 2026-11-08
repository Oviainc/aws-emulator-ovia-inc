Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__lambda__s3__stepfunctions.lambda-sdk-startexecution-denied-without-permission events__lambda__s3__stepfunctions.sfn-task-token-lambda-missing-sendtask-permission-blocks-resume
Issue-Key: events__lambda__s3__stepfunctions.lambda-sdk-startexecution-denied-without-permission|run#9|AssertionError|assert result["ok"] is False, f"expected the SDK call to be denied, got {result!r}"
Issue-Key: events__lambda__s3__stepfunctions.sfn-task-token-lambda-missing-sendtask-permission-blocks-resume|run#12|AssertionError|assert evidence["error_code"] == "AccessDeniedException", evidence
Issue-Commit: 742d95e483c6290726dac54edb7cc16a76e76b19

# events__lambda__s3__stepfunctions: 2 net-new causes — a Lambda execution role with no states: grant still starts an execution, and a denied SendTaskSuccess is reported TaskTimedOut instead of AccessDeniedException

> _Note: this issue was identified against the code base at commit `742d95e483c6290726dac54edb7cc16a76e76b19`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExecution

## Description
Two net-new root causes. Both are the emulator serving a Step Functions call made with a Lambda execution role's credentials WITHOUT evaluating that role's identity policy.

Cause 1 of 2: states:StartExecution from inside a Lambda handler is served although the execution role grants it nothing.
  AWS: Step Functions evaluates the signing principal's identity-based policy before dispatch. A Lambda whose execution role has no `states:StartExecution` statement for the target state-machine ARN gets the standard IAM refusal — HTTP 400 with error code `AccessDeniedException` and the implicit-deny message `User: <assumed-role session ARN> is not authorized to perform: states:StartExecution on resource: <stateMachineArn> because no identity-based policy allows the states:StartExecution action`. Fixed text: `User: `, ` is not authorized to perform: `, ` on resource: `, ` because no identity-based policy allows the `, ` action`. Variable parts, taken from the request and the emulator's IAM state: the assumed-role session ARN of the function's execution role, the action name, and the `stateMachineArn` the request names. Smithy (com-amazonaws-sfn-awsstepfunctions, StartExecution) models only ExecutionAlreadyExists / ExecutionLimitExceeded / InvalidArn / InvalidExecutionInput / InvalidName / Kms* / StateMachineDeleting / StateMachineDoesNotExist / ValidationException — IAM authorization is not modeled per-operation, so this is the framework refusal and recorded behavior only.
  Emulator: the call is served 200 and returns a real executionArn. Replay shows `{'ok': True, 'executionArn': 'arn:aws:states:<region>:<account>:execution:<sm-name>:<exec-name>'}` where the recording has the call refused.
  Why: a client cannot test least-privilege. Any permission-boundary assertion inverts — code that expects a denial sees a started execution, and an IAM policy regression in the user's own stack is invisible against the emulator.
  Fix: the Step Functions request-authorization path — the identity-policy evaluation that must run in the StartExecution handler before it touches execution state, and the credential/principal plumbing that carries the Lambda execution role's session identity into that evaluation.
  Traces: events__lambda__s3__stepfunctions.lambda-sdk-startexecution-denied-without-permission.

Cause 2 of 2: a waitForTaskToken callback's SendTaskSuccess made under a role with no states:SendTaskSuccess grant is not refused at authorization; it reaches the callback layer and comes back as TaskTimedOut.
  AWS: the same identity-policy gate runs first, so the unpermitted call is refused HTTP 400 `AccessDeniedException` with the message above (action `states:SendTaskSuccess`, resource the execution/state-machine ARN the token belongs to), and the execution stays RUNNING because the callback never landed. `TaskTimedOut` is modeled for SendTaskSuccess (Smithy: SendTaskSuccess errors include TaskTimedOut, TaskDoesNotExist, InvalidToken) but describes a token whose HeartbeatSeconds/TimeoutSeconds elapsed, not an authorization failure.
  Emulator: the call is dispatched unauthorized and then fails with error code `TaskTimedOut` and message `Task Timed Out: '<executionArn>'`. Replay evidence: `{'attempt': 'denied', 'error_code': 'TaskTimedOut', 'error_message': "Task Timed Out: '<executionArn>'"}` where `AccessDeniedException` is required.
  Why: a client gets the wrong error class for a permissions bug, so retry/alarm logic keyed on AccessDeniedException never fires and the operator is pointed at a timeout tuning problem that does not exist.
  Fix: the same Step Functions request-authorization path, applied to the SendTaskSuccess/SendTaskFailure/SendTaskHeartbeat handlers so authorization is decided before the task-token state lookup.
  Traces: events__lambda__s3__stepfunctions.sfn-task-token-lambda-missing-sendtask-permission-blocks-resume.

Related open issues that do NOT cover these (different service scope / different symptom): 20261106T214711Z-events-lambda-s3-stepfunctions-a-waitfortasktoken-lambda-callback.md and 20261106T151148Z-events-lambda-s3-stepfunctions-in-function-events-putevents.md.

## Evidence
Cause 1 — events__lambda__s3__stepfunctions.lambda-sdk-startexecution-denied-without-permission:
Expected: the in-handler states:StartExecution is refused (HTTP 400, AccessDeniedException), so the handler returns ok=False.
Actual: AssertionError: expected the SDK call to be denied, got {'ok': True, 'executionArn': 'arn:aws:states:us-west-2:000000000000:execution:sim2real-sm-b9f52238af7a:sim2real-exec-denied-95d091ebf45f'}

Cause 2 — events__lambda__s3__stepfunctions.sfn-task-token-lambda-missing-sendtask-permission-blocks-resume:
Expected: evidence['error_code'] == 'AccessDeniedException' (and the execution stays RUNNING).
Actual: AssertionError: {'token': '<base64 task token>', 'attempt': 'denied', 'error_code': 'TaskTimedOut', 'error_message': "Task Timed Out: 'arn:aws:states:us-west-2:000000000000:execution:sfn-behav-sm-b4203a372f12:f9a575a1-e76b-464f-84df-07be1bfc95b5'"}
(The ARNs, account id, token and generated names above are per-run values; the requirement is the status, the error code and the message template.)

## Steps to Reproduce
1. git_clone(); run_behavioral_suite().
2. Cause 1: create a state machine; create a Lambda execution role whose inline policy grants only s3 (no states:*); deploy a handler that calls stepfunctions.start_execution(stateMachineArn=<sm>, name=..., input=...); Invoke it. The emulator returns an executionArn instead of AccessDeniedException.
3. Cause 2: create a state machine with a Task state using `arn:aws:states:::lambda:invoke.waitForTaskToken`; give the invoked Lambda's role no states:SendTaskSuccess; have the handler call send_task_success(taskToken=..., output=...). The emulator raises TaskTimedOut instead of AccessDeniedException, and the denied-then-granted sequence never reproduces.
4. Re-replay the two check ids in trace_ids to confirm.
