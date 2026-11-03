Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__opensearch__s3.s3-notification-missing-lambda-permission-negative
Issue-Key: lambda__opensearch__s3.s3-notification-missing-lambda-permission-negative|_poll_until#18|Timeout|time.sleep(interval_s)
Issue-Commit: 5d4a88d9a344cd4338be4fa17dd8ed09c8bb6d4a

# S3 ObjectCreated notification to a granted Lambda destination never invokes the function, so the post-grant marker object is never written

> _Note: this issue was identified against the code base at commit `5d4a88d9a344cd4338be4fa17dd8ed09c8bb6d4a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutBucketNotificationConfiguration

## Description
One net-new root cause (Cause 1 of 1).

Cause 1 of 1: with the Lambda destination's resource policy granting s3.amazonaws.com `lambda:InvokeFunction`, the notification configuration is accepted but a subsequent PutObject produces no invocation, so the handler's marker object is never written.

AWS: Smithy — `PutBucketNotificationConfiguration` (PUT /{Bucket}?notification, 200, no modeled errors) exposes `SkipDestinationValidation` on header `x-amz-skip-destination-validation`, documented "Skips validation of Amazon SQS, Amazon SNS, and Lambda destinations"; the operation documentation states "in the case of Lambda destinations, Amazon S3 verifies that the Lambda function permissions grant Amazon S3 permission to invoke the function from the Amazon S3 bucket". Without that grant the PUT is refused whole: HTTP 400, code `InvalidArgument`, message `Unable to validate the following destination configurations` (fixed text; the unvalidated destination ARN rides in the error document, taken from the request), and nothing is persisted. After `lambda:AddPermission` (Action=lambda:InvokeFunction, Principal=s3.amazonaws.com, SourceArn=<bucket ARN>) the identical PUT returns 200, GetBucketNotificationConfiguration lists the function ARN under LambdaFunctionConfigurations, and a following PutObject invokes the function asynchronously with the `s3:ObjectCreated:*` Records envelope, so the handler's own S3 write (here `processed/<key>.marker`) becomes head-able within seconds.

Emulator: the check never completes. The replay reports `Error: Check timed out after 60.0s`. The terminal leg — waiting for the marker object the invoked handler must write after the grant — never satisfies its predicate, so the check is cut off before it can assert. No invocation is observable and `processed/trigger.txt.marker` never exists.

Why: a client that grants s3.amazonaws.com invoke rights correctly gets a 200 and a configuration that reads back intact, yet no event is ever delivered. The indexing/processing pipeline the function implements silently never runs, and because the control-plane read-back looks healthy there is nothing in the API surface to diagnose.

Fix: S3's notification delivery path for Lambda destinations (the dispatcher that must invoke the function after a recorded object change) and the Lambda invoke path it calls into. Related but NOT covering this check: open issues `20261103T020000Z-lambda-opensearch-s3-invoke-execution-and-cloudformation-stack-causes.md` and `20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md`; the replay attributed this check to neither.

Traces: lambda__opensearch__s3.s3-notification-missing-lambda-permission-negative. Components to inspect: services/service-com-amazonaws-s3-amazons3/src/state/notifications.rs and services/service-com-amazonaws-s3-amazons3/src/ops/put_bucket_notification_configuration.rs.

## Evidence
Expected (AWS):
- No grant yet: put_bucket_notification_configuration → HTTP 400, Error.Code == "InvalidArgument", Error.Message == "Unable to validate the following destination configurations"; get_bucket_notification_configuration shows no LambdaFunctionConfigurations.
- After add_permission for s3.amazonaws.com: the identical PUT succeeds, LambdaFunctionConfigurations contains the function ARN, and after put_object(Key="trigger.txt") head_object on "processed/trigger.txt.marker" succeeds.

Actual (emulator): the marker object is never written and the check is cut off by the harness:

  [mismatch] lambda__opensearch__s3.s3-notification-missing-lambda-permission-negative
  Error: Check timed out after 60.0s

## Steps to Reproduce
1. Create a bucket; create an IAM role for Lambda allowing s3:PutObject on arn:aws:s3:::<bucket>/*.
2. Create a Lambda function whose handler writes "processed/<key>.marker" into MARKER_BUCKET for each S3 record. Do NOT call add_permission.
3. s3.put_bucket_notification_configuration with LambdaFunctionConfigurations=[{LambdaFunctionArn: <fn arn>, Events: ["s3:ObjectCreated:*"]}] — expect 400 InvalidArgument "Unable to validate the following destination configurations" and no persisted config.
4. lambda.add_permission(Action="lambda:InvokeFunction", Principal="s3.amazonaws.com", SourceArn=<bucket arn>).
5. Re-issue the identical PUT (expect 200) and confirm LambdaFunctionConfigurations lists the ARN.
6. s3.put_object(Bucket=<bucket>, Key="trigger.txt", Body=b"invoke-me"), then poll head_object(Bucket=<bucket>, Key="processed/trigger.txt.marker"): observe it never exists.
