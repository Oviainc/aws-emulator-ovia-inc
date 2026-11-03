Issue-Source: trace-sweep-divergence
Issue-Trace: s3tables-rename-table-conflict-matrix
Issue-Step: task#14
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3tables-rename-table-conflict-matrix s3tables-s3t-get-table-with-both-the-arn-and-the-triple-rejected s3tables-s3t-table-bucket-identity-matches-the-listing-row s3tables-s3t-uppercase-and-hyphenated-table-names-rejected s3tables-sct-create-table-input-mistakes s3tables-sct-sort-order-at-create-via-write-order s3tables-sct-table-bucket-name-mistakes
Issue-Shape-Id: com.amazonaws.s3tables#S3TableBuckets
Issue-Key: s3tables|ListTables|2xx->2xx|$.tables[name="sim2real_tbl_one_9410bac6"].createdAt
Issue-Key: s3tables|ListTables|2xx->2xx|$.tables[name="sim2real_tbl_one_9410bac6"].modifiedAt
Issue-Key: s3tables|ListTables|2xx->2xx|$.tables[name="sim2real_tbl_one_9410bac6"].tableARN
Issue-Key: s3tables|ListTables|2xx->2xx|$.tables[name="sim2real_tbl_two_9410bac6"].createdAt
Issue-Key: s3tables|ListTables|2xx->2xx|$.tables[name="sim2real_tbl_two_9410bac6"].modifiedAt
Issue-Key: s3tables|ListTables|2xx->2xx|$.tables[name="sim2real_tbl_two_9410bac6"].tableARN
Issue-Key: s3tables|GetTable|2xx->2xx|$.versionToken
Issue-Key: s3tables|ListTableBuckets|2xx->2xx|$.tableBuckets[*].createdAt
Issue-Key: s3tables|ListTables|2xx->2xx|$.tables[name="user_events_with_sort_compaction"].createdAt
Issue-Key: s3tables|ListTables|2xx->2xx|$.tables[name="user_events_with_sort_compaction"].modifiedAt
Issue-Key: s3tables|ListTables|2xx->2xx|$.tables[name="user_events_no_columns"].createdAt
Issue-Key: s3tables|ListTables|2xx->2xx|$.tables[name="user_events_no_columns"].modifiedAt
Issue-Key: s3tables|ListTables|2xx->2xx|$.tables[name="user_events_with_sort_compaction"].tableARN
Issue-Key: s3tables|ListTables|2xx->2xx|$.tables[name="user_events_without_compaction"].createdAt
Issue-Key: s3tables|ListTables|2xx->2xx|$.tables[name="user_events_without_compaction"].modifiedAt
Issue-Key: s3tables|ListTableBuckets|2xx->2xx|$.tableBuckets[*].tableBucketId
Issue-Related: 20261101T230800Z-s3tables-read-responses-diverge-timestamps-emitted-as-epoch.md
Issue-Commit: 56c3e2d5db4619f120a986f75a3622f8f9af8979

# s3tables list and get responses emit epoch timestamps and non-UUID server ids

> _Note: this issue was identified against the code base at commit `56c3e2d5db4619f120a986f75a3622f8f9af8979`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListTables

## Description
Successful s3tables reads return the right members with the wrong wire shapes: timestamps are serialized as epoch numbers rather than the modeled date-time strings, and server-minted identifiers are 32-character hex strings rather than the UUID form the service mints (including the id segment inside tableARN). 3 causes, all in the s3tables response serializer / id minting in state.

Two open issues carry this fault for other clusters - 20261101T230800Z-s3tables-read-responses-diverge-timestamps-emitted-as-epoch.md and 20261101T230800Z-s3tables-listtables-summaries-carry-epoch-timestamps-and-unrecorded.md - but the replay reports the eight clusters below as unowned, so they are filed here and those issues are named for cross-reference.

Cause 1 of 3 - timestamps are JSON numbers instead of date-time strings.
AWS: Smithy: GetTable.createdAt / GetTable.modifiedAt and GetTableBucket.createdAt target Timestamp with timestampFormat 'date-time'; the TableSummary and TableBucketSummary members in ListTables/ListTableBuckets carry the same timestamps. Recorded responses carry RFC3339 strings with nanosecond precision (e.g. an ISO-8601 'Z' instant), never a number.
Emulator: emits an epoch float (a bare JSON number) for createdAt and modifiedAt in ListTables summaries, ListTableBuckets summaries and GetTable.
Why: 'date-time' means the generated SDKs parse a string; a JSON number either fails to deserialize or is silently interpreted as a different instant, so every client reading creation/modification time off a list or get is broken.
Fix: the s3tables response serializer must render Timestamp members per the modeled date-time format for all read responses, not only the ones already covered by the open issues.
Traces: s3tables-rename-table-conflict-matrix#task#14 (representative) and #task#15, s3tables-s3t-uppercase-and-hyphenated-table-names-rejected#task#7, s3tables-sct-create-table-input-mistakes#task#9, s3tables-sct-sort-order-at-create-via-write-order#task#12 (ListTables); s3tables-s3t-table-bucket-identity-matches-the-listing-row#task#3 and s3tables-sct-table-bucket-name-mistakes#task#4 (ListTableBuckets); s3tables-s3t-get-table-with-both-the-arn-and-the-triple-rejected#task#6 (GetTable).

