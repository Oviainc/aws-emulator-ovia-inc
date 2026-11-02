Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__lambda__s3__sfn.tenant-setup-rerun-execution-name-rejected-and-put-item-replaces-record dynamodb__lambda__s3__sfn.tenant-workflow-input-without-tenant-id-fails-at-first-task
Issue-Key: dynamodb__lambda__s3__sfn.tenant-setup-rerun-execution-name-rejected-and-put-item-replaces-record|_await_workflow_role#1|AssertionError|assert not assumed and time.monotonic() < deadline, (
Issue-Key: dynamodb__lambda__s3__sfn.tenant-workflow-input-without-tenant-id-fails-at-first-task|_await_workflow_role#1|AssertionError|assert not assumed and time.monotonic() < deadline, (
Issue-Commit: fbef683f21970efb4631f2658975dd3b2c5af139

# dynamodb__lambda__s3__stepfunctions: a Task state invoking Lambda ends FAILED with error States.Runtime and an emulator-authored cause, so no tenant-provisioning workflow can run (1 cause)

> _Note: this issue was identified against the code base at commit `fbef683f21970efb4631f2658975dd3b2c5af139`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExecution

## Description
One net-new root cause in this service group.

Cause 1 of 1: every Step Functions execution whose Task state invokes a Lambda function terminates FAILED with error `States.Runtime` and an emulator-authored cause, so the tenant-provisioning estate's role barrier never observes a single SUCCEEDED control execution and neither scenario reaches its subject assertions.
Failing checks: dynamodb__lambda__s3__sfn.tenant-setup-rerun-execution-name-rejected-and-put-item-replaces-record, dynamodb__lambda__s3__sfn.tenant-workflow-input-without-tenant-id-fails-at-first-task.

AWS: a Task state whose Resource is the optimized `arn:aws:states:::lambda:invoke` integration (or a bare function ARN) actually invokes the function. On success the execution advances and, for a one-state machine, DescribeExecution reports `status="SUCCEEDED"` with the function's result as the execution output and no `error`/`cause`. When the invocation genuinely fails, DescribeExecution reports a Step Functions error NAME that reflects the real cause — an unhandled function exception as `Lambda.Unhandled` (or the exception class, e.g. `Lambda.AWSLambdaException` for a denied `lambda:InvokeFunction`) and, where a Catch or Fail state is reached, THAT state's own `Error` string — with `cause` carrying the function's structured error document (`{"errorType": ..., "errorMessage": ..., "trace": [...]}`). `States.Runtime` is reserved for interpreter-level faults such as an unresolvable InputPath/OutputPath. Smithy (sfn): DescribeExecution output models `status`, `error` and `cause`, so these are the modeled carriers of the classification a client branches on; Smithy (lambda): `Invoke` is a modeled data-plane operation returning `Payload`/`StatusCode`/`FunctionError`, not a refusal.
Emulator: both executions end `status='FAILED'`, `error='States.Runtime'`, `cause="the 'Invoke' operation of Lambda is not implemented by this emulator, so nothing was run for this task"`. The fixed text of that cause is entirely emulator-authored — AWS never emits the phrase "is not implemented by this emulator"; its variable part here is the operation name (`Invoke`) and the service (`Lambda`), neither of which belongs in an AWS cause document at all.
Why: a client cannot provision anything through a Lambda-backed state machine, and its error handling takes the wrong branch or none: `States.Runtime` matches no Retry/Catch `ErrorEquals` entry a real workload declares (`Lambda.Unhandled`, `Lambda.AWSLambdaException`, `States.TaskFailed`), and the cause is not a parseable `{errorType, errorMessage}` document, so code that reads `json.loads(cause)["errorMessage"]` fails. Downstream state (the DynamoDB tenant record, the S3 objects, the provisioned URL) is never written, so idempotency and replacement semantics are unobservable.
Fix: the Lambda `Invoke` data-plane handler in the lambda crate (it must execute the function package rather than answer NotImplemented) and the Task dispatch + failure classification of the Step Functions interpreter in the sfn crate (map a task failure to the AWS error name for its cause, let a Catch/Fail target set the execution's `error`/`cause`, and reserve `States.Runtime` for interpreter faults).
Related open issues 20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md, 20261023T114649Z-sfn-step-functions-never-executes-task-states-every-execution.md and 20261027T173205Z-sfn-step-functions-never-dispatches-the-optimized-lambda-invoke.md describe neighbouring symptoms; none of them carries these two replay keys, which is why they are filed here.
Traces: the two failing check ids listed above.

## Evidence
Cause 1 — Expected: the role-barrier control execution of the provisioning state machine reaches `status="SUCCEEDED"` (its Lambda task having run), after which the checks can assert on StartExecution's `ExecutionAlreadyExists` refusal and on the DynamoDB record the workflow wrote.
Actual, identically in both checks: `AssertionError: role barrier execution 1 ended 'FAILED': 'States.Runtime' "the 'Invoke' operation of Lambda is not implemented by this emulator, so nothing was run for this task"` — i.e. DescribeExecution answers status=FAILED, error=States.Runtime and a cause string that is emulator prose rather than the function's structured error document. No DynamoDB item and no S3 object is produced by either run.

## Steps to Reproduce
1. Create an IAM role trusted by lambda.amazonaws.com and a Python function (any handler) with it.
2. Create an IAM role trusted by states.amazonaws.com with `lambda:InvokeFunction` on that function ARN.
3. CreateStateMachine (STANDARD) with a single Task state: `{"Type": "Task", "Resource": "arn:aws:states:::lambda:invoke", "Parameters": {"FunctionName": "<fn arn>", "Payload.$": "$"}, "End": true}`.
4. StartExecution with any JSON input and poll DescribeExecution until the status is not RUNNING.
On AWS the execution reaches SUCCEEDED with the function's result as output. On the emulator it reaches FAILED with error `States.Runtime` and cause "the 'Invoke' operation of Lambda is not implemented by this emulator, so nothing was run for this task".
