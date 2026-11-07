Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__lambda__s3__stepfunctions.cfn-deploy-sfn-task-token-lambda-callback-stack events__lambda__s3__stepfunctions.cfn-deploy-eventbridge-fanout-lambda-sfn-stack events__lambda__s3__stepfunctions.sfn-execution-role-wrong-trust-blocks-lambda-task events__lambda__s3__stepfunctions.sfn-task-lambda-writes-result-to-s3
Issue-Key: events__lambda__s3__stepfunctions.cfn-deploy-sfn-task-token-lambda-callback-stack|run#13|AssertionError|assert final["status"] == "SUCCEEDED", final.get("error")
Issue-Key: events__lambda__s3__stepfunctions.cfn-deploy-eventbridge-fanout-lambda-sfn-stack|_poll_until#16|Timeout|time.sleep(interval_s)
Issue-Key: events__lambda__s3__stepfunctions.sfn-execution-role-wrong-trust-blocks-lambda-task|_poll_execution#20|Timeout|time.sleep(interval_s)
Issue-Key: events__lambda__s3__stepfunctions.sfn-task-lambda-writes-result-to-s3|_wait_execution#11|Timeout|time.sleep(2)
Issue-Commit: 569322fd02c2429e9947d96bd3e38eb76e62453b

# events__lambda__s3__stepfunctions: the in-function SDK has no Step Functions client so a waitForTaskToken callback fails with UnknownServiceError, and Lambda-Task / EventBridge-fanout executions never produce their effect or a terminal status (2 causes)

