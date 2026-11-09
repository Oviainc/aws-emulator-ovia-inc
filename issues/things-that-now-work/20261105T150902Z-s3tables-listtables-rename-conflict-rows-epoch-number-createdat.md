Issue-Source: trace-sweep-divergence
Issue-Trace: s3tables-rename-table-conflict-matrix
Issue-Step: task#14
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3tables-rename-table-conflict-matrix
Issue-Shape-Id: com.amazonaws.s3tables#S3TableBuckets
Issue-Key: s3tables|ListTables|2xx->2xx|$.tables[name="sim2real_tbl_one_cab610af"].createdAt
Issue-Key: s3tables|ListTables|2xx->2xx|$.tables[name="sim2real_tbl_one_cab610af"].modifiedAt
Issue-Key: s3tables|ListTables|2xx->2xx|$.tables[name="sim2real_tbl_one_cab610af"].tableARN
Issue-Key: s3tables|ListTables|2xx->2xx|$.tables[name="sim2real_tbl_two_cab610af"].createdAt
Issue-Key: s3tables|ListTables|2xx->2xx|$.tables[name="sim2real_tbl_two_cab610af"].modifiedAt
Issue-Key: s3tables|ListTables|2xx->2xx|$.tables[name="sim2real_tbl_two_cab610af"].tableARN
Issue-Commit: a03774f23cc5bdf1e5b1829d2570750b16716bf1

# s3tables ListTables rename-conflict rows: epoch-number createdAt/modifiedAt and non-UUID tableARN ids

