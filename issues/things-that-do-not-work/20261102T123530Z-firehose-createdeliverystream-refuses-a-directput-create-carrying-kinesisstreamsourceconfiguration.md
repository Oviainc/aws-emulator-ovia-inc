Issue-Source: trace-sweep-divergence
Issue-Trace: firehose-kfi-traj-directput-silently-drops-kinesis-source
Issue-Step: task#8
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: firehose-kfi-traj-directput-silently-drops-kinesis-source firehose-kfi-traj-stream-before-glue-table
Issue-Shape-Id: com.amazonaws.firehose#Firehose_20150804
Issue-Key: firehose|CreateDeliveryStream|2xx->4xx|$.DeliveryStreamARN
Issue-Key: firehose|CreateDeliveryStream|2xx->4xx|status
Issue-Key: firehose|DescribeDeliveryStream|2xx->4xx|$
Issue-Key: firehose|DescribeDeliveryStream|2xx->4xx|$.DeliveryStreamDescription
Issue-Key: firehose|DescribeDeliveryStream|2xx->4xx|$.DeliveryStreamDescription.DeliveryStreamStatus
Issue-Key: firehose|DescribeDeliveryStream|2xx->4xx|status
Issue-Key: firehose|DeleteDeliveryStream|2xx->4xx|status
Issue-Commit: 81fc6333f2dd5c26c146222a43c6f73baa469449

# firehose: CreateDeliveryStream refuses a DirectPut create carrying KinesisStreamSourceConfiguration, cascading into Describe and Delete (3 causes)

> _Note: this issue was identified against the code base at commit `81fc6333f2dd5c26c146222a43c6f73baa469449`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDeliveryStream

## Description
Three net-new wire causes on the Firehose control-plane create path. Causes 2 and 3 are the downstream effect of cause 1 on the SAME delivery stream, but they are separate replay keys and each must be re-replayed before this issue is closed. No open issue carries these replay keys (replay reported owned_by empty for all three).

Cause 1 of 3: CreateDeliveryStream refuses a create AWS accepts, so no DeliveryStreamARN is returned.
AWS: Smithy: CreateDeliveryStream input has DeliveryStreamType (enum DatabaseAsSource|DirectPut|KinesisStreamAsSource|MSKAsSource) and the independent optional members DirectPutSourceConfiguration, KinesisStreamSourceConfiguration, MSKSourceConfiguration and DatabaseSourceConfiguration; the model declares NO mutual exclusion between a DirectPut type and a KinesisStreamSourceConfiguration, and the operation's only modeled client errors are InvalidArgumentException, InvalidKMSResourceException, LimitExceededException and ResourceInUseException (all HTTP 400). Output is HTTP 200 carrying DeliveryStreamARN (@length 1..512, pattern ^arn:.*:firehose:[a-zA-Z0-9\-]+:\d{12}:deliverystream/[a-zA-Z0-9._-]+$). The recording shows AWS answering 200 with the delivery stream's ARN for a DirectPut create that ALSO supplies KinesisStreamSourceConfiguration: the source configuration that does not belong to the declared type is silently ignored, not an argument fault.
Emulator: HTTP 400 and no DeliveryStreamARN member at all. The create handler's source-configuration gate refuses any source configuration whose member name is not the one the declared stream type names (services/service-com-amazonaws-firehose-firehose-20150804/src/ops/create_delivery_stream.rs, source_description()/its 'the source configuration a create supplies must be the one its declared type names' rule), raising InvalidArgumentException.
Why: a client that reuses one request template across stream types — the common case when a script is parameterised over DirectPut and KinesisStreamAsSource — cannot create a DirectPut stream at all against the emulator, while the same call succeeds on AWS. Everything downstream of the create (the ARN, the stream, its records) is unreachable.
Fix: the CreateDeliveryStream handler's source-configuration validation (src/ops/create_delivery_stream.rs) must drop a source configuration that does not match the declared DeliveryStreamType rather than refuse the request, and still answer 200 with the minted DeliveryStreamARN.
Traces: firehose-kfi-traj-directput-silently-drops-kinesis-source#task#8 (representative). The cluster's other member, firehose-kfi-traj-stream-before-glue-table#task#12, shows the same 200->400 signature but arises from the missing Glue-table check filed separately (see the Iceberg create/update validation issue of this sweep).

