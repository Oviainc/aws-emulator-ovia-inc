Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: firehose.create-stream-invalid-compression-type-rejected
Issue-Shape-Id: com.amazonaws.firehose#Firehose_20150804
Issue-Key: firehose.create-stream-invalid-compression-type-rejected|run#7|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Commit: d6495890c37f2f3170333ee7f057f680b864ff4b

# CreateDeliveryStream's CompressionFormat ValidationException lists the enum value set out of the model's declared member order

> _Note: this issue was identified against the code base at commit `d6495890c37f2f3170333ee7f057f680b864ff4b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDeliveryStream

## Description
Cause 1 of 1: the enum-value-set rendered into the ValidationException message for an unmodeled CompressionFormat is not in the Smithy model's declared member order. Failing check: firehose.create-stream-invalid-compression-type-rejected.

AWS: Smithy (services/service-com-amazonaws-firehose-firehose-20150804 model): shape com.amazonaws.firehose#CompressionFormat is an enum whose members are declared, in order, ZIP, ZSTD, HADOOP_SNAPPY, Snappy, GZIP, UNCOMPRESSED; ExtendedS3DestinationConfiguration.CompressionFormat targets it. A CreateDeliveryStream carrying a value outside that set is refused with HTTP 400 and error code ValidationException, message template: "<N> validation error detected: Value at '<memberPath>' failed to satisfy constraint: Member must satisfy enum value set: [<members, in the model's declared order>]". Fixed text of the template: "validation error detected: Value at '", "' failed to satisfy constraint: Member must satisfy enum value set: [", "]". Variable parts, taken from the request and the model: <N> (the number of violations detected in that request), <memberPath> (the lowerCamel wire path of the offending member in the request, here extendedS3DestinationConfiguration.compressionFormat) and the bracketed value list (the target enum's members, in declared order, comma-space separated). The recorded response shows that list as [ZIP, ZSTD, HADOOP_SNAPPY, Snappy, GZIP, UNCOMPRESSED].

Emulator: the status, the error code (ValidationException), the violation count and the member path all match, but the value list is emitted as [ZIP, ZSTD, HADOOP_SNAPPY, Snappy, UNCOMPRESSED, GZIP] — GZIP and UNCOMPRESSED are transposed, i.e. the list is not built from the enum's declared member order. Hypothesis: the list is produced from an unordered/re-sorted collection of the enum variants rather than iterating the model's member order.

Why: the enum value set is part of the error message, and the message is a requirement. A client (or a test fixture, or a human runbook) that compares the refusal message against AWS's sees a mismatch for every rejected enum on this path; and because the ordering comes from however the variants happen to be collected, the same defect will surface for any other enum member whose validation message the emulator renders, not only CompressionFormat.

Fix: the CreateDeliveryStream input validation that renders the enum-constraint ValidationException message for the firehose service — make the bracketed value list iterate the target enum shape's members in declared (model) order instead of an unordered or sorted collection.

Traces: firehose.create-stream-invalid-compression-type-rejected (the check's remaining assertions — ResourceNotFoundException for the name afterwards and the GZIP positive control — were not reached because the message assertion fails first).

## Evidence
Check firehose.create-stream-invalid-compression-type-rejected, [mismatch]:

AssertionError: expected message "1 validation error detected: Value at 'extendedS3DestinationConfiguration.compressionFormat' failed to satisfy constraint: Member must satisfy enum value set: [ZIP, ZSTD, HADOOP_SNAPPY, Snappy, GZIP, UNCOMPRESSED]", got "1 validation error detected: Value at 'extendedS3DestinationConfiguration.compressionFormat' failed to satisfy constraint: Member must satisfy enum value set: [ZIP, ZSTD, HADOOP_SNAPPY, Snappy, UNCOMPRESSED, GZIP]"

Expected (AWS, and the model's declared order): ... [ZIP, ZSTD, HADOOP_SNAPPY, Snappy, GZIP, UNCOMPRESSED]
Actual   (emulator):                              ... [ZIP, ZSTD, HADOOP_SNAPPY, Snappy, UNCOMPRESSED, GZIP]

Model confirmation — smithy_query shape CompressionFormat:
{"shape_id": "com.amazonaws.firehose#CompressionFormat", "type": "enum", "values": ["ZIP", "ZSTD", "HADOOP_SNAPPY", "Snappy", "GZIP", "UNCOMPRESSED"]}

HTTP status (400) and error code (ValidationException) match; only the message's value list diverges.

## Steps to Reproduce
1. Create an S3 bucket and an IAM role trusted by firehose.amazonaws.com with s3:PutObject/GetBucketLocation/ListBucket on that bucket.
2. Call firehose CreateDeliveryStream with DeliveryStreamName=<any new name> and ExtendedS3DestinationConfiguration={RoleARN, BucketARN, CompressionFormat: "BOGUS_FORMAT"}.
3. Inspect the ClientError: Error.Code is ValidationException (correct) and Error.Message's bracketed enum value set ends "..., UNCOMPRESSED, GZIP]" instead of AWS's "..., GZIP, UNCOMPRESSED]".
4. Compare against the model's declared member order for com.amazonaws.firehose#CompressionFormat.
