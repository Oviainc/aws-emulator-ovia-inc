Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__sqs__stepfunctions.sfn-wait-for-task-token-lambda-resumes lambda__sqs__stepfunctions.sfn-sqs-waitfortasktoken-heartbeat-timeout-negative lambda__sqs__stepfunctions.sfn-task-token-invalid-token-sendtasksuccess-negative
Issue-Key: lambda__sqs__stepfunctions.sfn-wait-for-task-token-lambda-resumes|_wait_for_messages#7|Timeout|resp = sqs.receive_message(
Issue-Key: lambda__sqs__stepfunctions.sfn-sqs-waitfortasktoken-heartbeat-timeout-negative|_receive_token#7|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=1, WaitTimeSeconds=5)
Issue-Key: lambda__sqs__stepfunctions.sfn-task-token-invalid-token-sendtasksuccess-negative|_receive_token#6|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=1, WaitTimeSeconds=5)
Issue-Commit: e20658567def5986c911ca4169de4ecb96db84bd

# lambda__sqs__stepfunctions: the arn:aws:states:::sqs:sendMessage.waitForTaskToken callback never resolves — no task token is ever usable, so resume, heartbeat timeout and invalid-token refusal all hang to the harness cap (1 cause)

> _Note: this issue was identified against the code base at commit `e20658567def5986c911ca4169de4ecb96db84bd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SendTaskSuccess

## Description
One net-new root cause behind 3 failing checks.

Cause 1 of 1: a Task state with `Resource: arn:aws:states:::sqs:sendMessage.waitForTaskToken` never produces a resolvable callback. The execution never reaches the state where a token delivered on the queue can be redeemed with SendTaskSuccess, never times out on `HeartbeatSeconds`, and never reaches a terminal state, so all three callback-lifecycle checks run out the 60 s harness deadline with no verdict.

AWS: the `.waitForTaskToken` pattern sends the message (with `$$.Task.Token` substituted into MessageBody), then PARKS the Task. SendTaskSuccess with that token resumes the execution, which then reaches SUCCEEDED with the callback output; SendTaskSuccess with a token that was never issued is refused with `TaskDoesNotExist` (HTTP 400 — Smithy: SendTaskSuccess errors InvalidToken, TaskDoesNotExist, TaskTimedOut in services/service-com-amazonaws-sfn-awsstepfunctions, taskToken @length 1..2048, output @length 0..262144); a token whose heartbeat window has elapsed fails the Task with `States.Timeout` and any later SendTaskSuccess for it is refused `TaskTimedOut`. Each refusal message is AWS's fixed text for that class with the offending token/ARN taken from the request; the error CODE and the 400 are the requirement. Nothing in AWS makes these calls hang.

Emulator: all three checks are killed at `Check timed out after 60.0s` — no terminal execution status, no delivered token that SendTaskSuccess accepts, and no refusal for the fabricated token. The callback is not merely misworded: it never completes in either direction, including the in-file positive controls.

Why: a client relying on AWS's behavior cannot implement the human/external-approval callback pattern at all. A resumed execution never finishes, a never-heartbeated task never times out (so no `States.Timeout` and no `TaskTimedOut` on a late send), and a forged token is neither accepted nor refused — so an application cannot distinguish a valid token from a fabricated one, and every caller blocks indefinitely instead of receiving a 400.

Fix: the `.waitForTaskToken` callback machinery in the sfn service crate's Task-state handler and the SendTaskSuccess/SendTaskFailure/SendTaskHeartbeat handlers — mint and register the token, park the Task, resume it on SendTaskSuccess, enforce `HeartbeatSeconds` by failing the Task `States.Timeout`, and refuse unknown tokens `TaskDoesNotExist` and expired ones `TaskTimedOut` with AWS's status and codes. The open issue 20261105T081644Z-lambda-sqs-stepfunctions-no-aws-sdk-in-the.md touches task tokens for sibling checks of this suite and 20261029T183117Z-lambda-sfn-sqs-nine-net-new-causes-sqs.md owns other sqs-integration causes; neither carries these checks' replay keys.

Traces: lambda__sqs__stepfunctions.sfn-wait-for-task-token-lambda-resumes (positive resume), lambda__sqs__stepfunctions.sfn-sqs-waitfortasktoken-heartbeat-timeout-negative (heartbeat expiry), lambda__sqs__stepfunctions.sfn-task-token-invalid-token-sendtasksuccess-negative (fabricated token).

## Evidence
Expected (AWS):
- resume: the Task parks; the token is read off the SQS message; SendTaskSuccess returns 200 and the execution reaches SUCCEEDED with the callback output.
- heartbeat timeout: a never-heartbeated task fails with `States.Timeout`, and a later SendTaskSuccess for that token is refused HTTP 400 `TaskTimedOut`; the heartbeated sibling still SUCCEEDS.
- fabricated token: SendTaskSuccess is refused HTTP 400 `TaskDoesNotExist` and resumes nothing, while a real token still resolves.

Actual (emulator): all three checks report only `Error: Check timed out after 60.0s` — no terminal execution status, no accepted token, and no 400/error code for the fabricated or expired token.

## Steps to Reproduce
1. Create an IAM role trusted by states.amazonaws.com granting sqs:SendMessage, and one or two SQS queues.
2. CreateStateMachine (STANDARD) with a single Task state: Resource=arn:aws:states:::sqs:sendMessage.waitForTaskToken, Parameters MessageBody={"token.$": "$$.Task.Token"}, HeartbeatSeconds set.
3. StartExecution, then ReceiveMessage on the queue and read the token from the body.
4. Call SendTaskSuccess with that token — expect 200 and the execution to reach SUCCEEDED; observed: never resolves.
5. Call SendTaskSuccess with a fabricated token — expect 400 TaskDoesNotExist; observed: no answer before the deadline.
6. Let a second execution's heartbeat window elapse — expect the Task to fail States.Timeout and a later SendTaskSuccess to be refused TaskTimedOut; observed: neither occurs.
