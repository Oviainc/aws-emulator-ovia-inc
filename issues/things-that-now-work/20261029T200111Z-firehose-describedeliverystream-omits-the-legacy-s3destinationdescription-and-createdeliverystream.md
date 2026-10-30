Issue-Source: trace-sweep-divergence
Issue-Trace: describe-delivery-stream-initial-shape
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: describe-delivery-stream-initial-shape firehose-create-delivery-stream-argument-validation
Issue-Shape-Id: com.amazonaws.firehose#Firehose_20150804
Issue-Key: firehose|DescribeDeliveryStream|2xx->2xx|$.DeliveryStreamDescription.Destinations[*].S3DestinationDescription
Issue-Key: firehose|CreateDeliveryStream|4xx->4xx|$.message
Issue-Commit: c912e232849d0cf345219eaaac304bd62b7f7616

# firehose: DescribeDeliveryStream omits the legacy S3DestinationDescription and CreateDeliveryStream's no-destination refusal is misworded (2 causes)

> _Note: this issue was identified against the code base at commit `c912e232849d0cf345219eaaac304bd62b7f7616`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDeliveryStream

## Description
Two net-new root causes in the firehose service, both wire divergences from the recorded corpus. The service is bootstrapped (get_bootstrap_status reports service-com-amazonaws-firehose-firehose-20150804), so these are fidelity defects, not bootstrap gaps.

Cause 1 of 2 (DescribeDeliveryStream): the rendered destination carries no legacy S3DestinationDescription mirror.
AWS: Smithy (com.amazonaws.firehose, DescribeDeliveryStream -> DeliveryStreamDescription.Destinations[*] : DestinationDescription) models DestinationDescription$S3DestinationDescription ("[Deprecated] The destination in Amazon S3") alongside the variant-specific ExtendedS3DestinationDescription; shape S3DestinationDescription requires RoleARN, BucketARN, BufferingHints, CompressionFormat and EncryptionConfiguration and additionally carries Prefix, ErrorOutputPrefix and CloudWatchLoggingOptions. Recorded behavior: a DescribeDeliveryStream on a stream with an S3-backed destination returns, inside Destinations[0], a fully populated S3DestinationDescription whose members are the destination's effective configuration — the destination RoleARN and BucketARN as supplied, and the documented defaults materialised where the create left them out (BufferingHints IntervalInSeconds 300 / SizeInMBs 5, CompressionFormat UNCOMPRESSED, EncryptionConfiguration {NoEncryptionConfig: NoEncryption}, CloudWatchLoggingOptions {Enabled: false}). The ARNs, the bucket name and the role name are per-environment values that fill that template; the presence of the member and its member set are the requirement.
Emulator: Destinations[0].S3DestinationDescription is absent (null) in the 200 response — only the variant-specific description is emitted. Every other member of the description matched, so the omission is specific to the legacy mirror.
Why: SDK clients and tools that read a stream's S3 destination through the long-standing DestinationDescription.S3DestinationDescription member (the member that predates ExtendedS3DestinationDescription, and the one Firehose has always reported) get None against the emulator and a populated structure against AWS. Code that round-trips a description back into UpdateDestination, or that asserts on the destination bucket/role/buffering it reads from that member, sees an unconfigured destination and either crashes on the missing structure or concludes the stream has no S3 destination at all.
Fix: the DescribeDeliveryStream handler's destination-rendering stage — the per-variant match that builds each DestinationDescription in the operation's handler module — must populate the legacy S3DestinationDescription mirror for an S3-backed destination in addition to the variant-specific description, filling it from the same effective (default-materialised) destination configuration the variant description is rendered from.
Traces: describe-delivery-stream-initial-shape#task#4.

