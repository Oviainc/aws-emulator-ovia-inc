Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__lambda__s3__stepfunctions.sfn-task-token-lambda-missing-sendtask-permission-blocks-resume
Issue-Key: events__lambda__s3__stepfunctions.sfn-task-token-lambda-missing-sendtask-permission-blocks-resume|run#15|AssertionError|assert evidence.get("error_message") == expected_message, evidence
Issue-Commit: d6495890c37f2f3170333ee7f057f680b864ff4b

# Step Functions: the AccessDeniedException for states:SendTaskSuccess names 'resource: *' instead of the state machine ARN

> _Note: this issue was identified against the code base at commit `d6495890c37f2f3170333ee7f057f680b864ff4b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SendTaskSuccess

## Description
Cause 1 of 1: the identity-based denial message carries a wildcard where AWS names the concrete resource.
AWS: when a caller (here a Lambda holding a `waitForTaskToken` task token) lacks `states:SendTaskSuccess`, Step Functions refuses with HTTP 400 `AccessDeniedException` whose message is `User: arn:aws:sts::<account>:assumed-role/<role-name>/<session-name> is not authorized to perform: states:SendTaskSuccess on resource: <stateMachineArn> because no identity-based policy allows the states:SendTaskSuccess action`. Fixed text is the `User: ... is not authorized to perform: <action> on resource: <resource> because no identity-based policy allows the <action> action` frame; the variable parts are the assumed-role ARN and session name from the caller's credentials, the action, and — critically — the RESOURCE the call acted on, which for SendTaskSuccess is the token's state machine ARN.
Emulator: the code and the frame are right, but the resource is rendered as the literal `*`: `... is not authorized to perform: states:SendTaskSuccess on resource: * because no identity-based policy allows the states:SendTaskSuccess action`. It appears to echo the policy's Resource wildcard rather than the request's target resource.
Why: the message is the only place a client learns WHICH resource was denied; `*` makes the denial undiagnosable, breaks log-based alerting that extracts the ARN, and any client or test matching AWS's message text fails.
Fix: the authorization denial-message builder on the SendTaskSuccess handler (and the sibling task-token operations) — it must substitute the request's resolved resource ARN, not the matched policy statement's Resource.
Traces: events__lambda__s3__stepfunctions.sfn-task-token-lambda-missing-sendtask-permission-blocks-resume.

## Evidence
Expected: `User: arn:aws:sts::<account>:assumed-role/<lambda-role>/<function-name> is not authorized to perform: states:SendTaskSuccess on resource: <stateMachineArn> because no identity-based policy allows the states:SendTaskSuccess action`.
Got (recorded evidence object): `'error_code': 'AccessDeniedException', 'error_message': 'User: arn:aws:sts::000000000000:assumed-role/sfn-behav-lam-role-ca815a0f35fa/sfn-behav-fn-e3407de98c33 is not authorized to perform: states:SendTaskSuccess on resource: * because no identity-based policy allows the states:SendTaskSuccess action'` — the role/function/account values are per-run; the divergence is `resource: *` where the state machine ARN belongs.

## Steps to Reproduce
1. Create a state machine with a `waitForTaskToken` Lambda task whose Lambda role has NO `states:SendTaskSuccess` grant.
2. StartExecution; from the Lambda, call `sfn.send_task_success(taskToken=<token>, output='{}')` and capture the ClientError.
3. Compare the message's `on resource:` segment against the state machine ARN.
4. Replay events__lambda__s3__stepfunctions.sfn-task-token-lambda-missing-sendtask-permission-blocks-resume.
