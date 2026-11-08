Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__sqs__stepfunctions.lambda-list-sfn-executions-missing-permission-negative lambda__sqs__stepfunctions.lambda-sfn-stopexecution-missing-permission-negative lambda__sqs__stepfunctions.lambda-start-sfn-missing-permission-negative lambda__sqs__stepfunctions.sfn-sqs-send-message-wait-for-task-token-missing-perm-negative
Issue-Key: lambda__sqs__stepfunctions.lambda-list-sfn-executions-missing-permission-negative|run#22|AssertionError|assert payload.get("ok") is False, (
Issue-Key: lambda__sqs__stepfunctions.lambda-sfn-stopexecution-missing-permission-negative|run#16|AssertionError|assert body["errorCode"] == "AccessDeniedException", body
Issue-Key: lambda__sqs__stepfunctions.lambda-start-sfn-missing-permission-negative|_poll_invoke_until#26|AssertionError|raise AssertionError(
Issue-Key: lambda__sqs__stepfunctions.sfn-sqs-send-message-wait-for-task-token-missing-perm-negative|run#15|AssertionError|assert evidence["attempt"] == "denied", (
Issue-Commit: 742d95e483c6290726dac54edb7cc16a76e76b19

# lambda__sqs__stepfunctions: 4 net-new causes — in-Lambda ListExecutions, StopExecution, StartExecution and a waitForTaskToken SendTaskSuccess are all served (or carry a null error code) under a role that does not grant them

> _Note: this issue was identified against the code base at commit `742d95e483c6290726dac54edb7cc16a76e76b19`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExecution

## Description
Four net-new root causes, one per Step Functions operation reached from a Lambda handler. All share the missing gate: the emulator does not evaluate the Lambda execution role's identity policy for role-signed Step Functions requests, and where it does surface a failure it carries no AWS error code.

Shared AWS behavior: Step Functions evaluates the signing principal's identity-based policy before dispatch. The refusal is the standard IAM one — HTTP 400 with error code `AccessDeniedException` and message `User: <assumed-role session ARN> is not authorized to perform: <action> on resource: <resource ARN> because no identity-based policy allows the <action> action`. Fixed text: `User: `, ` is not authorized to perform: `, ` on resource: `, ` because no identity-based policy allows the `, ` action`. Variable parts come from the request and the emulator's IAM state: the function execution role's assumed-role session ARN, the action name, and the ARN the request names. Smithy (com-amazonaws-sfn-awsstepfunctions) models no AccessDeniedException on these operations (ListExecutions: InvalidArn/InvalidToken/ResourceNotFound/StateMachineDoesNotExist/StateMachineTypeNotSupported/ValidationException; StopExecution: ExecutionDoesNotExist/InvalidArn/Kms*/ValidationException; StartExecution: ExecutionAlreadyExists/ExecutionLimitExceeded/InvalidArn/InvalidExecutionInput/InvalidName/Kms*/StateMachineDeleting/StateMachineDoesNotExist/ValidationException; SendTaskSuccess: InvalidOutput/InvalidToken/Kms*/TaskDoesNotExist/TaskTimedOut), so authorization is the framework refusal — recorded behavior only.

Cause 1 of 4: states:ListExecutions is served under a role with no such grant.
  Emulator: the handler reports `{'ok': True, 'error': None, 'count': 1}` — the list is returned, including the execution.
  Why: the caller reads execution metadata (names, ARNs, statuses) a least-privilege policy forbids; the denial path is untestable.
  Fix: the ListExecutions handler's authorization step.
  Traces: lambda__sqs__stepfunctions.lambda-list-sfn-executions-missing-permission-negative.

Cause 2 of 4: an unpermitted states:StopExecution fails with no error code at all.
  Emulator: the handler's recorded failure is `{'errorCode': None}` — there is no `AccessDeniedException` (nor any other AWS error code) on the response, so the client cannot classify the failure.
  Why: an un-coded failure breaks every client that branches on `Error.Code`; a permissions problem is indistinguishable from a transport or runtime fault, and retry/alarm logic keyed on AccessDeniedException never fires.
  Fix: the StopExecution handler's authorization step and the error serializer that must emit the code and message on the refusal (a refusal must carry an error code, never a null one).
  Traces: lambda__sqs__stepfunctions.lambda-sfn-stopexecution-missing-permission-negative.

