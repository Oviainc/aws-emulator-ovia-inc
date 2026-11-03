Issue-Source: trace-sweep-divergence
Issue-Trace: s3tables-error-duplicate-and-version-conflict
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3tables-error-duplicate-and-version-conflict s3tables-error-paths-table-conflicts-and-notfound s3tables-error-paths-bucket-namespace-conflicts s3tables-table-bucket-lifecycle s3tables-namespace-error-paths s3tables-table-bucket-create-with-config s3tables-table-bucket-deleted-then-not-found s3tables-table-bucket-notfound-errors
Issue-Shape-Id: com.amazonaws.s3tables#S3TableBuckets
Issue-Key: s3tables|CreateTable|4xx->4xx|$.message
Issue-Key: s3tables|CreateTableBucket|4xx->4xx|$.message
Issue-Key: s3tables|CreateNamespace|4xx->4xx|$.message
Issue-Key: s3tables|GetTableBucket|4xx->4xx|$.message
Issue-Commit: 56c3e2d5db4619f120a986f75a3622f8f9af8979

# s3tables conflict and not-found messages are emulator-authored on CreateTable, CreateTableBucket, CreateNamespace and GetTableBucket

> _Note: this issue was identified against the code base at commit `56c3e2d5db4619f120a986f75a3622f8f9af8979`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTable

## Description
The s3tables error path reaches the right status and error shape but composes its own message text: it interpolates the requested resource names into a sentence the service never emits. AWS's ConflictException/NotFoundException messages for these classes are FIXED sentences with no variable parts at all, so every one of these messages is wrong for a client that reads it. 4 causes, all in the same message-composition layer.

An open issue, 20261101T230800Z-s3tables-conflict-and-not-found-errors-use-invented.md, already carries this fault for DeleteTable's version-token conflict and DeleteTableBucket's not-empty/not-found paths; the four causes below are separate operations/messages that it does not cover and that the replay reports as unowned.

Cause 1 of 4 - CreateTable on a name that already exists in the namespace.
AWS: Smithy: CreateTable errors include ConflictException (httpError 409). Recorded response: 409 with body message exactly 'A table with an identical name already exists in the namespace.' - fixed text, no request values interpolated.
Emulator: 409 with 'A table with the name <tableName> already exists in the namespace <namespace>.' (recording shows the probe's table and namespace names substituted in).
Why: a client matching or displaying the documented message does not recognise the refusal; the emulator's sentence is not a message AWS ever returns.
Fix: CreateTable's conflict branch in the operation handler must emit the fixed ConflictException text rather than formatting one from request input.
Traces: s3tables-error-duplicate-and-version-conflict#task#4 (representative), s3tables-error-paths-table-conflicts-and-notfound#task#5.

Cause 2 of 4 - CreateTableBucket on an existing bucket name.
AWS: Smithy: CreateTableBucket errors include ConflictException (httpError 409). Recorded response: 409 with message exactly 'The bucket that you tried to create already exists, and you own it.' - fixed text; note it says 'bucket', not 'table bucket', and names nothing.
Emulator: 409 with 'A table bucket with the name <name> already exists.'
Why: same as cause 1 - the message is not AWS's, and it leaks the request's bucket name into text that has no variable part.
Fix: CreateTableBucket's conflict branch in the operation handler / its error constructor.
Traces: s3tables-error-duplicate-and-version-conflict#task#6 (representative), s3tables-error-paths-bucket-namespace-conflicts#task#3, s3tables-table-bucket-lifecycle#task#4.

Cause 3 of 4 - CreateNamespace against a table bucket ARN that does not exist.
AWS: Smithy: CreateNamespace errors include NotFoundException (httpError 404). Recorded response: 404 with message exactly 'The specified bucket does not exist.'
Emulator: 404 with 'The specified table bucket does not exist.' - the word 'table' is inserted into the fixed text.
Why: the status and error type are right but the message is not the service's wording, so message-based assertions and user-facing surfaces diverge.
Fix: the shared not-found error text used by CreateNamespace's bucket lookup (the operation's validation/error construction), which must use the bucket wording AWS uses.
Traces: s3tables-namespace-error-paths#task#10.

Cause 4 of 4 - GetTableBucket and DeleteTableBucket for a deleted/absent table bucket.
AWS: Smithy: GetTableBucket errors include NotFoundException (httpError 404). Recorded response: 404 with message exactly 'The specified bucket does not exist.'
Emulator: 404 with 'The specified table bucket does not exist.'
Why: identical to cause 3 - clients see a message the service does not emit.
Fix: same shared bucket-not-found text, reached from GetTableBucket's state lookup.
Traces: s3tables-table-bucket-create-with-config#teardown#7 (representative), s3tables-table-bucket-deleted-then-not-found#task#3, s3tables-table-bucket-notfound-errors#task#3.

## Evidence
Cause 1 (CreateTable, 409->409): expected $.message 'A table with an identical name already exists in the namespace.'; actual 'A table with the name sim2real_probe_s3tables_conf_tbl_7243778b already exists in the namespace sim2real_probe_s3tables_conf_ns_7243778b.' Only the fixed sentence is the requirement; the probe names shown are this run's values.
Cause 2 (CreateTableBucket, 409->409): expected 'The bucket that you tried to create already exists, and you own it.'; actual 'A table bucket with the name sim2real-probe-s3tables-s3t-conf-bkt-463ad650 already exists.'
Cause 3 (CreateNamespace, 404->404): expected 'The specified bucket does not exist.'; actual 'The specified table bucket does not exist.'
Cause 4 (GetTableBucket, 404->404): expected 'The specified bucket does not exist.'; actual 'The specified table bucket does not exist.'
Status codes and x-amzn-errortype matched in all four clusters; the message is the sole divergence.

## Steps to Reproduce
1. Replay fixture s3tables-error-duplicate-and-version-conflict step task#4: CreateTable twice with the same namespace+name and read the 409 body's message member.
2. Replay s3tables-error-duplicate-and-version-conflict step task#6: CreateTableBucket twice with the same name and read the 409 message.
3. Replay s3tables-namespace-error-paths step task#10: CreateNamespace under a table bucket ARN that was never created and read the 404 message.
4. Replay s3tables-table-bucket-create-with-config step teardown#7: GetTableBucket on a deleted table bucket ARN and read the 404 message.
