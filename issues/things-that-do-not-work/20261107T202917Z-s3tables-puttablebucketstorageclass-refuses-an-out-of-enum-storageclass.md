Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3tables.put-table-bucket-storage-class-invalid-value-rejected s3tables.put-table-bucket-storage-class-conflicting-active-tables-blocked
Issue-Shape-Id: com.amazonaws.s3tables#S3TableBuckets
Issue-Key: s3tables.put-table-bucket-storage-class-invalid-value-rejected|run#5|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code}"
Issue-Key: s3tables.put-table-bucket-storage-class-conflicting-active-tables-blocked|run#5|AssertionError|assert code == "ValidationException", f"code={code}"
Issue-Commit: 21ae9ed441ac6fbe6706ed72d6750ad7607a2918

# s3tables PutTableBucketStorageClass refuses an out-of-enum storageClass with BadRequestException instead of ValidationException and the exact enum-constraint sentence

> _Note: this issue was identified against the code base at commit `21ae9ed441ac6fbe6706ed72d6750ad7607a2918`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutTableBucketStorageClass

## Description
REOPENS 20261101T233620Z-s3tables-puttablebucketstorageclass-stub.md: its fix left the call refused. That issue's fix replaced the scaffold stub, so the operation now reaches validation (GetTableBucketStorageClass answers, and the 501 is gone), but a PutTableBucketStorageClass whose storageClassConfiguration.storageClass is outside the modeled enum is still refused with the wrong identity: the error code AWS uses for this framework-validated member constraint is lost. Related open issue 20261102T055316Z-s3tables-constraint-violation-refusals-are-now-blanket-400.md records the same blanket-BadRequestException relabelling on the PutTableBucketReplication and PutTableRecordExpirationConfiguration paths; it does not name the storage-class path and the replay did not mark these two checks as owned by it, so it does not cover this cause.

Cause 1 of 1: an out-of-enum storageClass is refused as BadRequestException rather than ValidationException, and without AWS's fixed enum-constraint sentence.
AWS: PutTableBucketStorageClass takes a required storageClassConfiguration whose required member storageClass targets the enum StorageClass with exactly the values STANDARD and INTELLIGENT_TIERING (Smithy, services/service-com-amazonaws-s3tables-s3tablebuckets/smithy-model/com-amazonaws-s3tables-s3tablebuckets.json: StorageClassConfiguration.storageClass -> StorageClass @enum [STANDARD, INTELLIGENT_TIERING], required; operation http PUT /buckets/{tableBucketARN}/storage-class; modeled errors BadRequestException(400), ConflictException(409), ForbiddenException(403), NotFoundException(404), TooManyRequestsException(429), InternalServerErrorException(500) — no ValidationException shape is bound in the service model, so this code is the framework-level untyped validation refusal: recorded behavior only). A value outside that set is refused with HTTP 400 and error code ValidationException, message "1 validation error detected: Value at '<member path>' failed to satisfy constraint: Member must satisfy enum value set: [<enum values>]". Fixed text: "1 validation error detected: Value at '", "' failed to satisfy constraint: Member must satisfy enum value set: [", "]"; variable parts: the member path derived from the request's shape path (here storageClassConfiguration.storageClass) and the enum's value list taken from the model, rendered in the model's order and comma-space separated — for this member [INTELLIGENT_TIERING, STANDARD]. AWS does NOT quote the offending value in this sentence. The refused call has no effect: a following GetTableBucketStorageClass still reports the previous storageClass, and a subsequent valid put (the other enum value) succeeds and is reflected by GetTableBucketStorageClass.
Emulator: the call is refused with error code BadRequestException (HTTP 400). Both checks stop at the code assertion, so the message assertion that follows was never evaluated; the message requirement above is stated so a fixer closes both halves of the same refusal.
Why: BadRequestException is also what s3tables uses for its business-rule refusals (an invalid open-table format, a non-empty namespace, a conflicting active-table state), and it carries no sub-code. A client that branches on ValidationException to tell a malformed-parameter refusal from a service-semantic one takes the service-error branch for a plain bad enum value, a `catch ValidationException` arm never fires, and retry/report policies keyed on parameter errors are applied to the wrong class. With the generic text the message is not usable as a discriminator either, because it varies per request where AWS's does not.
Fix: the s3tables request-validation layer's refusal classification for framework-validated member constraints on this operation's input (the storageClass enum check in front of services/service-com-amazonaws-s3tables-s3tablebuckets/src/ops/put_table_bucket_storage_class.rs) — answer ValidationException with the fixed enum-constraint sentence instead of the blanket BadRequestException, and keep BadRequestException for the operation's own semantic refusals.
Traces: s3tables.put-table-bucket-storage-class-invalid-value-rejected, s3tables.put-table-bucket-storage-class-conflicting-active-tables-blocked.

