Issue-Source: trace-sweep-divergence
Issue-Trace: s3tables-rename-table-conflict-matrix
Issue-Step: task#14
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3tables-rename-table-conflict-matrix
Issue-Shape-Id: com.amazonaws.s3tables#S3TableBuckets
Issue-Key: s3tables|ListTables|2xx->2xx|$.tables[name="sim2real_tbl_one_1e4c9b38"].createdAt
Issue-Key: s3tables|ListTables|2xx->2xx|$.tables[name="sim2real_tbl_one_1e4c9b38"].modifiedAt
Issue-Key: s3tables|ListTables|2xx->2xx|$.tables[name="sim2real_tbl_one_1e4c9b38"].tableARN
Issue-Key: s3tables|ListTables|2xx->2xx|$.tables[name="sim2real_tbl_two_1e4c9b38"].createdAt
Issue-Key: s3tables|ListTables|2xx->2xx|$.tables[name="sim2real_tbl_two_1e4c9b38"].modifiedAt
Issue-Key: s3tables|ListTables|2xx->2xx|$.tables[name="sim2real_tbl_two_1e4c9b38"].tableARN
Issue-Commit: 839dd8f917d4af53295e5afaaac8e9ae2e4714f5

# s3tables rename-conflict ListTables rows ship epoch-number timestamps and digest table ids