Cause 2 of 2 (CreateDeliveryStream): the refusal for a create that names NO destination configuration uses the emulator's own sentence instead of Firehose's.
AWS: Smithy models CreateDeliveryStream's client errors as InvalidArgumentException (HTTP 400, "The specified input parameter has a value that is not valid."), InvalidKMSResourceException, LimitExceededException and ResourceInUseException; the destination members (S3DestinationConfiguration, ExtendedS3DestinationConfiguration, RedshiftDestinationConfiguration, ...) are separate optional members with no union over them, so the arity rule is stated only in prose ("You can specify only one destination"). Recorded behavior: a CreateDeliveryStream carrying no destination configuration at all is refused at HTTP 400 with the message "Exactly one destination configuration is supported for a Firehose". That message is entirely FIXED text — it interpolates nothing from the request or from state, so it is reproducible verbatim.
Emulator: HTTP 400 with message "a destination configuration must be specified" (status and error class otherwise agree).
Why: the message is the only part of this refusal that tells a caller what the rule is; a client or test that matches Firehose's sentence (or a human reading the failure) does not recognise the emulator's wording, so negative tests around destination arity pass in AWS and fail here, and the diagnostic text differs from every AWS-sourced runbook.
Fix: the CreateDeliveryStream handler's destination-arity validation (the sole-destination check that today raises "a destination configuration must be specified" when no destination member was supplied) must raise InvalidArgumentException with Firehose's fixed sentence. Hypothesis (not stated by the model or this recording): Firehose words the more-than-one case with the same sentence, in which case the emulator's "Exactly one of <members> must be specified." enumeration for that branch is misworded too — that branch is not exercised by this fixture and should be confirmed against a recording before being changed.
Traces: firehose-create-delivery-stream-argument-validation#task#3.

Related open issue, which does NOT cover either cause: "Firehose CreateDeliveryStream/PutRecordBatch are overly permissive and misclassify rejections (7 causes)" (20261022T064119Z-firehose-createdeliverystream-putrecordbatch-are-overly-permissive-and-misclassify.md) is about CreateDeliveryStream accepting input AWS refuses and about error-code classification; it says nothing about the no-destination message or about the described destination's shape.

## Evidence
Cause 1 — fixture describe-delivery-stream-initial-shape, step task#4, DescribeDeliveryStream, verdict mismatch, expected HTTP 200 / actual HTTP 200. Replay detail, verbatim: "$.DeliveryStreamDescription.Destinations[0].S3DestinationDescription: missing (expected {'BucketARN': 'arn:aws:s3:::sim2real-probe-firehose-bkt-500d8a7f', 'BufferingHints': {'IntervalInSeconds': 300, 'SizeInMBs': 5}, 'CloudWatchLoggingOptions': {'Enabled': False}, 'CompressionFormat': 'UNCOMPRESSED', 'EncryptionConfiguration': {'NoEncryptionConfig': 'NoEncryption'}, 'RoleARN': 'arn:aws:iam::000000000000:role/sim2real-probe-firehose-role-eb8c1f40'}, got None)". The bucket ARN, role ARN and their random suffixes are that run's values; the requirement is that the member is present and carries the destination's effective RoleARN/BucketARN plus the materialised BufferingHints, CompressionFormat, EncryptionConfiguration and CloudWatchLoggingOptions.

Cause 2 — fixture firehose-create-delivery-stream-argument-validation, step task#3, CreateDeliveryStream, verdict mismatch, expected HTTP 400 / actual HTTP 400. Replay detail, verbatim: "$.message: value_mismatch (expected 'Exactly one destination configuration is supported for a Firehose', got 'a destination configuration must be specified')". Expected message text (all fixed, no interpolated values): "Exactly one destination configuration is supported for a Firehose". Actual: "a destination configuration must be specified".

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (trace set simparity-wire-aws-v1).
2. Cause 1: replay fixture describe-delivery-stream-initial-shape and inspect step task#4 — create a Firehose stream with an S3-backed destination, wait for it to go ACTIVE, then call DescribeDeliveryStream and read DeliveryStreamDescription.Destinations[0].S3DestinationDescription. Observe the member is absent where the recording carries the populated structure (RoleARN, BucketARN, BufferingHints 300/5, CompressionFormat UNCOMPRESSED, EncryptionConfiguration NoEncryption, CloudWatchLoggingOptions Enabled=false).
3. Cause 2: replay fixture firehose-create-delivery-stream-argument-validation and inspect step task#3 — call CreateDeliveryStream with only DeliveryStreamName (no S3DestinationConfiguration, no ExtendedS3DestinationConfiguration, no other destination member) and read the error message from the 400 response. Observe "a destination configuration must be specified" instead of "Exactly one destination configuration is supported for a Firehose".
