Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__s3__sns__sqs.refarch-s3ev-e2e-diagnose-s3-event-notification-break-lifecycle
Issue-Key: lambda__s3__sns__sqs.refarch-s3ev-e2e-diagnose-s3-event-notification-break-lifecycle|_handler_evidence#9|Timeout|time.sleep(5.0)
Issue-Commit: 5b7d66e77a060af6cd000fd4d7e09192c88c4502

# S3 event-notification break/diagnose lifecycle never completes: the check hangs past 60s

> _Note: this issue was identified against the code base at commit `5b7d66e77a060af6cd000fd4d7e09192c88c4502`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutBucketNotificationConfiguration

## Description
Cause 1 of 1: the diagnose-and-repair lifecycle over an S3 -> SNS/SQS -> Lambda notification chain never reaches an observable state, so the check times out rather than passing or being refused.
AWS: each step of the lifecycle answers promptly - PutBucketNotificationConfiguration either succeeds or is refused with InvalidArgument (400) when the destination cannot be invoked, PutObject returns, and the delivery to the Lambda/SQS/SNS destination either arrives within the polling window or is provably absent.
Emulator: "Error: Check timed out after 60.0s" - one of the steps (notification wiring or the delivery it should produce) neither completes nor fails inside the deadline. Recorded behavior only: no response is recorded for the hanging step.
Why: a client cannot diagnose a broken notification chain - the scenario this check exists for - because the emulator neither delivers nor refuses.
Fix: the S3 bucket-notification configuration handler and the notification delivery path to the SNS/SQS/Lambda destinations - every step must settle with a response.
Traces: lambda__s3__sns__sqs.refarch-s3ev-e2e-diagnose-s3-event-notification-break-lifecycle.

## Evidence
Expected (AWS): every step of the lifecycle returns (success or a modelled error) and the repaired notification delivers the object-created event to its destination within the polling window.
Actual (emulator): Error: Check timed out after 60.0s.

## Steps to Reproduce
1. Replay the behavioral suite and re-run lambda__s3__sns__sqs.refarch-s3ev-e2e-diagnose-s3-event-notification-break-lifecycle.
2. Wire a bucket notification to a Lambda (and to SNS/SQS), break the invoke permission, PutObject, observe the absence, repair the permission and PutObject again - timing each call.
