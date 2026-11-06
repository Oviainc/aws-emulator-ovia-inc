Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: firehose.create-stream-processing-configuration-validated
Issue-Shape-Id: com.amazonaws.firehose#Firehose_20150804
Issue-Key: firehose.create-stream-processing-configuration-validated|run#13|AssertionError|assert msg2 == "1 validation error detected: Value at 'extendedS3DestinationConfiguration.processingConfiguration.processors.1.member.type' failed to satisfy co
Issue-Commit: 833ea3b7ebe3cc8f31843ce2d010e71f4791a4d1

# firehose: the enum-constraint ValidationException for an out-of-enum ProcessorType lists the ProcessorType value set in the wrong order (1 cause)

> _Note: this issue was identified against the code base at commit `833ea3b7ebe3cc8f31843ce2d010e71f4791a4d1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDeliveryStream

## Description
One net-new root cause on the Firehose control plane, surviving after the create-time ProcessingConfiguration validation landed. No open issue covers it: 20261102T100157Z-firehose-createdeliverystream-does-not-validate-processingconfiguration-a-lambda.md was filed on leg 1 of the same check (a Lambda processor with no LambdaArn parameter accepted with HTTP 200) and that leg now passes — the refusal exists, and the surviving divergence is only the wording of leg 2's enum-constraint sentence; 20261029T175617Z-firehose-lambda-s3-the-processingconfiguration-lambda-processor-is.md covers the data-plane invocation of an accepted processor, not this message. The suite did not mark this check as owned by either.

Cause 1 of 1: the generic request-validator refusal for a Processor whose Type is outside the modeled enum renders the ProcessorType value set in the Smithy model's member-declaration order instead of the order AWS emits, so the ValidationException message does not match AWS byte-for-byte.
AWS: Smithy: CreateDeliveryStream input ExtendedS3DestinationConfiguration.ProcessingConfiguration{Enabled, Processors:[Processor{Type: ProcessorType, Parameters}]}; com.amazonaws.firehose#ProcessorType is an enum with members RecordDeAggregation, Decompression, CloudWatchLogProcessing, Lambda, MetadataExtraction, AppendDelimiterToRecord. A Type outside that set is refused by the generic request validator — not by Firehose's own InvalidArgumentException layer — with ValidationException (HTTP 400) and the standard constraint sentence. Recorded behavior: the message is "1 validation error detected: Value at '<member path>' failed to satisfy constraint: Member must satisfy enum value set: [Decompression, Lambda, RecordDeAggregation, MetadataExtraction, AppendDelimiterToRecord, CloudWatchLogProcessing]". Fixed text: everything except <member path>; the member path is the only part taken from the request (here extendedS3DestinationConfiguration.processingConfiguration.processors.1.member.type, i.e. the lowerCamel member path with the 1-based list index and the '.member.' hop). The bracketed value set is fixed text for ProcessorType, and AWS's order is NOT the model's declaration order.
Emulator: returns the right code (ValidationException) and the right member path, but the bracketed value set in the model's declaration order: "1 validation error detected: Value at 'extendedS3DestinationConfiguration.processingConfiguration.processors.1.member.type' failed to satisfy constraint: Member must satisfy enum value set: [RecordDeAggregation, Decompression, CloudWatchLogProcessing, Lambda, MetadataExtraction, AppendDelimiterToRecord]".
Why: the HTTP status, error code and error message are all requirements. A client (or a test harness, log scraper or SDK-level error mapper) that matches or displays AWS's exact constraint sentence sees a message AWS never emits; the enumerated value set it would show a user is in an order that does not reproduce, so the refusal is not interchangeable with the real one.
Fix: the enum-constraint branch of the generic request validator / its ValidationException message serializer for the Firehose service crate — the component that renders "Member must satisfy enum value set: [...]" — must emit the ProcessorType values in AWS's order rather than iterating the Smithy enum's member order; the member-path rendering is already correct.
Traces: firehose.create-stream-processing-configuration-validated (leg 2, check assertion at check line 378).

## Evidence
Cause 1 — firehose.create-stream-processing-configuration-validated (leg 2):
  request:  CreateDeliveryStream(DeliveryStreamName=<n2>, ExtendedS3DestinationConfiguration={RoleARN:<role>, BucketARN:'arn:aws:s3:::<bucket>', BufferingHints:{SizeInMBs:1, IntervalInSeconds:60}, ProcessingConfiguration:{'Enabled':true,'Processors':[{'Type':'NotAProcessor','Parameters':[{'ParameterName':'LambdaArn','ParameterValue':'x'}]}]}})
  expected: ClientError ValidationException, HTTP 400, Message exactly:
            1 validation error detected: Value at 'extendedS3DestinationConfiguration.processingConfiguration.processors.1.member.type' failed to satisfy constraint: Member must satisfy enum value set: [Decompression, Lambda, RecordDeAggregation, MetadataExtraction, AppendDelimiterToRecord, CloudWatchLogProcessing]
  actual:   ClientError ValidationException with Message:
            1 validation error detected: Value at 'extendedS3DestinationConfiguration.processingConfiguration.processors.1.member.type' failed to satisfy constraint: Member must satisfy enum value set: [RecordDeAggregation, Decompression, CloudWatchLogProcessing, Lambda, MetadataExtraction, AppendDelimiterToRecord]
  AssertionError: expected the exact ValidationException message AWS returns, got "...[RecordDeAggregation, Decompression, CloudWatchLogProcessing, Lambda, MetadataExtraction, AppendDelimiterToRecord]" (check line 378)
  Diff is ONLY the ordering of the six values inside the brackets: the emulator uses the Smithy declaration order of com.amazonaws.firehose#ProcessorType (RecordDeAggregation, Decompression, CloudWatchLogProcessing, Lambda, MetadataExtraction, AppendDelimiterToRecord); AWS emits Decompression, Lambda, RecordDeAggregation, MetadataExtraction, AppendDelimiterToRecord, CloudWatchLogProcessing. The code (ValidationException), the status and the member path all match; the leg-1 InvalidArgumentException refusal ('LambdaArn is required when Lambda processor is used.') and the no-stream-left-behind assertions now pass, and the check proceeds to this assertion.

## Steps to Reproduce
1. Create an S3 bucket and an IAM role trusted by firehose.amazonaws.com with the usual s3 delivery permissions on that bucket.
2. Call CreateDeliveryStream with DeliveryStreamName=<n2> and ExtendedS3DestinationConfiguration={RoleARN:<role>, BucketARN:'arn:aws:s3:::<bucket>', BufferingHints:{SizeInMBs:1, IntervalInSeconds:60}, ProcessingConfiguration:{'Enabled':true,'Processors':[{'ParameterName-less bad type'}]} — concretely Processors=[{'Type':'NotAProcessor','Parameters':[{'ParameterName':'LambdaArn','ParameterValue':'x'}]}].
3. Observe the ClientError: the Code must be ValidationException (HTTP 400).
4. Compare Error.Message byte-for-byte with: "1 validation error detected: Value at 'extendedS3DestinationConfiguration.processingConfiguration.processors.1.member.type' failed to satisfy constraint: Member must satisfy enum value set: [Decompression, Lambda, RecordDeAggregation, MetadataExtraction, AppendDelimiterToRecord, CloudWatchLogProcessing]". The emulator's message differs: the six enum values appear in the model's declaration order.
5. DescribeDeliveryStream(<n2>) must still raise ResourceNotFoundException (this already holds).
