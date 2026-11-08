Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__s3__stepfunctions.sfn-invokes-lambda-via-task-positive lambda__s3__stepfunctions.sfn-task-lambda-passes-input-output lambda__s3__stepfunctions.sfn-pipeline-lambda-writes-s3 lambda__s3__stepfunctions.sfn-map-state-parallel-lambda-invocations lambda__s3__stepfunctions.sfn-express-workflow-lambda-sync-positive lambda__s3__stepfunctions.sfn-catch-lambda-error-and-write-s3 lambda__s3__stepfunctions.sfn-task-lambda-missing-invoke-permission-negative lambda__s3__stepfunctions.sfn-task-nonexistent-lambda-arn-negative lambda__s3__stepfunctions.s3-notification-triggers-lambda-positive lambda__s3__stepfunctions.s3-notification-payload-format-preserved lambda__s3__stepfunctions.s3-event-filter-prefix-negative lambda__s3__stepfunctions.s3-trigger-lambda-starts-sfn-pipeline lambda__s3__stepfunctions.sfn-pipeline-lambda-broken-s3-permission-negative lambda__s3__stepfunctions.s3-notification-missing-invoke-permission-negative lambda__s3__stepfunctions.cfn-deploy-s3-notification-to-lambda-positive
Issue-Key: lambda__s3__stepfunctions.sfn-invokes-lambda-via-task-positive|run#20|AssertionError|assert result["status"] == "SUCCEEDED", (
Issue-Key: lambda__s3__stepfunctions.sfn-task-lambda-passes-input-output|run#22|AssertionError|assert result["status"] == "SUCCEEDED", (
Issue-Key: lambda__s3__stepfunctions.sfn-pipeline-lambda-writes-s3|run#22|AssertionError|assert result["status"] == "SUCCEEDED", (
Issue-Key: lambda__s3__stepfunctions.sfn-map-state-parallel-lambda-invocations|run#20|AssertionError|assert result["status"] == "SUCCEEDED", (
Issue-Key: lambda__s3__stepfunctions.sfn-express-workflow-lambda-sync-positive|run#22|AssertionError|assert sync_resp["status"] == "SUCCEEDED", (
Issue-Key: lambda__s3__stepfunctions.sfn-catch-lambda-error-and-write-s3|run#25|AssertionError|assert result["status"] == "SUCCEEDED", (
Issue-Key: lambda__s3__stepfunctions.sfn-task-lambda-missing-invoke-permission-negative|run#24|AssertionError|assert positive["status"] == "SUCCEEDED", (
Issue-Key: lambda__s3__stepfunctions.sfn-task-nonexistent-lambda-arn-negative|run#13|AssertionError|assert positive_result["status"] == "SUCCEEDED", (
Issue-Key: lambda__s3__stepfunctions.s3-notification-triggers-lambda-positive|_poll_until#12|Timeout|time.sleep(interval_s)
Issue-Key: lambda__s3__stepfunctions.s3-notification-payload-format-preserved|_poll_until#13|Timeout|time.sleep(interval_s)
Issue-Key: lambda__s3__stepfunctions.s3-event-filter-prefix-negative|_poll_until#15|Timeout|time.sleep(interval_s)
Issue-Key: lambda__s3__stepfunctions.s3-trigger-lambda-starts-sfn-pipeline|_poll_until#23|Timeout|time.sleep(interval_s)
Issue-Key: lambda__s3__stepfunctions.sfn-pipeline-lambda-broken-s3-permission-negative|_run_until#26|Timeout|time.sleep(backoff_s)
Issue-Key: lambda__s3__stepfunctions.s3-notification-missing-invoke-permission-negative|_error_code#9|AssertionError|raise AssertionError(
Issue-Key: lambda__s3__stepfunctions.cfn-deploy-s3-notification-to-lambda-positive|_deploy_cfn#4|AssertionError|raise AssertionError(f"Stack {name} ended in {stack['StackStatus']}")
Issue-Commit: dfc2bec1feea4e1eda9981ef982ae4c91760421c

# lambda__s3__sfn: four net-new causes — raw-ARN Lambda Tasks are never invoked, S3 notifications never reach Lambda, PutBucketNotificationConfiguration skips the invoke-permission check, and the CFN notification stack rolls back (4 causes)

