Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3tables.create-table-invalid-format-rejected s3tables.put-table-bucket-encryption-invalid-algorithm-rejected s3tables.put-table-bucket-encryption-invalid-kms-key-rejected s3tables.delete-namespace-with-tables-rejected s3tables.put-table-bucket-replication-malformed-config-rejected s3tables.record-expiration-invalid-days-rejected s3tables.delete-table-stale-version-token-rejected s3tables.table-record-expiration-invalid-days-rejected
Issue-Shape-Id: com.amazonaws.s3tables#S3TableBuckets
Issue-Key: s3tables.create-table-invalid-format-rejected|run#6|AssertionError|assert code == "BadRequestException", f"expected BadRequestException, got {code}"
Issue-Key: s3tables.put-table-bucket-encryption-invalid-algorithm-rejected|run#4|AssertionError|assert code == "BadRequestException", f"expected BadRequestException, got {code!r}"
Issue-Key: s3tables.put-table-bucket-encryption-invalid-kms-key-rejected|run#7|AssertionError|assert code == "BadRequestException", f"expected BadRequestException, got {code!r}"
Issue-Key: s3tables.delete-namespace-with-tables-rejected|run#12|AssertionError|assert code == "BadRequestException", f"expected BadRequestException, got {code!r}"
Issue-Key: s3tables.put-table-bucket-replication-malformed-config-rejected|run#8|AssertionError|assert message == expected, f"message={message!r}"
Issue-Key: s3tables.record-expiration-invalid-days-rejected|run#16|AssertionError|assert message == expected, f"message={message!r}"
Issue-Key: s3tables.delete-table-stale-version-token-rejected|run#17|AssertionError|assert message == "Provided version token does not match the table version token.", message
Issue-Key: s3tables.table-record-expiration-invalid-days-rejected|run#8|AssertionError|assert invalid_code == "MethodNotAllowedException", (
Issue-Commit: 2483fdb6af8e19e16b529c88e6a2504bdf5a7762

# s3tables refusals diverge: enum/pattern violations answer ValidationException instead of BadwRequestException, non-empty DeleteNamespace answers 409, ValidationException text echoes the value and anchors the pattern, and an over-range days value pre-empts the record-expiration gate

> _Note: this issue was identified against the code base at commit `2483fdb6af8e19e16b529c88e6a2504bdf5a7762`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutTableBucketEncryption

## Description
Seven causes in the s3tables refusal paths that the generic framework validator and the handlers' error construction get wrong. They are independent of the scaffold-stub issues in this run: every one of these refusals is produced BEFORE the operation's handler runs (the generic constraint validator) or by an implemented handler, so implementing the stubs does not close any of them. Each cause must be closed before this issue is submitted.

Cause 1 of 7: CreateTable with a `format` outside the OpenTableFormat enum.
AWS: 400 BadRequestException with message exactly "The specified open table format is not valid" — all fixed text, no variable part; no table is created. Smithy: CreateTableRequest.format targets OpenTableFormat (enum, sole value ICEBERG) and CreateTable models BadRequestException(400); there is NO ValidationException shape anywhere in com.amazonaws.s3tables.
Emulator: 400 with error code `ValidationException` (unmodeled) and the framework's generic constraint text.
Why: `catch BadRequestException` never fires and the SDK maps the unmodeled code to an unknown error, so validation/retry branches written against AWS take the wrong path.
Fix: CreateTable's input validation — refuse an out-of-enum format with the service's BadRequestException and AWS's fixed message instead of letting the generic validator answer.
Traces: s3tables.create-table-invalid-format-rejected.

Cause 2 of 7: PutTableBucketEncryption with an unsupported sseAlgorithm.
AWS: 400 BadRequestException with message exactly "The specified SSE algorithm is invalid." (fixed text); the bucket's default stays SSE-S3/AES256. Smithy: EncryptionConfiguration.sseAlgorithm targets SSEAlgorithm (enum: AES256, aws:kms), required; PutTableBucketEncryption models BadRequestException(400).
Emulator: 400 `ValidationException` from the generic enum check.
Why/Fix: as cause 1, in PutTableBucketEncryption's validation.
Traces: s3tables.put-table-bucket-encryption-invalid-algorithm-rejected.

