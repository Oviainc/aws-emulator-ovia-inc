Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: firehose.create-stream-processing-configuration-validated
Issue-Shape-Id: com.amazonaws.firehose#Firehose_20150804
Issue-Key: firehose.create-stream-processing-configuration-validated|_err#6|AssertionError|raise AssertionError(f"the call succeeded where a ClientError was expected: {result!r}")
Issue-Commit: ebea418ed7238d7557ed4699ab6be023240b4505

# firehose: CreateDeliveryStream does not validate ProcessingConfiguration — a Lambda processor with no LambdaArn parameter is accepted with HTTP 200 (1 cause)

> _Note: this issue was identified against the code base at commit `ebea418ed7238d7557ed4699ab6be023240b4505`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDeliveryStream

## Description
One net-new root cause on the Firehose control plane. No open issue covers it: 20261022T064119Z-firehose-createdeliverystream-putrecordbatch-are-overly-permissive-and-misclassify.md covers CompressionFormat, bucket-ARN shape and PutRecordBatch size permissiveness, and 20261029T175617Z-firehose-lambda-s3-the-processingconfiguration-lambda-processor-is.md covers the DATA-PLANE invocation of an already-accepted processor — neither covers create-time validation of the ProcessingConfiguration structure itself.

Cause 1 of 1: CreateDeliveryStream accepts an ExtendedS3 destination whose ProcessingConfiguration declares a Lambda processor with no LambdaArn parameter, instead of refusing the request.
AWS: Smithy: CreateDeliveryStream takes ExtendedS3DestinationConfiguration.ProcessingConfiguration{Enabled, Processors:[Processor{Type: ProcessorType, Parameters}]} and is modeled with InvalidArgumentException (errorKind client, HTTP 400). ProcessorType is an enum whose members are RecordDeAggregation, Decompression, CloudWatchLogProcessing, Lambda, MetadataExtraction, AppendDelimiterToRecord. Real Firehose validates the processor synchronously and refuses a Lambda processor that carries no LambdaArn parameter with InvalidArgumentException (HTTP 400) and the fixed message `LambdaArn is required when Lambda processor is used.` (no variable part), and the refused create leaves NO delivery stream behind (a later DescribeDeliveryStream on that name is ResourceNotFoundException). A Processor whose Type is outside the modeled enum is refused one layer earlier, by the generic request validator, with ValidationException whose message is the standard constraint sentence `1 validation error detected: Value at '<member path>' failed to satisfy constraint: Member must satisfy enum value set: [Decompression, Lambda, RecordDeAggregation, MetadataExtraction, AppendDelimiterToRecord, CloudWatchLogProcessing]` — the member path is taken from the request (here extendedS3DestinationConfiguration.processingConfiguration.processors.1.member.type) and the value set is the ProcessorType enum; the two refusals must not share one code.
Emulator: the create SUCCEEDS — HTTP 200 with a DeliveryStreamARN (arn:aws:firehose:<region>:<account>:deliverystream/<name>) for the request whose only processor is {"Type": "Lambda"} with no Parameters. Because leg 1 returned 200, the check aborted there and the out-of-enum ProcessorType leg was never reached, so the emulator's behavior on that leg is unknown; the requirement above is what it must satisfy once leg 1 refuses.
Why: a client that mis-specifies its transform gets an ACTIVE stream that can never invoke a transform; the authoritative create-time signal naming the missing LambdaArn is lost, and the stream silently delivers untransformed records forever. A client cannot tell a structural request error (ValidationException) from Firehose's semantic verdict (InvalidArgumentException) either.
Fix: the CreateDeliveryStream handler's destination validation (services/service-com-amazonaws-firehose-firehose-20150804/src/ops/create_delivery_stream.rs) must validate every ProcessingConfiguration.Processors entry before any state is created — a Lambda processor requires a LambdaArn parameter (InvalidArgumentException with AWS's sentence), an unmodeled Type is a generic enum ValidationException — and a refused create must not persist a delivery stream.
Traces: firehose.create-stream-processing-configuration-validated (check line 335, leg 1).

## Evidence
Cause 1 — firehose.create-stream-processing-configuration-validated:
  request:  CreateDeliveryStream(DeliveryStreamName=<name>, ExtendedS3DestinationConfiguration={RoleARN, BucketARN, BufferingHints{SizeInMBs:1, IntervalInSeconds:60}, ProcessingConfiguration={"Enabled": true, "Processors": [{"Type": "Lambda"}]}})
  expected: ClientError InvalidArgumentException, HTTP 400, Message exactly 'LambdaArn is required when Lambda processor is used.' and no stream created
  actual:   HTTP 200, {'DeliveryStreamARN': 'arn:aws:firehose:us-west-2:000000000000:deliverystream/sim2real-behav-fh-noarn-proc-<suffix>'} (the ARN's account/region/suffix are per-run values; the divergence is the 200 itself)
  AssertionError: the call succeeded where a ClientError was expected (check helper _err, check line 71 raised from line 335)
  Not exercised because of the above: the second leg asserts an out-of-enum ProcessorType is refused with ValidationException and the enum-value-set message, and the control leg asserts the identical create with ProcessingConfiguration {Enabled: false, Processors: []} is accepted and echoed by DescribeDeliveryStream.

## Steps to Reproduce
1. Create an S3 bucket and an IAM role trusted by firehose.amazonaws.com with the usual s3 delivery permissions on that bucket.
2. Call CreateDeliveryStream with DeliveryStreamName=<n1> and ExtendedS3DestinationConfiguration={RoleARN:<role>, BucketARN:'arn:aws:s3:::<bucket>', BufferingHints:{SizeInMBs:1, IntervalInSeconds:60}, ProcessingConfiguration:{'Enabled':true,'Processors':[{'Type':'Lambda'}]}} — it must fail InvalidArgumentException (HTTP 400) with Message 'LambdaArn is required when Lambda processor is used.'
3. DescribeDeliveryStream(<n1>) must raise ResourceNotFoundException: the refused create must leave nothing behind.
4. Repeat with Processors=[{'Type':'NotAProcessor','Parameters':[{'ParameterName':'LambdaArn','ParameterValue':'x'}]}] — it must fail ValidationException (a different code from step 2) with the '1 validation error detected: Value at '...processingConfiguration.processors.1.member.type' failed to satisfy constraint: Member must satisfy enum value set: [...]' message.
5. Repeat with ProcessingConfiguration={'Enabled':false,'Processors':[]} — this must be accepted, reach ACTIVE, and DescribeDeliveryStream must echo ProcessingConfiguration.Enabled=false.
