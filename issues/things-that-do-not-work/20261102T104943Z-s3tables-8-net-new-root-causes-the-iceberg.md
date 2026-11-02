Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3tables.s3t-iceberg-rest-config-returns-the-warehouse-arn-as-the-prefix s3tables.s3t-iceberg-rest-and-the-control-plane-list-the-same-tables s3tables.s3t-iceberg-rest-load-table-assigns-field-ids-in-declaration-order s3tables.s3t-iceberg-rest-commit-with-a-mismatched-table-uuid-rejected s3tables.s3t-a-sort-order-commit-advances-the-metadata-location-and-version-token s3tables.s3t-two-sibling-tables-hold-opposite-compaction-statuses s3tables.s3t-bucket-policy-delete-when-absent-is-a-no-op-like-metrics-configuration s3tables.s3t-invalid-namespace-rejected-by-create-but-notfound-elsewhere s3tables.s3t-uppercase-and-hyphenated-table-names-rejected s3tables.s3t-get-table-with-both-the-arn-and-the-triple-rejected s3tables.s3t-non-empty-namespace-and-bucket-deletions-rejected s3tables.s3t-duplicate-table-name-rejected-while-a-second-namespace-accepts-it s3tables.s3t-namespace-round-trips-as-a-single-element-list-with-its-bucket-id s3tables.s3t-table-bucket-identity-matches-the-listing-row s3tables.s3t-rename-table-moves-the-name-and-keeps-the-table-arn
Issue-Shape-Id: com.amazonaws.s3tables#S3TableBuckets
Issue-Key: s3tables.s3t-iceberg-rest-config-returns-the-warehouse-arn-as-the-prefix|run#6|AssertionError|assert status == 200, (
Issue-Key: s3tables.s3t-iceberg-rest-and-the-control-plane-list-the-same-tables|run#16|AssertionError|assert status == 200, (
Issue-Key: s3tables.s3t-iceberg-rest-load-table-assigns-field-ids-in-declaration-order|run#11|AssertionError|assert status == 200, (
Issue-Key: s3tables.s3t-iceberg-rest-commit-with-a-mismatched-table-uuid-rejected|run#12|AssertionError|assert status == 200, f"loading the table returned HTTP {status}: {loaded!r}"
Issue-Key: s3tables.s3t-a-sort-order-commit-advances-the-metadata-location-and-version-token|run#13|AssertionError|assert status == 200, f"loading the table before the commit returned HTTP {status}: {loaded!r}"
Issue-Key: s3tables.s3t-two-sibling-tables-hold-opposite-compaction-statuses|run#13|AssertionError|assert status == 200, f"loading {with_sort!r} over the catalog returned HTTP {status}: {loaded!r}"
Issue-Key: s3tables.s3t-bucket-policy-delete-when-absent-is-a-no-op-like-metrics-configuration|run#5|AssertionError|assert missing_code == "NotFoundException", (
Issue-Key: s3tables.s3t-invalid-namespace-rejected-by-create-but-notfound-elsewhere|run#9|AssertionError|assert code == "NotFoundException", (
Issue-Key: s3tables.s3t-uppercase-and-hyphenated-table-names-rejected|run#9|AssertionError|assert get_code == "NotFoundException", (
Issue-Key: s3tables.s3t-get-table-with-both-the-arn-and-the-triple-rejected|_error#10|AssertionError|raise AssertionError(
Issue-Key: s3tables.s3t-non-empty-namespace-and-bucket-deletions-rejected|run#10|AssertionError|assert occupied_code == "BadRequestException", (
Issue-Key: s3tables.s3t-duplicate-table-name-rejected-while-a-second-namespace-accepts-it|run#12|AssertionError|assert got == expected, (
Issue-Key: s3tables.s3t-namespace-round-trips-as-a-single-element-list-with-its-bucket-id|run#16|AssertionError|assert str(_uuid.UUID(described["namespaceId"])) == described["namespaceId"], (
Issue-Key: s3tables.s3t-table-bucket-identity-matches-the-listing-row|run#13|AssertionError|assert str(_uuid.UUID(described["tableBucketId"])) == described["tableBucketId"], (
Issue-Key: s3tables.s3t-rename-table-moves-the-name-and-keeps-the-table-arn|run#12|AssertionError|assert "modifiedBy" not in before, (
Issue-Commit: 717da2edf9c8dae639a46753310fbbbfd49465e2

