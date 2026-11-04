Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__s3__sqs.s3-notification-multipart-upload-complete-triggers-positive lambda__s3__sqs.s3-notification-multiple-destinations-fanout-positive lambda__s3__sqs.s3-notification-object-removed-to-lambda-positive lambda__s3__sqs.s3-notification-event-name-filter-mismatch-negative lambda__s3__sqs.s3-notification-wrong-lambda-arn-in-config-negative
Issue-Key: lambda__s3__sqs.s3-notification-multipart-upload-complete-triggers-positive|_poll_until#12|TimeoutError|raise TimeoutError(f"{description}: the run budget ended first; not graded")
Issue-Key: lambda__s3__sqs.s3-notification-multiple-destinations-fanout-positive|_poll_until#18|Timeout|time.sleep(interval_s)
Issue-Key: lambda__s3__sqs.s3-notification-object-removed-to-lambda-positive|_poll_until#12|Timeout|time.sleep(interval_s)
Issue-Key: lambda__s3__sqs.s3-notification-event-name-filter-mismatch-negative|_poll_until#11|Timeout|time.sleep(interval_s)
Issue-Key: lambda__s3__sqs.s3-notification-wrong-lambda-arn-in-config-negative|_poll_until#23|Timeout|time.sleep(interval_s)
Issue-Commit: b4ead8169e48e6047949699a772a41fd75a6bc4d

# S3 bucket notifications never invoke their Lambda target for any event type

> _Note: this issue was identified against the code base at commit `b4ead8169e48e6047949699a772a41fd75a6bc4d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutBucketNotificationConfiguration

## Description
Cause 1 of 1: a bucket notification configuration with a LambdaFunctionConfiguration must invoke the named function for the configured events — s3:ObjectCreated:Put, s3:ObjectCreated:CompleteMultipartUpload, s3:ObjectRemoved:* — once per matching event, to each configured destination, and must not invoke it when the event name does not match or when the configured function ARN does not exist.
AWS: recorded behavior only — each of these five recordings puts the notification configuration, grants lambda:InvokeFunction to s3.amazonaws.com with the bucket as SourceArn, performs the object operation, and then reads the handler's own 'S3_EVT::' marker line out of /aws/lambda/<function-name>; on AWS each lands inside the suite's 60s budget, and the negative twins observe the absence only after their positive control has fired.
Emulator: four of the checks are cut off with "Error: Check timed out after 60.0s" and the multipart one aborts with "TimeoutError: log message containing 'S3_EVT::' for <function-name>: the run budget ended first; not graded". PutBucketNotificationConfiguration is accepted, but the function is never invoked for any event, so no marker ever appears.
Why: every S3-triggered design is inert while the bucket reports the notification installed. Because the negative legs also only time out, a client cannot distinguish a wrong event-name filter, a wrong function ARN or a missing resource policy from a delivery path that never runs — and multi-destination fan-out and ObjectRemoved handling are not observable at all.
Fix: the S3 notification dispatcher that evaluates the configured event names/filters against an object operation and invokes each Lambda destination (including the CompleteMultipartUpload and ObjectRemoved event sources), together with the resource-policy evaluation on that invoke.
Traces: lambda__s3__sqs.s3-notification-multipart-upload-complete-triggers-positive, .s3-notification-multiple-destinations-fanout-positive, .s3-notification-object-removed-to-lambda-positive, .s3-notification-event-name-filter-mismatch-negative, .s3-notification-wrong-lambda-arn-in-config-negative.

## Evidence
Expected (AWS): the configured function is invoked once per matching event (Put, CompleteMultipartUpload, ObjectRemoved), to every configured destination, and not invoked for a non-matching event name or a non-existent function ARN.
Actual (emulator): four checks end with 'Error: Check timed out after 60.0s'; the multipart check ends with "TimeoutError: log message containing 'S3_EVT::' for sim2real-s3lam-fn-...: the run budget ended first; not graded". No invocation is ever observed.

## Steps to Reproduce
1. CreateBucket and a Lambda; AddPermission for s3.amazonaws.com with the bucket ARN as SourceArn.
2. PutBucketNotificationConfiguration with LambdaFunctionConfigurations for s3:ObjectCreated:* and s3:ObjectRemoved:* (and a second destination).
3. PutObject, complete a multipart upload, and DeleteObject; poll each function's log group for its 'S3_EVT::' marker. Observe no invocation within 60s.
