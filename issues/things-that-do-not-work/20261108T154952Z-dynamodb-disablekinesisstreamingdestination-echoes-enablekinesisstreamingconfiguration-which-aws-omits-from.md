Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb.disable-kinesis-streaming-destination-omits-configuration
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb.disable-kinesis-streaming-destination-omits-configuration|run#17|AssertionError|assert "EnableKinesisStreamingConfiguration" not in disabled, (
Issue-Commit: 32a26f18a0fb4dc356c9d3ce1ecec069a0aff304

# DynamoDB DisableKinesisStreamingDestination echoes EnableKinesisStreamingConfiguration, which AWS omits from the stop response

> _Note: this issue was identified against the code base at commit `32a26f18a0fb4dc356c9d3ce1ecec069a0aff304`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DisableKinesisStreamingDestination

## Description
Cause 1 of 1: DisableKinesisStreamingDestination's 200 response carries an EnableKinesisStreamingConfiguration member that AWS does not emit for this operation. Failing check: dynamodb.disable-kinesis-streaming-destination-omits-configuration.

AWS: Smithy — DisableKinesisStreamingDestination and EnableKinesisStreamingDestination share the output shape KinesisStreamingDestinationOutput {TableName, StreamArn, DestinationStatus, EnableKinesisStreamingConfiguration}, and every member of it is OPTIONAL (none is @required), so the model permits but does not require the configuration member on either operation. The model is therefore not decisive here, and the requirement is recorded behavior only: real DynamoDB answers DisableKinesisStreamingDestination with TableName, StreamArn and DestinationStatus ONLY, and omits EnableKinesisStreamingConfiguration entirely — even when the destination being stopped was enabled with an explicit ApproximateCreationDateTimePrecision. The asymmetry is the point: EnableKinesisStreamingDestination DOES echo EnableKinesisStreamingConfiguration with the requested precision (the check asserts that as its positive control, and the emulator passes that part), while Disable does not. The stop response is a 200 with no error code; the divergence is purely in which output members are present.

Emulator: the Disable response includes the extra member. Replay observed a 200 whose body deserialized to {'TableName': <table name from the request>, 'StreamArn': <stream ARN from the request>, 'DestinationStatus': 'DISABLING', 'EnableKinesisStreamingConfiguration': {'ApproximateCreationDateTimePrecision': <the precision the destination was enabled with>}}. The TableName/StreamArn/DestinationStatus members are correct (DISABLING is a valid immediate status); the fourth member should not be there at all. Its presence is unconditional, not a function of the request: the handler always constructs it, so it appears even when the destination carries no precision, in which case it serializes as an empty EnableKinesisStreamingConfiguration object. Note the per-run values above (the generated table name, the generated stream ARN, the request id) are environment noise and are not part of the requirement; the requirement is the member set.

Why: a client cannot distinguish the enable path from the disable path by the shape of the reply. Code that branches on the presence of EnableKinesisStreamingConfiguration to decide whether a call enabled or merely acknowledged a stop — or that treats the echoed configuration as proof that replication is still configured under that precision — reads the emulator's stop reply as an enable-style acknowledgement and concludes the destination is still (or newly) configured for streaming. Strict-shape consumers and response snapshot/golden comparisons also diverge, since an unexpected member appears in a response AWS keeps to three fields.

Fix: the operation's serializer/response construction in its handler — services/service-com-amazonaws-dynamodb-dynamodb-20120810/src/ops/disable_kinesis_streaming_destination.rs, which unconditionally sets enable_kinesis_streaming_configuration: Some(EnableKinesisStreamingConfiguration { .. }) when building DisableKinesisStreamingDestinationOutput. That member must be left None for this operation (it stays populated for EnableKinesisStreamingDestination). No state change and no status/error change is involved; the destination lifecycle (DISABLING then settling to DISABLED) and the DescribeKinesisStreamingDestination view are correct and must keep reporting the enabled precision.

Traces: dynamodb.disable-kinesis-streaming-destination-omits-configuration (assertion at check line 77: "expected DisableKinesisStreamingDestination's response to omit EnableKinesisStreamingConfiguration").

## Evidence
Failing check: dynamodb.disable-kinesis-streaming-destination-omits-configuration [mismatch]

Expected (real AWS): DisableKinesisStreamingDestination -> HTTP 200, body members TableName, StreamArn, DestinationStatus only; NO EnableKinesisStreamingConfiguration member, even though the destination was enabled with ApproximateCreationDateTimePrecision=MICROSECOND. EnableKinesisStreamingDestination, by contrast, DOES echo EnableKinesisStreamingConfiguration.

Actual (emulator): HTTP 200 with a fourth member present:
  AssertionError: expected DisableKinesisStreamingDestination's response to omit EnableKinesisStreamingConfiguration, got {'TableName': 'sim2real-behav-kds-tbl-...', 'StreamArn': 'arn:aws:kinesis:us-west-2:000000000000:stream/sim2real-behav-kds-...', 'DestinationStatus': 'DISABLING', 'EnableKinesisStreamingConfiguration': {'ApproximateCreationDateTimePrecision': 'MICROSECOND'}, ...}
(The table name, stream ARN, region/account and request id above are per-run values, not requirements.)

Smithy model (services/service-com-amazonaws-dynamodb-dynamodb-20120810/smithy-model/com-amazonaws-dynamodb-dynamodb-20120810.json): DisableKinesisStreamingDestination output_shape = KinesisStreamingDestinationOutput, members TableName / StreamArn / DestinationStatus / EnableKinesisStreamingConfiguration, all optional — shared with EnableKinesisStreamingDestination, so the model does not forbid the member; the omission on Disable is recorded behavior only.

Source: services/service-com-amazonaws-dynamodb-dynamodb-20120810/src/ops/disable_kinesis_streaming_destination.rs lines 225-236 build the output with `enable_kinesis_streaming_configuration: Some(EnableKinesisStreamingConfiguration { approximate_creation_date_time_precision: in_force })` unconditionally.

Related open issues that do NOT cover this (different causes, same operation family): 20261103T022510Z-dynamodb-kinesis-streaming-destinations-invalid-arns-are-refused.md and 20261108T092824Z-dynamodb-kinesis-an-enable-failed-kinesis-streaming-destination.md — those concern ENABLE_FAILED settling and DestinationStatusDescription, not the member set of the Disable response.

## Steps to Reproduce
1. Create a Kinesis stream (ON_DEMAND) and wait for StreamStatus=ACTIVE.
2. CreateTable with a single HASH key, BillingMode=PAY_PER_REQUEST; wait for TableStatus=ACTIVE.
3. EnableKinesisStreamingDestination(TableName=<table>, StreamArn=<stream arn>, EnableKinesisStreamingConfiguration={ApproximateCreationDateTimePrecision: 'MICROSECOND'}). Observe the response DOES echo EnableKinesisStreamingConfiguration (correct; positive control).
4. Poll DescribeKinesisStreamingDestination until that destination reports DestinationStatus=ACTIVE with ApproximateCreationDateTimePrecision=MICROSECOND.
5. DisableKinesisStreamingDestination(TableName=<table>, StreamArn=<stream arn>).
6. Inspect the 200 response body: it contains an EnableKinesisStreamingConfiguration member. Real AWS returns only TableName, StreamArn and DestinationStatus.

Replay: run_behavioral_suite and re-run check dynamodb.disable-kinesis-streaming-destination-omits-configuration.
