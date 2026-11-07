Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__lambda__s3__secretsmanager.s3-notification-lambda-missing-secretsmanager-permission-negative
Issue-Key: dynamodb__lambda__s3__secretsmanager.s3-notification-lambda-missing-secretsmanager-permission-negative|_poll_until#20|AssertionError|raise AssertionError(f"condition not met within {timeout_s}s; last={last!r}")
Issue-Commit: 569322fd02c2429e9947d96bd3e38eb76e62453b

# dynamodb__lambda__s3__secretsmanager: an S3 ObjectCreated notification never invokes its Lambda destination, so the in-function secretsmanager:GetSecretValue denial is unobservable (1 cause)

> _Note: this issue was identified against the code base at commit `569322fd02c2429e9947d96bd3e38eb76e62453b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutBucketNotificationConfiguration

## Description
One net-new root cause in the S3 -> Lambda -> Secrets Manager -> DynamoDB pipeline.

Cause 1 of 1: a bucket whose NotificationConfiguration declares a LambdaFunctionConfiguration for s3:ObjectCreated:* does not invoke that function when an object is written, so nothing the handler would do — including writing its own error-marker object — ever happens.

AWS: S3 PutBucketNotificationConfiguration registers a LambdaFunctionConfiguration; a subsequent PutObject matching the configured event type causes S3 to invoke the named function asynchronously (lambda Invoke with InvocationType Event; Smithy: lambda Invoke, POST /2015-03-31/functions/{FunctionName}/invocations, InvocationType on header X-Amz-Invocation-Type). Inside that invocation, a secretsmanager GetSecretValue made by a role whose identity policy does not allow the action is refused by IAM, not by the service model: Smithy GetSecretValue models only DecryptionFailure / InternalServiceError / InvalidParameterException / InvalidRequestException / ResourceNotFoundException, so the refusal is the common IAM denial — HTTP 400, error code AccessDeniedException, message "User: <assumed-role session ARN> is not authorized to perform: secretsmanager:GetSecretValue on resource: <SecretId> because no identity-based policy allows the secretsmanager:GetSecretValue action". In that message the fixed text is "User: ", " is not authorized to perform: secretsmanager:GetSecretValue on resource: " and " because no identity-based policy allows the secretsmanager:GetSecretValue action"; the variable parts are the caller's assumed-role session ARN (arn:<partition>:sts::<account>:assumed-role/<execution-role-name>/<function-name>) and the SecretId exactly as the request named it. Recorded behavior additionally shows the function's own s3:PutObject of an errors/<key>.err marker succeeding under the same role.

Emulator: after PutObject of incoming/object-a.bin the marker object errors/incoming/object-a.bin.err never appears. The check polls GetObject for it every 2 s for 30 s and gets nothing: "AssertionError: condition not met within 30s; last=None". Because the handler is never reached, neither the AccessDeniedException code nor its message is produced at all — the emulator's divergence is the absent asynchronous invocation, which hides every downstream assertion (the denial code, the denial message, and the absence of the DynamoDB write).

Why: a client that wires an S3 event notification to a Lambda function gets a silently dead pipeline. Uploads are accepted, the notification configuration reads back, and nothing runs — no object transformation, no DynamoDB row, and no error surface either, so the failure is invisible to the caller. Negative permission tests built on this pattern cannot distinguish "denied" from "never invoked".

Why (second order): the same shape also makes an IAM-denial regression untestable, since the only side channel an async invoke has is an object the handler writes.

Fix: the S3 notification dispatcher — the component that, on a successful object write, matches the bucket's stored NotificationConfiguration and issues the asynchronous lambda Invoke for a LambdaFunctionConfiguration — plus, once invocations land, the identity-policy evaluation on the secretsmanager GetSecretValue handler so it refuses with AccessDeniedException and the message above rather than succeeding.

Note: open issues 20261104T044826Z-lambda-s3-sqs-s3-bucket-notifications-never-invoke-their-lambda-target.md and 20261105T081139Z-dynamodb-lambda-s3-secretsmanager-the-in-process-lambda.md describe neighbouring symptoms, but the replay does not mark this check as owned by either, so it is filed here.

Traces: dynamodb__lambda__s3__secretsmanager.s3-notification-lambda-missing-secretsmanager-permission-negative.

## Evidence
Check: dynamodb__lambda__s3__secretsmanager.s3-notification-lambda-missing-secretsmanager-permission-negative [mismatch]

Expected (AWS): PutObject(Bucket=<bucket>, Key=incoming/object-a.bin) triggers an asynchronous Invoke of the notification's Lambda target. The handler's secretsmanager GetSecretValue is refused with HTTP 400 / AccessDeniedException and message "User: arn:<partition>:sts::<account>:assumed-role/<role-name>/<function-name> is not authorized to perform: secretsmanager:GetSecretValue on resource: <SecretId> because no identity-based policy allows the secretsmanager:GetSecretValue action"; the handler writes that {code, message} pair to errors/incoming/object-a.bin.err, and no DynamoDB item exists for the key.

Actual (emulator): errors/incoming/object-a.bin.err never exists. GetObject for it is polled every 2 s for 30 s and returns nothing:
  File "check_dynamodb__lambda__s3__secretsmanager_s3_notification_lambda_missing_secretsmanager_permission_negative.py", line 107, in run
    marker_body = _poll_until(...)
  File "...", line 203, in _poll_until
    raise AssertionError(f"condition not met within {timeout_s}s; last={last!r}")
  AssertionError: condition not met within 30s; last=None

So no invocation occurred: neither the AccessDeniedException code nor its message was ever emitted.

## Steps to Reproduce
1. Create a bucket, a DynamoDB table and a secret.
2. Create an IAM role for lambda.amazonaws.com granting s3:GetObject + s3:PutObject on arn:aws:s3:::<bucket>/* and dynamodb:PutItem on the table, and deliberately NOT secretsmanager:GetSecretValue.
3. Create a python3.12 function with that role whose handler reads the S3 object, calls secretsmanager GetSecretValue, and on ClientError writes {code, message} to errors/<key>.err in the same bucket.
4. lambda PutFunctionEventInvokeConfig(FunctionName=<fn>, MaximumRetryAttempts=0).
5. lambda AddPermission for principal s3.amazonaws.com, then s3 PutBucketNotificationConfiguration with a LambdaFunctionConfiguration for s3:ObjectCreated:* pointing at the function ARN.
6. PutObject(Bucket=<bucket>, Key=incoming/object-a.bin, Body=<bytes>).
7. Poll GetObject(Bucket=<bucket>, Key=errors/incoming/object-a.bin.err) for 30 s.
   Expected: the marker object exists and holds code AccessDeniedException with the IAM denial message above.
   Actual: the key never exists — the function was never invoked.