# s3tables: 8 net-new root causes — the Iceberg REST catalog is entirely unserved (501), GetTableBucketPolicy answers 501, lookups re-validate names instead of answering NotFound, GetTable accepts ARN+triple together, non-empty deletes answer 409, invented conflict message, non-UUID server ids, and modifiedBy present on a never-modified table

> _Note: this issue was identified against the code base at commit `717da2edf9c8dae639a46753310fbbbfd49465e2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetTable

## Description
Eight distinct net-new root causes in the s3tables emulator, each grounded in failing behavioral checks. NOTE: this issue carries 8 SEPARATE causes — all 8 must be closed before submitting.

=== Cause 1 of 8: the Iceberg REST catalog surface is not served at all (HTTP 501 NotImplemented) ===
AWS: recorded behavior only — the Iceberg REST catalog is not part of the s3tables Smithy model (no operation in services/service-com-amazonaws-s3tables-s3tablebuckets/smithy-model/com-amazonaws-s3tables-s3tablebuckets.json binds /iceberg/v1/*). Real S3 Tables serves an Iceberg REST catalog alongside the control plane at /iceberg/v1: GET /iceberg/v1/config returns 200 with the warehouse table-bucket ARN as the `prefix`/overrides value; GET /iceberg/v1/{prefix}/namespaces returns 200 with the same namespaces the control plane's ListNamespaces lists; GET /iceberg/v1/{prefix}/namespaces/{namespace}/tables/{table} (LoadTable) returns 200 with the table's metadata document (schema fields carrying field-ids in declaration order, metadata-location, sort order); and the commit endpoint accepts a table update, advances the metadata location and the version token, and refuses a commit whose table UUID does not match the loaded table.
Emulator: every /iceberg/v1 route answers HTTP 501 with body {"__type": "NotImplemented", "message": "operation GET /iceberg/v1/... is not implemented in this emulator"} — observed for GET /iceberg/v1/config, GET /iceberg/v1/<url-encoded bucket ARN>/namespaces and GET /iceberg/v1/<url-encoded bucket ARN>/namespaces/<namespace>/tables/<table>. The fixed text is "operation <METHOD> <path> is not implemented in this emulator"; the path, bucket ARN, namespace and table name in each recorded message are values taken from the request, not part of the requirement.
Why: any Iceberg engine (Spark, Trino, PyIceberg, Athena) configured against the emulator's S3 Tables catalog cannot discover the warehouse, list namespaces, load a table or commit a snapshot — the entire data-plane path of S3 Tables is unusable, and a client that only reads tables through the catalog sees a service with zero tables.
Fix: the s3tables crate's Iceberg REST catalog router and its handlers (the non-Smithy /iceberg/v1 surface of services/service-com-amazonaws-s3tables-s3tablebuckets), projecting the same namespace/table state the control-plane ops use.
Traces: s3tables.s3t-iceberg-rest-config-returns-the-warehouse-arn-as-the-prefix, s3tables.s3t-iceberg-rest-and-the-control-plane-list-the-same-tables, s3tables.s3t-iceberg-rest-load-table-assigns-field-ids-in-declaration-order, s3tables.s3t-iceberg-rest-commit-with-a-mismatched-table-uuid-rejected, s3tables.s3t-a-sort-order-commit-advances-the-metadata-location-and-version-token, s3tables.s3t-two-sibling-tables-hold-opposite-compaction-statuses.

=== Cause 2 of 8: GetTableBucketPolicy answers 501 NotImplemented instead of 404 NotFoundException for a bucket with no policy ===
AWS: Smithy: GetTableBucketPolicy GET /buckets/{tableBucketARN}/policy, output member `resourcePolicy` required; modeled errors include NotFoundException (httpError 404, "The request was rejected because the specified resource could not be found."). Recorded behavior: reading a policy that was never put is refused with code NotFoundException and the message "The specified bucket policy does not exist." (fixed text, no variable parts), and the same 404 is the answer after DeleteTableBucketPolicy no-ops past an absent policy and after a real policy is deleted.
Emulator: the call is refused with code 'NotImplemented' (HTTP 501), so the whole bucket-policy sub-resource round trip (put -> get -> delete -> get) cannot be exercised.
Why: a client cannot distinguish "no policy" from "service broken"; policy-gated flows and any caller that catches NotFoundException to mean "unset" see a retryable/server error instead, and SDK error handling for the modeled 404 never triggers.
Fix: the GetTableBucketPolicy handler and the bucket-policy state in the s3tables crate's ops (services/service-com-amazonaws-s3tables-s3tablebuckets/src/ops/get_table_bucket_policy.rs and the matching put/delete).
Traces: s3tables.s3t-bucket-policy-delete-when-absent-is-a-no-op-like-metrics-configuration.

=== Cause 3 of 8: lookup operations re-run the create-time name format check and answer BadRequestException where AWS answers NotFoundException ===
AWS: Smithy: NamespaceName @pattern ^[0-9a-z_]*$ and TableName @pattern ^[0-9a-z_]*$; CreateNamespace/CreateTable refuse a non-conforming name with BadRequestException (400). Recorded behavior: read/lookup operations handed the same non-conforming string do NOT format-check it — they look it up, find nothing, and answer NotFoundException (404). GetNamespace, DeleteNamespace, CreateTable (on the namespace path) and ListTables against namespace "Analytics" all answer NotFoundException with the fixed message "The specified namespace does not exist."; GetTable on table name "User_Events_With_Sort_Compaction" answers NotFoundException with the fixed message "The specified table does not exist." (The create-time refusals carry their own fixed texts: "The specified namespace name isn’t valid. Specify a different namespace name, and then try again." and "The specified table name is not valid.")
Emulator: GetNamespace on 'Analytics' is rejected with 'BadRequestException', and GetTable on 'User_Events_With_Sort_Compaction' is rejected with 'BadRequestException' — the input pattern is enforced as request validation on read paths instead of being treated as a lookup key.
Why: callers that treat 404 as "absent" and 400 as "my request is malformed" branch wrongly: existence probes raise a non-retryable client error, and idempotent "delete if present" / "create if missing" logic breaks on any legacy or externally supplied name.
Fix: the validation layer for the read/lookup handlers (GetNamespace, DeleteNamespace, ListTables, GetTable, and CreateTable's namespace resolution) in the s3tables crate — the member pattern must gate only the create paths, with lookups resolving against state. Related open issue 20261102T055316Z-s3tables-constraint-violation-refusals-are-now-blanket-400.md covers the code/text of constraint refusals; it does not cover lookups validating at all.
Traces: s3tables.s3t-invalid-namespace-rejected-by-create-but-notfound-elsewhere, s3tables.s3t-uppercase-and-hyphenated-table-names-rejected.

=== Cause 4 of 8: GetTable accepts mutually exclusive input shapes instead of refusing them ===
AWS: Smithy: GetTable GET /get-table with four optional httpQuery members (tableBucketARN, namespace, name, tableArn). Recorded behavior: exactly one of the two legal shapes is allowed — the table ARN alone, OR all three of (tableBucketARN, namespace, name) together. Both the ARN-and-triple-together form and a PARTIAL triple (bucket ARN alone, or namespace+name with no bucket ARN) are refused with BadRequestException (400) and the identical fixed message "Invalid input: Must provide either table arn alone OR all of (table bucket arn, namespace, table name) together" (entirely fixed text, no variable parts).
Emulator: GetTable with tableArn AND the full triple SUCCEEDS with HTTP 200 and returns the table record (name, tableARN, namespace, versionToken, ... ), so the ambiguous request is silently resolved instead of refused.
Why: a client that passes a stale ARN together with a fresh triple (or vice versa) gets an answer derived from whichever member the emulator happened to prefer rather than a refusal, so a real mismatch between the two identifiers is never surfaced; negative-path tests written against AWS pass against the emulator and fail in production.
Fix: GetTable's input validation in the s3tables crate's ops (services/service-com-amazonaws-s3tables-s3tablebuckets/src/ops/get_table.rs) — reject before resolution when both forms are supplied and when the triple is partial.
Traces: s3tables.s3t-get-table-with-both-the-arn-and-the-triple-rejected.

=== Cause 5 of 8: non-empty DeleteNamespace / DeleteTableBucket are refused as 409 ConflictException instead of 400 BadRequestException ===
AWS: Smithy: DeleteNamespace DELETE /namespaces/{tableBucketARN}/{namespace} models both BadRequestException (400) and ConflictException (409). Recorded behavior: deleting a namespace that still holds a table is refused with BadRequestException and the fixed message "The namespace that you tried to delete is not empty."; deleting a table bucket that still holds a namespace is refused with BadRequestException and the fixed message "The bucket that you tried to delete is not empty." Both messages are entirely fixed text. The refusals leave the namespace, table and bucket intact, and the deletes are accepted bottom-up once each is emptied.
Emulator: deleting a namespace that still holds a table is refused with 'ConflictException' (HTTP 409).
Why: a client distinguishing a retryable write conflict (409 ConflictException, which the model documents as "you can retry the request") from a permanent precondition failure retries the delete forever instead of emptying the namespace first; teardown automation hangs rather than failing fast.
Fix: the non-empty precondition in the DeleteNamespace and DeleteTableBucket handlers / their error mapping in the s3tables crate. Related open issues 20261101T230800Z-s3tables-conflict-and-not-found-errors-use-invented.md and 20261101T233620Z-s3tables-refusals-diverge-enum-pattern-violations-answer-validationexception.md describe the same family but do not carry this check's replay key.
Traces: s3tables.s3t-non-empty-namespace-and-bucket-deletions-rejected.

=== Cause 6 of 8: the duplicate-table-name ConflictException message is an invented per-request template ===
AWS: Smithy: CreateTable PUT /tables/{tableBucketARN}/{namespace} models ConflictException (409, "The request failed because there is a conflict with a previous write."). Recorded behavior: re-creating a table whose name already exists in the same namespace is refused with ConflictException and the fixed message "A table with an identical name already exists in the namespace." — the message is entirely fixed text and interpolates NEITHER the table name NOR the namespace name.
Emulator: the same call answers ('ConflictException', 'A table with the name <table> already exists in the namespace <namespace>.') — the code is right but the message is a locally invented template that splices the request's table and namespace names in. (The recorded run shows 'user_events_with_sort_compaction' and a generated namespace; those values fill the emulator's template and are not part of the requirement.)
Why: clients and test suites that match on AWS's exact message text (or log/ticket on it) do not recognise the refusal; the message is part of the error contract.
Fix: the duplicate-name conflict message in the CreateTable handler of the s3tables crate (services/service-com-amazonaws-s3tables-s3tablebuckets/src/ops/create_table.rs).
Traces: s3tables.s3t-duplicate-table-name-rejected-while-a-second-namespace-accepts-it.

=== Cause 7 of 8: namespaceId and tableBucketId are 32-char hex strings, not canonical UUIDs ===
AWS: Smithy: GetNamespace output member namespaceId -> NamespaceId (string), tableBucketId -> TableBucketId (string); the model states no pattern, so recorded behavior only: S3 Tables returns these server-assigned ids in canonical UUID form (8-4-4-4-12 lowercase hex with hyphens), and the id in the describe response equals the id on the corresponding listing row.
Emulator: GetNamespace returns a namespaceId such as a bare 32-hex string with no hyphens, and GetTableBucket/DescribeTableBucket returns a tableBucketId in the same bare 32-hex form, so neither parses back to its canonical UUID spelling. The specific recorded values are per-run and not part of the requirement; the shape (hyphen-less 32 hex vs canonical UUID) is.
Why: clients that parse these ids into a UUID type (or validate them against a UUID regex before storing them) fail outright, and ids round-tripped through such a client will not match what the emulator returns.
Fix: the id generation/serialization for namespaces and table buckets in the s3tables crate's state and read handlers (GetNamespace, ListNamespaces, GetTableBucket, ListTableBuckets). Related open issue 20261101T230800Z-s3tables-read-responses-diverge-timestamps-emitted-as-epoch.md names the same family but does not carry these checks' replay keys.
Traces: s3tables.s3t-namespace-round-trips-as-a-single-element-list-with-its-bucket-id, s3tables.s3t-table-bucket-identity-matches-the-listing-row.

=== Cause 8 of 8: GetTable reports modifiedBy on a table that was never modified ===
AWS: Smithy: GetTable output declares modifiedAt and modifiedBy (AccountId, 12 digits). Recorded behavior: a table that has never been modified since CreateTable comes back WITHOUT the modifiedBy key; the key appears only once the table has actually been modified (for example by RenameTable), and its value is then the modifying account id.
Emulator: immediately after CreateTable, GetTable already reports modifiedBy set to the caller's account id (recorded as the emulator's default 12-digit account; that value is environmental, the presence of the key is the divergence).
Why: a client that uses the presence of modifiedBy to tell "untouched since creation" from "modified" — audit/drift reporting, and the before/after comparison across a RenameTable — sees every freshly created table as already modified.
Fix: the GetTable (and ListTables/table projection) serializer and the table record's modification bookkeeping in the s3tables crate — emit modifiedBy only once a modification has been recorded.
Traces: s3tables.s3t-rename-table-moves-the-name-and-keeps-the-table-arn.

## Evidence
Behavioral suite: 11/149 passed; s3tables 5/139. The 15 failing checks below are the ones NOT marked ALREADY FILED / OWNED by an existing issue.

Cause 1 (Iceberg REST, 501):
- s3t-iceberg-rest-config-returns-the-warehouse-arn-as-the-prefix: AssertionError: GET /iceberg/v1/config for warehouse 'arn:aws:s3tables:<region>:<acct>:bucket/<bucket>' returned HTTP 501: {'__type': 'NotImplemented', 'message': 'operation GET /iceberg/v1/config is not implemented in this emulator'} — expected HTTP 200.
- s3t-iceberg-rest-and-the-control-plane-list-the-same-tables: AssertionError: GET /iceberg/v1/<prefix>/namespaces returned HTTP 501: {'__type': 'NotImplemented', ...} — expected HTTP 200.
- s3t-iceberg-rest-load-table-assigns-field-ids-in-declaration-order / s3t-iceberg-rest-commit-with-a-mismatched-table-uuid-rejected / s3t-a-sort-order-commit-advances-the-metadata-location-and-version-token / s3t-two-sibling-tables-hold-opposite-compaction-statuses: AssertionError: loading the table returned HTTP 501: {'__type': 'NotImplemented', 'message': 'operation GET /iceberg/v1/<encoded bucket ARN>/namespaces/<ns>/tables/<table> is not implemented in this emulator'} — expected HTTP 200.

Cause 2 (GetTableBucketPolicy):
- s3t-bucket-policy-delete-when-absent-is-a-no-op-like-metrics-configuration: AssertionError: reading a policy that was never put was refused with 'NotImplemented'; real S3 Tables answers NotFoundException rather than an empty document. Expected: code NotFoundException (HTTP 404), message "The specified bucket policy does not exist."

Cause 3 (lookups re-validate the name):
- s3t-invalid-namespace-rejected-by-create-but-notfound-elsewhere: AssertionError: get_namespace against the namespace 'Analytics', which CreateNamespace just refused to make, was rejected with 'BadRequestException'. Expected: NotFoundException, message "The specified namespace does not exist." (same for delete_namespace / create_table / list_tables on that namespace).
- s3t-uppercase-and-hyphenated-table-names-rejected: AssertionError: GetTable on the rejected name was answered with 'BadRequestException'. Expected: NotFoundException, message "The specified table does not exist." (CreateTable's own refusal: BadRequestException, "The specified table name is not valid.")

Cause 4 (GetTable ARN+triple):
- s3t-get-table-with-both-the-arn-and-the-triple-rejected: AssertionError: the call was expected to be rejected but succeeded, returning HTTPStatusCode 200 with the full table record ({'name': ..., 'tableARN': ..., 'versionToken': ..., 'format': 'ICEBERG', ...}). Expected: ('BadRequestException', 'Invalid input: Must provide either table arn alone OR all of (table bucket arn, namespace, table name) together'); the same refusal is expected for a partial triple (bucket ARN only, and namespace+name without the bucket ARN).

Cause 5 (non-empty deletes):
- s3t-non-empty-namespace-and-bucket-deletions-rejected: AssertionError: deleting a namespace that still holds a table was rejected with 'ConflictException'; real S3 Tables answers BadRequestException. Expected: BadRequestException (400) with "The namespace that you tried to delete is not empty.", and for the bucket "The bucket that you tried to delete is not empty."

Cause 6 (duplicate-name message):
- s3t-duplicate-table-name-rejected-while-a-second-namespace-accepts-it: AssertionError: re-creating '<table>' in namespace '<ns>' answered ('ConflictException', 'A table with the name <table> already exists in the namespace <ns>.'); real S3 Tables answers ('ConflictException', 'A table with an identical name already exists in the namespace.').

Cause 7 (non-UUID ids):
- s3t-namespace-round-trips-as-a-single-element-list-with-its-bucket-id: AssertionError: namespaceId '<32 hex chars, e.g. 1ad498cf33f0aa2b52667b26f7065f7e>' is not the canonical form of a UUID.
- s3t-table-bucket-identity-matches-the-listing-row: AssertionError: tableBucketId '<32 hex chars, e.g. f7da7773f9a515c406c220e94c6def55>' is not the canonical form of a UUID.

Cause 8 (modifiedBy):
- s3t-rename-table-moves-the-name-and-keeps-the-table-arn: AssertionError: a table that has never been modified already reports modifiedBy '<account id>'; the key appearing is part of what the rename is graded on.

## Steps to Reproduce
Boot the emulator and replay the behavioral suite, or reproduce by hand against the s3tables endpoint:

Cause 1: create_table_bucket -> create_namespace -> create_table(format='ICEBERG'); then GET /iceberg/v1/config?warehouse=<bucketARN>, GET /iceberg/v1/<url-encoded bucketARN>/namespaces, and GET /iceberg/v1/<url-encoded bucketARN>/namespaces/<ns>/tables/<table>. Each answers 501 NotImplemented; AWS answers 200.
Cause 2: create_table_bucket, then get_table_bucket_policy(tableBucketARN=arn) before any put. Emulator: 501 NotImplemented; AWS: 404 NotFoundException "The specified bucket policy does not exist." Then delete_table_bucket_policy (no-op success), get again (same 404), put_table_bucket_policy, get (round-trips), delete, get (same 404).
Cause 3: create_namespace(namespace=['Analytics']) -> 400 BadRequestException (correct). Then get_namespace(namespace='Analytics') / delete_namespace / create_table(namespace='Analytics') / list_tables(namespace='Analytics'): emulator 400 BadRequestException, AWS 404 NotFoundException "The specified namespace does not exist." Likewise create_table(name='User_Events_With_Sort_Compaction') -> 400 (correct), then get_table(tableBucketARN, namespace, name='User_Events_With_Sort_Compaction'): emulator 400, AWS 404 "The specified table does not exist."
Cause 4: create a table, then get_table(tableArn=<arn>, tableBucketARN=<arn>, namespace=<ns>, name=<table>). Emulator returns 200 with the record; AWS refuses with BadRequestException and the 'either table arn alone OR all of ...' message. Also try get_table(tableBucketARN=<arn>) alone and get_table(namespace=<ns>, name=<table>).
Cause 5: create bucket + namespace + table, then delete_namespace -> emulator 409 ConflictException, AWS 400 BadRequestException "The namespace that you tried to delete is not empty."; then delete_table_bucket while the namespace exists -> expect 400 "The bucket that you tried to delete is not empty."
Cause 6: create_table twice with the same name in one namespace; compare the ConflictException message with AWS's fixed "A table with an identical name already exists in the namespace."
Cause 7: create_namespace then get_namespace -> inspect namespaceId; create_table_bucket then get_table_bucket -> inspect tableBucketId. Both are 32 hex chars without hyphens; AWS returns canonical UUIDs.
Cause 8: create_table, then get_table immediately and check for the modifiedBy key — the emulator already sets it; AWS omits it until the table is modified (e.g. by rename_table).