> _Note: this issue was identified against the code base at commit `a03774f23cc5bdf1e5b1829d2570750b16716bf1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListTables

## Description
s3tables ListTables returns the correct member set, but every TableSummary row carries two wrong wire forms: createdAt/modifiedAt are serialized as bare JSON numbers (epoch floats) instead of the modeled date-time strings, and the identifier segment after /table/ inside tableARN is a 32-character unhyphenated hex digest instead of the hyphenated 8-4-4-4-12 UUID form the service mints throughout this corpus. 2 causes, one per replay cluster; the replay reports BOTH clusters as UNOWNED (owned_by empty), so no existing issue carries their replay keys and both must be closed here.

CROSS-REFERENCE ONLY (never coverage): the following open issues describe this same family of faults for other clusters, or carry replay keys pinned to per-run table names from earlier runs (e.g. sim2real_tbl_one_b35a5337) that do not match this run's rows: 20261104T200516Z-s3tables-tablesummary-wire-forms-number-timestamps-and-digest.md, 20261104T190415Z-s3tables-rename-conflict-matrix-listtables-rows-table-summary.md, 20261104T103635Z-s3tables-rename-conflict-listtables-rows-ship-epoch-number.md, 20261103T164116Z-s3tables-listtables-table-summaries-serialize-createdat-modifiedat-as.md, 20261103T052004Z-s3tables-listtables-summaries-in-the-rename-conflict-matrix.md, 20261103T033559Z-s3tables-list-and-get-responses-emit-epoch-timestamps.md, 20261101T230800Z-s3tables-listtables-summaries-carry-epoch-timestamps-and-unrecorded.md, 20261104T092732Z-s3tables-5-net-new-causes-listtables-summary-wire.md. Because this replay still reports the two clusters below unowned, a fixer must re-replay BOTH named steps before closing.

Cause 1 of 2: ListTables over the rename-conflict table bucket after the matrix has created its two tables (fixture s3tables-rename-table-conflict-matrix, step task#14).
AWS: Smithy (model services/service-com-amazonaws-s3tables-s3tablebuckets/smithy-model/com-amazonaws-s3tables-s3tablebuckets.json): TableSummary.createdAt and TableSummary.modifiedAt target Timestamp with timestampFormat "date-time" and are @required; TableSummary.tableARN targets TableARN, @required, @length 1..2048, pattern ^(arn:aws[-a-z0-9]*:[a-z0-9]+:[-a-z0-9]*:[0-9]{12}:bucket/[a-z0-9_-]{3,63}/table/[a-zA-Z0-9-_]{1,255})$, arnReference AWS::S3Tables::Table; ListTablesResponse.tables targets TableSummaryList and is @required. The recorded step is HTTP 200 with no error. For the id FORM after /table/: recorded behavior only — everywhere in this corpus that segment is a canonical hyphenated 8-4-4-4-12 UUID (a CreateTable response in the same corpus records '.../table/37c67843-744e-48c1-8d27-4aeac8da2079'), so a listing row for the same table carries that form too.
Emulator: answers 200, but renders createdAt and modifiedAt in every summary row as an epoch float emitted as a bare JSON number, and assembles tableARN with a 32-character unhyphenated hex digest after /table/.
Why: timestampFormat "date-time" means every generated SDK deserializes these members from an RFC3339 string; a bare JSON number either fails to parse or is coerced to a different instant, so a client reading a table's creation or modification time from a listing breaks on two @required members. The table ARN is the opaque handle a client stores and re-submits (GetTable/DeleteTable/TagResource all accept it) and the key by which a listing row is correlated with an id obtained elsewhere, so a digest-form id makes the emulator's ARNs non-interchangeable with AWS's.
Fix: the s3tables ListTables response serializer must render TableSummary Timestamp members in the modeled date-time format; the table-id minting in the s3tables state layer (the value fed to the tableARN assembler, shared with CreateTable/GetTable) must produce the service's identifier form.
Traces: s3tables-rename-table-conflict-matrix#task#14.

Cause 2 of 2: the same two shape faults recur on the surviving summary row when the fixture re-lists after the conflicting rename (fixture s3tables-rename-table-conflict-matrix, step task#15).
AWS: same model grounding as Cause 1 — TableSummary.createdAt/.modifiedAt are @required date-time Timestamps and tableARN matches the TableARN pattern with a UUID-form id after /table/ per the recordings; recorded status 200.
Emulator: answers 200 and again emits createdAt/modifiedAt as bare JSON numbers and a 32-hex digest table id inside tableARN for the row that survived the rename conflict.
Why: a client that re-lists after a failed rename to confirm which table name survived cannot deserialize the timestamps, and the ARN it reads back for the surviving table is not in AWS's id form, so a comparison against a stored handle fails.
Fix: same components as Cause 1 — the ListTables summary serializer and the state layer's table-id minting; this cluster confirms the post-rename listing path goes through the same serializer.
Traces: s3tables-rename-table-conflict-matrix#task#15.

No status, error code or error message diverges in either cause: both steps answered HTTP 200 exactly as recorded, so no refusal text is involved — the divergence is confined to the JSON type of two @required members and the form of one ARN segment.

## Evidence
Cause 1 — fixture s3tables-rename-table-conflict-matrix, step task#14, ListTables, expected status 200 / actual status 200 (verdict mismatch, cluster unowned):
  $.tables[name="sim2real_tbl_one_..."].createdAt: value_mismatch (expected a shaped date-time string, got 1793891230.2413328)
  $.tables[name="sim2real_tbl_one_..."].modifiedAt: value_mismatch (expected a shaped date-time string, got 1793891230.2413328)
  $.tables[name="sim2real_tbl_one_..."].tableARN: value_mismatch (expected the recorded ARN shape, got 'arn:aws:s3tables:us-west-2:000000000000:bucket/sim2real-probe-s3t-rename-.../table/018ca7894a7ed84d6323aacc1442f0c3')
  $.tables[name="sim2real_tbl_two_..."].createdAt / .modifiedAt: same fault, got 1793891230.2461076
  $.tables[name="sim2real_tbl_two_..."].tableARN: got '.../table/b6c427054d24f7b2557b5b9ba97ef63c'

Cause 2 — same fixture, step task#15, ListTables, expected 200 / actual 200 (verdict mismatch, cluster unowned):
  $.tables[name="sim2real_tbl_one_..."].createdAt: value_mismatch (expected a shaped date-time string, got 1793891230.2506852)
  $.tables[name="sim2real_tbl_one_..."].modifiedAt: same fault, got 1793891230.2506852
  $.tables[name="sim2real_tbl_one_..."].tableARN: got '.../table/8454faa8a278040dfb4ccc093436da6b'

Requirement vs per-run value: the requirement is (a) the JSON TYPE of createdAt/modifiedAt — an RFC3339 date-time string per timestampFormat "date-time" on TableSummary.createdAt/.modifiedAt, never a number — and (b) the FORM of the id segment after /table/ in tableARN, a hyphenated 8-4-4-4-12 UUID as recorded elsewhere in this corpus. The particular instants (1793891230.24...), the particular digests (018ca789..., b6c42705..., 8454faa8...), the per-run table names and the bucket-name suffix are per-run values and are NOT requirements. The arn partition, service, region, account and bucket segments all matched; only the id segment diverges in form.

No diverging HTTP status, error code or error message: both clusters are 200 responses with the full member set present.

## Steps to Reproduce
1. Replay fixture s3tables-rename-table-conflict-matrix against the emulator.
2. At step task#14 (ListTables over the rename-conflict table bucket, after the matrix has created its two tables), inspect each element of $.tables: createdAt and modifiedAt come back as bare JSON numbers where the recording has an RFC3339 date-time string.
3. In those same elements, inspect the segment after /table/ in tableARN: the emulator returns a 32-character unhyphenated hex digest where the recording has a hyphenated UUID.
4. Continue to step task#15 (ListTables after the conflicting rename) and observe both divergences again on the surviving summary row.
5. Both steps answer HTTP 200 as recorded, so only the serialized wire shapes differ.
