Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__lambda__s3.s3-put-triggers-lambda-via-notification dynamodb__lambda__s3.saga-s3-upload-processing-with-compensation
Issue-Key: dynamodb__lambda__s3.s3-put-triggers-lambda-via-notification|_poll_until#13|Timeout|time.sleep(interval_s)
Issue-Key: dynamodb__lambda__s3.saga-s3-upload-processing-with-compensation|_poll_until#15|Timeout|time.sleep(interval_s)
Issue-Commit: 77172d4b913d3cd289b60abc3e2fa4659d3ee8a2

# dynamodb__lambda__s3: an S3 ObjectCreated notification configured to a Lambda function never invokes it, so S3-triggered pipelines never start and the checks exhaust their budget

> _Note: this issue was identified against the code base at commit `77172d4b913d3cd289b60abc3e2fa4659d3ee8a2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutBucketNotificationConfiguration

## Description
ONE net-new root cause. Neither failing check is marked ALREADY FILED or OWNED in the replay report.

Nearest open neighbour, named so it is not re-fixed and is not mistaken for coverage of these two replay keys: 20261029T180026Z-dynamodb-lambda-s3-s3-notifications-never-invoke-lambda-are-accepted-without.md (the same undelivered-notification family, reached through other checks in this heading; it carries neither of these check ids, so these two supply the additional replay keys and the issues should be closed together). Also nearby: 20261103T101621Z-dynamodb-lambda-s3-after-lambda-addpermission-grants-s3-amazonaws-com-an.md.

Cause 1 of 1: a bucket notification with LambdaFunctionConfigurations for s3:ObjectCreated:* produces no invocation when an object is written.
AWS: recorded behavior only (S3's notification delivery is not expressed in a Smithy member). PutBucketNotificationConfiguration with a LambdaFunctionConfigurations entry (LambdaFunctionArn + Events ['s3:ObjectCreated:*'], optionally Filter prefix/suffix) is accepted only once lambda:AddPermission has granted s3.amazonaws.com InvokeFunction on that function; thereafter every matching PutObject asynchronously invokes the function with an event whose Records[0].s3.bucket.name is the bucket and Records[0].s3.object.key is the URL-encoded key, so the handler's side effects (here, a PutItem into a DynamoDB table keyed by that bucket/key) become observable within seconds.
Emulator: the configuration is accepted, the object is written, and no invocation ever happens — the handler's DynamoDB row never appears, so each check polls until the harness kills it: "Check timed out after 60.0s". No function error, no log group, nothing to distinguish "not delivered" from "handler failed".
Why: S3-event-driven processing is one of the most common serverless shapes; against the emulator every such pipeline silently does nothing, and compensating/saga logic that is supposed to run on the trigger (reserve → fail → roll back) can never be exercised, so a client's end-to-end test passes vacuously or hangs.
Fix: the S3 notification dispatch path — PutObject (and the other ObjectCreated producers) must match the bucket's LambdaFunctionConfigurations (event-name match plus prefix/suffix Filter) and asynchronously Invoke the configured function with the S3 event envelope, subject to the function's resource policy for s3.amazonaws.com.
Traces: dynamodb__lambda__s3.s3-put-triggers-lambda-via-notification, dynamodb__lambda__s3.saga-s3-upload-processing-with-compensation.

## Evidence
Suite simparity-behavioral-aws-v1 replayed against this checkout: 630/1315 passed. Service heading `dynamodb__lambda__s3` (2/19 passed, 17 failed); dynamodb, lambda and s3 are all stood up (only iot is pending bootstrap). Both checks below are marked neither ALREADY FILED nor OWNED.

[mismatch] dynamodb__lambda__s3.s3-put-triggers-lambda-via-notification
  Error: Check timed out after 60.0s
  Shape: create a bucket and a DynamoDB table; create a python handler that, for each event['Records'] entry, reads record['s3']['bucket']['name'] and record['s3']['object']['key'] and PutItems them into the table; wire the bucket's notification to the function for s3:ObjectCreated:*; PutObject; poll the table.
  Expected: the function is invoked and the row for that bucket/key appears. Actual: nothing is ever written, so the poll runs to the 60 s budget.

[mismatch] dynamodb__lambda__s3.saga-s3-upload-processing-with-compensation
  Error: Check timed out after 60.0s
  Shape: same notification wiring; the handler PutItems status='reserved', then status='completed', and on a simulated RuntimeError (key containing 'fail') writes the compensating rollback.
  Expected: the reserved → completed transition for a normal key and the compensation for the failing key are both observable in the table. Actual: no invocation at all, so no status row ever appears and the check exhausts its budget.

The 60.0 s figure is the harness budget, not a requirement; generated bucket/table/function names are this run's values.

## Steps to Reproduce
1. Boot the emulator from this checkout; point s3, lambda and dynamodb clients at it.
2. CreateBucket B; CreateTable T with pk(HASH,S).
3. CreateFunction F (python runtime) whose handler iterates event['Records'] and calls dynamodb PutItem(TableName=T, Item={'pk':{'S': record['s3']['object']['key']}, ...}) using the bucket/key from the record; give its role dynamodb:PutItem on T.
4. lambda AddPermission on F: Principal 's3.amazonaws.com', Action 'lambda:InvokeFunction', SourceArn arn:aws:s3:::B.
5. PutBucketNotificationConfiguration on B with LambdaFunctionConfigurations=[{LambdaFunctionArn: <F arn>, Events: ['s3:ObjectCreated:*']}].
6. PutObject(Bucket=B, Key='upload-1.txt', Body=...), then poll GetItem/Scan on T for a few seconds. No row ever appears and no invocation is recorded; the row must appear with the bucket name and the URL-decoded key the handler read.
7. Repeat with a key containing 'fail' and a handler that raises after writing status='reserved': the compensating write must also be observable.
   Replay: dynamodb__lambda__s3.s3-put-triggers-lambda-via-notification, dynamodb__lambda__s3.saga-s3-upload-processing-with-compensation