Cause 3 of 4: states:StartExecution stays allowed after the grant is revoked.
  Emulator: after revoking the statement, repeated Invokes keep returning `{'ok': True, 'error': None, 'executionArn': ...}` for the whole 90 s budget — the revocation is never observed and an execution AWS would refuse is actually created.
  Why: a real side effect is produced under a policy that forbids it, so the emulator diverges in state as well as in response, and a policy regression passes locally.
  Fix: the StartExecution handler's authorization step and the per-request IAM state lookup (policy changes must be read per request, not cached or skipped).
  Traces: lambda__sqs__stepfunctions.lambda-start-sfn-missing-permission-negative.

Cause 4 of 4: a waitForTaskToken callback's SendTaskSuccess succeeds under a role with no states:SendTaskSuccess grant.
  Emulator: the callback is accepted — recorded attempt `succeeded` — and the execution is resumed, where AWS refuses the call AccessDeniedException and leaves the execution RUNNING.
  Why: the callback-authorization boundary does not exist, so a task token can be redeemed by any principal; the denied-then-granted sequence (deny → grant → identical call succeeds) cannot be reproduced at all.
  Fix: the SendTaskSuccess/SendTaskFailure/SendTaskHeartbeat handlers' authorization step, decided before the task-token state lookup.
  Traces: lambda__sqs__stepfunctions.sfn-sqs-send-message-wait-for-task-token-missing-perm-negative.

Related open issues that do NOT cover these (different symptom: one reports the AccessDenied principal ARN wording, one reports the grant→revoke→restore cycle never completing): 20261106T134622Z-lambda-sqs-stepfunctions-an-accessdenied-names-the-role.md, 20261107T164117Z-lambda-sqs-stepfunctions-a-function-s-in-handler.md, 20261107T044335Z-lambda-sqs-stepfunctions-waitfortasktoken-callbacks-never-resume-the.md.

## Evidence
Cause 1 — lambda__sqs__stepfunctions.lambda-list-sfn-executions-missing-permission-negative:
Expected: payload['ok'] is False (HTTP 400 AccessDeniedException).
Actual: AssertionError: Expected failure for ListExecutions, got: {'ok': True, 'error': None, 'count': 1}

Cause 2 — lambda__sqs__stepfunctions.lambda-sfn-stopexecution-missing-permission-negative:
Expected: body['errorCode'] == 'AccessDeniedException'.
Actual: AssertionError: {'errorCode': None}

Cause 3 — lambda__sqs__stepfunctions.lambda-start-sfn-missing-permission-negative:
Expected: after revoking states:StartExecution the call is refused.
Actual: AssertionError: the revoked lambda role loses states:StartExecution not observed within 90.0s. Last result: {'status_code': 200, 'payload': {'ok': True, 'error': None, 'executionArn': 'arn:aws:states:us-west-2:000000000000:execution:sm-noperm-b8bea7a66746:bdfad3e7-5f2f-4d5f-80c9-930127ef23d7'}}

Cause 4 — lambda__sqs__stepfunctions.sfn-sqs-send-message-wait-for-task-token-missing-perm-negative:
Expected: evidence['attempt'] == 'denied' (SendTaskSuccess refused AccessDeniedException; execution stays RUNNING).
Actual: AssertionError: Expected the callback's SendTaskSuccess to be denied, got: {'token': '<base64 task token>', 'attempt': 'succeeded'}
(ARNs, account id, tokens and generated names are per-run values; the requirement is the 400 status, the AccessDeniedException code and the message template.)

## Steps to Reproduce
1. git_clone(); run_behavioral_suite().
2. Cause 1: create a state machine, start an execution; create a Lambda whose role grants no states:ListExecutions; have the handler call list_executions(stateMachineArn=<sm>). The emulator returns ok=True with count=1.
3. Cause 2: with a role that does not grant states:StopExecution, have the handler call stop_execution(executionArn=<exec>) and record the botocore error code. The emulator reports errorCode None instead of 'AccessDeniedException'.
4. Cause 3: grant states:StartExecution, confirm success, revoke it, re-Invoke repeatedly for 90 s. The emulator keeps returning ok=True with an executionArn.
5. Cause 4: build a state machine with `arn:aws:states:::sqs:sendMessage.waitForTaskToken`; have the consuming Lambda, under a role with no states:SendTaskSuccess, call send_task_success(taskToken=..., output=...). The emulator accepts it and resumes the execution.
6. Re-replay the four check ids in trace_ids to confirm.
