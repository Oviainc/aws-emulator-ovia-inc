Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3tables.put-table-bucket-encryption-kms-missing-key-rejected s3tables.concurrent-put-table-bucket-encryption-conflict-detected s3tables.concurrent-put-table-bucket-metrics-configuration-conflict s3tables.concurrent-put-table-bucket-metrics-configuration-last-writer-wins s3tables.concurrent-put-table-replication-conflict-detected s3tables.put-table-bucket-replication-conflicting-state-rejected
Issue-Shape-Id: com.amazonaws.s3tables#S3TableBuckets
Issue-Key: s3tables.put-table-bucket-encryption-kms-missing-key-rejected|run#5|AssertionError|assert code == "BadRequestException", f"expected BadRequestException, got {code!r}"
Issue-Key: s3tables.concurrent-put-table-bucket-encryption-conflict-detected|run#11|AssertionError|assert all(r == "ok" for r in results), f"expected both concurrent puts to succeed, got {results}"
Issue-Key: s3tables.concurrent-put-table-bucket-metrics-configuration-conflict|run#6|AssertionError|assert not failures, f"expected no errors from concurrent idempotent puts, got {results}"
Issue-Key: s3tables.concurrent-put-table-bucket-metrics-configuration-last-writer-wins|run#8|AssertionError|assert not failures, f"expected all 8 concurrent idempotent puts to succeed, got failures: {failures}"
Issue-Key: s3tables.concurrent-put-table-replication-conflict-detected|run#12|AssertionError|assert first == "ok", f"expected the first PutTableReplication to succeed, got {first}"
Issue-Key: s3tables.put-table-bucket-replication-conflicting-state-rejected|run#7|AssertionError|assert code == "NotFoundException", f"expected NotFoundException, got {code}"
Issue-Commit: bf05938345e3744e1c8e3343a2f8d43b66feefe9

# s3tables encryption / metrics / replication Put operations answer 501 NotImplemented, so their KMS validation, version-token preconditions and concurrent-write convergence are all unreachable

