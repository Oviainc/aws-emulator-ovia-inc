Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__s3__sqs.cli-put-bucket-notification-missing-permission-negative
Issue-Key: lambda__s3__sqs.cli-put-bucket-notification-missing-permission-negative|run#22|Timeout|time.sleep(3)
Issue-Commit: 5d4a88d9a344cd4338be4fa17dd8ed09c8bb6d4a

# CLI-driven S3 → Lambda notification wiring: after the s3.amazonaws.com invoke grant, an upload still triggers no invocation and no processed object

> _Note: this issue was identified against the code base at commit `5d4a88d9a344cd4338be4fa17dd8ed09c8bb6d4a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutBucketNotificationConfiguration

## Description
One net-new root cause (Cause 1 of 1).

Cause 1 of 1: with the Lambda destination's resource policy granting s3.amazonaws.com `lambda:InvokeFunction`, `put-bucket-notification-configuration` is accepted but an upload still produces no invocation, so the handler writes no processed object.

AWS: Smithy — `PutBucketNotificationConfiguration` (PUT /{Bucket}?notification, 200, no modeled errors) exposes `SkipDestinationValidation` on header `x-amz-skip-destination-validation`, documented "Skips validation of Amazon SQS, Amazon SNS, and Lambda destinations"; the operation documentation states "in the case of Lambda destinations, Amazon S3 verifies that the Lambda function permissions grant Amazon S3 permission to invoke the function from the Amazon S3 bucket". With no grant the call is refused whole: HTTP 400, code `InvalidArgument`, message `Unable to validate the following destination configurations` — fixed text; the destination ARN that failed validation travels in the error document and comes from the request — and nothing is persisted. After `aws lambda add-permission --action lambda:InvokeFunction --principal s3.amazonaws.com --source-arn <bucket ARN>`, the identical `aws s3api put-bucket-notification-configuration` exits 0, the configuration reads back with the function ARN, and a following `aws s3 cp`/`put-object` invokes the function asynchronously with the `s3:ObjectCreated:*` Records envelope, so the handler's write (here `processed/<key>`) becomes listable within seconds.

Emulator: the check never completes. The replay reports `Error: Check timed out after 60.0s`. The terminal leg — the upload-triggered invocation after the grant — never produces its effect, so the check is cut off before asserting. No `processed/<key>` object appears.

Why: a user following the documented CLI recipe (create function, add-permission for s3.amazonaws.com, put-bucket-notification-configuration, upload) gets success exit codes at every step and a configuration that reads back correctly, but the pipeline never fires. The emulator reports no error anywhere, so the break cannot be found from the CLI.

Fix: S3's notification delivery path for Lambda destinations (the dispatcher that must invoke the function after a recorded object change) and the Lambda invoke path it calls into. Related but NOT covering this check: open issues `20261029T193114Z-lambda-s3-sqs-5-net-new-causes-sqs.md` and `20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md`; the replay attributed this check to neither.

Traces: lambda__s3__sqs.cli-put-bucket-notification-missing-permission-negative. Components to inspect: services/service-com-amazonaws-s3-amazons3/src/state/notifications.rs and services/service-com-amazonaws-s3-amazons3/src/ops/put_bucket_notification_configuration.rs. (The check shells out to the `aws` CLI; the CLI itself was present — the failure is the absent delivery, not a missing tool.)

## Evidence
Expected (AWS):
- No grant yet: `aws s3api put-bucket-notification-configuration` fails with HTTP 400, Error.Code == "InvalidArgument", Error.Message == "Unable to validate the following destination configurations", and the bucket keeps no Lambda notification config.
- After `aws lambda add-permission --principal s3.amazonaws.com --action lambda:InvokeFunction --source-arn arn:aws:s3:::<bucket>`: the identical put-bucket-notification-configuration succeeds, and after uploading an object the handler's `processed/<key>` object exists.

Actual (emulator): no processed object is ever written and the check is cut off by the harness:

  [mismatch] lambda__s3__sqs.cli-put-bucket-notification-missing-permission-negative
  Error: Check timed out after 60.0s

## Steps to Reproduce
1. `aws s3api create-bucket` a bucket; create an IAM role for Lambda allowing s3:PutObject on arn:aws:s3:::<bucket>/*.
2. `aws lambda create-function` with a handler that writes "processed/<key>" into OUT_BUCKET for each S3 record. Do NOT add-permission yet.
3. `aws s3api put-bucket-notification-configuration --bucket <bucket> --notification-configuration '{"LambdaFunctionConfigurations":[{"LambdaFunctionArn":"<fn arn>","Events":["s3:ObjectCreated:*"]}]}'` — expect it to fail with InvalidArgument "Unable to validate the following destination configurations".
4. `aws lambda add-permission --function-name <fn> --statement-id allow-s3 --action lambda:InvokeFunction --principal s3.amazonaws.com --source-arn arn:aws:s3:::<bucket>`.
5. Re-run the same put-bucket-notification-configuration (expect success), then upload an object into the bucket.
6. `aws s3api list-objects-v2 --bucket <bucket> --prefix processed/`: observe it stays empty.
