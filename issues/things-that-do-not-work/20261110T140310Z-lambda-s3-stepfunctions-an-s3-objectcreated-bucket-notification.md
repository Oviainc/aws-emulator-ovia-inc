Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__s3__stepfunctions.s3-notification-triggers-lambda-positive
Issue-Key: lambda__s3__stepfunctions.s3-notification-triggers-lambda-positive|_poll_until#12|Timeout|time.sleep(interval_s)
Issue-Commit: 5598dd881b8e225c34bc5a9814ddc1ff981b448f

# lambda__s3__stepfunctions: an S3 ObjectCreated bucket notification with a Lambda destination never invokes the function, so the handler's processed/ marker object is never written

> _Note: this issue was identified against the code base at commit `5598dd881b8e225c34bc5a9814ddc1ff981b448f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutBucketNotificationConfiguration

## Description
One net-new root cause, grounded in one failing behavioral check the replay report left unmarked (no ALREADY-FILED marker and no OWNED marker).

Cause 1 of 1 — a bucket whose notification configuration names a Lambda function for s3:ObjectCreated:* never delivers: a PutObject into the bucket produces no invocation, so the function's side effect (an object under the processed/ prefix) never appears.
Failing check id: lambda__s3__stepfunctions.s3-notification-triggers-lambda-positive.
AWS: recorded behavior only for the delivery itself (neither service's Smithy model describes event delivery). In AWS, once PutBucketNotificationConfiguration has accepted a LambdaFunctionConfiguration for s3:ObjectCreated:* and the function's resource policy grants s3.amazonaws.com lambda:InvokeFunction for that bucket, every matching PutObject asynchronously invokes the function with an event whose Records[].s3.object.key is the percent-encoded object key; the handler's own AWS calls then take effect, so the marker object it writes is readable with GetObject/ListObjectsV2 within seconds.
Emulator: the check is killed by the harness with "Error: Check timed out after 60.0s" while polling for the handler's marker object. The upload is accepted, the notification configuration is accepted, and nothing is ever invoked — the processed/ object never appears and no error is reported on any API call.
Why: the entire S3-triggered pipeline pattern silently does nothing against this emulator. A client gets a 200 on every call it makes — PutBucketNotificationConfiguration, AddPermission, PutObject — and no event, no invocation and no side effect, so there is nothing to diagnose and a working configuration is indistinguishable from a broken one.
Fix: the S3 object-write path's notification dispatch — after a successful PutObject it must match the bucket's notification configuration (event-type and filter rules) and perform the asynchronous Lambda Invoke for each LambdaFunctionConfiguration, building the event record (including the percent-encoded key) that the function receives.
Related but NOT covering this check (the replay report marked it owned by nothing): open issues 20261104T232822Z-lambda-s3-stepfunctions-the-cloudformation-deployed-s3-notification.md (the CloudFormation-deployed variant) and 20261107T185637Z-lambda-s3-stepfunctions-2-net-new-causes-s3-objectcreated-notifications-never.md.
Traces: lambda__s3__stepfunctions.s3-notification-triggers-lambda-positive.

## Evidence
lambda__s3__stepfunctions.s3-notification-triggers-lambda-positive — verdict mismatch.
Expected: with a bucket, a Python function whose handler re-uploads each record's key under the processed/ prefix, lambda:AddPermission granting s3.amazonaws.com lambda:InvokeFunction for that bucket, and PutBucketNotificationConfiguration binding s3:ObjectCreated:* to the function, a PutObject of an input object results in a processed/<filename> object becoming readable within seconds.
Actual: "Error: Check timed out after 60.0s" — the function is never invoked and the processed/ marker never appears; every API call in the setup returned success, so the failure surfaces only as the missing effect.

## Steps to Reproduce
1. s3:CreateBucket -> <bucket>.
2. iam:CreateRole for Lambda with s3 read/write on <bucket>; lambda:CreateFunction with a handler that, for each event Records[] entry, unquotes record['s3']['object']['key'] and PutObject's a marker under "processed/<filename>" in BUCKET_NAME.
3. lambda:AddPermission(FunctionName=<fn>, Principal="s3.amazonaws.com", Action="lambda:InvokeFunction", SourceArn=arn:aws:s3:::<bucket>).
4. s3:PutBucketNotificationConfiguration with LambdaFunctionConfigurations=[{LambdaFunctionArn:<fn arn>, Events:["s3:ObjectCreated:*"]}].
5. s3:PutObject(Bucket=<bucket>, Key="incoming/sample.txt", Body=...).
6. s3:GetObject / ListObjectsV2 for "processed/sample.txt" in a loop — expected: present within seconds. Observed: never appears; the check is killed at the 60 s harness cap.
