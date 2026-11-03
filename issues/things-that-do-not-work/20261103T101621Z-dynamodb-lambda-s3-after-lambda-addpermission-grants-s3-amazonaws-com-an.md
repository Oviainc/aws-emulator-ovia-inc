Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__lambda__s3.s3-notification-without-invoke-permission-blocks-delivery
Issue-Key: dynamodb__lambda__s3.s3-notification-without-invoke-permission-blocks-delivery|_poll_until#18|Timeout|time.sleep(interval_s)
Issue-Commit: 5d4a88d9a344cd4338be4fa17dd8ed09c8bb6d4a

# After lambda:AddPermission grants s3.amazonaws.com, an S3 ObjectCreated notification still never invokes the Lambda destination, so the granted end-to-end path produces no effect

> _Note: this issue was identified against the code base at commit `5d4a88d9a344cd4338be4fa17dd8ed09c8bb6d4a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutBucketNotificationConfiguration

## Description
One net-new root cause (Cause 1 of 1).

Cause 1 of 1: once the Lambda destination's resource-based policy DOES grant s3.amazonaws.com `lambda:InvokeFunction`, S3 accepts the notification configuration but a subsequent PutObject still produces no invocation, so the function's side effect never lands.

AWS: Smithy — `PutBucketNotificationConfiguration` (PUT /{Bucket}?notification, 200, no modeled errors) carries `SkipDestinationValidation` on header `x-amz-skip-destination-validation`, documented as "Skips validation of Amazon SQS, Amazon SNS, and Lambda destinations", and the operation's own documentation states "in the case of Lambda destinations, Amazon S3 verifies that the Lambda function permissions grant Amazon S3 permission to invoke the function from the Amazon S3 bucket". So with no grant the PUT is refused whole with HTTP 400, error code `InvalidArgument` and the message `Unable to validate the following destination configurations` — fixed text; the destination ARN that failed travels in the error document's ArgumentValue, taken from the request — and nothing is persisted (GetBucketNotificationConfiguration still reports no LambdaFunctionConfigurations). After `lambda:AddPermission` with Action=lambda:InvokeFunction, Principal=s3.amazonaws.com and SourceArn=<bucket ARN>, the IDENTICAL PUT returns 200, GetBucketNotificationConfiguration lists the LambdaFunctionConfigurations entry, and a following PutObject causes S3 to invoke the function asynchronously with an `s3:ObjectCreated:*` Records envelope, so the handler's write (here a DynamoDB PutItem keyed by the object key) becomes readable within seconds.

Emulator: the check never completes. The replay reports `Error: Check timed out after 60.0s`. The terminal leg — polling DynamoDB for the item the invoked handler must write after the permission is granted and the object is put — never satisfies its predicate, so the check exhausts the harness window instead of reaching an assertion. No invocation is observable and no item appears.

Why: a client that wires a bucket to a function and correctly grants s3.amazonaws.com invoke rights gets a 200 from S3 and a notification configuration that reads back correctly, but no event is ever delivered. Nothing reports an error, so every integration built on S3→Lambda fan-out silently does nothing; worse, the configuration read-back makes the wiring look healthy, so the failure is undiagnosable from the API surface.

Fix: S3's notification delivery path for Lambda destinations (the dispatcher that must invoke the function after a recorded object change) and Lambda's invoke path it calls into. Related but NOT covering this check: open issues `20261029T180026Z-dynamodb-lambda-s3-s3-notifications-never-invoke-lambda-are-accepted-without.md` and `20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md` describe neighbouring symptoms; the replay did not attribute this check to either, and this cause is specifically the post-grant, validated-destination delivery leg.

Traces: dynamodb__lambda__s3.s3-notification-without-invoke-permission-blocks-delivery. Components to inspect: services/service-com-amazonaws-s3-amazons3/src/state/notifications.rs (deliver/pending) and services/service-com-amazonaws-s3-amazons3/src/ops/put_bucket_notification_configuration.rs.

## Evidence
Expected (AWS):
- Before the grant: put_bucket_notification_configuration → HTTP 400, Error.Code == "InvalidArgument", Error.Message == "Unable to validate the following destination configurations"; get_bucket_notification_configuration reports no LambdaFunctionConfigurations.
- After lambda.add_permission(Action="lambda:InvokeFunction", Principal="s3.amazonaws.com", SourceArn=<bucket arn>): the identical PUT returns 200, and after s3.put_object(Key="authorized.txt") the handler's DynamoDB item with pk={"S":"authorized.txt"} is readable within seconds.

Actual (emulator): the granted end-to-end leg never happens — the DynamoDB item never appears and the check is cut off by the harness:

  [mismatch] dynamodb__lambda__s3.s3-notification-without-invoke-permission-blocks-delivery
  Error: Check timed out after 60.0s

## Steps to Reproduce
1. Create a bucket and a DynamoDB table.
2. Create a Lambda function whose handler writes one DynamoDB item per S3 record (pk = the object key), with an execution role allowing dynamodb:PutItem on that table. Do NOT call lambda.add_permission yet.
3. s3.put_bucket_notification_configuration with LambdaFunctionConfigurations=[{LambdaFunctionArn: <fn arn>, Events: ["s3:ObjectCreated:*"]}] — expect 400 InvalidArgument "Unable to validate the following destination configurations" and no persisted config.
4. lambda.add_permission(FunctionName=<fn>, StatementId="allow-s3-invoke", Action="lambda:InvokeFunction", Principal="s3.amazonaws.com", SourceArn=<bucket arn>).
5. Re-issue the identical put_bucket_notification_configuration (expect 200), then s3.put_object(Bucket=<bucket>, Key="authorized.txt", Body=b"x").
6. Poll dynamodb.get_item(Key={"pk":{"S":"authorized.txt"}}): observe the item never appears.
