Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__s3__stepfunctions.s3-notification-missing-invoke-permission-negative
Issue-Key: lambda__s3__stepfunctions.s3-notification-missing-invoke-permission-negative|_poll_until#22|Timeout|raise AssertionError(
Issue-Commit: 7650240ce53678dc538ca74d4cdf45ca34efd73a

# lambda__s3__sfn: a bucket notification whose Lambda destination is now correctly validated still never delivers — the granted s3.amazonaws.com invoke never produces an invocation (1 cause)

> _Note: this issue was identified against the code base at commit `7650240ce53678dc538ca74d4cdf45ca34efd73a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutBucketNotificationConfiguration

## Description
Cause 1 of 1: once PutBucketNotificationConfiguration's Lambda destination validation passes (the admission check added for the earlier invoke-permission divergence now accepts the function after lambda:AddPermission grants s3.amazonaws.com), the configured wiring still delivers nothing: a PutObject that matches the bucket's LambdaFunctionConfiguration never invokes the function, so the check that proves "correctly blocked" is not the same as "no delivery path at all" can never reach its positive control and hangs to the 60 s harness cap.

  AWS: Smithy (s3) models PutBucketNotificationConfiguration, whose own documentation states that "in the case of Lambda destinations, Amazon S3 verifies that the Lambda function permissions grant Amazon S3 permission to invoke the function from the Amazon S3 bucket" — a destination that fails that check is refused whole with HTTP 400 `InvalidArgument` and the fixed sentence "Unable to validate the following destination configurations" (the ARNs named in the refusal come from the request), and "Amazon S3 will not add the configuration to your bucket". Once the grant exists, the IDENTICAL call succeeds, and thereafter every matching object write (recorded behavior only for the delivery; the event document is modelled separately from the operation) invokes the target function once per matching object with the S3 `Records` envelope (`Records[].s3.bucket.name` / `Records[].s3.object.key`, `eventName` `ObjectCreated:Put`), honouring the configuration's key-name filter rules.

  Emulator: the refusal half now behaves (this is the part the resolved issue 20261029T182036Z-lambda-opensearch-s3-lambda-driven-s3-opensearch-pipelines.md fixed, and it is NOT being reported as broken here — the previously recorded `_error_code` AssertionError is gone). What remains is that the post-grant delivery never happens: the check's `_poll_until` for the handler's marker object (`triggered/marker.txt`, written by the function via s3:PutObject) never observes it and the check ends `Check timed out after 60.0s` with no error document to quote. In the checkout, s3 DOES build the delivery (`services/service-com-amazonaws-s3-amazons3/src/state/notifications.rs` constructs `Delivery::Function { region, arn, payload }` for a `NotificationDestination::LambdaFunction`), but the Lambda `Invoke` operation it must land on is an unimplemented scaffold stub in this emulator — other checks in this suite surface that directly as `FAILED cause="the 'Invoke' operation of Lambda is not implemented by this emulator, so nothing was run for this task"` — so the delivery is dispatched into a 501 and nothing runs.

  Why: a client that configures an S3 -> Lambda notification gets a 200 and a stored configuration that is inert. The negative class this check grades becomes ungradeable: a mis-permissioned configuration and a correctly permissioned one are indistinguishable downstream, because neither delivers, so an operator cannot tell a blocked wiring from a working one.

  Fix: the notification delivery path in the s3 service crate (`src/state/notifications.rs` delivery of `Delivery::Function`) together with the Lambda `Invoke` handler it depends on — the invocation has to actually execute the function under the S3 service principal for the delivery to have any effect. Related OPEN issues that describe adjacent halves of this but do not carry this check's replay key: 20261029T183117Z-lambda-s3-sfn-four-net-new-causes-raw.md (its Cause 2, S3 notifications never reach Lambda, and its Cause 3, now resolved) and 20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md.

  Traces: lambda__s3__stepfunctions.s3-notification-missing-invoke-permission-negative.

## Evidence
Cause 1 — expected: (a) PutBucketNotificationConfiguration naming a Lambda function with no s3.amazonaws.com invoke permission is refused with HTTP 400 `InvalidArgument` / "Unable to validate the following destination configurations" and persists nothing (GetBucketNotificationConfiguration reports no LambdaFunctionConfigurations); (b) after lambda:AddPermission(StatementId=..., Action=lambda:InvokeFunction, Principal=s3.amazonaws.com, SourceArn=<bucket arn>) the IDENTICAL call succeeds and the configuration is readable back; (c) a subsequent PutObject on a matching key invokes the function, which writes `triggered/marker.txt` into the bucket, observable through ListObjectsV2(Prefix='triggered/').
Actual: (a) and (b) now behave; (c) never happens — the poll for the invocation marker never succeeds and the check ends with `Check timed out after 60.0s`. No HTTP status or error code is produced for the missing delivery: the divergence is the ABSENCE of the invocation. The emulator's own words for the underlying stub, seen on checks that call it directly in this same suite run: `cause="the 'Invoke' operation of Lambda is not implemented by this emulator, so nothing was run for this task"`. The bucket name, function name and statement id are per-run values; the marker key and the fact that an invocation must occur are the requirement.

## Steps to Reproduce
1. CreateBucket; create an IAM role trusting lambda.amazonaws.com with s3:PutObject on the bucket; CreateFunction (python3.12) whose handler calls s3.put_object(Bucket=$BUCKET_NAME, Key='triggered/marker.txt').
2. PutBucketNotificationConfiguration with LambdaFunctionConfigurations=[{LambdaFunctionArn: <fn arn>, Events: ['s3:ObjectCreated:*']}] BEFORE granting any permission — observe the 400 InvalidArgument refusal and that GetBucketNotificationConfiguration reports no LambdaFunctionConfigurations (this half now matches AWS).
3. lambda AddPermission(FunctionName=<fn>, StatementId='s3-invoke', Action='lambda:InvokeFunction', Principal='s3.amazonaws.com', SourceArn=<bucket arn>), then repeat the identical PutBucketNotificationConfiguration — it succeeds and the configuration reads back.
4. PutObject(Bucket=<bucket>, Key='trigger.txt', Body='test'), then poll ListObjectsV2(Bucket=<bucket>, Prefix='triggered/') — on AWS the marker appears; on the emulator it never does and the check hangs to the harness cap.