> _Note: this issue was identified against the code base at commit `569322fd02c2429e9947d96bd3e38eb76e62453b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExecution

## Description
Two net-new root causes in this service group. Both must be closed before submitting.

Cause 1 of 2: a Lambda handler that calls Step Functions cannot construct the client, so the waitForTaskToken callback never happens and the execution's error is the client-construction failure itself.
Failing check ids: events__lambda__s3__stepfunctions.cfn-deploy-sfn-task-token-lambda-callback-stack.
AWS: a Task whose Resource is arn:aws:states:::lambda:invoke.waitForTaskToken hands the function $$.Task.Token and leaves the Task RUNNING until the function calls states SendTaskSuccess(taskToken, output) (or SendTaskFailure). The managed Lambda python runtime bundles an AWS SDK that includes the Step Functions client, so `boto3.client('stepfunctions')` resolves and the callback lands; the execution then SUCCEEDS and its output is the JSON the callback supplied ({"doubled": <2 * input value>} here — the shape is the requirement, the number is this run's value). Recorded behavior only for the stack wiring: the state machine, role and function come from a CloudFormation stack that must reach CREATE_COMPLETE first.
Emulator: the execution reaches a terminal state but not SUCCEEDED, and DescribeExecution's `error` is "UnknownServiceError" — the botocore name for "this installation has no service model called stepfunctions". So the handler never reaches send_task_success: the emulator's in-process python runtime ships an SDK without a Step Functions client, and the resulting client-construction exception is surfaced as the execution's error instead of a task result.
Why: every callback pattern (waitForTaskToken driven by a Lambda, human-approval and long-poll workflows) is unusable, and the failure a client sees names a botocore internal rather than anything about their workflow, so it reads as a harness bug rather than a missing grant or a bad token.
Fix: the in-process Lambda runtime's bundled SDK/service-model set — the component that provisions the python runtime's botocore data — must carry the Step Functions service model (and, generally, the models for every service the emulator serves) so `boto3.client('stepfunctions')` constructs; the Task's error should then come from SendTaskSuccess itself rather than from client construction.

Cause 2 of 2: a Lambda Task state produces neither the function's side effect nor a terminal execution status, so Lambda-Task and EventBridge-fanout scenarios hang until the harness kills them.
Failing check ids: events__lambda__s3__stepfunctions.cfn-deploy-eventbridge-fanout-lambda-sfn-stack, events__lambda__s3__stepfunctions.sfn-execution-role-wrong-trust-blocks-lambda-task, events__lambda__s3__stepfunctions.sfn-task-lambda-writes-result-to-s3.
AWS: a Task state whose Resource is a function ARN (or arn:aws:states:::lambda:invoke) invokes that function under the state machine's execution role; the function's own side effects become visible (here, an S3 object written by the handler), DescribeExecution leaves RUNNING for a terminal status within seconds, and GetExecutionHistory carries LambdaFunctionScheduled / LambdaFunctionSucceeded entries whose input and output are exactly the payload the Task was given. When the execution role's trust policy names lambda.amazonaws.com instead of states.amazonaws.com, the execution ends FAILED with a cause containing "not authorized to assume" and the function is never invoked (its object is absent, HeadObject 404) — the fixed part is the status FAILED and the "not authorized to assume" wording; the role ARN inside the cause is taken from the request. An EventBridge rule fanning out to both a function and a state machine produces both effects: the function's marker object and a matching execution that reaches SUCCEEDED with the event's detail in its output.
Emulator: all three scenarios run to the 60 s harness cap with "Check timed out after 60.0s". Nothing converges: the Lambda Task's S3 object never appears, DescribeExecution never leaves RUNNING for either the wrong-trust negative or its correctly-trusted twin, and the fanout rule's lambda-marker object never appears. Because no terminal status is ever reported, neither the FAILED status nor the "not authorized to assume" cause nor the SUCCEEDED output is produced at all.
Why: a client cannot build any Lambda-backed workflow. StartExecution returns an executionArn that never resolves, so a caller polling DescribeExecution waits forever instead of getting a result or a diagnosable failure; and because the wrong-trust negative also hangs rather than failing, a genuinely misconfigured execution role is indistinguishable from a healthy one.
Fix: the Step Functions interpreter's Task-state dispatcher — the component that invokes the target function under the execution role, records LambdaFunctionScheduled/Succeeded/Failed history and drives the execution to a terminal status — plus the execution-role trust evaluation that must end the execution FAILED with the "not authorized to assume" cause when the role does not trust states.amazonaws.com, and the EventBridge target dispatcher for a state-machine/function fan-out.

Note: open issues 20261104T221115Z-events-lambda-s3-stepfunctions-every-lambda-task-state.md, 20261104T221123Z-events-lambda-s3-stepfunctions-a-lambda-invoking-task.md and 20261105T081644Z-events-lambda-s3-stepfunctions-the-lambda-python-runtime.md describe neighbouring symptoms in this group, but the replay does not mark any of these four checks as owned, so they are filed here.

Traces: the four check ids listed under the two causes above.

## Evidence
Cause 1 — events__lambda__s3__stepfunctions.cfn-deploy-sfn-task-token-lambda-callback-stack [mismatch]
Expected (AWS): the CFN-deployed waitForTaskToken Task hands the function $$.Task.Token; the handler's boto3 stepfunctions client calls SendTaskSuccess(taskToken=<token>, output={"doubled": <value*2>}); DescribeExecution then reports status SUCCEEDED with that output.
Actual (emulator):
  File "check_events__lambda__s3__stepfunctions_cfn_deploy_sfn_task_token_lambda_callback_stack.py", line 158, in run
    assert final["status"] == "SUCCEEDED", final.get("error")
  AssertionError: UnknownServiceError
The execution's `error` is "UnknownServiceError" — `boto3.client('stepfunctions')` could not be constructed inside the function, so SendTaskSuccess was never called.

Cause 2 — three checks, all "Error: Check timed out after 60.0s":
* sfn-task-lambda-writes-result-to-s3: expected the Lambda Task (Resource=<function ARN>) to run, DescribeExecution to reach SUCCEEDED with output echoing the StartExecution input, GetExecutionHistory to carry LambdaFunctionScheduled/LambdaFunctionSucceeded with that same payload, and GetObject to return the handler-written JSON. Actual: no terminal status and no object within the cap.
* sfn-execution-role-wrong-trust-blocks-lambda-task: expected the wrong-trust execution to reach status FAILED with a cause containing "not authorized to assume" and HeadObject on its key to answer 404, and the correctly-trusted twin to reach SUCCEEDED with the object body b"invoked". Actual: DescribeExecution never leaves RUNNING for either; no status, no cause.
* cfn-deploy-eventbridge-fanout-lambda-sfn-stack: expected PutEvents (FailedEntryCount 0) to fan out so that the Lambda target writes lambda-marker-<marker>.txt with the marker as its body AND a matching execution reaches SUCCEEDED with output.detail.marker == <marker>. Actual: the marker object never appears, so the execution leg is never reached.

## Steps to Reproduce
Cause 1:
1. Deploy a CloudFormation stack containing an IAM role for lambda.amazonaws.com with states:SendTaskSuccess, a python function whose handler builds boto3.client('stepfunctions') and calls send_task_success(taskToken=event['token'], output=json.dumps({'doubled': event['input']['value']*2})), a states role, and an AWS::StepFunctions::StateMachine whose single Task has Resource arn:aws:states:::lambda:invoke.waitForTaskToken, Parameters {FunctionName: <fn ARN>, Payload: {"token.$": "$$.Task.Token", "input.$": "$"}} and TimeoutSeconds 60.
2. StartExecution with input {"value": <n>} and poll DescribeExecution.
   Expected: status SUCCEEDED, output {"doubled": <2n>}.
   Actual: not SUCCEEDED; error == "UnknownServiceError".

Cause 2:
3. Create a bucket; create a lambda.amazonaws.com role granting s3:PutObject on arn:aws:s3:::<bucket>/*; create a python function whose handler writes an object with boto3 s3 put_object.
4. Create a states.amazonaws.com role granting lambda:InvokeFunction on the function, and a state machine with one Task whose Resource is the function ARN.
5. StartExecution and poll DescribeExecution + GetExecutionHistory, then GetObject the key the handler wrote.
   Expected: SUCCEEDED within seconds, LambdaFunctionScheduled/Succeeded in the history, the object readable.
   Actual: DescribeExecution stays RUNNING and no object appears; the check is killed at 60 s.
6. Repeat step 4 with a role whose trust policy names lambda.amazonaws.com instead of states.amazonaws.com and StartExecution again.
   Expected: FAILED with a cause containing "not authorized to assume", and HeadObject on the Task's key answering 404.
   Actual: the execution never leaves RUNNING.
7. Deploy a CFN stack with a bucket, a Lambda writing lambda-marker-<marker>.txt from event.detail.marker, a Pass-only state machine, and an EventBridge rule targeting both; PutEvents a matching entry and poll for the marker object and a matching execution.
   Expected: the marker object holds <marker> and the execution reaches SUCCEEDED with output.detail.marker == <marker>.
   Actual: neither effect occurs before the 60 s cap.
