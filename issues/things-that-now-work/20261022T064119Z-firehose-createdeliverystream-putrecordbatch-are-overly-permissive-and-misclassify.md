Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: firehose.create-delivery-stream-untrusted-role-rejected firehose.create-stream-nonexistent-role-arn-rejected firehose.create-stream-role-missing-trust-policy-denied firehose.create-stream-role-missing-trust-policy-rejected firehose.create-stream-malformed-bucket-arn-rejected firehose.create-stream-rejects-non-s3-bucket-arn firehose.create-stream-malformed-role-arn-rejected firehose.create-stream-role-arn-malformed-rejected firehose.create-stream-role-arn-error-classification firehose.create-stream-invalid-kms-key-arn firehose.create-stream-processing-configuration-validated firehose.put-record-batch-request-size-limit-enforced firehose.create-stream-invalid-compression-type-rejected
Issue-Shape-Id: com.amazonaws.firehose#Firehose_20150804
Issue-Key: firehose.create-delivery-stream-untrusted-role-rejected|_error#9|AssertionError|raise AssertionError(f"expected this call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: firehose.create-stream-nonexistent-role-arn-rejected|_error_code#7|AssertionError|raise AssertionError(f"{expect_rejection_of} should be rejected but SUCCEEDED: {result!r}")
Issue-Key: firehose.create-stream-role-missing-trust-policy-denied|_error_code#10|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: firehose.create-stream-role-missing-trust-policy-rejected|_error_code#9|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: firehose.create-stream-malformed-bucket-arn-rejected|_error_code_and_message#14|AssertionError|raise AssertionError(f"expected a ClientError, call succeeded and returned {result!r}")
Issue-Key: firehose.create-stream-rejects-non-s3-bucket-arn|_err#8|AssertionError|raise AssertionError(f"expected this call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: firehose.create-stream-malformed-role-arn-rejected|run#6|AssertionError|assert code == "InvalidParameterValueException", f"CODE={code} expected InvalidParameterValueException"
Issue-Key: firehose.create-stream-role-arn-malformed-rejected|run#4|AssertionError|assert code == "InvalidParameterValueException", f"code={code}"
Issue-Key: firehose.create-stream-role-arn-error-classification|run#9|AssertionError|assert malformed_code == "InvalidParameterValueException", (
Issue-Key: firehose.create-stream-invalid-kms-key-arn|run#6|AssertionError|raise AssertionError("expected InvalidKMSResourceException for an unusable KMS key ARN")
Issue-Key: firehose.create-stream-processing-configuration-validated|run#7|AssertionError|assert code1 == "InvalidArgumentException", (
Issue-Key: firehose.put-record-batch-request-size-limit-enforced|_err#10|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded: {result!r}")
Issue-Key: firehose.create-stream-invalid-compression-type-rejected|run#7|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Commit: 360c4c1618594432a64f84ba94d0fa368ed8f619

# Firehose CreateDeliveryStream/PutRecordBatch are overly permissive and misclassify rejections (7 causes)

> _Note: this issue was identified against the code base at commit `360c4c1618594432a64f84ba94d0fa368ed8f619`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDeliveryStream

