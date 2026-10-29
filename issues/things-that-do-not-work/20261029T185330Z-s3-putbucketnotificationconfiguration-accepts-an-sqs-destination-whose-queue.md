Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue__sqs.glue-crawler-sqs-queue-policy-blocks-send-rejected
Issue-Key: glue__sqs.glue-crawler-sqs-queue-policy-blocks-send-rejected|_error#8|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Commit: d4c3381173b09dc1be1da315e64a87a78d438272

# s3: PutBucketNotificationConfiguration accepts an SQS destination whose queue policy does not allow s3.amazonaws.com to send, instead of refusing the configuration (1 cause)

> _Note: this issue was identified against the code base at commit `d4c3381173b09dc1be1da315e64a87a78d438272`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutBucketNotificationConfiguration

## Description
1 cause, grounded in one failing glue__sqs check.

Cause 1 of 1: the destination's resource policy is not validated when a bucket notification configuration is written.
AWS: recorded behavior (S3 validates destinations synchronously): PutBucketNotificationConfiguration with a QueueConfiguration whose Queue ARN names a queue whose resource policy does NOT allow Principal {"Service":"s3.amazonaws.com"} to sqs:SendMessage on that queue is REFUSED — S3 answers HTTP 400 with error code `InvalidArgument` and a message naming the destination it could not validate (`Unable to validate the following destination configurations` plus the destination's ARN and the reason, i.e. that permissions on the destination queue do not allow S3 to publish to it). Nothing is stored: a following GetBucketNotificationConfiguration shows no QueueConfiguration. Granting the policy and repeating the identical call succeeds.
Emulator: the call returns HTTP 200 (empty body) and the configuration is stored, so a bucket is wired to a queue that could never receive its events.
Why: an overly-permissive accept on the only call that can report this misconfiguration. A caller — here a Glue event-mode crawler being wired to its event queue — is told the wiring is in place while in AWS the same deployment fails fast, so the missing queue policy surfaces only as silently absent notifications later, and every negative test of destination validation passes vacuously.
Fix: the PutBucketNotificationConfiguration handler of the s3 service crate must validate each configured destination (Queue/Topic/LambdaFunction) against that destination's resource policy for the s3.amazonaws.com principal before storing, and refuse with HTTP 400 InvalidArgument and AWS's destination-validation message, leaving the existing configuration unchanged.
Traces: glue__sqs.glue-crawler-sqs-queue-policy-blocks-send-rejected.

## Evidence
glue__sqs.glue-crawler-sqs-queue-policy-blocks-send-rejected (check line 48, s3.put_bucket_notification_configuration with a QueueConfiguration pointing at a queue whose policy does not allow s3.amazonaws.com):
  expected: HTTP 400 / InvalidArgument / a message naming the destination that could not be validated
  actual:   AssertionError: expected the call to be rejected, but it SUCCEEDED and returned
            {'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'application/xml', 'content-length': '0', …}}}
  (the request id / host id in the response are per-run values and are not part of the requirement)

## Steps to Reproduce
1. sqs:CreateQueue(QueueName=Q) with NO policy granting s3.amazonaws.com sqs:SendMessage.
2. s3:CreateBucket(Bucket=B).
3. s3:PutBucketNotificationConfiguration(Bucket=B, NotificationConfiguration={'QueueConfigurations':[{'Id':'ev','QueueArn':<Q arn>,'Events':['s3:ObjectCreated:*']}]}) -> must fail with HTTP 400, Error.Code 'InvalidArgument' and a message naming the unvalidatable destination; s3:GetBucketNotificationConfiguration(B) must then show no QueueConfigurations.
4. sqs:SetQueueAttributes(Q, Policy=<allow Principal Service s3.amazonaws.com to sqs:SendMessage on Q's ARN with aws:SourceArn = B's ARN>) and repeat step 3: it must now succeed.
