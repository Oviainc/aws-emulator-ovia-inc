Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__lambda__sqs__stepfunctions.sfn-task-invokes-lambda-propagates-payload events__lambda__sqs__stepfunctions.sfn-waitfortasktoken-sqs-callback
Issue-Key: events__lambda__sqs__stepfunctions.sfn-task-invokes-lambda-propagates-payload|run#7|AssertionError|assert result["status"] == "SUCCEEDED", f"execution did not succeed: {result}"
Issue-Key: events__lambda__sqs__stepfunctions.sfn-waitfortasktoken-sqs-callback|_receive_messages#6|Timeout|resp = sqs.receive_message(
Issue-Commit: 39700f19148179f97c0fef93ca04528bc9d9d02f

# events__lambda__sqs__stepfunctions: a Lambda Task state fails States.Runtime instead of propagating the function's payload, and sqs:sendMessage.waitForTaskToken delivers no callback message at all (2 causes)

> _Note: this issue was identified against the code base at commit `39700f19148179f97c0fef93ca04528bc9d9d02f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExecution

## Description
Two net-new root causes in the Step Functions data plane as exercised through SQS- and Lambda-integrated Task states. Neither failing check is carried as a replay key by an open issue; the closest open issues are named per cause.

Cause 1 of 2: a Task state targeting a Lambda function never invokes it, so the function's payload is never propagated and the execution terminates FAILED with an interpreter error.
AWS: a Task whose Resource is a Lambda function ARN (or `arn:aws:states:::lambda:invoke`) calls Lambda Invoke, binds the function's returned JSON as the state result, and the execution reaches SUCCEEDED; DescribeExecution then reports `output` carrying that payload and populates neither `error` nor `cause`. Smithy (sfn): DescribeExecutionOutput.status is the required ExecutionStatus enum; `output` is SensitiveData @length max 262144; `error` (SensitiveError, max 256) and `cause` (SensitiveCause, max 32768) are set only for a failed execution.
Emulator: the execution terminates FAILED with `error` = the fixed string `States.Runtime` and `cause` = the emulator-authored sentence "the 'Invoke' operation of Lambda is not implemented by this emulator, so nothing was run for this task" (only the operation name `Invoke` and the service name `Lambda` vary; AWS never emits this sentence), and no `output` is produced. DescribeExecution answers HTTP 200.
Why: Lambda cannot be orchestrated. The caller's input is never delivered to the handler and the handler's result never becomes the execution output, so any pipeline that transforms data in Lambda produces nothing; and because the single reported name is `States.Runtime` — which AWS reserves for interpreter/definition faults — a Catch or Retry keyed on the function's own error name can never match.
Fix: the Lambda `Invoke` handler (no in-process execution of a deployment package, so Invoke refuses) plus the Step Functions Task-state integration path that maps the invocation result into the task outcome, the TaskSucceeded/TaskFailed history events and the execution `output`. Related open issues that do NOT carry this check: 20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md and cause 1 of 20261024T043150Z-sfn-5-of-the-6-interpreter-causes-remain-open.md.
Traces (failing check): events__lambda__sqs__stepfunctions.sfn-task-invokes-lambda-propagates-payload.

Cause 2 of 2: a Task whose Resource ends in `.waitForTaskToken` sends nothing to its target, so no task token is ever produced and the callback round trip never starts.
AWS: for `arn:aws:states:::sqs:sendMessage.waitForTaskToken`, Step Functions mints a task token, substitutes it wherever the Parameters reference `$$.Task.Token`, calls SQS SendMessage with the rendered MessageBody, and then SUSPENDS the execution in status RUNNING until the token is resolved. ReceiveMessage on the target queue therefore returns one message whose body is the rendered JSON (here {"token": <the minted task token>, "job": <value from the execution input>}); DescribeExecution reports status RUNNING; SendTaskSuccess(taskToken=<that token>, output=<JSON>) then drives the execution to SUCCEEDED with `output` equal to the JSON passed to SendTaskSuccess. Smithy (sfn): SendTaskSuccess requires taskToken (TaskToken @length 1..2048, sensitive) and output (SensitiveData, max 262144) and models TaskDoesNotExist / TaskTimedOut / InvalidToken for an unresolvable token, which only makes sense if a token is minted during the walk. TimeoutSeconds on the state bounds the wait.
Emulator: no message ever reaches the queue. ReceiveMessage returns empty for the whole polling window across repeated StartExecution attempts, so the check never gets a token and never reaches its RUNNING / SendTaskSuccess / SUCCEEDED assertions — it runs out the harness's 60 s cap and is reported as "Check timed out after 60.0s". The callback integration pattern is refused inside the interpreter rather than being executed, so `$$.Task.Token` is never substituted and the execution never suspends.
Why: every human-in-the-loop or external-worker workflow is unusable: the worker is never handed a token, so SendTaskSuccess/SendTaskFailure can never be exercised, and a client cannot tell a suspended execution from a dead one. It also makes the check unable to reach a verdict at all rather than failing with a diagnosis.
Fix: the Task-state integration dispatch for the `.waitForTaskToken` pattern in the Step Functions interpreter — mint the token during the walk, substitute `$$.Task.Token` into the rendered Parameters, perform the target call (SQS SendMessage) and suspend the run until SendTaskSuccess/SendTaskFailure/SendTaskHeartbeat resolves or TimeoutSeconds expires. Cause 5 of the open issue 20261024T043150Z-sfn-5-of-the-6-interpreter-causes-remain-open.md describes this same gap but does not carry this check as a replay key.
Traces (failing check): events__lambda__sqs__stepfunctions.sfn-waitfortasktoken-sqs-callback.

## Evidence
Cause 1 — check events__lambda__sqs__stepfunctions.sfn-task-invokes-lambda-propagates-payload
Expected: DescribeExecution status SUCCEEDED, with the function's payload (derived from the execution input {"n": 21}) as the execution output.
Actual: AssertionError: execution did not succeed: {'executionArn': 'arn:aws:states:...:execution:<sm>:<name>', 'status': 'FAILED', 'input': '{"n": 21}', 'error': 'States.Runtime', 'cause': "the 'Invoke' operation of Lambda is not implemented by this emulator, so nothing was run for this task", 'redriveStatus': 'REDRIVABLE', HTTPStatusCode 200} — no `output` member.

Cause 2 — check events__lambda__sqs__stepfunctions.sfn-waitfortasktoken-sqs-callback
State machine: one state {"Type":"Task","Resource":"arn:aws:states:::sqs:sendMessage.waitForTaskToken","Parameters":{"QueueUrl":<queue>,"MessageBody":{"token.$":"$$.Task.Token","job.$":"$.job"}},"TimeoutSeconds":300,"End":true}; role grants sqs:SendMessage on the queue.
Expected: ReceiveMessage on the queue returns one message whose JSON body is {"token": <minted task token>, "job": "j-token"}; DescribeExecution reports RUNNING; SendTaskSuccess with that token drives the execution to SUCCEEDED with output == the JSON passed to SendTaskSuccess.
Actual: Error: Check timed out after 60.0s. Across three StartExecution attempts the queue stays empty (the check's own guard is "callback task never delivered a message to SQS"), so no token exists and SendTaskSuccess is never reachable.

## Steps to Reproduce
Cause 1:
1. lambda CreateFunction with a handler that returns a value derived from its event (e.g. {"doubled": event["n"] * 2}).
2. IAM CreateRole trusted by states.amazonaws.com granting lambda:InvokeFunction on that function ARN.
3. sfn CreateStateMachine (STANDARD) with one Task state targeting the function ARN; StartExecution with input {"n": 21}; poll DescribeExecution.
4. Observe FAILED / error 'States.Runtime' / cause "the 'Invoke' operation of Lambda is not implemented by this emulator, so nothing was run for this task" and no `output`, instead of SUCCEEDED with the function's payload.

Cause 2:
1. sqs CreateQueue; IAM CreateRole trusted by states.amazonaws.com with an inline Allow of sqs:SendMessage on that queue ARN.
2. sfn CreateStateMachine (STANDARD) with one state: {"Type":"Task","Resource":"arn:aws:states:::sqs:sendMessage.waitForTaskToken","Parameters":{"QueueUrl":"<queue url>","MessageBody":{"token.$":"$$.Task.Token","job.$":"$.job"}},"TimeoutSeconds":300,"End":true}.
3. StartExecution with input {"job":"j-token"}, then ReceiveMessage(QueueUrl=<queue url>, WaitTimeSeconds=5) in a loop for ~25 s.
4. Observe that no message ever arrives, so no task token can be read and SendTaskSuccess cannot be called; DescribeExecution never reports a suspended RUNNING state resolvable by a callback.
