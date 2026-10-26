Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: firehose.put-record-invalid-source-kinesis-backed-stream firehose.put-record-invalid-source-rejected firehose.put-record-kinesis-source-rejected firehose.put-record-rejected-on-kinesis-source-stream firehose.deploy-cfn-firehose-stream-becomes-active firehose.deploy-cfn-stack-deletion-removes-delivery-stream firehose.kinesis-source-stream-rejects-put-record firehose.put-record-on-kinesis-source-stream-rejected
Issue-Shape-Id: com.amazonaws.firehose#Firehose_20150804
Issue-Key: firehose.put-record-invalid-source-kinesis-backed-stream|run#18|AssertionError|raise AssertionError(
Issue-Key: firehose.put-record-invalid-source-rejected|_error_code#16|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded: {result!r}")
Issue-Key: firehose.put-record-kinesis-source-rejected|_error_code#9|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded: {result!r}")
Issue-Key: firehose.put-record-rejected-on-kinesis-source-stream|_error_code#17|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded: {result!r}")
Issue-Key: firehose.deploy-cfn-firehose-stream-becomes-active|run#10|AssertionError|assert desc["DeliveryStreamStatus"] == "ACTIVE", (
Issue-Key: firehose.deploy-cfn-stack-deletion-removes-delivery-stream|run#13|AssertionError|assert active_desc["DeliveryStreamStatus"] == "ACTIVE", (
Issue-Key: kinesis|AddTagsToStream|ResourceInUseException
Issue-Commit: 047d51b04da9109ccc060d7d15a53e485d40f72b

# Firehose: PutRecord accepted on KinesisStreamAsSource streams, CFN-deployed streams stay CREATING after CREATE_COMPLETE, and a just-created Kinesis source stream cannot be tagged (3 causes)

