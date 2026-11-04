Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__lambda__sqs__stepfunctions.sfn-task-invokes-lambda-propagates-payload events__lambda__sqs__stepfunctions.sfn-waitfortasktoken-sqs-callback
Issue-Key: events__lambda__sqs__stepfunctions.sfn-task-invokes-lambda-propagates-payload|run#7|AssertionError|assert result["status"] == "SUCCEEDED", f"execution did not succeed: {result}"
Issue-Key: events__lambda__sqs__stepfunctions.sfn-waitfortasktoken-sqs-callback|_receive_messages#6|Timeout|resp = sqs.receive_message(
Issue-Commit: d9cf74aafe764d4a8806a821a1189d14d75bb9bb

# events__lambda__sqs__stepfunctions: a Lambda Task state fails States.Runtime with emulator wording, and the arn:aws:states:::sqs:sendMessage.waitForTaskToken integration sends no message to the queue (2 causes)

> _Note: this issue was identified against the code base at commit `d9cf74aafe764d4a8806a821a1189d14d75bb9bb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExecution

## Description
Two net-new root causes in the events/lambda/sqs/stepfunctions surface.

Cause 1 of 2: a Task state that invokes a Lambda function is never dispatched; the execution ends FAILED with error "States.Runtime" and an emulator-authored cause, so the payload never propagates. Failing check: events__lambda__sqs__stepfunctions.sfn-task-invokes-lambda-propagates-payload.
  AWS:      Smithy: DescribeExecution output carries status (ExecutionStatus enum) plus output (SensitiveData, max 262144), and error (SensitiveError, max 256) / cause (SensitiveCause, max 32768) only on failure. A Task state whose Resource is a Lambda function ARN invokes the function with the state input as its event payload; on success the execution reaches SUCCEEDED and `output` is the function's return value, with error/cause absent. A genuine invocation failure is reported with an AWS error name (Lambda.ServiceException, Lambda.AWSLambdaException, States.TaskFailed, or the function-reported error name) and an AWS- or interpreter-authored cause.
  Emulator: the execution ends ~0.5 s after StartExecution with status "FAILED", error "States.Runtime" and cause "the 'Invoke' operation of Lambda is not implemented by this emulator, so nothing was run for this task". The function body is never run, so the input payload is never transformed and no output is produced.
  Why:      no Lambda-backed state machine can complete, so a client sees FAILED for a definition AWS runs to SUCCEEDED; retry/catch logic keyed on Lambda.* or States.TaskFailed never matches States.Runtime, and anything that reads `cause` receives emulator prose rather than an AWS cause.
  Fix:      the Step Functions interpreter's Lambda Task dispatch plus the Lambda Invoke handler it calls, and the failure mapping that currently synthesises error "States.Runtime" with the emulator sentence.
  Related:  open issues 20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md and 20261102T091831Z-events-lambda-stepfunctions-every-task-state-invoking-lambda.md record the same shape under other service headings; neither carries this check's replay key.
  Traces:   events__lambda__sqs__stepfunctions.sfn-task-invokes-lambda-propagates-payload

Cause 2 of 2: the optimized SQS integration with a task token, Resource "arn:aws:states:::sqs:sendMessage.waitForTaskToken", delivers no message to the queue, so the callback can never be completed. Failing check: events__lambda__sqs__stepfunctions.sfn-waitfortasktoken-sqs-callback.
  AWS:      recorded behavior only (service integrations are not expressed in the Smithy API model). A Task state with Resource arn:aws:states:::sqs:sendMessage.waitForTaskToken and Parameters {QueueUrl, MessageBody} sends exactly one SQS message whose body is the rendered MessageBody — including the value of the reserved context path $$.Task.Token as the task token and the payload fields resolved from the state input — using the state machine role's sqs:SendMessage grant. The execution then stays in status RUNNING (DescribeExecution reports RUNNING) until the token is redeemed: SendTaskSuccess makes the execution SUCCEEDED with `output` equal to the supplied output, and SendTaskFailure (Smithy: taskToken required, error/cause optional) makes it FAILED carrying that error/cause. TimeoutSeconds bounds the wait.
  Emulator: no message ever appears on the queue across three StartExecution attempts (25 s receive window each), so the task token is never handed out and the check is cut off at the 60 s harness cap ("Check timed out after 60.0s"). The token-bearing message body, the RUNNING hold and the SendTaskSuccess resumption are all unreachable.
  Why:      the entire human-/worker-in-the-loop pattern is dead: a client that queues work with $$.Task.Token and resumes with SendTaskSuccess/SendTaskFailure never receives the work item, so its worker idles and the execution's state can never be advanced or distinguished from a hung one.
  Fix:      the Step Functions interpreter's optimized-integration dispatch for sqs:sendMessage and its .waitForTaskToken variant — Parameters rendering (including the $$.Task.Token context value), the SQS SendMessage call under the state machine role, and the callback registry that holds the execution RUNNING and matches an incoming SendTaskSuccess/SendTaskFailure token.
  Traces:   events__lambda__sqs__stepfunctions.sfn-waitfortasktoken-sqs-callback

## Evidence
Cause 1 — events__lambda__sqs__stepfunctions.sfn-task-invokes-lambda-propagates-payload [mismatch]
  Expected: DescribeExecution status SUCCEEDED with the Lambda's return value as output for input {"n": 21}.
  Actual:   AssertionError: execution did not succeed: {'status': 'FAILED', 'input': '{"n": 21}', 'error': 'States.Runtime', 'cause': "the 'Invoke' operation of Lambda is not implemented by this emulator, so nothing was run for this task", 'redriveStatus': 'REDRIVABLE'} — stopDate 0.5 s after startDate.

Cause 2 — events__lambda__sqs__stepfunctions.sfn-waitfortasktoken-sqs-callback [mismatch]
  Definition: single Task, Resource "arn:aws:states:::sqs:sendMessage.waitForTaskToken", Parameters {QueueUrl: <queue>, MessageBody: {"token.$": "$$.Task.Token", "job.$": "$.job"}}, TimeoutSeconds 300; role trusts states.amazonaws.com with sqs:SendMessage on the queue ARN.
  Expected: one SQS message whose body contains job = the input's job value and a non-empty token; DescribeExecution RUNNING while the token is outstanding; after SendTaskSuccess, status SUCCEEDED and output equal to the supplied output.
  Actual:   no message is received on the queue after three StartExecution attempts; the harness aborts — Error: Check timed out after 60.0s (the check's own guard is AssertionError "callback task never delivered a message to SQS").

## Steps to Reproduce
Cause 1: create a Lambda function and an IAM role trusting states.amazonaws.com with lambda:InvokeFunction on it; create a state machine with one Task state whose Resource is the function ARN; StartExecution with input {"n": 21}; poll DescribeExecution. Observe status FAILED, error 'States.Runtime', cause "the 'Invoke' operation of Lambda is not implemented by this emulator, ...".

Cause 2: create an SQS queue and an IAM role trusting states.amazonaws.com with sqs:SendMessage on the queue ARN; create a state machine whose single Task has Resource "arn:aws:states:::sqs:sendMessage.waitForTaskToken", Parameters {QueueUrl: <queue url>, MessageBody: {"token.$": "$$.Task.Token", "job.$": "$.job"}}, TimeoutSeconds 300, End true; StartExecution with {"job": "j-token"}; ReceiveMessage on the queue. No message arrives, so there is no token to pass to SendTaskSuccess and DescribeExecution never shows the RUNNING-then-SUCCEEDED transition.

Replay: run_behavioral_suite and re-run the two checks listed in trace_ids.
