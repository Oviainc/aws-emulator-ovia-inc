Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__firehose__s3.firehose-s3-destination-wrong-bucket-arn-rejected
Issue-Key: events__firehose__s3.firehose-s3-destination-wrong-bucket-arn-rejected|run#14|AssertionError|raise AssertionError(
Issue-Commit: a1aff8283ff7a4c3e09b44355afd91a57915fcd2

# events__firehose__s3: Firehose UpdateDestination accepts an ExtendedS3DestinationUpdate naming a bucket that does not exist, and re-points the live destination at it (1 cause)

> _Note: this issue was identified against the code base at commit `a1aff8283ff7a4c3e09b44355afd91a57915fcd2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateDestination

## Description
Cause 1 of 1: UpdateDestination performs no existence check on ExtendedS3DestinationUpdate.BucketARN.
AWS: Smithy: firehose#UpdateDestination takes DeliveryStreamName, CurrentDeliveryStreamVersionId, DestinationId and one of the per-type *DestinationUpdate members (here ExtendedS3DestinationUpdate), and models the errors ConcurrentModificationException, InvalidArgumentException, ResourceInUseException and ResourceNotFoundException — all client errors at HTTP 400. The recording shows a BucketARN whose bucket does not exist being refused with InvalidArgumentException, and the message is built from the request so it is part of the rule: 'S3 Bucket <bucket-name> does not exist.' The refusal has no effect: the stream's VersionId does not advance and DescribeDeliveryStream still reports the previous ExtendedS3DestinationDescription.BucketARN.
Emulator: the call returns 200. No existence check is made against S3, the destination is re-pointed at the non-existent bucket and the stream version advances. The check fails at the point of acceptance: AssertionError 'update_destination unexpectedly accepted a non-existent bucket ARN'.
Why: this is the pre-flight that stops a delivery stream from being configured into a black hole. A client that relies on it — a deployment pipeline swapping a destination, or an operator fixing a typo — gets a success for a configuration that can never deliver, and the stream silently stops producing objects with no error at the API that caused it. Worse, the accepted update destroys the previously working destination, so the failure is not even recoverable by ignoring the response.
Fix: the validation step of the UpdateDestination handler — resolve each submitted destination's BucketARN against S3 before mutating state, and on a missing bucket raise InvalidArgumentException (HTTP 400) with the message naming the bucket, leaving the stream's VersionId and stored destination configuration untouched.
Traces: events__firehose__s3.firehose-s3-destination-wrong-bucket-arn-rejected.

Related but not covering: the open issue 20261024T005347Z-events-firehose-s3-eventbridge-to-firehose-to-s3-records-put-into.md reports that records put into a delivery stream never become S3 objects; that is the delivery leg. This cause is the control-plane pre-flight on UpdateDestination, which the emulator skips entirely, and it is also distinct from 20261025T002357Z-events-firehose-s3-firehose-accepts-an-s3-destination-role-whose-trust.md (role trust policy, on CreateDeliveryStream).

## Evidence
Expected: UpdateDestination(DeliveryStreamName=S, CurrentDeliveryStreamVersionId=V, DestinationId=D, ExtendedS3DestinationUpdate={'BucketARN':'arn:aws:s3:::<missing-bucket>', ...}) raises HTTP 400 InvalidArgumentException with Message 'S3 Bucket <missing-bucket> does not exist.'; afterwards DescribeDeliveryStream still reports VersionId=V and Destinations[0].ExtendedS3DestinationDescription.BucketARN equal to the original bucket ARN. Actual: the call succeeds (no ClientError raised at all), so the check raises AssertionError 'update_destination unexpectedly accepted a non-existent bucket ARN' at check_events__firehose__s3_firehose_s3_destination_wrong_bucket_arn_rejected.py line 90, and the subsequent assertions on the unchanged VersionId/BucketARN are never reached.

## Steps to Reproduce
Create an S3 bucket B and an IAM role R trusted by firehose.amazonaws.com; CreateDeliveryStream with an ExtendedS3DestinationConfiguration pointing at B with role R; DescribeDeliveryStream to read VersionId and DestinationId; then call UpdateDestination with ExtendedS3DestinationUpdate.BucketARN='arn:aws:s3:::<a bucket name that was never created>'. The emulator returns 200 and advances the stream version; AWS returns 400 InvalidArgumentException 'S3 Bucket <name> does not exist.' and leaves the stream unchanged.

Replay: events__firehose__s3.firehose-s3-destination-wrong-bucket-arn-rejected.
