Issue-Source: trace-sweep-divergence
Issue-Trace: s3tables-error-paths-bucket-namespace-conflicts
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3tables-error-paths-bucket-namespace-conflicts s3tables-error-duplicate-and-version-conflict s3tables-error-paths-table-conflicts-and-notfound s3tables-namespace-duplicate-and-notfound-errors s3tables-namespace-error-paths s3tables-table-bucket-lifecycle s3tables-table-bucket-create-with-config s3tables-table-bucket-deleted-then-not-found s3tables-table-bucket-notfound-errors
Issue-Shape-Id: com.amazonaws.s3tables#S3TableBuckets
Issue-Key: s3tables|DeleteTableBucket|4xx->4xx|$.message
Issue-Key: s3tables|DeleteTableBucket|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: s3tables|DeleteTableBucket|4xx->4xx|status
Issue-Key: s3tables|DeleteTable|4xx->4xx|$.message
Issue-Related: 20261101T230800Z-s3tables-rejects-invalid-input-with-unmodeled-validationexception-instead.md
Issue-Commit: 8715ba3f180c27028dee3267eb478eb6947c4fd2

# s3tables conflict and not-found errors use invented messages, and non-empty bucket delete returns the wrong status

> _Note: this issue was identified against the code base at commit `8715ba3f180c27028dee3267eb478eb6947c4fd2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteTableBucket

## Description
Eight causes with one shared fault: the emulator writes its own prose for s3tables' conflict and not-found errors (interpolating resource names AWS does not mention), and in one case classifies the failure as the wrong error shape and status. AWS's messages for these classes are FIXED text with no variable part — nothing from the request or the store appears in them. Every cause must be closed before this issue is submitted.

Cause 1 of 8: DeleteTableBucket on a non-empty bucket (fixture s3tables-error-paths-bucket-namespace-conflicts, step task#5) — the severe one.
AWS: 400 BadRequestException, x-amzn-errortype 'BadRequestException:http://internal.amazon.com/coral/com.amazon.s3tablebuckets/', message exactly "The bucket that you tried to delete is not empty." (all fixed text). Smithy: DeleteTableBucket models BadRequestException(400) and ConflictException(409) separately; the recorded response is the 400.
Emulator: 409 with x-amzn-errortype 'ConflictException' and message "The table bucket sim2real-probe-s3tables-s3t-err-bucket-5bb082cb is not empty. Delete the namespaces it contains before deleting it." (bucket name interpolated).
Why: 409 is a retryable write-conflict in AWS's own wording ("conflict with a previous write. You can retry the request"), so a client retries a delete that can never succeed until the bucket is emptied, instead of failing fast on a 400 client error.
Fix: the DeleteTableBucket handler's non-empty check — raise BadRequestException(400) with AWS's fixed message, not ConflictException.
Traces: s3tables-error-paths-bucket-namespace-conflicts#task#5.

Cause 2 of 8: CreateTable on an existing table name (fixture s3tables-error-duplicate-and-version-conflict, step task#4).
AWS: 409 ConflictException, message exactly "A table with an identical name already exists in the namespace." (fixed text; no name, no namespace).
Emulator: 409 but message "A table with the name <table> already exists in the namespace <namespace>." with both names interpolated.
Why: a client matching on the error message (the only discriminator ConflictException carries) does not recognise the duplicate-name case.
Fix: the CreateTable handler's duplicate check / error construction.
Traces: s3tables-error-duplicate-and-version-conflict#task#4 (cluster also covers s3tables-error-paths-table-conflicts-and-notfound#task#5).

Cause 3 of 8: CreateNamespace on an existing namespace (same fixture, step task#5).
AWS: 409 ConflictException, message exactly "A namespace with an identical name already exists in the bucket." (fixed text).
Emulator: 409 with "A namespace with the name <namespace> already exists in the table bucket.".
Why/Fix: as cause 2, in CreateNamespace's duplicate check.
Traces: s3tables-error-duplicate-and-version-conflict#task#5 (cluster also covers s3tables-error-paths-bucket-namespace-conflicts#task#4, s3tables-namespace-duplicate-and-notfound-errors#task#4, s3tables-namespace-error-paths#task#6).

Cause 4 of 8: CreateTableBucket on an existing bucket (same fixture, step task#6).
AWS: 409 ConflictException, message exactly "The bucket that you tried to create already exists, and you own it." (fixed text).
Emulator: 409 with "A table bucket with the name <bucket> already exists.".
Why/Fix: as cause 2, in CreateTableBucket's duplicate check.
Traces: s3tables-error-duplicate-and-version-conflict#task#6 (cluster also covers s3tables-error-paths-bucket-namespace-conflicts#task#3, s3tables-table-bucket-lifecycle#task#4).

Cause 5 of 8: DeleteTable with a stale versionToken (same fixture, step task#7).
AWS: 409 ConflictException, message exactly "Provided version token does not match the table version token." (fixed text).
Emulator: 409 with "The version token provided for the table <table> does not match its current version token.".
Why/Fix: as cause 2, in DeleteTable's version-token precondition check.
Traces: s3tables-error-duplicate-and-version-conflict#task#7.

Cause 6 of 8: CreateNamespace against a missing table bucket (fixture s3tables-namespace-error-paths, step task#10).
AWS: 404 NotFoundException, message exactly "The specified bucket does not exist." (fixed text — AWS says "bucket", not "table bucket").
Emulator: 404 with "The specified table bucket does not exist.".
Why/Fix: as cause 2, in the shared bucket-lookup error of CreateNamespace.
Traces: s3tables-namespace-error-paths#task#10.

Cause 7 of 8: GetTableBucket on a deleted/missing bucket (fixture s3tables-table-bucket-create-with-config, step teardown#7).
AWS: 404 NotFoundException with message exactly "The specified bucket does not exist.".
Emulator: 404 with "The specified table bucket does not exist.".
Why/Fix: as cause 6, in GetTableBucket's lookup error.
Traces: s3tables-table-bucket-create-with-config#teardown#7 (cluster also covers s3tables-table-bucket-deleted-then-not-found#task#3, s3tables-table-bucket-notfound-errors#task#3).

Cause 8 of 8: DeleteTableBucket on a missing bucket (fixture s3tables-table-bucket-notfound-errors, step task#4).
AWS: 404 NotFoundException with message exactly "The specified bucket does not exist.".
Emulator: 404 with "The specified table bucket does not exist.".
Why/Fix: as cause 6, in DeleteTableBucket's lookup error.
Traces: s3tables-table-bucket-notfound-errors#task#4.

## Evidence
Replay of simparity-wire-aws-v1 (8 clusters, 20 steps):
- DeleteTableBucket (non-empty) s3tables-error-paths-bucket-namespace-conflicts#task#5: status expected 400, got 409; expected $.message 'The bucket that you tried to delete is not empty.', got 'The table bucket sim2real-probe-s3tables-s3t-err-bucket-5bb082cb is not empty. Delete the namespaces it contains before deleting it.'; expected x-amzn-errortype 'BadRequestException:http://internal.amazon.com/coral/com.amazon.s3tablebuckets/', got 'ConflictException'.
- CreateTable#task#4: 409/409, expected 'A table with an identical name already exists in the namespace.', got 'A table with the name sim2real_probe_s3tables_conf_tbl_7243778b already exists in the namespace sim2real_probe_s3tables_conf_ns_7243778b.'.
- CreateNamespace#task#5: 409/409, expected 'A namespace with an identical name already exists in the bucket.', got 'A namespace with the name sim2real_probe_s3tables_conf_ns_7243778b already exists in the table bucket.'.
- CreateTableBucket#task#6: 409/409, expected 'The bucket that you tried to create already exists, and you own it.', got 'A table bucket with the name sim2real-probe-s3tables-s3t-conf-bkt-1e66c6a6 already exists.'.
- DeleteTable#task#7: 409/409, expected 'Provided version token does not match the table version token.', got 'The version token provided for the table sim2real_probe_s3tables_conf_tbl_7243778b does not match its current version token.'.
- CreateNamespace (missing bucket) s3tables-namespace-error-paths#task#10, GetTableBucket s3tables-table-bucket-create-with-config#teardown#7, DeleteTableBucket s3tables-table-bucket-notfound-errors#task#4: 404/404, expected 'The specified bucket does not exist.', got 'The specified table bucket does not exist.'.
The bucket/namespace/table names quoted in the emulator messages are per-run fixture values; the defect is that AWS's message for each of these classes contains NO such value and is the fixed sentence above.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay simparity-wire-aws-v1.
2. Fixture s3tables-error-paths-bucket-namespace-conflicts step task#5: delete a table bucket that still holds a namespace — observe 409/ConflictException instead of 400/BadRequestException.
3. Fixture s3tables-error-duplicate-and-version-conflict steps task#4 (duplicate table), task#5 (duplicate namespace), task#6 (duplicate bucket), task#7 (DeleteTable with a stale versionToken) — read $.message on each 409.
4. Fixture s3tables-namespace-error-paths step task#10, s3tables-table-bucket-create-with-config step teardown#7 and s3tables-table-bucket-notfound-errors step task#4 — read $.message on each 404.
