Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__sqs__stepfunctions.sfn-lambda-wait-for-task-token-positive lambda__sqs__stepfunctions.sfn-sqs-send-message-wait-for-task-token-missing-perm-negative lambda__sqs__stepfunctions.sfn-sqs-waitfortasktoken-heartbeat-timeout-negative
Issue-Key: lambda__sqs__stepfunctions.sfn-lambda-wait-for-task-token-positive|_poll_until#20|AssertionError|raise AssertionError(
Issue-Key: lambda__sqs__stepfunctions.sfn-sqs-send-message-wait-for-task-token-missing-perm-negative|run#16|AssertionError|assert evidence.get("error_code") == "AccessDeniedException", (
Issue-Key: lambda__sqs__stepfunctions.sfn-sqs-waitfortasktoken-heartbeat-timeout-negative|run#16|AssertionError|assert "cause" not in desc_bad, repr(desc_bad.get("cause"))
Issue-Commit: 5c9d64a1694efbc03a63b34aabdcd776fcb5feb2

# lambda__sqs__stepfunctions: waitForTaskToken callbacks never resume the execution, a denied SendTaskSuccess carries no AccessDeniedException code, and a heartbeat timeout adds a 'cause' AWS omits (3 causes)

> _Note: this issue was identified against the code base at commit `5c9d64a1694efbc03a63b34aabdcd776fcb5feb2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SendTaskSuccess

## Description
Three root causes in the Step Functions task-token (.waitForTaskToken) callback path exercised through SQS and Lambda.

Cause 1 of 3: SendTaskSuccess from the callback consumer never resumes the waiting execution.
AWS: recorded behavior only for the resumption (the Smithy model gives SendTaskSuccess input taskToken @length 1-2048 and output @length 0-262144, and an empty output shape, but no resumption semantics). The recording shows that a Task with Resource 'arn:aws:states:::sqs:sendMessage.waitForTaskToken' delivers the MessageBody — including $$.Task.Token — to the queue, suspends the execution in RUNNING, and that a SendTaskSuccess carrying that token completes the task so the execution becomes SUCCEEDED with DescribeExecution.output equal to the output the callback sent (here the body {"callback": "done"}; the value is whatever the caller passed, not a literal).
Emulator: the execution stays RUNNING indefinitely. DescribeExecution still reads status 'RUNNING', redriveStatus 'NOT_REDRIVABLE', redriveStatusReason 'Execution is RUNNING and cannot be redriven' 30s after the SQS-event-source-mapped Lambda consumer ran, so either the token message/ESM delivery or the SendTaskSuccess resume path never completes the task. Expected SUCCEEDED with the callback's output.
Failing check: lambda__sqs__stepfunctions.sfn-lambda-wait-for-task-token-positive.

Cause 2 of 3: a SendTaskSuccess by a principal lacking states:SendTaskSuccess is refused, but the refusal carries no resolvable error code.
AWS: SendTaskSuccess's modeled errors are InvalidOutput, InvalidToken, TaskDoesNotExist, TaskTimedOut and the Kms* family — an authorization failure is not one of them; AWS returns the service-common authorization error HTTP 400 with code 'AccessDeniedException' and message 'User: <assumed-role principal ARN> is not authorized to perform: states:SendTaskSuccess on resource: <state machine ARN> because no identity-based policy allows the states:SendTaskSuccess action'. The fixed text is 'User: ... is not authorized to perform: ... on resource: ... because no identity-based policy allows the ... action'; the principal ARN, the resource ARN and the action name are taken from the request and the caller's identity, never from one recording.
Emulator: the call is refused (the consumer records attempt 'denied') but the error document carries no code the SDK can resolve — botocore finds no Error.Code, so the check reads error_code 'unknown' instead of 'AccessDeniedException'. A client cannot branch on the authorization failure at all.
Failing check: lambda__sqs__stepfunctions.sfn-sqs-send-message-wait-for-task-token-missing-perm-negative.

Cause 3 of 3: a heartbeat timeout populates DescribeExecution.cause, which AWS leaves absent.
AWS: DescribeExecution's output models 'cause' (SensitiveCause) as an OPTIONAL member, and the recording shows that an execution failing because a .waitForTaskToken task was never heartbeated within its HeartbeatSeconds reports status 'FAILED' with error 'States.Timeout' and NO cause member at all.
Emulator: the same execution returns a cause of "state 'SendToQueue' sent no heartbeat for its task token within the 5 seconds its 'HeartbeatSeconds' allows" (the state name and the seconds come from the definition), so clients that treat the presence of cause as 'this failure has a payload to parse' see an invented, non-AWS diagnostic string.
Failing check: lambda__sqs__stepfunctions.sfn-sqs-waitfortasktoken-heartbeat-timeout-negative.

Why: callback-style orchestration is unusable — an execution waiting on a task token never finishes (cause 1), a client cannot detect and remediate a missing states:SendTaskSuccess grant because the refusal is uncodeable (cause 2), and error-handling code keyed on the shape of a heartbeat timeout sees a cause where AWS has none (cause 3).

Fix: cause 1 — the Step Functions callback/resume path for .waitForTaskToken tasks (task-token registry plus the SQS send and the ESM delivery that carry it) must complete the task on SendTaskSuccess and drive the execution to SUCCEEDED with the sent output; cause 2 — the SendTaskSuccess handler's authorization layer must emit the service-common AccessDeniedException as a well-formed awsJson1_0 error document (status 400, code AccessDeniedException, message as worded above); cause 3 — the heartbeat-timeout path in the execution state/DescribeExecution serializer must set error 'States.Timeout' and omit the cause member.

Traces: lambda__sqs__stepfunctions.sfn-lambda-wait-for-task-token-positive, lambda__sqs__stepfunctions.sfn-sqs-send-message-wait-for-task-token-missing-perm-negative, lambda__sqs__stepfunctions.sfn-sqs-waitfortasktoken-heartbeat-timeout-negative

## Evidence
Cause 1 — expected DescribeExecution status 'SUCCEEDED' with output equal to the callback's SendTaskSuccess payload; actual: AssertionError 'condition not met within 30s. Last result: {... "status": "RUNNING", "redriveStatus": "NOT_REDRIVABLE", "redriveStatusReason": "Execution is RUNNING and cannot be redriven" ...}'.
Cause 2 — expected the callback's SendTaskSuccess to be refused with Error.Code 'AccessDeniedException'; actual: AssertionError "Expected AccessDeniedException, got: {'token': '...', 'attempt': 'denied', 'error_code': 'unknown'}" — the refusal carried no SDK-resolvable error code.
Cause 3 — expected DescribeExecution for the un-heartbeated execution to report status FAILED, error 'States.Timeout' and no 'cause' member; actual: AssertionError on the cause assertion with "state 'SendToQueue' sent no heartbeat for its task token within the 5 seconds its 'HeartbeatSeconds' allows".

## Steps to Reproduce
Cause 1: create an SQS queue and a Lambda (role granting states:SendTaskSuccess/SendTaskFailure and sqs:ReceiveMessage/DeleteMessage/GetQueueAttributes) that reads record bodies and calls SendTaskSuccess with body['taskToken']; wire an event source mapping from the queue to the function. Create a state machine whose single Task is Resource 'arn:aws:states:::sqs:sendMessage.waitForTaskToken' with Parameters {QueueUrl, MessageBody:{'taskToken.$':'$$.Task.Token','input.$':'$'}} and StartExecution. Poll DescribeExecution — it stays RUNNING instead of becoming SUCCEEDED with the callback output.
Cause 2: same wiring, but omit states:SendTaskSuccess from the consumer's role and have it report the caught exception's Error.Code to a second queue. The reported code is 'unknown' rather than 'AccessDeniedException'.
Cause 3: same state machine with HeartbeatSeconds 5; start an execution, receive the token from the queue, send no heartbeat, and wait for the execution to be terminal. DescribeExecution returns FAILED / error 'States.Timeout' but also a populated 'cause'.
