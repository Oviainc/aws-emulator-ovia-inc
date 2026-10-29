Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: firehose__kinesis__s3.firehose-kinesis-source-record-payload-preserved firehose__kinesis__s3.cli-kinesis-put-record-firehose-delivers-s3-workflow firehose__kinesis__s3.cfn-deploy-kinesis-firehose-s3-stack-positive firehose__kinesis__s3.cfn-firehose-role-trust-policy-from-template-positive firehose__kinesis__s3.cfn-stack-update-firehose-s3-prefix-positive firehose__kinesis__s3.cfn-deploy-firehose-missing-s3-permission-negative firehose__kinesis__s3.firehose-kinesis-source-missing-role-permission-negative firehose__kinesis__s3.firehose-s3-destination-mismatched-bucket-arn-negative firehose__kinesis__s3.s3-role-missing-trust-relationship-negative
Issue-Key: firehose__kinesis__s3.firehose-kinesis-source-record-payload-preserved|_poll_until#13|Timeout|time.sleep(interval_s)
Issue-Key: firehose__kinesis__s3.cli-kinesis-put-record-firehose-delivers-s3-workflow|run#23|Timeout|time.sleep(10.0)
Issue-Key: firehose__kinesis__s3.cfn-deploy-kinesis-firehose-s3-stack-positive|_poll_until#13|Timeout|time.sleep(interval_s)
Issue-Key: firehose__kinesis__s3.cfn-firehose-role-trust-policy-from-template-positive|_poll_until#28|Timeout|time.sleep(interval_s)
Issue-Key: firehose__kinesis__s3.cfn-stack-update-firehose-s3-prefix-positive|_poll_until#10|Timeout|time.sleep(interval_s)
Issue-Key: firehose__kinesis__s3.cfn-deploy-firehose-missing-s3-permission-negative|_poll_until#15|Timeout|time.sleep(interval_s)
Issue-Key: firehose__kinesis__s3.firehose-kinesis-source-missing-role-permission-negative|_create_delivery_stream_expect_denied#10|AssertionError|raise AssertionError(
Issue-Key: firehose__kinesis__s3.firehose-s3-destination-mismatched-bucket-arn-negative|_create_delivery_stream_expect_denied#8|AssertionError|raise AssertionError(
Issue-Key: firehose__kinesis__s3.s3-role-missing-trust-relationship-negative|run#15|AssertionError|assert status == "FAILED", f"expected FAILED replication status, got {status!r}"
Issue-Commit: f4e5251e6df88363d8e23ace2ff5bff3be31b423

# firehose__kinesis__s3: 4 net-new root causes — a KDS-sourced stream never ingests, CFN-deployed Kinesis→Firehose→S3 stacks never deliver, CreateDeliveryStream accepts an unreadable source role and a nonexistent destination bucket, and S3 replication reports COMPLETED for an untrusting role

> _Note: this issue was identified against the code base at commit `f4e5251e6df88363d8e23ace2ff5bff3be31b423`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDeliveryStream

## Description
Four net-new root causes. The two other failures in this service (firehose-kinesis-source-delivers-to-s3-positive, firehose-s3-buffering-hints-flush-positive) are OWNED by 20261026T074043Z-firehose-kinesis-s3-a-kinesisstreamassource-delivery-stream-never.md and are excluded.

=== Cause 1 of 4: a KinesisStreamAsSource delivery stream never consumes its source stream, so records put to Kinesis never become S3 objects ===
AWS: recorded behavior only (KinesisStreamSourceConfiguration is modelled; the ingestion loop is not). AWS behavior: once the stream is ACTIVE, Firehose reads the configured Kinesis shard iterator continuously; a kinesis:PutRecord payload becomes, after the buffering window, an object in the S3 destination whose bytes are the record's data preserved verbatim.
Emulator: nothing is ever read from the source Kinesis stream and no object appears at the destination, so both the API-driven and the CLI-driven variants exhaust their budget (`Check timed out after 60.0s`). The payload-preservation claim cannot be graded at all.
Why: the whole KDS->Firehose->S3 ingestion pattern produces no data; a client sees a healthy ACTIVE stream whose destination is permanently empty. Related open issue 20261026T074043Z names this family but does not carry these two checks' replay keys.
Fix: the Firehose ingestion worker for KinesisStreamAsSource streams (shard reader -> delivery buffer), which must be started when such a stream becomes ACTIVE.
Traces: firehose__kinesis__s3.firehose-kinesis-source-record-payload-preserved, firehose__kinesis__s3.cli-kinesis-put-record-firehose-delivers-s3-workflow.

