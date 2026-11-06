Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__lambda__s3__stepfunctions.sfn-task-token-lambda-missing-sendtask-permission-blocks-resume
Issue-Key: events__lambda__s3__stepfunctions.sfn-task-token-lambda-missing-sendtask-permission-blocks-resume|_poll_for_object#10|Timeout|time.sleep(interval_s)
Issue-Commit: e20658567def5986c911ca4169de4ecb96db84bd

# events__lambda__s3__stepfunctions: a waitForTaskToken Lambda callback never produces an AccessDeniedException for a missing states:SendTaskSuccess grant nor resumes after the grant is added — the check hangs to the harness cap (1 cause)

> _Note: this issue was identified against the code base at commit `e20658567def5986c911ca4169de4ecb96db84bd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SendTaskSuccess

## Description
One net-new root cause behind 1 failing check.

Cause 1 of 1: the `waitForTaskToken` Lambda callback path never settles. The in-function SendTaskSuccess neither returns AWS's authorization refusal while the Lambda role lacks `states:SendTaskSuccess`, nor succeeds and resumes the execution after the grant is attached, so the execution never reaches a terminal state and the check is killed at its 60 s deadline.

AWS: a Task state using the `.waitForTaskToken` pattern parks until the token is redeemed. A caller whose identity policy does not allow `states:SendTaskSuccess` gets an authorization refusal — error code `AccessDeniedException`, HTTP 400 — whose message is AWS's fixed text for that class (`User: <assumed-role session ARN> is not authorized to perform: states:SendTaskSuccess on resource: <stateMachine/execution ARN> because no identity-based policy allows the states:SendTaskSuccess action`), with the session ARN, action and resource taken from the request and the caller's identity; the parked execution stays RUNNING. Once the grant is added, the identical SendTaskSuccess returns HTTP 200 and the execution reaches SUCCEEDED with the callback output. Smithy: SendTaskSuccess (taskToken @length 1..2048, output @length 0..262144; errors InvalidToken, TaskDoesNotExist, TaskTimedOut) in services/service-com-amazonaws-sfn-awsstepfunctions; AccessDeniedException is the service-common authorization refusal.

Emulator: the check reports only `Error: Check timed out after 60.0s`. Neither arm answers: no AccessDeniedException while unpermitted, and no resume/SUCCEEDED after the grant.

Why: a client relying on AWS's behavior cannot build the callback pattern here and, worse, cannot tell an authorization failure from a hang. An unpermitted caller receives no error code to branch on, a permitted caller's execution never completes, and an IAM change (attaching the SendTaskSuccess grant) has no observable effect.

Fix: the `.waitForTaskToken` callback machinery in the sfn service crate's Task-state handler plus the SendTaskSuccess handler's authorization check — park the Task against a registered token, evaluate the caller's identity policy and refuse with `AccessDeniedException` (400) naming the assumed-role session ARN, the action and the resource, and on an allowed call resume the parked execution to SUCCEEDED. The open issue 20261105T081644Z-events-lambda-s3-stepfunctions-the-lambda-python-runtime.md owns the sibling task-token checks of this suite but does not carry this check's replay key.

Traces: events__lambda__s3__stepfunctions.sfn-task-token-lambda-missing-sendtask-permission-blocks-resume.

## Evidence
Expected (AWS): while the Lambda role lacks states:SendTaskSuccess, the in-function call fails with HTTP 400 `AccessDeniedException` and the execution remains RUNNING; after the grant is attached, the identical call returns 200 and the execution reaches SUCCEEDED with the callback output.

Actual (emulator): `Error: Check timed out after 60.0s` — neither the refusal nor the post-grant resume is ever observed, and the execution never reaches a terminal state.

Fixed text vs values in the expected refusal: the fixed text is `User: ... is not authorized to perform: ... on resource: ... because no identity-based policy allows the ... action`; the session ARN, the `states:SendTaskSuccess` action and the resource ARN are values from the request and the caller's identity.

## Steps to Reproduce
1. Create an S3 bucket, a Lambda role WITHOUT states:SendTaskSuccess, and a Lambda function that calls SendTaskSuccess with the token it receives.
2. Create a STANDARD state machine with a Task state Resource=arn:aws:states:::lambda:invoke.waitForTaskToken targeting that function.
3. StartExecution. Expected: the in-function SendTaskSuccess is refused 400 AccessDeniedException and the execution stays RUNNING.
4. Attach states:SendTaskSuccess to the Lambda role and retry. Expected: 200 and the execution reaches SUCCEEDED.
5. Observed: neither step answers; the check runs out its 60 s deadline.
