Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__lambda__s3__secretsmanager.deploy-cfn-role-trusts-s3-service-for-batch-operations
Issue-Key: dynamodb__lambda__s3__secretsmanager.deploy-cfn-role-trusts-s3-service-for-batch-operations|run#22|AssertionError|assert job["Status"] == "Complete", (
Issue-Commit: 690f4b176f649d001b95f75b53d79a24230cccca

# S3 Batch Operations never runs a LambdaInvoke job: the job settles Failed with an emulator-authored 'This emulator does not run the LambdaInvoke action' reason instead of Complete

> _Note: this issue was identified against the code base at commit `690f4b176f649d001b95f75b53d79a24230cccca`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateJob

## Description
Cause 1 of 1: an S3 Batch Operations job whose Operation is `LambdaInvoke` is accepted by CreateJob but its action is never executed; the job is driven straight to the terminal status `Failed` carrying an emulator-authored failure reason, so no manifest object is ever handed to the Lambda function.

AWS: Smithy (s3control) models `JobOperation.LambdaInvoke -> LambdaInvokeOperation{FunctionArn, InvocationSchemaVersion, UserArguments}` — "Directs the specified job to invoke an Lambda function on every object in the manifest." `JobStatus` enum: Active, Cancelled, Cancelling, Complete, Completing, Failed, Failing, New, Paused, Pausing, Preparing, Ready, Suspended. For a job created with ConfirmationRequired=false and a RoleArn that trusts batchoperations.s3.amazonaws.com and is granted lambda:InvokeFunction on the target function, AWS assumes that role, invokes the function once per manifest entry with the Batch Operations invocation payload (invocationId, invocationSchemaVersion, tasks[].taskId/s3BucketArn/s3Key), honours each returned per-task resultCode, and the job transits New -> Preparing -> Active -> Complete with `ProgressSummary.TotalNumberOfTasks` == manifest entry count, `NumberOfTasksSucceeded` == the number of tasks that answered `Succeeded`, and `NumberOfTasksFailed` == 0; a job that completes carries no `FailureReasons`. Recorded behavior: the manifest held 2 CSV entries (Bucket,Key) and the function tagged each object, so AWS ended the job `Complete` with 2 succeeded / 0 failed and both objects carrying the Lambda-applied tag. AWS only populates `FailureReasons` (`JobFailure{FailureCode <=64 chars, FailureReason <=256 chars}`) for a job it actually failed, and its `FailureCode`/`FailureReason` values are AWS's own fixed vocabulary (e.g. an unassumable role), never a statement about the implementation serving the request.

Emulator: DescribeJob reports `Status: 'Failed'` with `FailureReasons: [{'FailureCode': 'JobFailed', 'FailureReason': 'This emulator does not run the LambdaInvoke action of an S3 Batch Operations job.'}]`. Both strings are emulator-authored: `JobFailed` is not the code AWS reports for this path, and the reason sentence names the emulator itself. No invocation reaches the function, so neither manifest object is tagged and ProgressSummary never records a succeeded task.

Why: a client that uses S3 Batch Operations to fan a Lambda function out over a manifest (the standard bulk object-processing pattern) sees every job fail. Polling for `Complete` never succeeds, the per-object side effects the function performs never happen, and error handling keyed on AWS's FailureCode vocabulary cannot match `JobFailed` with an implementation sentence as its reason. It also makes the `batchoperations.s3.amazonaws.com` role-trust and the role's `lambda:InvokeFunction` grant unverifiable end to end, since the assume-role/invoke step is never attempted.

Fix: the S3 Control Batch Operations job executor in the s3control service crate — the component that dispatches a job's `Operation` union must implement the `LambdaInvoke` branch: assume the job's RoleArn, build the Batch Operations invocation payload per manifest entry, Invoke the FunctionArn, fold each returned `results[].resultCode` into the job's ProgressSummary, and settle the job `Complete` with no FailureReasons when no task permanently failed. While unimplemented it must not fabricate a `JobFailure` describing the emulator.

Note: several open issues cover S3 Batch Operations jobs that never reach a terminal status at all (20261102T121736Z-s3control-s3-control-batch-job-s3putobjecttagging-never-reaches-a.md, 20261029T195302Z-s3control-batch-operations-a-confirmationrequired-false-s3putobjecttagging-job.md, 20261107T182201Z-emr-glue-s3-2-net-new-root-causes-f0288f.md, 20261103T080052Z-s3control-s3-policy-enforcement-and-batch-operations-diverge.md). None of them covers this cause: here the job DOES settle, and it settles `Failed` because the `LambdaInvoke` action specifically is not executed.

Traces: dynamodb__lambda__s3__secretsmanager.deploy-cfn-role-trusts-s3-service-for-batch-operations.

## Evidence
Check dynamodb__lambda__s3__secretsmanager.deploy-cfn-role-trusts-s3-service-for-batch-operations (verdict: mismatch).

Expected (AWS): DescribeJob -> Job.Status == 'Complete', no FailureReasons, ProgressSummary.NumberOfTasksSucceeded == 2 (one per manifest entry), NumberOfTasksFailed == 0, and GetObjectTagging on each manifest key returns the Lambda-applied {'Key': 'processed', 'Value': 'true'} tag.

Actual (emulator): the job reaches terminal status 'Failed':
  AssertionError: batch job <job-id> settled into 'Failed' (reasons [{'FailureCode': 'JobFailed', 'FailureReason': 'This emulator does not run the LambdaInvoke action of an S3 Batch Operations job.'}]), expected Complete
(the job id is a per-run value; the diverging parts are the status 'Failed', the FailureCode 'JobFailed' and the fixed emulator-authored FailureReason sentence, which is identical on every run and names no request or state value). The function is never invoked, so no object carries the 'processed' tag and ProgressSummary records no succeeded task.

## Steps to Reproduce
1. Deploy a CloudFormation stack containing: an S3 bucket; a Lambda execution role trusting lambda.amazonaws.com with s3:PutObjectTagging/s3:GetObject on the bucket; a python3.12 function that handles the S3 Batch Operations payload and tags event['tasks'][0]['s3Key'] with processed=true, returning resultCode 'Succeeded'; and a role trusting batchoperations.s3.amazonaws.com with s3:GetObject/s3:GetBucketLocation on the bucket and lambda:InvokeFunction on the function.
2. Put two objects (obj1.txt, obj2.txt) and a manifest.csv of '<bucket>,<key>' lines into the bucket; note the manifest ETag.
3. s3control.create_job(AccountId=<account>, ConfirmationRequired=False, Operation={'LambdaInvoke': {'FunctionArn': <function arn>}}, Report={'Enabled': False}, Manifest={'Spec': {'Format': 'S3BatchOperations_CSV_20180820', 'Fields': ['Bucket','Key']}, 'Location': {'ObjectArn': 'arn:aws:s3:::<bucket>/manifest.csv', 'ETag': <etag>}}, Priority=1, RoleArn=<batch ops role arn>, ClientRequestToken=<token>).
4. Poll s3control.describe_job(AccountId=<account>, JobId=<job id>) until Job.Status is terminal.
5. Observe Status 'Failed' with FailureReasons [{'FailureCode': 'JobFailed', 'FailureReason': 'This emulator does not run the LambdaInvoke action of an S3 Batch Operations job.'}] instead of 'Complete' with 2 succeeded / 0 failed; s3.get_object_tagging on obj1.txt and obj2.txt shows no 'processed' tag.
