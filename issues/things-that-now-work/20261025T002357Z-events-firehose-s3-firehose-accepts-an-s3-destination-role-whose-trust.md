Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__firehose__s3.s3-role-without-trust-policy-rejected
Issue-Key: events__firehose__s3.s3-role-without-trust-policy-rejected|_poll_replication_status#11|Timeout|time.sleep(10)
Issue-Commit: 6f030629c911e30ed6652430413113629aa7dccf

# Firehose accepts an S3 destination role whose trust policy does not allow firehose.amazonaws.com, and the delivery stream produces no S3 object either way (1 cause)

> _Note: this issue was identified against the code base at commit `6f030629c911e30ed6652430413113629aa7dccf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDeliveryStream

## Description
Cause 1 of 1 — the S3 destination role's trust policy is not validated at CreateDeliveryStream, and the positive control that would prove the stream works never produces an object.
AWS: `CreateDeliveryStream` with an `ExtendedS3DestinationConfiguration` validates the supplied RoleARN at creation time: a role whose trust policy does not let firehose.amazonaws.com perform sts:AssumeRole is refused with a 400 client error (InvalidArgumentException), and no delivery stream is created. With a correctly trusted role the stream reaches ACTIVE and records put into it are delivered as objects under the configured bucket and prefix.
Emulator: the check cannot reach its verdict — it establishes the positive leg first (a correctly trusted role, records put, then poll S3 for the delivered object), that object never appears, and the replay cuts the check off at 60 s. So the untrusted-role refusal is never graded, and the emulator additionally delivers nothing on the working configuration.
Why: a client gets a delivery stream it believes is ACTIVE and correctly permissioned while no data ever lands in S3, and a misconfigured role — the single most common Firehose setup error, which AWS catches at create time — is not reported at the point of configuration.
Fix: RoleARN trust-policy validation in the CreateDeliveryStream handler (services/service-com-amazonaws-firehose-firehose-20150804/src/ops/create_delivery_stream.rs) and the S3 delivery path that must write buffered records as objects.
Note: two open issues touch this area but carry neither this check's replay key nor the trust-policy validation — 20261024T005347Z-events-firehose-s3-eventbridge-to-firehose-to-s3-records-put-into.md (records never become S3 objects, filed from the two sibling checks) and 20261022T194351Z-events-firehose-s3-eventbridge-firehose-s3-eventing-eventbridge-tagresource-and-s3.md.
Traces: events__firehose__s3.s3-role-without-trust-policy-rejected.

## Evidence
"Check timed out after 60.0s". Prior calls returned 2xx: CreateBucket, iam CreateRole/PutRolePolicy for both the correctly trusted and the untrusted role, and CreateDeliveryStream with ExtendedS3DestinationConfiguration. The terminal read — ListObjectsV2/GetObject for the object the stream should deliver on the positive leg — never returns it, so the negative leg (CreateDeliveryStream with the untrusted role must be refused with a 400) is never evaluated. Expected: the positive stream delivering an object to the bucket within the check's budget, and the untrusted-role CreateDeliveryStream refused with a 400 client error and no stream created.

## Steps to Reproduce
1. CreateBucket(b). CreateRole(good) with a trust policy allowing firehose.amazonaws.com sts:AssumeRole and an S3 write policy; CreateRole(bad) with the same permissions but a trust policy that does NOT allow firehose.amazonaws.com.
2. CreateDeliveryStream(DeliveryStreamName=s, ExtendedS3DestinationConfiguration={'RoleARN': <good>, 'BucketARN': <b arn>, 'Prefix': 'p/'}) -> wait for ACTIVE, PutRecord/PutRecordBatch, then poll b: no object ever appears.
3. CreateDeliveryStream with RoleARN=<bad> -> expected a 400 refusal with no stream created; cannot be graded because step 2 never succeeds.