> _Note: this issue was identified against the code base at commit `bf05938345e3744e1c8e3343a2f8d43b66feefe9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutTableBucketEncryption

## Description
Four S3 Tables write operations — PutTableBucketEncryption, PutTableBucketMetricsConfiguration, PutTableReplication and PutTableBucketReplication — answer every request with HTTP 501 and error code `NotImplemented`. `NotImplemented` is not a modeled error of any of them (Smithy, all four: BadRequestException 400, ConflictException 409, ForbiddenException 403, NotFoundException 404, TooManyRequestsException 429, InternalServerErrorException 500; the two replication ops add AccessDeniedException 403). Open issues `20261101T233620Z-s3tables-puttablebucketencryption-stub.md`, `...-puttablebucketmetricsconfiguration-stub.md`, `...-puttablereplication-stub.md`, `...-puttablebucketreplication-stub.md` and `20261101T233620Z-s3tables-six-replication-metrics-record-expiration-operations-are.md` ask that these handlers exist; none of them carries the five checks below, and none states the request-validation, precondition and concurrency semantics recorded here. Those semantics are this issue.

Cause 1 of 5: PutTableBucketEncryption accepts/ignores sseAlgorithm=aws:kms with no kmsKeyArn instead of refusing it.
AWS: `EncryptionConfiguration.sseAlgorithm` is @required (enum AES256 | aws:kms) and `kmsKeyArn` is optional with @length 1..2048 and pattern `^(arn:aws[-a-z0-9]*:kms:[-a-z0-9]*:[0-9]{12}:key/.+)$`; its doc states "This field is required only when sseAlgorithm is set to aws:kms" (Smithy: EncryptionConfiguration.kmsKeyArn). Recorded behavior: PutTableBucketEncryption with `{"sseAlgorithm": "aws:kms"}` and no `kmsKeyArn` is refused with HTTP 400, code `BadRequestException`, message `The specified KMS key ARN is invalid. Only key ARN format 'arn:<partition>:kms:<region>:<account-id>:key/<key-id>' is supported.` — the whole message is fixed text, including the literal placeholder-style ARN template; nothing is substituted from the request. The bucket's existing encryption state must be unchanged by the rejected call, and a subsequent valid `{"sseAlgorithm": "AES256"}` put must succeed on the same bucket.
Emulator: HTTP 501, code `NotImplemented`, message `operation PutTableBucketEncryption is scaffolded but not implemented in this emulator`.
Why: a client validating its encryption configuration sees a server-class 501 instead of the 400 that tells it the KMS key is missing, and retries a request AWS would never accept.
Fix: PutTableBucketEncryption's handler and its input validation in services/service-com-amazonaws-s3tables-s3tablebuckets/src/ops/put_table_bucket_encryption.rs — enforce the kmsKeyArn-required-when-aws:kms rule (and the key-ARN pattern) before any state write.
Traces: s3tables.put-table-bucket-encryption-kms-missing-key-rejected.

Cause 2 of 5: concurrent PutTableBucketEncryption calls never succeed, so last-writer convergence cannot be observed.
AWS: PutTableBucketEncryption carries no versionToken member (Smithy: input = tableBucketARN httpLabel + encryptionConfiguration only) and is @idempotent, HTTP PUT /buckets/{tableBucketARN}/encryption with success code 200. Recorded behavior: two concurrent puts with different configurations (AES256 vs aws:kms + key ARN) BOTH return success, and the resulting state is exactly one of the two submitted configurations — never a mix (never `aws:kms` with no key, never `AES256` with a key).
Emulator: both concurrent calls return HTTP 501 `NotImplemented`.
Why: callers that configure encryption from more than one worker get server errors, and once implemented without an atomic swap the bucket could be left with an algorithm/key pair that AWS can never produce, which later Get calls would report as a valid configuration.
Fix: the same handler plus the table-bucket encryption state — apply the whole EncryptionConfiguration as one atomic replacement.
Traces: s3tables.concurrent-put-table-bucket-encryption-conflict-detected.

Cause 3 of 5: concurrent PutTableBucketMetricsConfiguration calls never succeed, so idempotent convergence cannot be observed.
AWS: PutTableBucketMetricsConfiguration takes ONLY `tableBucketARN` (httpLabel, @required) — no body member and no versionToken — is @idempotent, and its success code is 204 (Smithy: http PUT /buckets/{tableBucketARN}/metrics, code 204). Recorded behavior: repeated and concurrent puts all succeed; a following GetTableBucketMetricsConfiguration returns a single stable record whose `tableBucketARN` is the bucket and whose `id` does not change across repeated reads, and a later non-concurrent put still succeeds.
Emulator: every concurrent put returns HTTP 501 `NotImplemented`.
Why: there is no payload for two callers to race over, so a client has no reason to serialise these calls; getting an error from each one makes an unconditionally safe, idempotent operation look unavailable.
Fix: the handler in services/service-com-amazonaws-s3tables-s3tablebuckets/src/ops/put_table_bucket_metrics_configuration.rs and the metrics-configuration state — create-or-reuse one record per bucket, returning 204 with no body and a stable id.
Traces: s3tables.concurrent-put-table-bucket-metrics-configuration-conflict, s3tables.concurrent-put-table-bucket-metrics-configuration-last-writer-wins.

Cause 4 of 5: PutTableReplication never establishes a configuration, so its stale-versionToken ConflictException is unreachable.
AWS: PutTableReplication takes `tableArn` and `versionToken` as httpQuery members plus a @required `configuration`, and returns @required `versionToken` + `status` (Smithy: http PUT /table-replication, errors include ConflictException 409 "The request failed because there is a conflict with a previous write. You can retry the request."). Recorded behavior: the first put succeeds and returns a version token; a put presenting the CURRENT token succeeds and advances it; a put presenting a now-stale token is refused with HTTP 409, code `ConflictException` and the message `The specified version token does not match the configuration.` (fixed text — the token itself is not echoed). After the refusal the configuration still holds the accepted writer's destination and the version token differs from the stale one.
Emulator: the first PutTableReplication already returns HTTP 501 `NotImplemented`.
Why: optimistic-concurrency clients key their retry/merge logic on ConflictException and the returned versionToken; a 501 gives them neither, and an implementation lacking the precondition would silently let a stale writer overwrite a newer configuration.
Fix: the handler in services/service-com-amazonaws-s3tables-s3tablebuckets/src/ops/put_table_replication.rs and the per-table replication state — store and advance a version token per configuration and compare the supplied `versionToken` before writing.
Traces: s3tables.concurrent-put-table-replication-conflict-detected.

Cause 5 of 5: PutTableBucketReplication with a versionToken and no existing configuration must be NotFoundException, not NotImplemented.
AWS: PutTableBucketReplication takes `tableBucketARN` and optional `versionToken` (@length 1..2048) as httpQuery members and a @required `configuration`, returning @required `versionToken` + `status`; NotFoundException (404, "The request was rejected because the specified resource could not be found.") is modeled (Smithy: http PUT /table-bucket-replication). Recorded behavior: supplying a `versionToken` when the bucket has no replication configuration yet is refused with HTTP 404, code `NotFoundException` and message `The specified replication configuration does not exist.` — fixed text, no interpolation — and it creates nothing (a following GetTableBucketReplication answers the same NotFoundException/message). The same configuration submitted WITHOUT a version precondition then succeeds, and GetTableBucketReplication echoes the submitted `role`.
Emulator: HTTP 501, code `NotImplemented`.
Why: a caller that passes a cached token cannot tell "no configuration exists, drop the precondition" from "the service is down"; the distinction between 404 here and 409 for a mismatched token is the whole contract of the precondition.
Fix: the handler in services/service-com-amazonaws-s3tables-s3tablebuckets/src/ops/put_table_bucket_replication.rs — when `versionToken` is present and no replication configuration exists for the bucket, answer NotFoundException with that message and write nothing; when absent, create the configuration and return its token and status.
Traces: s3tables.put-table-bucket-replication-conflicting-state-rejected.

## Evidence
Replaying the behavioral suite against the emulator:

- s3tables.put-table-bucket-encryption-kms-missing-key-rejected — `AssertionError: expected BadRequestException, got 'NotImplemented'`; the check requires code `BadRequestException` and message `The specified KMS key ARN is invalid. Only key ARN format 'arn:<partition>:kms:<region>:<account-id>:key/<key-id>' is supported.` for `encryptionConfiguration={"sseAlgorithm": "aws:kms"}`.
- s3tables.concurrent-put-table-bucket-encryption-conflict-detected — `AssertionError: expected both concurrent puts to succeed, got ['NotImplemented', 'NotImplemented']`.
- s3tables.concurrent-put-table-bucket-metrics-configuration-conflict — `AssertionError: expected no errors from concurrent idempotent puts, got [{'ok': False, 'code': 'NotImplemented', 'result': None}, {'ok': False, 'code': 'NotImplemented', 'result': None}]`.
- s3tables.concurrent-put-table-bucket-metrics-configuration-last-writer-wins — `AssertionError: expected all 8 concurrent idempotent puts to succeed, got failures: [{'ok': False, 'code': 'NotImplemented'} x8]`.
- s3tables.concurrent-put-table-replication-conflict-detected — `AssertionError: expected the first PutTableReplication to succeed, got NotImplemented`; the check then requires code `ConflictException` and message `The specified version token does not match the configuration.` for a stale token.
- s3tables.put-table-bucket-replication-conflicting-state-rejected — `AssertionError: expected NotFoundException, got NotImplemented`; the check requires code `NotFoundException` and message `The specified replication configuration does not exist.`

All six responses are HTTP 501 with `Error.Code = NotImplemented` and `Error.Message = operation <Op> is scaffolded but not implemented in this emulator`, a status/code pair none of the four operations models.

Related but non-covering open issues: `20261101T233620Z-s3tables-puttablebucketencryption-stub.md`, `20261101T233620Z-s3tables-puttablebucketmetricsconfiguration-stub.md`, `20261101T233620Z-s3tables-puttablereplication-stub.md`, `20261101T233620Z-s3tables-puttablebucketreplication-stub.md`, `20261101T233620Z-s3tables-six-replication-metrics-record-expiration-operations-are.md` — they require the handlers to exist and do not list any of the six checks above or the validation/precondition/concurrency rules stated here.

## Steps to Reproduce
1. Boot the s3tables emulator.
2. CreateTableBucket(name=<unique lowercase name>); keep `arn`.
3. PutTableBucketEncryption(tableBucketARN=<arn>, encryptionConfiguration={"sseAlgorithm": "aws:kms"}) — expect HTTP 400 BadRequestException with the fixed invalid-KMS-key message; observe HTTP 501 NotImplemented. Then PutTableBucketEncryption with {"sseAlgorithm": "AES256"} and expect success.
4. Fire two concurrent PutTableBucketEncryption calls on the same bucket, one AES256 and one aws:kms with a valid key ARN — expect both to succeed and the final GetTableBucketEncryption to equal exactly one of them; observe both refused with 501.
5. Fire 8 concurrent PutTableBucketMetricsConfiguration(tableBucketARN=<arn>) calls — expect every one to succeed (204) and GetTableBucketMetricsConfiguration to return one stable `id`; observe all refused with 501.
6. Create a namespace + table and a destination bucket, then PutTableReplication(tableArn=<table arn>, configuration=<role + destination>) — expect success and a versionToken; re-put with the current token (succeeds, token advances) and then with the stale token, expecting HTTP 409 ConflictException `The specified version token does not match the configuration.`; observe the first call already refused with 501.
7. PutTableBucketReplication(tableBucketARN=<source arn>, versionToken="<any bogus token>", configuration=<role + destination>) on a bucket with no replication configuration — expect HTTP 404 NotFoundException `The specified replication configuration does not exist.` and no configuration created; observe HTTP 501 NotImplemented.