Cause 3 of 7: PutTableBucketEncryption with a malformed kmsKeyArn under sseAlgorithm=aws:kms.
AWS: 400 BadRequestException with message exactly "The specified KMS key ARN is invalid. Only key ARN format 'arn:<partition>:kms:<region>:<account-id>:key/<key-id>' is supported." — fixed text (the <partition>/<region>/<account-id>/<key-id> placeholders are literal in AWS's message, not substituted); nothing is persisted. Smithy: EncryptionConfiguration.kmsKeyArn @length(1,2048) @pattern(^(arn:aws[-a-z0-9]*:kms:[-a-z0-9]*:[0-9]{12}:key/.+)$).
Emulator: 400 `ValidationException` quoting the supplied value and the regex.
Why/Fix: as cause 1, in PutTableBucketEncryption's KMS-ARN validation.
Traces: s3tables.put-table-bucket-encryption-invalid-kms-key-rejected.

Cause 4 of 7: DeleteNamespace on a namespace that still contains a table.
AWS: 400 BadRequestException with message exactly "The namespace that you tried to delete is not empty." (fixed text — no namespace or table name appears in it); the namespace and its table remain intact, and the delete succeeds once the table is removed. Smithy: DeleteNamespace models BadRequestException(400) and ConflictException(409) separately, and the recorded refusal is the 400.
Emulator: 409 `ConflictException`.
Why: AWS documents 409/ConflictException as "a conflict with a previous write. You can retry the request", so a client retries a delete that can never succeed until the namespace is emptied instead of failing fast on a 400 client error.
Fix: DeleteNamespace's non-empty check — raise BadRequestException(400) with AWS's fixed message. This is the DeleteNamespace twin of cause 1 of open issue 20261101T230800Z-s3tables-conflict-and-not-found-errors-use-invented.md (which covers DeleteTableBucket on a non-empty bucket only); that issue does not name DeleteNamespace.
Traces: s3tables.delete-namespace-with-tables-rejected.

Cause 5 of 7: the ValidationException message template on the paths where AWS really does answer ValidationException.
AWS: for PutTableBucketReplication with a destination that is not a well-formed table-bucket ARN, and for PutTableRecordExpirationConfiguration with value.settings.days below the modeled minimum, AWS answers ValidationException whose message is "1 validation error detected: Value at '<member path>' failed to satisfy constraint: <constraint>" — the offending VALUE is NOT quoted, and where the constraint is a pattern the regex is rendered WITHOUT the ^ and $ anchors. Fixed text: "1 validation error detected: Value at '", "' failed to satisfy constraint: Member must satisfy regular expression pattern: " / "Member must have value greater than or equal to "; variable parts: the member path (from the request's shape path, e.g. configuration.rules.1.member.destinations.1.member.destinationTableBucketARN, value.settings.days) and the constraint's own limit/pattern (from the model: (arn:aws[-a-z0-9]*:[a-z0-9]+:[-a-z0-9]*:[0-9]{12}:bucket/[a-z0-9_-]{3,63}), 1). Recorded behavior for the omission of the value.
Emulator: interpolates the request value into the message ("Value 'not-an-arn' at ...", "Value '0' at ...") and emits the pattern with its ^(...)$ anchors.
Why: a client matching the refusal reason on the message — the only discriminator these errors carry — does not recognise it, and the echoed value makes the message vary per request where AWS's does not.
Fix: the framework's generic constraint-violation message formatter used by this service's serializer: drop the quoted value and strip the ^/$ anchors from the reported pattern.
Traces: s3tables.put-table-bucket-replication-malformed-config-rejected, s3tables.record-expiration-invalid-days-rejected.

Cause 6 of 7: DeleteTable with a stale versionToken reports an invented message.
AWS: 409 ConflictException with message exactly "Provided version token does not match the table version token." — all fixed text, no table name; the table is NOT deleted and deleting with the current token still succeeds.
Emulator: 409 ConflictException with "The version token provided for the table <table-name> does not match its current version token.", interpolating the table name from state.
Why: the message is ConflictException's only discriminator, so a client cannot recognise the optimistic-concurrency failure.
Fix: DeleteTable's version-token precondition error construction. Same root cause as cause 5 of open issue 20261101T230800Z-s3tables-conflict-and-not-found-errors-use-invented.md, recorded there from the wire corpus; filed here because the behavioral check carrying this replay key is not listed on that issue, so re-running it is the only way to confirm the fix.
Traces: s3tables.delete-table-stale-version-token-rejected.

Cause 7 of 7: PutTableRecordExpirationConfiguration with days above the modeled maximum pre-empts the table-eligibility gate.
AWS: on a customer-created ICEBERG table, days=2147483648 (one past the modeled maximum) draws the SAME refusal as a well-formed request — 405 MethodNotAllowedException, message exactly "Table record expiration configuration API calls to this table are not allowed" (fixed text) — i.e. the table-eligibility gate runs before any upper-bound check, and AWS does not enforce the modeled maximum as a request-validation refusal. Smithy: TableRecordExpirationSettings.days targets PositiveInteger @range(min 1, max 2147483647); only the minimum is observable as a ValidationException (cause 5). Recorded behavior for the ordering.
Emulator: 400 `ValidationException` from the pre-handler range check, so the value never reaches the eligibility gate.
Why: a client sees a parameter error where AWS reports an unsupported operation for that table, and cannot tell that record expiration is unavailable for its table at all.
Fix: the record-expiration input validation / handler ordering — do not refuse the modeled upper bound before the table-eligibility gate.
Traces: s3tables.table-record-expiration-invalid-days-rejected.

## Evidence
Replay of simparity-behavioral-aws-v1 against this checkout (s3tables 1/110 passed). Status 400 is returned where 400 is expected in causes 1-3 and 5, so only the error code and message diverge there; causes 4 and 7 also diverge on status.
- s3tables.create-table-invalid-format-rejected: CreateTable(format="PARQUET") — "expected BadRequestException, got ValidationException"; expected $.message "The specified open table format is not valid".
- s3tables.put-table-bucket-encryption-invalid-algorithm-rejected: encryptionConfiguration={"sseAlgorithm":"des-not-a-real-algorithm"} — "expected BadRequestException, got 'ValidationException'"; expected "The specified SSE algorithm is invalid.".
- s3tables.put-table-bucket-encryption-invalid-kms-key-rejected: {"sseAlgorithm":"aws:kms","kmsKeyArn":"not-a-valid-kms-key-arn"} — "expected BadRequestException, got 'ValidationException'"; expected "The specified KMS key ARN is invalid. Only key ARN format 'arn:<partition>:kms:<region>:<account-id>:key/<key-id>' is supported.".
- s3tables.delete-namespace-with-tables-rejected: "expected BadRequestException, got 'ConflictException'" (409 instead of 400); expected "The namespace that you tried to delete is not empty.".
- s3tables.put-table-bucket-replication-malformed-config-rejected: code ValidationException matches, message does not — expected "1 validation error detected: Value at 'configuration.rules.1.member.destinations.1.member.destinationTableBucketARN' failed to satisfy constraint: Member must satisfy regular expression pattern: (arn:aws[-a-z0-9]*:[a-z0-9]+:[-a-z0-9]*:[0-9]{12}:bucket/[a-z0-9_-]{3,63})", got "1 validation error detected: Value 'not-an-arn' at 'configuration.rules.1.member.destinations.1.member.destinationTableBucketARN' failed to satisfy constraint: Member must satisfy regular expression pattern: ^(arn:aws[-a-z0-9]*:[a-z0-9]+:[-a-z0-9]*:[0-9]{12}:bucket/[a-z0-9_-]{3,63})$".
- s3tables.record-expiration-invalid-days-rejected: days=0 with client-side param validation disabled — expected "1 validation error detected: Value at 'value.settings.days' failed to satisfy constraint: Member must have value greater than or equal to 1", got "1 validation error detected: Value '0' at 'value.settings.days' failed to satisfy constraint: Member must have value greater than or equal to 1".
- s3tables.delete-table-stale-version-token-rejected: expected "Provided version token does not match the table version token.", got "The version token provided for the table sim2real_tbl_366e42eb3af0 does not match its current version token." — the table name is a per-run value; the defect is that AWS's message contains no such value.
- s3tables.table-record-expiration-invalid-days-rejected: days=2147483648 on a customer ICEBERG table — "expected the eligibility gate (MethodNotAllowedException) to precede days validation on an ineligible table, got 'ValidationException'" (400 instead of 405).
Model check (smithy_query, services/service-com-amazonaws-s3tables-s3tablebuckets/smithy-model/com-amazonaws-s3tables-s3tablebuckets.json): OpenTableFormat values = [ICEBERG]; SSEAlgorithm values = [AES256, aws:kms]; EncryptionConfiguration.kmsKeyArn @pattern ^(arn:aws[-a-z0-9]*:kms:[-a-z0-9]*:[0-9]{12}:key/.+)$; TableRecordExpirationSettings.days @range(1, 2147483647); DeleteNamespace errors include BadRequestException(400) and ConflictException(409); PutTableRecordExpirationConfiguration errors include MethodNotAllowedException(405). No ValidationException shape exists in the service model — related open issue 20261101T230800Z-s3tables-rejects-invalid-input-with-unmodeled-validationexception-instead.md records the same fault for the `name`/`namespace`/`resourceArn` members and does NOT cover `format`, `sseAlgorithm` or `kmsKeyArn`.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay simparity-behavioral-aws-v1.
2. Create a table bucket and a namespace, then CreateTable(format="PARQUET") — read the error code and message (expect 400 BadRequestException "The specified open table format is not valid").
3. On the bucket call PutTableBucketEncryption with encryptionConfiguration={"sseAlgorithm":"des-not-a-real-algorithm"}, then with {"sseAlgorithm":"aws:kms","kmsKeyArn":"not-a-valid-kms-key-arn"} — expect 400 BadRequestException with the two fixed messages above.
4. Create an ICEBERG table in the namespace and call DeleteNamespace — expect 400 BadRequestException "The namespace that you tried to delete is not empty.", observe 409 ConflictException.
5. Call PutTableBucketReplication with rules[0].destinations[0].destinationTableBucketARN="not-an-arn", and PutTableRecordExpirationConfiguration with value={"settings":{"days":0},"status":"enabled"} using a client configured with parameter_validation=False — compare $.message against the two expected strings (no quoted value, unanchored pattern).
6. DeleteTable with a versionToken that is not the table's current one — read $.message on the 409.
7. PutTableRecordExpirationConfiguration on that same customer-created ICEBERG table with value={"status":"enabled","settings":{"days":2147483648}} — expect 405 MethodNotAllowedException "Table record expiration configuration API calls to this table are not allowed", observe 400 ValidationException.
