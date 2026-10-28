Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: firehose.start-encryption-already-enabling-conflict-rejected
Issue-Shape-Id: com.amazonaws.firehose#Firehose_20150804
Issue-Key: firehose|StartDeliveryStreamEncryption|ResourceInUseException
Issue-Commit: 3d1fad8f4871dd362a3fc79086918715269856ce

# StartDeliveryStreamEncryption invents the ResourceInUseException message for a concurrent encryption change (1 cause)

> _Note: this issue was identified against the code base at commit `3d1fad8f4871dd362a3fc79086918715269856ce`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartDeliveryStreamEncryption

## Description
One net-new root cause in the firehose service, found by replaying the behavioral suite. firehose is reported bootstrapped by get_bootstrap_status (service-com-amazonaws-firehose-firehose-20150804), so this is a fidelity defect, not a bootstrap gap.

Related but NOT covering: the fixed issue 20261022T151017Z-firehose-argument-validation-admits-invalid-input-and-misclassifies.md covers StartDeliveryStreamEncryption's KeyARN/KeyType argument dependency (InvalidArgumentException), and 20261022T064119Z-firehose-startdeliverystreamencryption-stub.md covers the operation existing at all. Neither touches the ResourceInUseException message emitted for a conflicting in-flight encryption change; the operation is implemented and the refusal is correctly CLASSIFIED here, so this is not a reopened refusal — only the message text diverges.

Cause 1 of 1: the ResourceInUseException message for a second StartDeliveryStreamEncryption while the first change is still ENABLING is emulator-invented prose instead of AWS's wording.

AWS: Smithy: StartDeliveryStreamEncryption models ResourceInUseException (errorKind client, HTTP 400, documentation 'The resource is already in use and not available for this operation.'); the operation's documentation states 'Firehose first sets the encryption status of the stream to ENABLING, and then to ENABLED' and that the call is asynchronous, so a second start issued while the first is unsettled collides with an in-flight change. The model documents the error identity but not the message, so for the text: recorded behavior only — the recording shows HTTP 400, code ResourceInUseException, message 'Cannot enable encryption on firehose: <DeliveryStreamName> when encryption is in <EncryptionStatus> status'. Fixed text of that template: 'Cannot enable encryption on firehose: ' then ' when encryption is in ' then ' status'. Variable parts: <DeliveryStreamName> is taken verbatim from the request's DeliveryStreamName member, and <EncryptionStatus> is the stream's current DeliveryStreamEncryptionConfiguration.Status from emulator state (ENABLING in this recording; the same template carries whatever unsettled status the stream is in). The concrete stream name in the recording (sim2real-fh-enc-45fdcbc8b083) is a per-run generated name that fills the template and is not part of it.

Emulator: HTTP 400 with code ResourceInUseException (correct) but message 'server-side encryption for Firehose stream <name> is <status> and cannot be changed until that change settles' — e.g. 'server-side encryption for Firehose stream sim2real-fh-enc-45fdcbc8b083 is ENABLING and cannot be changed until that change settles'. None of AWS's fixed text is present: the sentence is differently worded, differently ordered, and does not use the 'Cannot enable encryption on firehose: ' / ' when encryption is in ' / ' status' frame.

Why: the error message is part of the response contract. A client, test harness or operator runbook that matches or logs AWS's documented refusal text for an encryption-change conflict sees text that no real Firehose endpoint produces, so message-based assertions and log-based alerting that pass against AWS fail against the emulator (and vice versa — an emulator-tuned matcher breaks in production). It also masks the conflict for anyone reading the message to learn which status is blocking, since the phrasing does not match any AWS-documented string.

Fix: the StartDeliveryStreamEncryption handler's in-flight-change refusal branch — the ResourceInUseException it raises when the stream's encryption change has not settled — must format AWS's message template, substituting the request's DeliveryStreamName and the stream's current encryption status; the same wording rule belongs with the handler's other encryption-conflict refusals so the operation does not carry two message vocabularies.

Traces: behavioral check firehose.start-encryption-already-enabling-conflict-rejected. The invented text is produced in services/service-com-amazonaws-firehose-firehose-20150804/src/ops/start_delivery_stream_encryption.rs lines 257-264 (the `stream.encryption_change_in_flight()` branch calling the local `in_use` helper at line 312).

## Evidence
Check firehose.start-encryption-already-enabling-conflict-rejected [mismatch]:

  AssertionError: ResourceInUseException message 'server-side encryption for Firehose stream sim2real-fh-enc-45fdcbc8b083 is ENABLING and cannot be changed until that change settles', expected 'Cannot enable encryption on firehose: sim2real-fh-enc-45fdcbc8b083 when encryption is in ENABLING status'

HTTP status (400) and error code (ResourceInUseException) agree with AWS; the message is the sole divergence.

Expected message template (AWS): 'Cannot enable encryption on firehose: {DeliveryStreamName} when encryption is in {EncryptionStatus} status'
  - fixed text: 'Cannot enable encryption on firehose: ', ' when encryption is in ', ' status'
  - from the request: {DeliveryStreamName}
  - from emulator/service state: {EncryptionStatus} (the stream's current DeliveryStreamEncryptionConfiguration.Status, ENABLING in this run)

Actual message template (emulator): 'server-side encryption for Firehose stream {name} is {status} and cannot be changed until that change settles'

The stream name shown is a per-run generated value, not a requirement.

Source of the actual text: services/service-com-amazonaws-firehose-firehose-20150804/src/ops/start_delivery_stream_encryption.rs:257-264.

## Steps to Reproduce
1. Clone the repo and boot the emulator from this checkout, then replay the behavioral suite.
2. Replay check firehose.start-encryption-already-enabling-conflict-rejected, or reproduce by hand:
   a. Create an S3 bucket and an IAM role trusted by firehose.amazonaws.com.
   b. CreateDeliveryStream with ExtendedS3DestinationConfiguration (RoleARN + BucketARN) and wait for DeliveryStreamStatus=ACTIVE.
   c. Call StartDeliveryStreamEncryption with DeliveryStreamEncryptionConfigurationInput={'KeyType': 'AWS_OWNED_CMK'}.
   d. Call StartDeliveryStreamEncryption again immediately, while DescribeDeliveryStream still reports DeliveryStreamEncryptionConfiguration.Status = ENABLING.
3. Read Error.Code and Error.Message of the second call. The code is ResourceInUseException (correct); the message is 'server-side encryption for Firehose stream <name> is ENABLING and cannot be changed until that change settles' instead of 'Cannot enable encryption on firehose: <name> when encryption is in ENABLING status'.
