Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: firehose.processing-configuration-parameter-normalization-and-update-replaces-block firehose.put-record-rejected-while-stream-creating
Issue-Shape-Id: com.amazonaws.firehose#Firehose_20150804
Issue-Key: firehose.processing-configuration-parameter-normalization-and-update-replaces-block|run#13|AssertionError|assert sorted(names) == sorted(
Issue-Key: firehose.put-record-rejected-while-stream-creating|run#11|AssertionError|assert msg == f"Firehose {name} is not active in {ctx.account} account.", (
Issue-Commit: 9fe95bf5a6f42fe93a77d740b9cb51055f43a772

# Firehose ProcessingConfiguration echo omits the service-filled processor parameters, and PutRecord's not-active refusal message is built by the wrong rule (2 causes)

> _Note: this issue was identified against the code base at commit `9fe95bf5a6f42fe93a77d740b9cb51055f43a772`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDeliveryStream

## Description
Two net-new root causes behind firehose.* behavioral failures that are not marked ALREADY FILED and are not OWNED by an open issue's replay key.

Cause 1 of 2: the processor parameter list is echoed exactly as submitted — the parameters Firehose fills in itself (NumberOfRetries and RoleArn) are never materialised.
AWS: Smithy ProcessorParameter.ParameterName targets the ProcessorParameterName enum (LambdaArn, NumberOfRetries, MetadataExtractionQuery, JsonParsingEngine, RoleArn, BufferSizeInMBs, BufferIntervalInSeconds, ...) and its documentation states the service's defaults: "Currently the following default values are supported: 3 for NumberOfRetries and 60 for the BufferIntervalInSeconds". Recorded behavior: after CreateDeliveryStream with a Lambda processor whose Parameters carry only LambdaArn, BufferSizeInMBs and BufferIntervalInSeconds, DescribeDeliveryStream returns that processor with FIVE parameters, each exactly once — LambdaArn (echoed), NumberOfRetries ('3', defaulted), RoleArn (stamped to the delivery stream's own RoleARN), BufferSizeInMBs and BufferIntervalInSeconds (echoed) — in the service's canonical order.
Emulator: the describe echo returns only the three submitted parameters, ['BufferIntervalInSeconds', 'BufferSizeInMBs', 'LambdaArn'] — NumberOfRetries and RoleArn are absent, and the list is in submission order rather than the canonical one.
Why: a client cannot read back the retry count or the role the processor actually runs under, so it cannot verify or reason about its transform's effective configuration; IaC tools that diff the described configuration against the declared one see permanent spurious drift and keep re-applying updates. Code that dereferences the documented defaults raises instead of reading them.
Fix: ProcessingConfiguration normalisation on the CreateDeliveryStream/UpdateDestination path (fill NumberOfRetries from the documented default and RoleArn from the stream's delivery role when the caller omits them, and emit the parameters in the ProcessorParameterName order) plus the ProcessingConfiguration serializer on the describe path, in the Firehose service crate. The open issue 20261022T194351Z-firehose-control-plane-configuration-echo-drops-members-tag.md (Cause 3) describes only the ORDERING half of this check and carries a stale replay key for it, so it does not cover the missing service-filled parameters.
Traces: firehose.processing-configuration-parameter-normalization-and-update-replaces-block (services/service-com-amazonaws-firehose-firehose-20150804/src/ops/create_delivery_stream.rs, update_destination.rs and the DescribeDeliveryStream destination serializer).

Cause 2 of 2: PutRecord against a stream that is not ACTIVE is refused with the right code but a re-synthesized message.
AWS: recorded behavior only — PutRecord on a stream still in CREATING is refused with HTTP 400 and error code ResourceNotFoundException (Smithy: PutRecord models ResourceNotFoundException, client, HTTP 400), and the message is produced by a fixed request-parameterized template: 'Firehose <DeliveryStreamName> is not active in <accountId> account.' The rule — the stream name taken from the request and the caller's account id — is the requirement, not the recorded sentence.
Emulator: the same condition is detected and answered with the same status and error code, but with freshly worded prose that names neither the account nor AWS's wording (example: 'Firehose stream sim2real-behav-fh-creating-put-0214663e4043 is in status CREATING; it accepts data only in status ACTIVE').
Why: the account id AWS reports is dropped, so the message carries different information from AWS's; clients, test suites and operators that match on Firehose's data-plane not-active text see a different sentence than AWS produces for the identical condition.
Fix: render this refusal from Firehose's own message template next to the error constructor in the PutRecord/PutRecordBatch not-active guard of the Firehose service crate, parameterized by the request's DeliveryStreamName and the request's account id. The open issue 20261022T194351Z-firehose-control-plane-configuration-echo-drops-members-tag.md (Cause 9) centralises the CONTROL-plane message templates and does not carry this data-plane check; the fixed issue 20261022T064119Z-firehose-rejection-messages-are-built-by-the-wrong.md covered a different set of templates.
Traces: firehose.put-record-rejected-while-stream-creating (services/service-com-amazonaws-firehose-firehose-20150804/src/ops/put_record.rs).

## Evidence
Cause 1 — expected the described processor's parameter names to be exactly {LambdaArn, NumberOfRetries, RoleArn, BufferSizeInMBs, BufferIntervalInSeconds}, each once, with NumberOfRetries == '3' and RoleArn == the stream's delivery RoleARN; actual AssertionError: "the service must normalize the processor's parameters to LambdaArn, NumberOfRetries, RoleArn, BufferSizeInMBs and BufferIntervalInSeconds, each once, got ['BufferIntervalInSeconds', 'BufferSizeInMBs', 'LambdaArn']".
Cause 2 — PutRecord on a CREATING stream: status 400 and code ResourceNotFoundException match AWS; message expected to be built as 'Firehose <name> is not active in <account> account.', actual 'Firehose stream sim2real-behav-fh-creating-put-0214663e4043 is in status CREATING; it accepts data only in status ACTIVE' (AssertionError: expected the exact ResourceNotFoundException message AWS returns). The generated stream name in both strings is environment-specific and is not part of the requirement; the template's shape is.

## Steps to Reproduce
1. Boot the emulator from this checkout and point botocore at it; fh = client('firehose').
2. Cause 1: create a bucket, a Firehose delivery role and a Lambda function; fh.create_delivery_stream(DeliveryStreamName=n, ExtendedS3DestinationConfiguration={'RoleARN': role, 'BucketARN': arn, 'BufferingHints': {...}, 'ProcessingConfiguration': {'Enabled': True, 'Processors': [{'Type': 'Lambda', 'Parameters': [{'ParameterName': 'BufferIntervalInSeconds', 'ParameterValue': '70'}, {'ParameterName': 'BufferSizeInMBs', 'ParameterValue': '2'}, {'ParameterName': 'LambdaArn', 'ParameterValue': fn_arn}]}]}}); wait for ACTIVE and fh.describe_delivery_stream(DeliveryStreamName=n) — expect five parameters including NumberOfRetries='3' and RoleArn=<delivery role>; observe only the three submitted ones.
3. Cause 2: fh.create_delivery_stream(...) and WITHOUT waiting for ACTIVE call fh.put_record(DeliveryStreamName=n, Record={'Data': b'too-early'}) — expect 400 ResourceNotFoundException with the message built as 'Firehose <n> is not active in <account> account.'; observe the re-worded CREATING/ACTIVE sentence.
4. Re-replay firehose.processing-configuration-parameter-normalization-and-update-replaces-block and firehose.put-record-rejected-while-stream-creating.
