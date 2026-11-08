Issue-Source: trace-sweep-divergence
Issue-Trace: s3tables-sct-create-table-input-mistakes
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3tables-sct-create-table-input-mistakes s3tables-sct-namespace-string-and-delete-while-in-use s3tables-sct-sort-order-at-create-via-write-order s3tables-table-bucket-replication-lifecycle
Issue-Shape-Id: com.amazonaws.s3tables#S3TableBuckets
Issue-Key: s3tables|CreateTable|4xx->4xx|$.message
Issue-Key: s3tables|CreateNamespace|4xx->4xx|$.message
Issue-Key: s3tables|PutTableBucketReplication|2xx->2xx|$.status
Issue-Related: 20261103T033559Z-s3tables-request-validation-diverges-generic-validator-text-replaces.md
Issue-Commit: 547f35aa751bdfccaa69888f930e73773cf7bea8

# s3tables request refusal messages and response field values diverge on create and replication

> _Note: this issue was identified against the code base at commit `547f35aa751bdfccaa69888f930e73773cf7bea8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTable

## Description
Four distinct net-new wire causes in the s3tables create/replication surface, none carried by an open issue's replay key. NOTE: this issue carries 4 SEPARATE causes — all four must be closed before submitting.

=== Cause 1 of 4: an invalid CreateTable `format` is refused with the framework's generic enum-validator text instead of the service's message ===
AWS: Smithy: CreateTable PUT /tables/{tableBucketARN}/{namespace}, required input member `format` (OpenTableFormat enum, sole value ICEBERG), modeled error BadRequestException (httpError 400, "The request is invalid or malformed."). Recorded behavior: a format outside the enum (e.g. the lowercase spelling of the one member) is refused with HTTP 400 and the message "The specified open table format is not valid" — entirely fixed text, with no echo of the offending value and no member path.
Emulator: HTTP 400 with the generic constraint report "1 validation error detected: Value 'iceberg' at 'format' failed to satisfy constraint: Member must satisfy enum value set: [ICEBERG]" — a different message shape; the quoted value and member path are per-request filler, the divergence is that the service's fixed text is replaced at all.
Why: clients and tests match on the refusal message to tell a bad table format from any other 400; the emulator's framework text never appears in AWS, so message-keyed handling and golden-output tests diverge on the emulator while the status agrees.
Fix: CreateTable's input validation in the s3tables crate (services/service-com-amazonaws-s3tables-s3tablebuckets/src/ops/create_table.rs) — override the generic enum-constraint message for `format` with the service's BadRequestException text. Open issue 20261102T055316Z-s3tables-constraint-violation-refusals-are-now-blanket-400 describes the same class of generic-validator text, but it does not carry this fixture step's replay key, so this step is unrun by any filed issue.
Traces: s3tables-sct-create-table-input-mistakes#task#3 (cluster also covers #task#4).

=== Cause 2 of 4: CreateNamespace with a scalar `namespace` is refused with a JSON-parser message instead of the service's invalid-input message ===
AWS: Smithy: CreateNamespace PUT /namespaces/{tableBucketARN}, required input member `namespace` -> NamespaceList (list of NamespaceName, length 1..1), modeled error BadRequestException (httpError 400). Recorded behavior: a request whose `namespace` is a JSON string rather than a one-element array is refused with HTTP 400 and the message "Invalid input. Consult the S3 Tables and/or Iceberg REST Catalog API reference for constraints on request parameters." — entirely fixed text — under the service's 400 error class.
Emulator: HTTP 400 with x-amzn-errortype 'ValidationException' and the deserializer's message "expected array start, got string" — the framework's parser text and an error type the operation does not model (CreateNamespace models BadRequestException, ConflictException, ForbiddenException, InternalServerErrorException, NotFoundException, TooManyRequestsException).
Why: the error type is part of the contract clients dispatch on; a ValidationException the model never declares is unmodeled in every generated SDK, so the client falls through to a generic error instead of the BadRequestException branch, and the message gives the caller no pointer to the API reference AWS names.
Fix: the body deserialization/refusal path for CreateNamespace in the s3tables crate (services/service-com-amazonaws-s3tables-s3tablebuckets/src/ops/create_namespace.rs and the shared JSON deserializer it is driven by) — map shape-mismatch parse failures onto BadRequestException with the service's invalid-input text.
Traces: s3tables-sct-namespace-string-and-delete-while-in-use#task#2.

=== Cause 3 of 4: a CreateTable AWS refuses for invalid metadata is answered 409 ConflictException because the earlier invalid create was stored ===
AWS: Smithy: CreateTable, optional input member `metadata` (TableMetadata), modeled errors BadRequestException (400) and ConflictException (409). Recorded behavior: a create whose metadata is malformed is refused with HTTP 400 and the message "The specified metadata is not valid" (fixed text) and creates NOTHING — so repeating the same bad create gets the same 400, never a name conflict.
Emulator: the repeat answers HTTP 409 with x-amzn-errortype 'ConflictException' and message "A table with an identical name already exists in the namespace." — because the first unchecked bad create was accepted (200) and took the name.
Why: both the status class and the meaning are wrong: the caller is told the name is taken when AWS is telling it the metadata is wrong, so retry/backfill logic deletes or renames a table instead of fixing the payload — and the name is in fact occupied by a table AWS would never have created.
Fix: CreateTable's metadata validation and its ordering against the name-conflict check and the state write (services/service-com-amazonaws-s3tables-s3tablebuckets/src/ops/create_table.rs) — validate metadata and refuse with BadRequestException before reserving the table name. Open issue 20261103T033559Z-s3tables-request-validation-diverges-generic-validator-text-replaces covers the unchecked-metadata accept itself and owns the sibling steps, but it does not carry this step's replay key, so this 409-instead-of-400 step is unrun by any filed issue.
Traces: s3tables-sct-create-table-input-mistakes#task#6 (cluster also covers s3tables-sct-sort-order-at-create-via-write-order#task#4).

=== Cause 4 of 4: PutTableBucketReplication returns the configuration's status instead of the operation's status ===
AWS: Smithy: PutTableBucketReplication PUT /table-bucket-replication, code 200, output PutTableBucketReplicationResponse with required members `versionToken` and `status` (String) documented as "The status of the replication configuration operation". Recorded behavior: a successful put answers HTTP 200 with status 'success' — a fixed literal describing the outcome of the call, not a state of the replication configuration.
Emulator: HTTP 200 with status 'enabled' — it echoes a configuration/rule state rather than the operation outcome.
Why: `status` is a required response member, so clients read it unconditionally; code that gates on the documented success literal treats every successful emulator put as an unrecognized outcome (and anything comparing against the configuration's own status silently inverts meaning once a rule is disabled).
Fix: the PutTableBucketReplication handler's response serialization in the s3tables crate (services/service-com-amazonaws-s3tables-s3tablebuckets/src/ops/put_table_bucket_replication.rs) — populate `status` with the operation outcome literal, independent of the stored rule state. Open issue 20261101T233620Z-s3tables-puttablebucketreplication-stub.md covers standing the operation up; it now answers 200, so it does not cover this field's value.
Traces: s3tables-table-bucket-replication-lifecycle#task#5.

## Evidence
Wire replay of simparity-wire-aws-v1 against the emulator built from this checkout: 564 steps, 388 passed, 171 mismatched, score 68.8%. The four clusters below are s3tables mismatches with no stub_path and no owning issue.

Cause 1 — s3tables-sct-create-table-input-mistakes#task#3, CreateTable (HTTP 400 both sides): $.message expected 'The specified open table format is not valid', got "1 validation error detected: Value 'iceberg' at 'format' failed to satisfy constraint: Member must satisfy enum value set: [ICEBERG]". 2 steps affected.

Cause 2 — s3tables-sct-namespace-string-and-delete-while-in-use#task#2, CreateNamespace (HTTP 400 both sides): $.message expected 'Invalid input. Consult the S3 Tables and/or Iceberg REST Catalog API reference for constraints on request parameters.', got 'expected array start, got string'; $headers.x-amzn-errortype got 'ValidationException' where the recorded response carried the service's 400 error type.

Cause 3 — s3tables-sct-create-table-input-mistakes#task#6, CreateTable: status expected 400, got 409; $.message expected 'The specified metadata is not valid', got 'A table with an identical name already exists in the namespace.'; $headers.x-amzn-errortype got 'ConflictException'. 2 steps affected.

Cause 4 — s3tables-table-bucket-replication-lifecycle#task#5, PutTableBucketReplication (HTTP 200 both sides): $.status expected 'success', got 'enabled'.

## Steps to Reproduce
Boot the emulator from this checkout and replay the wire corpus, or by hand against the s3tables endpoint:

Cause 1:
1. CreateTableBucket -> arn; CreateNamespace(tableBucketARN=arn, namespace=[<ns>]).
2. CreateTable(tableBucketARN=arn, namespace=<ns>, name=<table>, format='iceberg') — AWS 400 BadRequestException 'The specified open table format is not valid'; emulator 400 with the generic '1 validation error detected: ... Member must satisfy enum value set: [ICEBERG]' text.

Cause 2:
3. Send CreateNamespace PUT /namespaces/<url-encoded arn> with body {"namespace": "<ns>"} (a JSON string, not a one-element array) — AWS 400 'Invalid input. Consult the S3 Tables and/or Iceberg REST Catalog API reference for constraints on request parameters.'; emulator 400 ValidationException 'expected array start, got string'.

Cause 3:
4. CreateTable(..., name=<table>, format='ICEBERG', metadata=<malformed metadata>) — AWS 400 'The specified metadata is not valid' and no table is created; the emulator answers 200.
5. Repeat the identical call — AWS 400 again with the same message; the emulator answers 409 ConflictException 'A table with an identical name already exists in the namespace.'
6. Then CreateTable with the same name and VALID metadata — AWS 200 with tableARN; the emulator answers 409.

Cause 4:
7. CreateTableBucket -> source arn and a destination table bucket; PutTableBucketReplication(tableBucketARN=<source arn>, configuration={'role': <IAM role ARN>, 'rules': [<one rule>]}) — AWS 200 with status 'success'; emulator 200 with status 'enabled'.
