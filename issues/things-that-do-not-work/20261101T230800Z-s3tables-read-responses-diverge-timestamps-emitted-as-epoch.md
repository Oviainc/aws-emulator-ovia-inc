Issue-Source: trace-sweep-divergence
Issue-Trace: s3tables-error-paths-bucket-namespace-conflicts
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3tables-error-paths-bucket-namespace-conflicts s3tables-table-bucket-create-with-config s3tables-table-bucket-lifecycle s3tables-error-paths-table-conflicts-and-notfound s3tables-table-create-and-inspect s3tables-table-lifecycle-iceberg s3tables-namespace-error-paths s3tables-namespace-lifecycle s3tables-namespace-listing-and-pagination s3tables-table-replication
Issue-Shape-Id: com.amazonaws.s3tables#S3TableBuckets
Issue-Key: s3tables|GetTableBucket|2xx->2xx|$.createdAt
Issue-Key: s3tables|GetTableBucket|2xx->2xx|$.tableBucketId
Issue-Key: s3tables|GetTable|2xx->2xx|$.createdAt
Issue-Key: s3tables|GetTable|2xx->2xx|$.modifiedAt
Issue-Key: s3tables|GetTable|2xx->2xx|$.namespaceId
Issue-Key: s3tables|GetTable|2xx->2xx|$.tableBucketId
Issue-Key: s3tables|GetNamespace|2xx->2xx|$.createdAt
Issue-Key: s3tables|GetNamespace|2xx->2xx|$.namespaceId
Issue-Key: s3tables|GetNamespace|2xx->2xx|$.tableBucketId
Issue-Key: s3tables|ListTableBuckets|2xx->2xx|$.tableBuckets[arn="arn
Issue-Key: s3tables|CreateTable|2xx->2xx|$.tableARN
Issue-Key: s3tables|CreateTable|2xx->2xx|$.versionToken
Issue-Commit: 8715ba3f180c27028dee3267eb478eb6947c4fd2

# s3tables read responses diverge: timestamps emitted as epoch numbers and server ids not UUIDs

> _Note: this issue was identified against the code base at commit `8715ba3f180c27028dee3267eb478eb6947c4fd2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetTableBucket

## Description
Six causes, all in how the s3tables read/create responses render two kinds of server-produced value: modeled `date-time` timestamps and server-minted identifiers. Every cause below must be closed before this issue is submitted.

Cause 1 of 6: GetTableBucket (fixture s3tables-error-paths-bucket-namespace-conflicts, step task#6).
AWS: Smithy GetTableBucketResponse.createdAt targets com.amazonaws.s3tables#Timestamp with @timestampFormat("date-time"), so on this rest-json service it is serialized as an RFC3339/ISO-8601 instant STRING (recording: "2026-09-05T10:32:45.809209385Z"); `tableBucketId` is modeled as a plain string and the recording shows AWS mints a dashed UUID (recorded behavior only for the id form: recording shows "851c4c17-fa71-48aa-bc24-0d536d54f24b").
Emulator: returns 200 with `createdAt` as a JSON NUMBER of seconds since the epoch (e.g. 1793574058.0950868) and `tableBucketId` as a 32-character lowercase hex digest (e.g. "5202c8a4cec09a93cd014149e849ed8a").
Why: an AWS SDK deserializing a @timestampFormat("date-time") member against a bare number fails to parse the response, so GetTableBucket is unusable from a generated client; a client that parses or persists the bucket id as a UUID also rejects the hex form.
Fix: the output serializer for the GetTableBucket handler (timestamp rendering) and the id minter in the service state.
Traces: s3tables-error-paths-bucket-namespace-conflicts#task#6, and the same cluster at s3tables-table-bucket-create-with-config#task#2 and s3tables-table-bucket-lifecycle#task#2. Code: services/service-com-amazonaws-s3tables-s3tablebuckets/src/state.rs:305-338 (`mint_id`/`mint_bucket_id` return `hex::encode(&digest[..16])`).

Cause 2 of 6: GetTable (fixture s3tables-error-paths-table-conflicts-and-notfound, step task#6).
AWS: Smithy GetTableResponse.createdAt and .modifiedAt are required Timestamp members with @timestampFormat("date-time") -> ISO-8601 strings; `namespaceId` and `tableBucketId` are UUIDs in the recording (recorded behavior only).
Emulator: 200 with `createdAt`/`modifiedAt` as epoch numbers (1793574058.7750506) and `namespaceId`/`tableBucketId` as 32-hex digests ("9f1188b4764aa5e1b0fe7da4721df01d", "55d35ac1a83c277f8b49de0a668f68ac").
Why: same deserialization break, on the most used read of the service.
Fix: GetTable's output serializer plus the shared id minter.
Traces: s3tables-error-paths-table-conflicts-and-notfound#task#6 (cluster also covers #task#8, s3tables-table-create-and-inspect#task#4/#task#5, s3tables-table-lifecycle-iceberg#task#4/#task#5).

Cause 3 of 6: GetNamespace (fixture s3tables-namespace-error-paths, step task#7).
AWS: GetNamespaceResponse.createdAt is a required Timestamp with @timestampFormat("date-time") (recording: "2026-09-06T14:22:12.894238959Z"); namespaceId/tableBucketId are UUIDs in the recording.
Emulator: 200 with createdAt = 1793574062.7591405 and 32-hex ids.
Why: a client reading namespace metadata cannot deserialize the response.
Fix: GetNamespace's output serializer plus the id minter.
Traces: s3tables-namespace-error-paths#task#7 (cluster also covers s3tables-namespace-lifecycle#task#3 and s3tables-namespace-listing-and-pagination#task#9/#10/#11).

Cause 4 of 6: ListTableBuckets (fixture s3tables-table-bucket-lifecycle, step task#3).
AWS: TableBucketSummary.createdAt is a required Timestamp with @timestampFormat("date-time"); tableBucketId is a UUID in the recording.
Emulator: 200 but each summary carries createdAt = 1793574072.5312815 and tableBucketId = "53526c298f48c08052f3bf3dd3fc6180".
Why: the list form breaks the same deserialization, so even enumerating buckets fails.
Fix: the ListTableBuckets summary serializer plus the id minter.
Traces: s3tables-table-bucket-lifecycle#task#3.

Cause 5 of 6: CreateTable tableARN identifier form (fixture s3tables-table-lifecycle-iceberg, step task#3).
AWS: recorded behavior only — CreateTableResponse.tableARN is modeled ^arn:...:bucket/<name>/table/[a-zA-Z0-9-_]{1,255}$; the recorded response's shape assertion on the table-id segment does not accept the value the emulator produces.
Emulator: 200 with tableARN = arn:aws:s3tables:us-west-2:000000000000:bucket/<bucket>/table/afba8263203e2fc895fe053fbf9b5b79 — the last segment is the same 32-hex digest form as the other ids.
Why: a client that keys a table by the ARN's identifier segment, or validates it against the form AWS returns, does not round-trip against the emulator.
Fix: the id minter the table ARN is built from (same component as causes 1-4), not the ARN template.
Traces: s3tables-table-lifecycle-iceberg#task#3. Code: state.rs:351-362 (`mint_table_id`), state.rs:542-544 (`table_arn`).

Cause 6 of 6: CreateTable versionToken form (fixture s3tables-table-replication, step setup#5).
AWS: recorded behavior only — CreateTableResponse.versionToken is a required opaque string (length 1..2048); the recorded response's shape assertion on it does not accept the emulator's value.
Emulator: 200 with versionToken = "b35ee821fda16ebd34763b98dc708277", the same 32-hex digest form.
Why: callers pass this token back on DeleteTable/RenameTable/UpdateTableMetadataLocation; a token whose form differs from AWS's is not interchangeable with one captured from AWS.
Fix: the version-token minter in the service state.
Traces: s3tables-table-replication#setup#5. Code: state.rs:364-368 (`mint_version_token`).

## Evidence
Replay of simparity-wire-aws-v1 (6 clusters, 17 steps):
- GetTableBucket task#6: expected $.createdAt '2026-09-05T10:32:45.809209385Z', got 1793574058.0950868; expected $.tableBucketId '851c4c17-fa71-48aa-bc24-0d536d54f24b', got '5202c8a4cec09a93cd014149e849ed8a' (status 200 both).
- GetTable task#6: expected $.createdAt/$.modifiedAt '2026-09-05T10:31:31.378424172Z', got 1793574058.7750506; expected $.namespaceId '6fbf2002-66c1-4b4c-8e6e-0d2451eb2da4', got '9f1188b4764aa5e1b0fe7da4721df01d'; expected $.tableBucketId '55e8bcf2-938c-4546-9005-7ac39347e217', got '55d35ac1a83c277f8b49de0a668f68ac'.
- GetNamespace task#7: expected $.createdAt '2026-09-06T14:22:12.894238959Z', got 1793574062.7591405; expected $.namespaceId 'cb3fb782-f169-4285-a466-ae0351c04159', got '73ae0d7142ad60441181436025bb4abb'; expected $.tableBucketId 'c0a347f4-06bf-4573-a51e-5958e3938590', got 'c0b3a5b39207d65d9a8ec2386fd916ec'.
- ListTableBuckets task#3: expected tableBuckets[arn=...].createdAt '2026-09-06T13:24:18.753812971Z', got 1793574072.5312815; expected tableBucketId '2dcf28f3-7f3b-4660-a71b-9c67af840105', got '53526c298f48c08052f3bf3dd3fc6180'.
- CreateTable task#3: $.tableARN shape assertion failed, got 'arn:aws:s3tables:us-west-2:000000000000:bucket/sim2real-probe-s3tables-s3t-tbl-bkt-cbc275c6/table/afba8263203e2fc895fe053fbf9b5b79'.
- CreateTable setup#5: $.versionToken shape assertion failed, got 'b35ee821fda16ebd34763b98dc708277'.
The concrete instants, UUIDs and digests above are per-run values; the requirement is the FORM (ISO-8601 date-time string; AWS's UUID-shaped identifiers), not these literals.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the simparity-wire-aws-v1 corpus.
2. Fixture s3tables-error-paths-bucket-namespace-conflicts step task#6 (GetTableBucket) — inspect $.createdAt and $.tableBucketId in the 200 body.
3. Repeat for s3tables-error-paths-table-conflicts-and-notfound#task#6 (GetTable), s3tables-namespace-error-paths#task#7 (GetNamespace), s3tables-table-bucket-lifecycle#task#3 (ListTableBuckets).
4. Fixture s3tables-table-lifecycle-iceberg step task#3 (CreateTable) — inspect $.tableARN's final segment; fixture s3tables-table-replication step setup#5 — inspect $.versionToken.
