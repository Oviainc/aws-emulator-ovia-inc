Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: firehose.encryption-status-default-disabled firehose.e2e-encryption-enable-put-disable-flow firehose.extended-s3-destination-configuration-defaults
Issue-Shape-Id: com.amazonaws.firehose#Firehose_20150804
Issue-Key: firehose.encryption-status-default-disabled|run#3|KeyError|cfg = desc["DeliveryStreamEncryptionConfiguration"]
Issue-Key: firehose.e2e-encryption-enable-put-disable-flow|_encryption_status#10|KeyError|return desc["DeliveryStreamEncryptionConfiguration"]["Status"]
Issue-Key: firehose.extended-s3-destination-configuration-defaults|run#12|AssertionError|assert ext.get("S3BackupMode") == "Disabled", f"expected default S3BackupMode=Disabled, got {ext.get('S3BackupMode')}"
Issue-Commit: 360c4c1618594432a64f84ba94d0fa368ed8f619

# Firehose DescribeDeliveryStream omits DeliveryStreamEncryptionConfiguration and destination defaults (2 causes)

> _Note: this issue was identified against the code base at commit `360c4c1618594432a64f84ba94d0fa368ed8f619`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDeliveryStream

## Description
DescribeDeliveryStream's DeliveryStreamDescription is serialized without members real Firehose always populates, so clients that read them raise KeyError / see None. Two root causes, both in the firehose service (bootstrapped as service-com-amazonaws-firehose-firehose-20150804).

Cause 1 of 2: DeliveryStreamEncryptionConfiguration is never emitted, not even in its default disabled form.
AWS: Smithy: DeliveryStreamDescription.DeliveryStreamEncryptionConfiguration targets DeliveryStreamEncryptionConfiguration — "Indicates the server-side encryption (SSE) status for the Firehose stream"; that shape's doc states "You can get DeliveryStreamEncryptionConfiguration by invoking the DescribeDeliveryStream operation", and its KeyType member documents "The default setting is Amazon Web Services_OWNED_CMK". Recorded behavior: DescribeDeliveryStream on a stream that has never had encryption started still returns the DeliveryStreamEncryptionConfiguration structure carrying a Status (the disabled state), so the member is readable without first calling StartDeliveryStreamEncryption.
Emulator: the member is absent from the response entirely; reading description['DeliveryStreamEncryptionConfiguration'] raises KeyError both on a freshly created stream and in the enable/put/disable flow.
Why: any client that inspects SSE state — a console view, a compliance sweep, or code that decides whether to call StartDeliveryStreamEncryption — crashes or concludes "unknown" instead of reading the documented default status.
Fix: the DescribeDeliveryStream output serializer and the delivery-stream state record must always carry an encryption-configuration block, defaulting to the disabled status for a stream that was never encrypted.
Traces: firehose.encryption-status-default-disabled, firehose.e2e-encryption-enable-put-disable-flow.

Cause 2 of 2: ExtendedS3DestinationDescription omits the defaulted S3BackupMode.
AWS: Smithy: ExtendedS3DestinationDescription.S3BackupMode targets the S3BackupMode enum. Recorded behavior: for a stream created with an ExtendedS3DestinationConfiguration that does not specify S3BackupMode, DescribeDeliveryStream reports S3BackupMode='Disabled' — the service materializes the default into the description rather than leaving the member unset.
Emulator: the member is absent (reads as None), so no default is materialized. Other members defaulted by the same path (the model marks CompressionFormat, BufferingHints and EncryptionConfiguration @required in the description shape) should be reviewed with it.
Why: a client that round-trips a description back into UpdateDestination, or that compares desired vs actual configuration, sees a missing field where AWS reports an explicit value — producing spurious drift and, on a round-trip, dropping the backup mode.
Fix: the CreateDeliveryStream handler / delivery-stream state must apply destination-config defaults at admission, and the DescribeDeliveryStream serializer must emit them.
Traces: firehose.extended-s3-destination-configuration-defaults.

## Evidence
Cause 1 — expected: DeliveryStreamDescription contains DeliveryStreamEncryptionConfiguration with a disabled Status. Actual: firehose.encryption-status-default-disabled — KeyError: 'DeliveryStreamEncryptionConfiguration'; firehose.e2e-encryption-enable-put-disable-flow — KeyError: 'DeliveryStreamEncryptionConfiguration' (check line 45).

Cause 2 — expected: ExtendedS3DestinationDescription.S3BackupMode == 'Disabled'. Actual: firehose.extended-s3-destination-configuration-defaults — "AssertionError: expected default S3BackupMode=Disabled, got None" (assert ext.get("S3BackupMode") == "Disabled").

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the behavioral suite.
2. Cause 1: CreateDeliveryStream for a DirectPut stream with an ExtendedS3DestinationConfiguration, wait for ACTIVE, then call DescribeDeliveryStream and read DeliveryStreamDescription['DeliveryStreamEncryptionConfiguration'] — observe the member is missing instead of present with a disabled Status.
3. Cause 2: CreateDeliveryStream with an ExtendedS3DestinationConfiguration that omits S3BackupMode, then call DescribeDeliveryStream and read Destinations[0]['ExtendedS3DestinationDescription']['S3BackupMode'] — observe it is absent instead of 'Disabled'.
