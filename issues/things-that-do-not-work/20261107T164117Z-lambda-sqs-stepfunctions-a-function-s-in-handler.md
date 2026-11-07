Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__sqs__stepfunctions.lambda-start-sfn-missing-permission-negative
Issue-Key: lambda__sqs__stepfunctions.lambda-start-sfn-missing-permission-negative|_invoke_lambda#18|Timeout|resp = ctx.client("lambda").invoke(
Issue-Commit: 6bba0ea93be6d4db7a2caa629527597bd59ab221

# lambda__sqs__stepfunctions: a function's in-handler states:StartExecution never completes the grant→revoke→restore cycle, so the AccessDeniedException refusal is never produced

> _Note: this issue was identified against the code base at commit `6bba0ea93be6d4db7a2caa629527597bd59ab221`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExecution

## Description
1 net-new root cause in this service.

Cause 1 of 1: a Lambda handler that calls states:StartExecution with the AWS SDK never produces the three observable outcomes AWS gives as the function's own execution-role policy is changed — granted: a started execution that SUCCEEDS; revoked: AccessDeniedException and NO new execution; restored: success again. Failing check id: lambda__sqs__stepfunctions.lambda-start-sfn-missing-permission-negative

AWS: StartExecution is authorized against the CALLING identity — here the Lambda function's execution role. Granted, the call returns `executionArn` + `startDate` (Smithy: sfn#StartExecution output members `executionArn` (required) and `startDate` (required)) and DescribeExecution reports `status` reaching `SUCCEEDED` for a trivial `Pass`/`End` state machine. Revoked, the frontend refuses BEFORE any execution is created, with HTTP 400 and error code `AccessDeniedException` (the service-common IAM authorization refusal; it is not one of StartExecution's modeled errors — the model lists ExecutionAlreadyExists, ExecutionLimitExceeded, InvalidArn, InvalidExecutionInput, InvalidName, Kms*, StateMachineDeleting, StateMachineDoesNotExist, ValidationException). The message follows AWS's standard identity-based-denial template: the fixed text is `User: ` ... ` is not authorized to perform: ` ... ` on resource: ` ... ` because no identity-based policy allows the ` ... ` action`; the variable parts come from the request and state — the assumed-role ARN of the caller (`arn:aws:sts::<account>:assumed-role/<execution role name>/<session name>`), the action `states:StartExecution`, and the resource (the state machine ARN from the request). ListExecutions must show the same count before and after a denied call. Restored, the identical call succeeds again.

Emulator: the cycle never completes. The replay ends with `Check timed out after 60.0s` — no assertion text, no HTTP status and no error code — so the handler's payload never settles to the expected shape in any of the three phases: the re-invoke loop that waits for `ok: true` under the granting policy, for `ok: false` under the logs-only policy, and for `ok: true` again after restore never reaches its predicate inside the run budget. Consequently neither the AccessDeniedException code nor the denial message, nor the "no execution created" invariant, is ever produced.

Why: a client relying on IAM to gate a function's control-plane calls gets no enforcement signal it can act on. Either the in-handler SDK call does not behave like the real one (so an application cannot distinguish allowed from denied) or a revocation is never reflected — a denied StartExecution that silently creates an execution, or an allowed one that never starts anything, both corrupt the caller's state machine. Code that catches ClientError on `AccessDeniedException` and matches the documented message never takes that branch.

Fix: the authorization of StartExecution against the caller's identity policy and the in-function AWS SDK path that reaches it — the refusal must be produced as HTTP 400 / `AccessDeniedException` with AWS's identity-based-denial message built from the caller's assumed-role ARN, the action and the state-machine ARN, and must create no execution; the granted and restored phases must start an execution that reaches SUCCEEDED.

Traces: lambda__sqs__stepfunctions.lambda-start-sfn-missing-permission-negative. The open issue 20261105T081644Z-lambda-sqs-stepfunctions-no-aws-sdk-in-the.md (no AWS SDK in the in-process Lambda runtime) covers the POSITIVE in-handler Step Functions checks in this service and is a plausible upstream cause, but it is not registered as owning this check and does not cover the revoke/AccessDeniedException leg or the "no execution created" invariant asserted here.

## Evidence
Expected (AWS), per phase:
- Granted (role allows states:StartExecution on the state machine ARN): the handler returns {ok: true, executionArn: <arn>}; sfn:DescribeExecution(<arn>).status reaches SUCCEEDED for a {StartAt: PassState, States: {PassState: {Type: Pass, End: true}}} machine.
- Revoked (role reduced to logs:CreateLogGroup/CreateLogStream/PutLogEvents only): the identical invoke returns {ok: false, error: "AccessDeniedException", message: "User: arn:aws:sts::<account>:assumed-role/<role name>/<session name> is not authorized to perform: states:StartExecution on resource: <state machine arn> because no identity-based policy allows the states:StartExecution action"} — HTTP 400 on the wire — and sfn:ListExecutions(<state machine arn>) returns the same number of executions as before the call.
- Restored: the identical invoke returns {ok: true} again.

Actual (emulator): `Check timed out after 60.0s`. None of the three phases is ever observed; the replay reports no HTTP status, no error code and no message, so the AccessDeniedException refusal is absent rather than wrong.

Note on values: the account id, role name, session name, state-machine ARN and execution ARN are request/state values that fill the message template; the fixed text of the message, the code `AccessDeniedException`, the 400 status and the unchanged execution count are the requirement.

## Steps to Reproduce
1. iam:CreateRole R_sm trusting states.amazonaws.com; sfn:CreateStateMachine SM with definition {"StartAt":"PassState","States":{"PassState":{"Type":"Pass","End":true}}} and roleArn=R_sm.
2. Create a python3.12 Lambda whose handler calls boto3 stepfunctions.start_execution(stateMachineArn=os.environ['STATE_MACHINE_ARN'], input=json.dumps(event)) and returns {ok, error, message, executionArn}; give its execution role logs:* plus states:StartExecution on SM.
3. Invoke it: expect {ok: true} with an executionArn, and DescribeExecution(executionArn).status == SUCCEEDED.
4. iam:PutRolePolicy on the execution role, replacing the inline policy with logs-only (no states:StartExecution). Re-invoke until the denial is steady.
5. Record len(sfn:ListExecutions(SM).executions), invoke once more, and expect {ok: false, error: "AccessDeniedException"} with the identity-based-denial message naming the assumed-role ARN, states:StartExecution and SM; then expect ListExecutions to report the same count.
6. iam:PutRolePolicy restoring logs + states:StartExecution on SM; re-invoke and expect {ok: true}.
7. Observed: the run never settles any of these predicates and hits the 60s budget. Replay check id lambda__sqs__stepfunctions.lambda-start-sfn-missing-permission-negative to confirm.
