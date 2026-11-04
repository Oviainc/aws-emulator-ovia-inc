Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__lambda__s3__stepfunctions.lambda-task-sfn-resultpath-merges-with-original-input events__lambda__s3__stepfunctions.sfn-execution-role-wrong-trust-blocks-lambda-task events__lambda__s3__stepfunctions.sfn-startexecution-input-passthrough-to-lambda-task events__lambda__s3__stepfunctions.sfn-task-lambda-s3-write-denied-without-permission events__lambda__s3__stepfunctions.sfn-task-lambda-writes-result-to-s3 events__lambda__s3__stepfunctions.sfn-task-token-heartbeat-timeout-fails-execution events__lambda__s3__stepfunctions.sfn-waitfortasktoken-lambda-sendtaskfailure-fails-execution events__lambda__s3__stepfunctions.sfn-waitfortasktoken-lambda-sendtasksuccess-resumes events__lambda__sqs__stepfunctions.sfn-task-invokes-lambda-propagates-payload events__lambda__sqs__stepfunctions.sfn-task-sqs-sendmessage-delivers events__lambda__sqs__stepfunctions.sfn-task-sqs-denied-without-permission events__lambda__sqs__stepfunctions.sfn-waitfortasktoken-sqs-callback
Issue-Key: events__lambda__s3__stepfunctions.lambda-task-sfn-resultpath-merges-with-original-input|run#11|AssertionError|assert resp["status"] == "SUCCEEDED", f"execution ended in {resp['status']}"
Issue-Key: events__lambda__s3__stepfunctions.sfn-execution-role-wrong-trust-blocks-lambda-task|run#14|AssertionError|assert "not authorized to assume" in wrong_final.get("cause", ""), wrong_final
Issue-Key: events__lambda__s3__stepfunctions.sfn-startexecution-input-passthrough-to-lambda-task|run#11|AssertionError|assert execution["status"] == "SUCCEEDED", (
Issue-Key: events__lambda__s3__stepfunctions.sfn-task-lambda-s3-write-denied-without-permission|run#13|AssertionError|assert denied_resp["status"] == "SUCCEEDED", f"expected SUCCEEDED, got {denied_resp['status']}"
Issue-Key: events__lambda__s3__stepfunctions.sfn-task-lambda-writes-result-to-s3|run#12|AssertionError|assert resp["status"] == "SUCCEEDED", f"execution ended in {resp['status']}: {resp.get('output')}"
Issue-Key: events__lambda__s3__stepfunctions.sfn-task-token-heartbeat-timeout-fails-execution|run#10|AssertionError|assert final["error"] == "States.Timeout", final
Issue-Key: events__lambda__s3__stepfunctions.sfn-waitfortasktoken-lambda-sendtaskfailure-fails-execution|run#10|AssertionError|assert final["error"] == "LambdaReportedFailure", final
Issue-Key: events__lambda__s3__stepfunctions.sfn-waitfortasktoken-lambda-sendtasksuccess-resumes|run#9|AssertionError|assert final["status"] == "SUCCEEDED", f"unexpected status {final['status']}: {final}"
Issue-Key: events__lambda__sqs__stepfunctions.sfn-task-invokes-lambda-propagates-payload|run#7|AssertionError|assert result["status"] == "SUCCEEDED", f"execution did not succeed: {result}"
Issue-Key: events__lambda__sqs__stepfunctions.sfn-task-sqs-sendmessage-delivers|run#6|AssertionError|assert result["status"] == "SUCCEEDED", result
Issue-Key: events__lambda__sqs__stepfunctions.sfn-task-sqs-denied-without-permission|_execute_until_status#7|Timeout|time.sleep(interval_s)
Issue-Key: events__lambda__sqs__stepfunctions.sfn-waitfortasktoken-sqs-callback|_receive_messages#6|Timeout|resp = sqs.receive_message(
Issue-Commit: 048778663a26e363156c6db724999c4fb229af36

# Step Functions never executes Task states: every execution containing a Task fails immediately with States.Runtime

> _Note: this issue was identified against the code base at commit `048778663a26e363156c6db724999c4fb229af36`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExecution

## Description
Cause 1 of 1: the state-machine interpreter does not implement the Task state type, so any execution that reaches a Task terminates FAILED instead of performing the task.

AWS: a Task state invokes its Resource (arn:aws:states:::lambda:invoke, arn:aws:states:::sqs:sendMessage, .waitForTaskToken, .sync, an HTTP task, etc.), feeds the result into ResultPath/OutputPath and moves to Next; a successful machine ends SUCCEEDED and DescribeExecution reports output. Failures that AWS does produce for a Task are States.TaskFailed (the invoked target failed), States.Permissions (the execution role cannot call the target) and States.Timeout / States.HeartbeatTimeout for a waitForTaskToken task that is not answered in time. States.Runtime is reserved for an interpreter-level error such as an unresolvable path, and is never the right outcome for a well-formed Task. Smithy (sfn): StartExecution/DescribeExecution model status and the error/cause pair; the error-name taxonomy above is recorded behavior plus the Step Functions documented error names.

Emulator: the execution is created, then immediately ends status=FAILED with error='States.Runtime' and cause="state '<Name>' is of type 'Task', which this emulator does not execute, so nothing was run for it" (stopDate ~0.5s after startDate). The same cause appears for Lambda invoke tasks, SQS sendMessage tasks, Batch SubmitJob tasks, callback (.waitForTaskToken) tasks and heartbeat tasks.

Why: Step Functions is unusable as an orchestrator. Positive flows never reach their targets, so nothing downstream of a Task is exercised; and every negative flow is mis-attributed - a check expecting States.Permissions for a role without lambda:InvokeFunction, States.TaskFailed for a failing target, or a heartbeat timeout all get the same States.Runtime, so the error taxonomy a client branches on is wrong in every case. ResultPath merging, input pass-through and task-token callbacks cannot be observed at all.

Fix: the Task state implementation in the state-machine interpreter (Resource ARN dispatch to the integrated service, integration patterns .sync and .waitForTaskToken, Parameters/ResultSelector/ResultPath application, the execution-role permission check, and the States.TaskFailed / States.Permissions / States.Timeout / States.HeartbeatTimeout error mapping), plus the execution-history events for the task.

Traces: see trace_ids; the cause string is identical across them, differing only in the state name (e.g. 'InvokeLambda', 'Send', 'WaitForCallback', 'WaitForHeartbeat', 'SubmitJob', 'ProvisionTenantKeyspace').

## Evidence
Expected: DescribeExecution status SUCCEEDED with the Task's output (or, for the negative legs, error 'States.Permissions' / 'States.TaskFailed' / a heartbeat timeout).
Actual: status FAILED, error 'States.Runtime', cause "state 'InvokeLambda' is of type 'Task', which this emulator does not execute, so nothing was run for it", stopDate 0.5s after startDate.
Sample assertions: "execution must run to completion: expected status SUCCEEDED, got 'FAILED'"; "the failure is reported as States.TaskFailed; got 'States.Runtime' with cause \"state 'SubmitJob' is of type 'Task', which this emulator does not execute...\""; "role barrier execution 1 ended 'FAILED': 'States.Runtime' ...".

## Steps to Reproduce
1. CreateStateMachine with a definition whose single state is a Task with Resource 'arn:aws:states:::lambda:invoke' (or 'arn:aws:states:::sqs:sendMessage', or the .waitForTaskToken variant) and End: true.
2. StartExecution with any input.
3. DescribeExecution -> status FAILED, error 'States.Runtime', cause "state '<Name>' is of type 'Task', which this emulator does not execute, so nothing was run for it".