Cause 2 of 3: with the create refused, DescribeDeliveryStream answers ResourceNotFoundException for a stream AWS has ACTIVE.
AWS: Smithy: DescribeDeliveryStream returns DeliveryStreamDescription (required) and models only ResourceNotFoundException (client, HTTP 400). The recording shows HTTP 200 with DeliveryStreamDescription whose DeliveryStreamStatus reaches ACTIVE, DeliveryStreamType DirectPut, the stream ARN, DeliveryStreamEncryptionConfiguration{Status DISABLED}, VersionId '1', HasMoreDestinations false and one Destinations entry carrying the IcebergDestinationDescription.
Emulator: HTTP 400 ResourceNotFoundException with message 'Firehose under account <accountId> not found.' — correct for a stream that does not exist, but the stream does not exist only because cause 1 refused its create. The fixture's wait-for-ACTIVE poll therefore never satisfies and is capped at 60 s.
Why: a client cannot observe the stream it just asked for, and any wait-for-ACTIVE loop spins until its own timeout instead of proceeding.
Fix: no separate fix is expected beyond cause 1 — the DescribeDeliveryStream handler (src/ops/describe_delivery_stream.rs) is reporting state faithfully — but this replay key must be re-run to confirm the stream now exists and reaches ACTIVE.
Traces: firehose-kfi-traj-directput-silently-drops-kinesis-source#task#9.

Cause 3 of 3: DeleteDeliveryStream of the same stream answers 400 where AWS answers 200.
AWS: Smithy: DeleteDeliveryStream returns an empty output with HTTP 200 and models ResourceInUseException and ResourceNotFoundException (both client, HTTP 400). The recording shows HTTP 200 for the delete of the stream the create had made.
Emulator: HTTP 400, because the stream was never registered.
Why: a teardown step fails, so a client's cleanup path reports an error for a resource it legitimately created.
Fix: again downstream of cause 1 (src/ops/delete_delivery_stream.rs needs no change), but this replay key must be re-run.
Traces: firehose-kfi-traj-directput-silently-drops-kinesis-source#task#10.

## Evidence
Cause 1 — fixture firehose-kfi-traj-directput-silently-drops-kinesis-source step task#8 (CreateDeliveryStream):
  expected: HTTP 200, $.DeliveryStreamARN present and matching ^arn:.*:firehose:<region>:<account>:deliverystream/<name>$
  actual:   HTTP 400, $.DeliveryStreamARN missing
  (cluster also covers firehose-kfi-traj-stream-before-glue-table#task#12; affected_count 2)

Cause 2 — same fixture, step task#9 (DescribeDeliveryStream):
  expected: HTTP 200, $.DeliveryStreamDescription with DeliveryStreamStatus ACTIVE, DeliveryStreamType DirectPut, VersionId '1', HasMoreDestinations false, one Destinations entry
  actual:   HTTP 400, body {"__type": "ResourceNotFoundException", "message": "Firehose under account <accountId> not found."}; $.DeliveryStreamDescription missing; the wait-for-ACTIVE poll was capped at 60 s

Cause 3 — same fixture, step task#10 (DeleteDeliveryStream):
  expected: HTTP 200, empty body
  actual:   HTTP 400

## Steps to Reproduce
1. Replay fixture firehose-kfi-traj-directput-silently-drops-kinesis-source against the emulator.
2. Step task#8: CreateDeliveryStream with DeliveryStreamType='DirectPut', an IcebergDestinationConfiguration, AND a KinesisStreamSourceConfiguration{KinesisStreamARN, RoleARN}. AWS answers 200 with DeliveryStreamARN and ignores the Kinesis source; the emulator answers 400.
3. Step task#9: poll DescribeDeliveryStream for that name until DeliveryStreamStatus=='ACTIVE'. AWS returns the description; the emulator returns 400 ResourceNotFoundException until the poll's 60 s cap.
4. Step task#10: DeleteDeliveryStream for that name. AWS answers 200; the emulator answers 400.