> _Note: this issue was identified against the code base at commit `dfc2bec1feea4e1eda9981ef982ae4c91760421c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExecution

## Description
Four net-new root causes in this service group.

Cause 1 of 4: a Task state whose `Resource` is a bare Lambda function ARN (the legacy non-integration form) is never invoked — the execution fails immediately with `States.Runtime` and an emulator-authored cause.
  AWS: recorded behavior only for the dispatch (ASL's Task Resource form is not in the Smithy model). Real Step Functions treats a bare `arn:aws:lambda:...:function:<name>` Resource as a direct function invocation whose Task result is the function's RAW payload (no `Payload`/`StatusCode` envelope), and a thrown handler error becomes a TaskFailed whose `error` is the exception type and whose `cause` is the function's JSON error document; a nonexistent function ARN becomes `Lambda.ResourceNotFoundException`.
  Emulator: `error='States.Runtime'`, `cause="state '<name>': '<function arn>' names a resource of 'lambda' rather than a service integration, which this emulator does not invoke directly, so nothing was run for it"`. Every positive control in this family fails, so the Catch path, the Express sync result, Map fan-out, ResultPath propagation and the nonexistent-ARN negative are all unreachable. Checks: .sfn-invokes-lambda-via-task-positive, .sfn-task-lambda-passes-input-output, .sfn-pipeline-lambda-writes-s3, .sfn-map-state-parallel-lambda-invocations, .sfn-express-workflow-lambda-sync-positive, .sfn-catch-lambda-error-and-write-s3, .sfn-task-lambda-missing-invoke-permission-negative, .sfn-task-nonexistent-lambda-arn-negative.
  Why: the most common ASL form for calling a function does not work at all, and because every outcome is the same States.Runtime, Retry/Catch keyed on Lambda error names never matches.
  Fix: the Task-state Resource resolver and dispatcher in the sfn service crate — a bare Lambda function ARN must be resolved and invoked (raw payload in, raw payload out) under the execution role, with Lambda-prefixed error names on failure.

Cause 2 of 4: an S3 bucket notification configured to a Lambda function never invokes it, so no object-created event is ever delivered.
  AWS: recorded behavior only for delivery; Smithy (s3) models PutBucketNotificationConfiguration and the event document separately. In the recording, PutObject invokes the function once per matching object with the S3 event JSON (Records[].s3.bucket.name / object.key, eventName `ObjectCreated:Put`), honouring prefix/suffix filter rules.
  Emulator: nothing is ever invoked; checks .s3-notification-triggers-lambda-positive, .s3-notification-payload-format-preserved, .s3-event-filter-prefix-negative, .s3-trigger-lambda-starts-sfn-pipeline, .sfn-pipeline-lambda-broken-s3-permission-negative all end `Check timed out after 60.0s`.
  Why: S3-triggered processing is inert, and the filter-rule negative is indistinguishable from the positive because both deliver nothing.
  Fix: the notification dispatch path in the s3 service crate's object-write handlers (evaluate the bucket's notification configuration, including filter rules, and invoke each matching Lambda target).

Cause 3 of 4: PutBucketNotificationConfiguration accepts a Lambda target that has no resource policy allowing s3.amazonaws.com to invoke it.
  AWS: Smithy (s3) models PutBucketNotificationConfiguration; AWS validates the destination at admission and refuses with HTTP 400 `InvalidArgument`. The fixed text is AWS's wording for an un-invocable notification destination ("Unable to validate the following destination configurations"); the destination ARN in the message is taken from the request.
  Emulator: HTTP 200 with an empty body and the configuration stored.
  Why: a mis-permissioned notification is accepted and then silently delivers nothing instead of being refused where the operator can see it.
  Fix: admission validation in the PutBucketNotificationConfiguration handler of the s3 service crate.

Cause 4 of 4: a CloudFormation stack declaring an S3 bucket with a Lambda notification ends ROLLBACK_COMPLETE instead of CREATE_COMPLETE.
  AWS: recorded behavior only. In the recording the same template reaches CREATE_COMPLETE with the bucket, function, role and notification configuration all present.
  Emulator: `AssertionError: Stack <name> ended in ROLLBACK_COMPLETE`.
  Why: the deployed form of this reference architecture cannot be created at all.
  Fix: the AWS::S3::Bucket NotificationConfiguration / AWS::Lambda::Permission provisioners invoked by the CloudFormation stack-create handler, and the error propagation that turns a provisioner failure into a whole-stack rollback.

## Evidence
Cause 1: `AssertionError: Execution not SUCCEEDED: FAILED cause=state 'InvokeLambda': 'arn:aws:lambda:us-west-2:000000000000:function:<fn>' names a resource of 'lambda' rather than a service integration, which this emulator does not invoke directly, so nothing was run for it` (the function ARN is a per-run value; the States.Runtime classification is the defect).
Cause 2: five checks end `Check timed out after 60.0s` with no invocation for a PutObject that matches the notification.
Cause 3: `AssertionError: put_bucket_notification_configuration should be rejected without invoke permission, but it SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}}` — expected HTTP 400 InvalidArgument.
Cause 4: `AssertionError: Stack sim2real-xsvc-stk-30752ed2b9ba ended in ROLLBACK_COMPLETE` — expected CREATE_COMPLETE.

## Steps to Reproduce
Cause 1: create a Lambda function; create a STANDARD state machine whose Task Resource is the bare function ARN; StartExecution and read DescribeExecution — FAILED with States.Runtime.
Cause 2: PutBucketNotificationConfiguration with a LambdaFunctionConfiguration (and a prefix filter), add the s3.amazonaws.com invoke permission, PutObject, poll for the invocation — none arrives.
Cause 3: PutBucketNotificationConfiguration naming a function with no s3.amazonaws.com invoke permission — the call returns 200.
Cause 4: CreateStack with a template declaring AWS::S3::Bucket + NotificationConfiguration + AWS::Lambda::Function + AWS::Lambda::Permission — the stack rolls back.