## Evidence
Replay of simparity-behavioral-aws-v1 against this checkout (s3tables 50/139 passed, 89 failed). Both checks below are `mismatch`: the emulator answered with HTTP 400, so only the refusal's identity diverges.

- s3tables.put-table-bucket-storage-class-invalid-value-rejected: create a table bucket, read GetTableBucketStorageClass, then PutTableBucketStorageClass(tableBucketARN=<bucket arn>, storageClassConfiguration={"storageClass": "ULTRA_FAST_TIER"}) — AssertionError at `assert code == "ValidationException", f"expected ValidationException, got {code}"` -> `expected ValidationException, got BadRequestException`. The check's next assertion expects $.message exactly "1 validation error detected: Value at 'storageClassConfiguration.storageClass' failed to satisfy constraint: Member must satisfy enum value set: [INTELLIGENT_TIERING, STANDARD]" (no value quoted), then that GetTableBucketStorageClass is unchanged, then that a valid put of INTELLIGENT_TIERING is accepted and read back.
- s3tables.put-table-bucket-storage-class-conflicting-active-tables-blocked: same shape with storageClass="NOT_A_REAL_STORAGE_CLASS" — AssertionError at `assert code == "ValidationException", f"code={code}"` -> `code=BadRequestException`, then the same exact-message, effect-absent and positive-control assertions.

("ULTRA_FAST_TIER" and "NOT_A_REAL_STORAGE_CLASS" are the values these checks happen to send; they fill the template and are not part of the requirement.)

Model check via smithy_query on services/service-com-amazonaws-s3tables-s3tablebuckets/smithy-model/com-amazonaws-s3tables-s3tablebuckets.json: PutTableBucketStorageClass input = tableBucketARN (httpLabel, required, @pattern ^(arn:aws[-a-z0-9]*:[a-z0-9]+:[-a-z0-9]*:[0-9]{12}:bucket/[a-z0-9_-]{3,63})$) + storageClassConfiguration (required); StorageClass is an @enum with values [STANDARD, INTELLIGENT_TIERING]; errors as listed above, with no ValidationException shape anywhere in the service model.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the two checks in trace_ids (or drive the calls directly).
2. CreateTableBucket(name=<lowercase name>), then GetTableBucketStorageClass(tableBucketARN=<arn>) and record storageClassConfiguration.storageClass.
3. Call PutTableBucketStorageClass(tableBucketARN=<arn>, storageClassConfiguration={"storageClass": "ULTRA_FAST_TIER"}). Read $.Error.Code — expect `ValidationException` with HTTP 400, observe `BadRequestException`.
4. Compare $.message against "1 validation error detected: Value at 'storageClassConfiguration.storageClass' failed to satisfy constraint: Member must satisfy enum value set: [INTELLIGENT_TIERING, STANDARD]" — the member path comes from the request's shape path and the bracketed list from the model's StorageClass enum; the offending value must NOT be quoted in the sentence.
5. Call GetTableBucketStorageClass again and confirm the storage class is unchanged by the rejected call.
6. Positive control: PutTableBucketStorageClass with the other enum value (INTELLIGENT_TIERING when the bucket is STANDARD) must succeed, and GetTableBucketStorageClass must report the new value.