Cause 2 of 3 - namespaceId / tableBucketId are not UUIDs.
AWS: recorded behavior only (the model types NamespaceId/TableBucketId as plain strings): every recorded response mints these as a canonical 8-4-4-4-12 hyphenated UUID.
Emulator: mints a 32-character unhyphenated hex digest for namespaceId and tableBucketId (GetTable, GetNamespace, ListTables summaries, ListTableBuckets summaries).
Why: clients and fixtures that parse or validate these ids as UUIDs reject the emulator's value; ids are also opaque keys clients round-trip, so a non-conforming form leaks into their storage.
Fix: id minting for namespaces and table buckets in the service's state layer must produce the UUID form, and the serializer must emit it unchanged.
Traces: s3tables-s3t-get-table-with-both-the-arn-and-the-triple-rejected#task#6 (GetTable namespaceId/tableBucketId), s3tables-sct-table-bucket-name-mistakes#task#4 (ListTableBuckets tableBucketId).

Cause 3 of 3 - the table id inside tableARN, and versionToken, are not UUIDs either.
AWS: Smithy: TableARN pattern ^(arn:aws[-a-z0-9]*:[a-z0-9]+:[-a-z0-9]*:[0-9]{12}:bucket/[a-z0-9_-]{3,63}/table/[a-zA-Z0-9-_]{1,255})$, versionToken is a required string (length 1..2048). Recorded behavior: the segment after /table/ is a hyphenated UUID, and versionToken is likewise minted in the recorded id form.
Emulator: the /table/ segment is a 32-character hex digest, and versionToken is a 32-character hex digest.
Why: the ARN is the handle clients store and re-submit; a client or test that recognises the AWS id form in an ARN (or correlates an ARN with a UUID table id elsewhere) does not match, and the two id spaces are no longer interchangeable.
Fix: the table id minted in state (used to build tableARN) and the versionToken generator must use the service's id form; the ARN assembler then needs no change.
Traces: s3tables-rename-table-conflict-matrix#task#14 and #task#15 (ListTables tableARN), s3tables-sct-create-table-input-mistakes#task#9 and s3tables-sct-sort-order-at-create-via-write-order#task#12 (ListTables tableARN), s3tables-s3t-get-table-with-both-the-arn-and-the-triple-rejected#task#6 (GetTable versionToken).

## Evidence
Cause 1: ListTables - $.tables[name=...].createdAt / .modifiedAt expected a date-time string shape, actual a bare number (e.g. 1793676719.5148904); ListTableBuckets - $.tableBuckets[0].createdAt reported type_mismatch, string expected vs number returned; GetTable - $.createdAt / $.modifiedAt likewise numbers. The specific instants are per-run values; the JSON type is the requirement.
Cause 2: GetTable - $.namespaceId actual 'd860b616184dca7f4c9fdbcb58df5703', $.tableBucketId actual '6c8d396c2499a95365ba7bb1208b6848' against a recorded hyphenated-UUID shape; ListTableBuckets - $.tableBuckets[0].tableBucketId actual '81c2c4e030a917e1500ff2113269094d'. Recorded examples of the expected form: '6fbf2002-66c1-4b4c-8e6e-0d2451eb2da4', '851c4c17-fa71-48aa-bc24-0d536d54f24b' (per-run values; the 8-4-4-4-12 form is the requirement).
Cause 3: ListTables - $.tables[name=...].tableARN actual '...:bucket/<bucket>/table/5621a692db638ca859606a1ad9412486' where the recorded shape has a hyphenated UUID after /table/; GetTable - $.versionToken actual '184ca9ff6594b061944a1b5a7f42bfd1'. All eight clusters returned status 200 as expected; only the serialized shapes diverge.

## Steps to Reproduce
1. Replay s3tables-rename-table-conflict-matrix step task#14 (and task#15): ListTables and inspect each summary's createdAt/modifiedAt JSON type and the id segment after /table/ in tableARN.
2. Replay s3tables-s3t-get-table-with-both-the-arn-and-the-triple-rejected step task#6: GetTable and inspect createdAt, modifiedAt, namespaceId, tableBucketId and versionToken.
3. Replay s3tables-s3t-table-bucket-identity-matches-the-listing-row step task#3 and s3tables-sct-table-bucket-name-mistakes step task#4: ListTableBuckets and inspect tableBuckets[].createdAt (type) and tableBuckets[].tableBucketId (form).
4. Replay s3tables-s3t-uppercase-and-hyphenated-table-names-rejected step task#7, s3tables-sct-create-table-input-mistakes step task#9 and s3tables-sct-sort-order-at-create-via-write-order step task#12: ListTables summary timestamps and tableARNs.