=== Cause 2 of 4: CloudFormation-deployed Kinesis→Firehose→S3 stacks converge but deliver nothing, and a stack UPDATE of the S3 Prefix has no observable effect ===
AWS: recorded behavior only. A stack declaring AWS::Kinesis::Stream, AWS::IAM::Role (trusting firehose.amazonaws.com, built from the template) and AWS::KinesisFirehose::DeliveryStream with KinesisStreamSourceConfiguration converges to CREATE_COMPLETE with a working data path: a kinesis:PutRecord lands as an S3 object; after an UpdateStack that changes the destination Prefix, subsequent records land under the NEW prefix; and a stack whose role lacks s3:PutObject delivers nothing.
Emulator: all four CFN variants poll the destination and are killed at 60 s with nothing delivered — the positive stack, the trust-policy-from-template stack, the prefix-update stack and the missing-permission negative (whose positive control also never delivers, so the negative proves nothing).
Why: an IaC pipeline validated against the emulator reports a healthy stack with a dead data path, and a prefix change cannot be verified.
Fix: the CloudFormation resource providers for AWS::KinesisFirehose::DeliveryStream (create and update of the destination configuration) must register the stream with the ingestion/delivery workers of Cause 1.
Traces: firehose__kinesis__s3.cfn-deploy-kinesis-firehose-s3-stack-positive, firehose__kinesis__s3.cfn-firehose-role-trust-policy-from-template-positive, firehose__kinesis__s3.cfn-stack-update-firehose-s3-prefix-positive, firehose__kinesis__s3.cfn-deploy-firehose-missing-s3-permission-negative.

=== Cause 3 of 4: CreateDeliveryStream accepts a source role without Kinesis read permissions and an S3 destination bucket that does not exist, returning 200 where AWS refuses synchronously ===
AWS: Smithy: CreateDeliveryStream errors are InvalidArgumentException (client, HTTP 400), LimitExceededException, ResourceInUseException, InvalidKMSResourceException. Recorded behavior: Firehose validates the delivery/source role and the destination bucket synchronously during CreateDeliveryStream — it assumes the RoleARN and checks the required kinesis:GetRecords/GetShardIterator/DescribeStream and the destination bucket's existence — and refuses the create with InvalidArgumentException/400 when either check fails, so no stream is created.
Emulator: both calls succeed with HTTP 200 and a DeliveryStreamARN. The role's Kinesis permissions are not evaluated and the BucketARN is not resolved against S3, so a misconfigured stream is created and only then silently delivers nothing.
Why: a client gets a success for a stream that can never work, and loses the synchronous, actionable InvalidArgumentException that tells it which permission or bucket is wrong. Related open issue 20261022T064119Z-firehose-createdeliverystream-putrecordbatch-are-overly-permissive-and-misclassify.md covers other CreateDeliveryStream over-permissiveness (compression enum, malformed/non-S3 bucket ARNs, processor parameters) but not these two validations.
Fix: CreateDeliveryStream's validation stage — the source-role authorization check for KinesisStreamSourceConfiguration and the destination-bucket existence check for (Extended)S3DestinationConfiguration, both of which must refuse with InvalidArgumentException (HTTP 400) before any stream is persisted.
Traces: firehose__kinesis__s3.firehose-kinesis-source-missing-role-permission-negative, firehose__kinesis__s3.firehose-s3-destination-mismatched-bucket-arn-negative.