> _Note: this issue was identified against the code base at commit `047d51b04da9109ccc060d7d15a53e485d40f72b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutRecord

## Description
This issue bundles 3 net-new root causes of the firehose service. A fixer must close all three before submitting; each cause below names its own failing check ids.

Cause 1 of 3: direct-put ingestion is still not gated on the delivery stream's source type, so PutRecord on a stream created with DeliveryStreamType=KinesisStreamAsSource is accepted with 200.
Failing checks for this cause: firehose.put-record-invalid-source-kinesis-backed-stream, firehose.put-record-invalid-source-rejected, firehose.put-record-kinesis-source-rejected, firehose.put-record-rejected-on-kinesis-source-stream.
AWS: a Firehose stream whose source is a Kinesis data stream cannot also take direct puts — "When you configure a Kinesis data stream as the source of a Firehose stream, the PutRecord and PutRecordBatch operations are disabled" (https://docs.aws.amazon.com/firehose/latest/dev/writing-with-kinesis-streams.html). Smithy: PutRecord declares InvalidArgumentException (errorKind client, httpError 400) alongside InvalidKMSResourceException, InvalidSourceException, ResourceNotFoundException and ServiceUnavailableException (server, 503) — model services/service-com-amazonaws-firehose-firehose-20150804/smithy-model/com-amazonaws-firehose-firehose-20150804.json. The recorded behavior is a synchronous refusal with HTTP 400 and Error.Code=InvalidArgumentException; InvalidSourceException is NOT the code here (its documentation scopes it to the CloudWatch Logs decompression case). The requirement is the 400 and the code; the message the checks compare against, 'This operation is not permitted on KinesisStreamAsSource delivery stream type.', is the recorded example and it is built from the stream's source type, so emitting it is the safe choice.
Emulator: the put succeeds with HTTP 200 and mints a RecordId (e.g. {'RecordId': '000000000000000018e1a9e7c4288b8c0000000000000001', 'Encrypted': False, 'ResponseMetadata': {'HTTPStatusCode': 200}}), and the DirectPut positive control in the same check is indistinguishable from the KDS-sourced stream.
Why: a producer wrongly pointed at a KDS-sourced stream is told its write was accepted and handed a RecordId, while on real AWS that write is rejected and the client must repoint or create a DirectPut stream; the configuration error is hidden and any test asserting the refusal passes against the emulator and fails against AWS.
Fix: the source-type gate belongs in the firehose ingest state (a refusal variant beside the existing CloudWatch-Logs one) with the PutRecord handler mapping it to a 400 InvalidArgumentException, conditional on DeliveryStreamType/KinesisStreamSourceConfiguration rather than a blanket refusal.
Related but not covering this cause: open issue 20261025T002604Z-firehose-putrecord-putrecordbatch-are-accepted-with-200-on.md describes the identical defect but carries only the replay keys of four OTHER check ids (firehose.e2e-put-record-kinesis-source-rejected, firehose.put-record-invalid-source-stream-rejected, firehose.kinesis-source-preserves-sequence-number-order, firehose.put-record-batch-invalid-source-kinesis-backed-stream); the four check ids above are still unowned, and they additionally pin the refusal message and the 'nothing leaked into S3' consequence.
Traces: the four check ids above; the handler is services/service-com-amazonaws-firehose-firehose-20150804/src/ops/put_record.rs with the stream-refusal set in that crate's src/state.rs.

Cause 2 of 3: a CloudFormation stack that declares an AWS::KinesisFirehose::DeliveryStream reports CREATE_COMPLETE while the stream it created is still in DeliveryStreamStatus CREATING.
Failing checks for this cause: firehose.deploy-cfn-firehose-stream-becomes-active, firehose.deploy-cfn-stack-deletion-removes-delivery-stream.
AWS: recorded behavior only (CloudFormation resource stabilization is not expressed in the Firehose Smithy model). The recording shows that once the stack_create_complete waiter returns, DescribeDeliveryStream on the templated stream reports DeliveryStreamStatus=ACTIVE (Smithy: DescribeDeliveryStream output member DeliveryStreamDescription is required and carries DeliveryStreamStatus); CloudFormation does not mark a delivery-stream resource — and therefore the stack — complete until the stream has stabilized to ACTIVE.
Emulator: DescribeDeliveryStream answers 200 immediately after CREATE_COMPLETE with DeliveryStreamStatus='CREATING' ('expected delivery stream <name> ACTIVE, got CREATING'; 'expected ACTIVE after CREATE_COMPLETE, got CREATING'), and the stream does not advance to ACTIVE on its own, so neither check reaches its PutRecord/S3-delivery or stack-deletion assertions.
Why: every infrastructure-as-code workflow breaks at its first use of what it just deployed — a client that correctly waits for the stack and then describes or writes to the stream it declared sees a non-ACTIVE resource that, against AWS, cannot be observed at that point.
Fix: two places, either of which closes the divergence and both of which should be correct — (a) the CloudFormation create-stack resource provisioner must not report the AWS::KinesisFirehose::DeliveryStream resource complete until the stream has reached ACTIVE in Firehose state, and (b) the delivery-stream lifecycle in the firehose crate must actually advance a created stream from CREATING to ACTIVE.
Related but not covering this cause: open issue 20261025T025203Z-firehose-reopens-a-cfn-deployed-firehose-delivery-stream-never.md describes the same CREATE_COMPLETE-while-CREATING symptom but carries only firehose.deploy-cfn-stream-s3-destination-config-applied, and open issue 20261025T002604Z-cloudwatch-firehose-s3-cloudformation-deployed-metric-stream-pipeline-the-stack-reports.md carries only the cloudwatch__firehose__s3 check ids; neither carries the two replay keys above.
Traces: the two check ids above; the CloudFormation create-stack resource dispatch in services/service-com-amazonaws-cloudformation-cloudformation/ plus the stream lifecycle in services/service-com-amazonaws-firehose-firehose-20150804/src/state.rs.

Cause 3 of 3: the Kinesis source stream these firehose checks create for their KinesisStreamAsSource setup cannot be tagged right after CreateStream — AddTagsToStream is refused because the stream is left in CREATING — so both checks abort during setup and never exercise Firehose at all.
Failing checks for this cause: firehose.kinesis-source-stream-rejects-put-record, firehose.put-record-on-kinesis-source-stream-rejected.
AWS: recorded behavior only. The recording shows kinesis.create_stream(StreamName=..., ShardCount=1, StreamModeDetails={'StreamMode':'PROVISIONED'}) immediately followed by kinesis.add_tags_to_stream(StreamName=..., Tags={...}) succeeding with no wait in between (Smithy: AddTagsToStream has an empty output and declares AccessDeniedException, InvalidArgumentException, LimitExceededException, ResourceInUseException and ResourceNotFoundException, all client/400 — model services/service-com-amazonaws-kinesis-kinesis-20131202/smithy-model/com-amazonaws-kinesis-kinesis-20131202.json). Hypothesis: real AWS either tags during CREATING or has the newly created stream usable by the time the tag call lands; the deterministic fact is that the recorded sequence succeeded and the emulator refuses it.
Emulator: HTTP 400 ResourceInUseException on AddTagsToStream (example message: 'stream sim2real-behav-fh-src-12c4944a655b is in state CREATING, not ACTIVE; the stream must be ACTIVE for this operation'), raised before any firehose call is made.
Why: a client that creates a Kinesis stream and tags it in the same control-plane sequence — the standard create-then-tag pattern, and the setup of every KinesisStreamAsSource pipeline — gets a hard 400 that AWS does not produce, and the whole downstream pipeline cannot be built.
Fix: the Kinesis stream lifecycle/state must advance a created stream out of CREATING (as the firehose crate does for its own streams) and/or the AddTagsToStream handler must not gate tagging on ACTIVE. NOTE for triage: the kinesis service (service-com-amazonaws-kinesis-kinesis-20131202) is still PENDING bootstrap in this checkout, so if the fix belongs to that crate it may be subsumed by its bootstrap work; it is filed here because the failing checks are firehose checks and firehose is bootstrapped.
Traces: the two check ids above; the Kinesis control-plane state and src/ops/add_tags_to_stream.rs in services/service-com-amazonaws-kinesis-kinesis-20131202/.

## Evidence
Cause 1 (PutRecord on a KinesisStreamAsSource stream):
Expected (AWS): HTTP 400, Error.Code=InvalidArgumentException, nothing delivered; the sibling DirectPut stream in the same check keeps succeeding.
Actual (emulator): HTTP 200 with a RecordId.
  * firehose.put-record-invalid-source-rejected: AssertionError: expected the call to be rejected, but it succeeded: {'RecordId': '000000000000000018e1a9e7c4288b8c0000000000000001', 'Encrypted': False, 'ResponseMetadata': {'HTTPStatusCode': 200, ...}} (the check then asserts code == 'InvalidArgumentException' and message == 'This operation is not permitted on KinesisStreamAsSource delivery stream type.')
  * firehose.put-record-kinesis-source-rejected: AssertionError: expected the call to be rejected, but it succeeded: {'RecordId': '000000000000000018e1a7f92bd28c500000000000000001', ..., 'HTTPStatusCode': 200}
  * firehose.put-record-rejected-on-kinesis-source-stream: AssertionError: expected the call to be rejected, but it succeeded: {'RecordId': '000000000000000018e1a83977cc7b7e0000000000000001', ..., 'HTTPStatusCode': 200}
  * firehose.put-record-invalid-source-kinesis-backed-stream: AssertionError: expected InvalidArgumentException for PutRecord on a Kinesis-sourced stream

Cause 2 (CFN-deployed stream status):
Expected (AWS): after cfn.get_waiter('stack_create_complete').wait(...) on a template declaring an AWS::KinesisFirehose::DeliveryStream (DirectPut + S3DestinationConfiguration), describe_delivery_stream(...)['DeliveryStreamDescription']['DeliveryStreamStatus'] == 'ACTIVE'.
Actual (emulator): 'CREATING'.
  * firehose.deploy-cfn-firehose-stream-becomes-active: AssertionError: expected delivery stream sim2real-behav-fh-stream-8611a1068b2f ACTIVE, got CREATING (the stack itself reported CREATE_COMPLETE, so that assertion passed)
  * firehose.deploy-cfn-stack-deletion-removes-delivery-stream: AssertionError: expected ACTIVE after CREATE_COMPLETE, got CREATING

Cause 3 (tagging a freshly created Kinesis source stream):
Expected (AWS): kinesis.create_stream(...) then kinesis.add_tags_to_stream(StreamName=..., Tags={'sim2real': <check>}) returns 200 with an empty body.
Actual (emulator): botocore.errorfactory.ResourceInUseException: An error occurred (ResourceInUseException) when calling the AddTagsToStream operation: stream sim2real-behav-fh-src-12c4944a655b is in state CREATING, not ACTIVE; the stream must be ACTIVE for this operation — HTTP 400. Same failure in firehose.put-record-on-kinesis-source-stream-rejected (stream sim2real-behav-fhkstream-ea6fb8a58112). Both checks abort in setup. The status and error code are the requirement; the quoted messages are examples.

## Steps to Reproduce
Cause 1:
1. CreateDeliveryStream with DeliveryStreamType=KinesisStreamAsSource, KinesisStreamSourceConfiguration={KinesisStreamARN, RoleARN} and an ExtendedS3DestinationConfiguration; wait for DeliveryStreamStatus=ACTIVE.
2. PutRecord on that stream with any Record.Data -> emulator answers 200 + RecordId; AWS answers 400 InvalidArgumentException ('This operation is not permitted on KinesisStreamAsSource delivery stream type.').
3. Repeat on a sibling DirectPut stream: that must still succeed (positive control), and the rejected payload must never appear in the destination bucket.
4. Re-replay: firehose.put-record-invalid-source-kinesis-backed-stream, firehose.put-record-invalid-source-rejected, firehose.put-record-kinesis-source-rejected, firehose.put-record-rejected-on-kinesis-source-stream.

Cause 2:
1. cfn.create_stack with a template declaring an S3 bucket, a firehose delivery role and an AWS::KinesisFirehose::DeliveryStream (DeliveryStreamType DirectPut, S3DestinationConfiguration with BucketARN/RoleARN/Prefix and BufferingHints IntervalInSeconds=60, SizeInMBs=1), Capabilities=['CAPABILITY_NAMED_IAM'].
2. Wait with the stack_create_complete waiter; describe_stacks shows CREATE_COMPLETE.
3. firehose.describe_delivery_stream on the templated stream name -> observe DeliveryStreamStatus='CREATING' instead of 'ACTIVE'.
4. Re-replay: firehose.deploy-cfn-firehose-stream-becomes-active, firehose.deploy-cfn-stack-deletion-removes-delivery-stream.

Cause 3:
1. kinesis.create_stream(StreamName=<new>, ShardCount=1, StreamModeDetails={'StreamMode': 'PROVISIONED'}).
2. Immediately kinesis.add_tags_to_stream(StreamName=<same>, Tags={'sim2real': 'x'}) -> emulator answers 400 ResourceInUseException ('in state CREATING, not ACTIVE'); AWS accepts the call.
3. Re-replay: firehose.kinesis-source-stream-rejects-put-record, firehose.put-record-on-kinesis-source-stream-rejected.