> _Note: this issue was identified against the code base at commit `839dd8f917d4af53295e5afaaac8e9ae2e4714f5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListTables

## Description
Successful s3tables ListTables responses carry the right members with the wrong wire shapes: each table summary's createdAt/modifiedAt are serialized as bare JSON numbers instead of the modeled date-time strings, and the identifier segment inside tableARN is a 32-character unhyphenated hex digest instead of the identifier form the service mints. 2 causes, both reported UNOWNED by the replay (their replay keys are keyed on per-run table names that no open issue's keys match).

Related open issues carry this same family of faults for OTHER clusters and are named here for cross-reference only, not as coverage: 20261103T052004Z-s3tables-listtables-summaries-in-the-rename-conflict-matrix.md, 20261103T164116Z-s3tables-listtables-table-summaries-serialize-createdat-modifiedat-as.md, 20261103T033559Z-s3tables-list-and-get-responses-emit-epoch-timestamps.md, 20261101T230800Z-s3tables-listtables-summaries-carry-epoch-timestamps-and-unrecorded.md and 20261104T092732Z-s3tables-5-net-new-causes-listtables-summary-wire.md. The replay still reports the two clusters below as unowned, so both must be closed here.

Cause 1 of 2: ListTables summary rows for the two tables created by the rename-conflict matrix serialize createdAt/modifiedAt as JSON numbers and mint a digest-form table id inside tableARN (fixture s3tables-rename-table-conflict-matrix, step task#14).
AWS: Smithy: com.amazonaws.s3tables#TableSummary.createdAt and .modifiedAt both target Timestamp with timestampFormat "date-time" and are @required; TableSummary.tableARN targets TableARN, @required, @length 1..2048, pattern ^(arn:aws[-a-z0-9]*:[a-z0-9]+:[-a-z0-9]*:[0-9]{12}:bucket/[a-z0-9_-]{3,63}/table/[a-zA-Z0-9-_]{1,255})$, arnReference AWS::S3Tables::Table; ListTables output member `tables` targets TableSummaryList and is @required (ListTables GET /tables/{tableBucketARN}, 200). For the table-id FORM: recorded behavior only — in the recordings the segment after /table/ is a canonical hyphenated 8-4-4-4-12 UUID (a CreateTable step in the same corpus records that segment as a UUID), and the step answered 200 as recorded.
Emulator: answers 200, but every summary row emits createdAt and modifiedAt as an epoch float rendered as a bare JSON number, and assembles tableARN with a 32-character unhyphenated hex digest after /table/.
Why: timestampFormat "date-time" means every generated SDK deserializes these members from a string; a bare number either fails to parse or is coerced to a different instant, so any client reading a table's creation or modification time from a listing breaks, and a @required member is violated in shape. The table ARN is the opaque handle a client stores and re-submits (GetTable/DeleteTable/TagResource all take it) and the key by which a listing row is correlated with a table id obtained elsewhere, so a digest-form id makes the emulator's ARNs non-interchangeable with the ids AWS mints.
Fix: the s3tables ListTables response serializer must render Timestamp members in the modeled date-time format for table summaries; the table-id minting in the s3tables state layer (the value fed into the tableARN assembler, shared with CreateTable/GetTable) must produce the service's identifier form.
Traces: s3tables-rename-table-conflict-matrix#task#14.

Cause 2 of 2: the same two shape faults recur on the surviving summary row after the conflicting rename (fixture s3tables-rename-table-conflict-matrix, step task#15).
AWS: same model grounding as Cause 1 — TableSummary.createdAt/.modifiedAt are date-time Timestamps, tableARN matches the TableARN pattern with a UUID-form id after /table/ in the recordings; status 200.
Emulator: answers 200 and again emits createdAt/modifiedAt as bare JSON numbers and a 32-hex digest table id inside tableARN for the row that survived the rename conflict.
Why: a client that re-lists after a failed rename to confirm which table name survived cannot deserialize the timestamps, and the ARN it reads back for the surviving table is not in AWS's id form, so a stored handle comparison fails.
Fix: same components as Cause 1 — the ListTables summary serializer and the state layer's table-id minting; this cluster confirms the post-rename listing path goes through the same serializer.
Traces: s3tables-rename-table-conflict-matrix#task#15.

No status or error-code divergence in either cause: both steps returned 200 exactly as recorded; only the serialized JSON shapes diverge.

## Evidence
Cause 1 — fixture s3tables-rename-table-conflict-matrix, step task#14, ListTables, expected status 200 / actual status 200:
  $.tables[name="sim2real_tbl_one_..."].createdAt: value_mismatch (expected a shaped date-time string, got 1793788479.4467993)
  $.tables[name="sim2real_tbl_one_..."].modifiedAt: value_mismatch (expected a shaped date-time string, got 1793788479.4467993)
  $.tables[name="sim2real_tbl_one_..."].tableARN: value_mismatch (expected the recorded ARN shape, got 'arn:aws:s3tables:us-west-2:000000000000:bucket/sim2real-probe-s3t-rename-.../table/d1f1d52ae453707c9c9e226ab302b577')
  $.tables[name="sim2real_tbl_two_..."].createdAt / .modifiedAt: same fault, got 1793788479.4507632
  $.tables[name="sim2real_tbl_two_..."].tableARN: got '.../table/2ff7de7d0a22887674363249b4ac7cc9'

Cause 2 — same fixture, step task#15, ListTables, expected 200 / actual 200:
  $.tables[name="sim2real_tbl_one_..."].createdAt: value_mismatch (expected a shaped date-time string, got 1793788479.4545524)
  $.tables[name="sim2real_tbl_one_..."].modifiedAt: same, got 1793788479.4545524
  $.tables[name="sim2real_tbl_one_..."].tableARN: got '.../table/6bf1206eef0423651b0149cc076c4c3d'

What is the requirement vs. what is a per-run value: the requirement is the JSON TYPE of createdAt/modifiedAt (an RFC3339 date-time string per timestampFormat "date-time", not a number) and the FORM of the id segment after /table/ in tableARN (a hyphenated 8-4-4-4-12 UUID, as recorded elsewhere in this corpus, e.g. '.../table/37c67843-744e-48c1-8d27-4aeac8da2079' from a CreateTable response). The particular instants (1793788479.45...) and the particular ids (d1f1d52a..., 6bf1206e...) are per-run values and are not themselves requirements. The account, region and bucket-name segments of the ARN matched; only the id segment diverges in form.

No diverging error code or error message is involved: both steps are 200 responses, so the status (200) and the member set matched, and the divergence is confined to the two timestamp members' JSON type and the tableARN id form.

## Steps to Reproduce
1. Replay fixture s3tables-rename-table-conflict-matrix against the emulator.
2. At step task#14 (ListTables over the rename-conflict table bucket, after the matrix has created two tables), inspect each element of $.tables: createdAt and modifiedAt come back as bare JSON numbers where the recording has an RFC3339 date-time string.
3. In those same elements, inspect the segment after /table/ in tableARN: the emulator returns a 32-character unhyphenated hex digest where the recording has a hyphenated UUID.
4. Continue to step task#15 (ListTables after the conflicting rename) and observe both divergences again on the surviving summary row.
5. Both steps return HTTP 200 as recorded, so only the serialized shapes differ.