=== Cause 4 of 4: S3 replication configured with a role that does not trust s3.amazonaws.com reports ReplicationStatus COMPLETED instead of FAILED ===
AWS: recorded behavior only (HeadObject's x-amz-replication-status is modelled as a header, its values are not enumerated for this case). AWS behavior: S3 replication performs the cross-bucket copy as the replication configuration's Role. When that role's trust policy does not allow s3.amazonaws.com, S3 can never assume it, the replication terminally fails, and HeadObject on the source object reports ReplicationStatus `FAILED`.
Emulator: HeadObject reports ReplicationStatus `COMPLETED` for an object whose replication role cannot be assumed — the role is never assumed or evaluated and the status is set optimistically.
Why: a client monitoring x-amz-replication-status to detect a broken replication role is told replication succeeded when nothing was replicated, so a misconfiguration is never surfaced.
Fix: the S3 replication worker — it must assume the replication configuration's Role before copying and set ReplicationStatus FAILED (not COMPLETED) when the trust policy forbids s3.amazonaws.com. Open issue 20261028T105230Z-events-firehose-s3-s3-replication-accepts-a-role.md records the same defect under the events__firehose__s3 service token; this check belongs to firehose__kinesis__s3 and is not covered by it.
Traces: firehose__kinesis__s3.s3-role-missing-trust-relationship-negative.

## Evidence
Cause 1 — firehose__kinesis__s3.firehose-kinesis-source-record-payload-preserved and .cli-kinesis-put-record-firehose-delivers-s3-workflow: Error: Check timed out after 60.0s, both while polling the S3 destination for the record put to the source Kinesis stream.

Cause 2 — firehose__kinesis__s3.cfn-deploy-kinesis-firehose-s3-stack-positive, .cfn-firehose-role-trust-policy-from-template-positive, .cfn-stack-update-firehose-s3-prefix-positive, .cfn-deploy-firehose-missing-s3-permission-negative: all four report Error: Check timed out after 60.0s with an empty destination prefix.

Cause 3 — firehose__kinesis__s3.firehose-kinesis-source-missing-role-permission-negative:
  AssertionError: expected CreateDeliveryStream(<name>) to be rejected for missing kinesis read permissions, but it succeeded
firehose__kinesis__s3.firehose-s3-destination-mismatched-bucket-arn-negative:
  AssertionError: expected CreateDeliveryStream(<name>) to be rejected for a nonexistent destination bucket, but it succeeded
  Expected: HTTP 400, Error.Code InvalidArgumentException, naming the failed role authorization / the unresolvable BucketARN (the role ARN and bucket ARN are values from the request).
  Actual: HTTP 200 with a DeliveryStreamARN.

Cause 4 — firehose__kinesis__s3.s3-role-missing-trust-relationship-negative:
  assert status == "FAILED", f"expected FAILED replication status, got {status!r}"
  AssertionError: expected FAILED replication status, got 'COMPLETED'

## Steps to Reproduce
1. Clone the repo and boot the emulator (run_behavioral_suite).
2. Cause 1: create a Kinesis stream, an S3 bucket, a role trusting firehose.amazonaws.com with kinesis read + s3:PutObject; CreateDeliveryStream with DeliveryStreamType=KinesisStreamAsSource and KinesisStreamSourceConfiguration pointing at it; wait ACTIVE; kinesis PutRecord a unique payload; poll the destination bucket — nothing ever arrives (same via the AWS CLI).
3. Cause 2: deploy a CloudFormation stack with AWS::Kinesis::Stream + AWS::IAM::Role + AWS::KinesisFirehose::DeliveryStream (KinesisStreamAsSource, S3 destination with a Prefix); wait CREATE_COMPLETE; kinesis PutRecord; poll the bucket — empty. Then UpdateStack changing the destination Prefix and repeat — still empty.
4. Cause 3: CreateDeliveryStream with KinesisStreamAsSource whose RoleARN has NO kinesis read permissions — it returns 200. CreateDeliveryStream with (Extended)S3DestinationConfiguration whose BucketARN names a bucket that does not exist — it returns 200.
5. Cause 4: PutBucketReplication on a versioned source bucket with a Role whose trust policy omits s3.amazonaws.com, PutObject into the source, then HeadObject it and read ReplicationStatus — 'COMPLETED'.