## Description
CreateDeliveryStream (and PutRecordBatch's request-size guard) accept input that real Firehose refuses, and where the emulator does refuse it picks the wrong error identity. Seven distinct root causes, each with its own failing checks. All are in the firehose service, which get_bootstrap_status() reports as bootstrapped (service-com-amazonaws-firehose-firehose-20150804).

Cause 1 of 7: RoleARN is never resolved against IAM (neither existence nor trust policy).
AWS: Smithy: CreateDeliveryStream.ExtendedS3DestinationConfiguration.RoleARN is @required with pattern ^arn:.*:iam::\d{12}:role/[a-zA-Z_0-9+=,.@\-_/]+$; CreateDeliveryStream models InvalidArgumentException (client, HTTP 400). Recorded behavior: CreateDeliveryStream is REFUSED with a 400 client error when the RoleARN names an IAM role that does not exist, and when it names a role whose trust policy does not allow sts:AssumeRole for the firehose.amazonaws.com service principal (Firehose proves assumability at create time).
Emulator: all four calls return HTTP 200 with a DeliveryStreamARN. No IAM lookup and no trust-policy evaluation are performed.
Why: a client that relies on CreateDeliveryStream as the gate for role misconfiguration sees a healthy stream that can never deliver; negative tests for unassumable roles pass against the emulator and fail in AWS, and IAM wiring bugs surface only as silent delivery loss.
Fix: the CreateDeliveryStream handler's validation stage must resolve RoleARN in the IAM state store and evaluate the role's trust policy for the firehose service principal before admitting the stream.
Traces: firehose.create-delivery-stream-untrusted-role-rejected, firehose.create-stream-nonexistent-role-arn-rejected, firehose.create-stream-role-missing-trust-policy-denied, firehose.create-stream-role-missing-trust-policy-rejected.

Cause 2 of 7: BucketARN is not validated — a malformed ARN and an ARN for a non-S3 service are both accepted.
AWS: Smithy: ExtendedS3DestinationDescription.BucketARN is @required with pattern ^arn:.*:s3:::[\w\.\-]{1,255}$. Recorded behavior: CreateDeliveryStream is refused with a 400 client error when BucketARN is malformed, and when a syntactically valid ARN for another service (an SQS queue ARN) is supplied in its place.
Emulator: HTTP 200 with a DeliveryStreamARN in both cases.
Why: a typo'd or wrong-service destination ARN is accepted, so the misconfiguration is only discoverable by the absence of delivered objects instead of at the API.
Fix: the CreateDeliveryStream handler's destination-config validation must enforce the modeled BucketARN pattern and require the s3 service namespace.
Traces: firehose.create-stream-malformed-bucket-arn-rejected, firehose.create-stream-rejects-non-s3-bucket-arn.

Cause 3 of 7: a malformed RoleARN is refused with the wrong error code.
AWS: recorded behavior only — a malformed RoleARN ('not-an-arn') is refused with error code InvalidParameterValueException at HTTP 400. Smithy: CreateDeliveryStream's modeled errors are InvalidArgumentException, InvalidKMSResourceException, LimitExceededException, ResourceInUseException; ValidationException is NOT a modeled error of this operation, so the emulator's code cannot be produced by real Firehose.
Emulator: error code ValidationException, with a generic Smithy constraint message, e.g. "1 validation error detected: Value 'not-an-arn' at 'extendedS3DestinationConfiguration.roleARN' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:.*:iam::\d{12}:role/...$" (example only).
Why: clients branch on Error.Code; an unmodeled ValidationException is not in any generated SDK's error union for this operation, so retry/classification logic that handles InvalidParameterValueException falls through to a generic failure.
Fix: the CreateDeliveryStream handler must raise the service's parameter-value error for ARN-shape violations rather than letting the generic Smithy constraint validator answer.
Traces: firehose.create-stream-malformed-role-arn-rejected, firehose.create-stream-role-arn-malformed-rejected, firehose.create-stream-role-arn-error-classification.

Cause 4 of 7: an unusable KMS key ARN in DeliveryStreamEncryptionConfigurationInput is not rejected.
AWS: Smithy: CreateDeliveryStream models InvalidKMSResourceException (client, HTTP 400), documented as "Firehose throws this exception when an attempt to put records or to start or stop Firehose stream encryption fails. This happens when the KMS service throws AccessDeniedException, InvalidStateException, DisabledException, or NotFoundException." Recorded behavior: a CMK ARN that cannot be used is refused with InvalidKMSResourceException.
Emulator: no InvalidKMSResourceException is raised for an unusable key ARN.
Why: streams are created with encryption pointing at a key that does not resolve, so a KMS misconfiguration that AWS surfaces synchronously at create becomes an undetectable delivery failure.
Fix: the CreateDeliveryStream handler's encryption-config validation must resolve the KeyARN against KMS state and map an unusable key to InvalidKMSResourceException.
Traces: firehose.create-stream-invalid-kms-key-arn.

Cause 5 of 7: ProcessingConfiguration is not validated — a Lambda processor with no LambdaArn parameter is accepted.
AWS: Smithy: CreateDeliveryStream models InvalidArgumentException (client, HTTP 400) — "The specified input parameter has a value that is not valid." Recorded behavior: a Processor of type Lambda carrying no LambdaArn parameter is refused with InvalidArgumentException; the parameter is mandatory for that processor type even though the model expresses Parameters as a generic list.
Emulator: the call succeeds; no error is raised (the check observed code None).
Why: a transform pipeline is created with a processor that can never be invoked, and the client gets no signal until records silently stop being transformed.
Fix: the CreateDeliveryStream handler must validate ProcessingConfiguration.Processors per processor type, requiring LambdaArn for Lambda processors.
Traces: firehose.create-stream-processing-configuration-validated.

Cause 6 of 7: PutRecordBatch does not enforce the aggregate request-size ceiling.
AWS: Smithy constrains only the per-record and per-list dimensions (PutRecordBatch.Records @length 1..500; record Data @length max 1024000). The aggregate request-body ceiling is recorded behavior only: a PutRecordBatch whose records are individually legal but whose total request size exceeds the service's per-request limit is refused with a 400 client error and NO record is accepted.
Emulator: HTTP 200, FailedPutCount 0 and a RecordId minted for every record.
Why: a client that sizes batches against the documented request ceiling and relies on the all-or-nothing rejection gets a partial/oversized write accepted in the emulator and a 400 in AWS, so batching logic is validated against the wrong boundary.
Fix: the PutRecordBatch handler's validation must sum the decoded record payloads plus envelope and refuse the whole request when it exceeds the per-request ceiling.
Traces: firehose.put-record-batch-request-size-limit-enforced.

Cause 7 of 7: the CompressionFormat value set is missing ZSTD.
AWS: recorded behavior only for the value list — the rejection for an invalid CompressionFormat enumerates the accepted set including ZSTD (recorded example: "Member must satisfy enum value set: [ZIP, ZSTD, HADOOP_SNAPPY, Snappy, GZIP, UNCOMPRESSED]"). Smithy: the on-disk model's com.amazonaws.firehose#CompressionFormat carries only [UNCOMPRESSED, GZIP, ZIP, Snappy, HADOOP_SNAPPY], i.e. the checked-in model predates ZSTD; the emulator faithfully reflects the stale model, not the service.
Emulator: the accepted set is [UNCOMPRESSED, GZIP, ZIP, Snappy, HADOOP_SNAPPY]; ZSTD is absent, so a stream configured with CompressionFormat=ZSTD would be refused even though AWS accepts it.
Why: a client that selects ZSTD compression — a value real Firehose accepts — cannot create the stream at all. (Set ORDER in the message is not a requirement; membership is.)
Fix: extend the CompressionFormat enum used by the CreateDeliveryStream destination validation to include ZSTD.
Traces: firehose.create-stream-invalid-compression-type-rejected.

## Evidence
Cause 1 — expected: 400 client rejection. Actual: firehose.create-delivery-stream-untrusted-role-rejected — "expected this call to be rejected, but it SUCCEEDED and returned {'DeliveryStreamARN': 'arn:aws:firehose:...:deliverystream/sim2real-behav-fh-badtrust-...', 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}"; firehose.create-stream-nonexistent-role-arn-rejected — "CreateDeliveryStream with a RoleARN for a non-existent IAM role should be rejected but SUCCEEDED"; firehose.create-stream-role-missing-trust-policy-denied / -rejected — "expected the call to be refused, but it succeeded" (HTTP 200).

Cause 2 — expected: 400 client rejection. Actual: firehose.create-stream-malformed-bucket-arn-rejected — "expected a ClientError, call succeeded and returned {... 'HTTPStatusCode': 200 ...}"; firehose.create-stream-rejects-non-s3-bucket-arn — "expected this call to be rejected, but it SUCCEEDED" for an SQS ARN in BucketARN.

Cause 3 — expected Error.Code InvalidParameterValueException (HTTP 400). Actual Error.Code ValidationException: firehose.create-stream-malformed-role-arn-rejected "CODE=ValidationException expected InvalidParameterValueException"; firehose.create-stream-role-arn-malformed-rejected "code=ValidationException"; firehose.create-stream-role-arn-error-classification "a malformed RoleARN must be refused with InvalidParameterValueException, got 'ValidationException'".

Cause 4 — expected Error.Code InvalidKMSResourceException (HTTP 400). Actual: firehose.create-stream-invalid-kms-key-arn — "expected InvalidKMSResourceException for an unusable KMS key ARN".

Cause 5 — expected Error.Code InvalidArgumentException (HTTP 400). Actual: firehose.create-stream-processing-configuration-validated — "a Lambda processor with no LambdaArn parameter must be refused with InvalidArgumentException, got None: None".

Cause 6 — expected 400 client rejection with no record accepted. Actual: firehose.put-record-batch-request-size-limit-enforced — "expected the call to be rejected, but it succeeded: {'FailedPutCount': 0, 'Encrypted': False, 'RequestResponses': [{'RecordId': ...} x5], 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}".

Cause 7 — expected enum value set containing ZSTD. Actual: firehose.create-stream-invalid-compression-type-rejected — emulator reported "Member must satisfy enum value set: [UNCOMPRESSED, GZIP, ZIP, Snappy, HADOOP_SNAPPY]" against the recorded "[ZIP, ZSTD, HADOOP_SNAPPY, Snappy, GZIP, UNCOMPRESSED]".

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the behavioral suite.
2. Cause 1: create an IAM role with no trust policy for firehose.amazonaws.com (and separately, reference an IAM role ARN for a role that was never created), then call CreateDeliveryStream with ExtendedS3DestinationConfiguration.RoleARN set to it — observe HTTP 200 instead of a 400 client error.
3. Cause 2: call CreateDeliveryStream with ExtendedS3DestinationConfiguration.BucketARN set to a malformed string, and again with an arn:aws:sqs:... queue ARN — observe HTTP 200 both times.
4. Cause 3: call CreateDeliveryStream with RoleARN='not-an-arn' and read Error.Code — observe ValidationException instead of InvalidParameterValueException.
5. Cause 4: call CreateDeliveryStream with DeliveryStreamEncryptionConfigurationInput naming an unusable CMK ARN — observe no InvalidKMSResourceException.
6. Cause 5: call CreateDeliveryStream with ProcessingConfiguration.Processors=[{Type: 'Lambda', Parameters: []}] — observe success instead of InvalidArgumentException.
7. Cause 6: create a DirectPut stream, then call PutRecordBatch with a handful of records that are each under 1024000 bytes but whose total exceeds the per-request ceiling — observe HTTP 200 with RecordIds.
8. Cause 7: call CreateDeliveryStream with ExtendedS3DestinationConfiguration.CompressionFormat='ZSTD' (and with a bogus value, to read the reported value set) — observe that ZSTD is not an accepted value.
