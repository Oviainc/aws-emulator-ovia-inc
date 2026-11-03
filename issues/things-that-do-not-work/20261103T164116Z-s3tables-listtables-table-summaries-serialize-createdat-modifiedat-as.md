Issue-Source: trace-sweep-divergence
Issue-Trace: s3tables-rename-table-conflict-matrix
Issue-Step: task#14
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3tables-rename-table-conflict-matrix
Issue-Shape-Id: com.amazonaws.s3tables#S3TableBuckets
Issue-Key: s3tables|ListTables|2xx->2xx|$.tables[name="sim2real_tbl_one_f7d6e2c9"].createdAt
Issue-Key: s3tables|ListTables|2xx->2xx|$.tables[name="sim2real_tbl_one_f7d6e2c9"].modifiedAt
Issue-Key: s3tables|ListTables|2xx->2xx|$.tables[name="sim2real_tbl_one_f7d6e2c9"].tableARN
Issue-Key: s3tables|ListTables|2xx->2xx|$.tables[name="sim2real_tbl_two_f7d6e2c9"].createdAt
Issue-Key: s3tables|ListTables|2xx->2xx|$.tables[name="sim2real_tbl_two_f7d6e2c9"].modifiedAt
Issue-Key: s3tables|ListTables|2xx->2xx|$.tables[name="sim2real_tbl_two_f7d6e2c9"].tableARN
Issue-Commit: 414c0f188a7c5d9bd8cd80ed6987d7d1a0f2219a

# s3tables ListTables table summaries serialize createdAt/modifiedAt as epoch numbers and mint digest-form table ids in tableARN

> _Note: this issue was identified against the code base at commit `414c0f188a7c5d9bd8cd80ed6987d7d1a0f2219a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListTables

## Description
s3tables ListTables answers with the correct status (200) and the correct set of members, but two members of every TableSummary row are serialized in the wrong wire form: the timestamps are bare JSON numbers instead of the modeled date-time strings, and the identifier segment inside tableARN is a 32-character unhyphenated hex digest instead of the identifier form the service mints. 2 causes, both in the s3tables ListTables response serializer and in the table-id minting in the s3tables state layer.

The replay reports BOTH of these clusters as UNOWNED. Four open issues carry the same family of faults for OTHER clusters and are named here for cross-reference only: 20261103T052004Z-s3tables-listtables-summaries-in-the-rename-conflict-matrix.md, 20261103T033559Z-s3tables-list-and-get-responses-emit-epoch-timestamps.md, 20261101T230800Z-s3tables-listtables-summaries-carry-epoch-timestamps-and-unrecorded.md and 20261101T230800Z-s3tables-read-responses-diverge-timestamps-emitted-as-epoch.md. Their recorded replay keys embed per-run table names (sim2real_tbl_one_<suffix>), so they do not match this run's clusters and do not gate them.

Cause 1 of 2: ListTables summary createdAt / modifiedAt are emitted as JSON numbers instead of date-time strings.
AWS: Smithy: com.amazonaws.s3tables#TableSummary.createdAt and TableSummary.modifiedAt both target Timestamp with timestampFormat "date-time" and are @required; ListTables output member `tables` targets TableSummaryList and is @required. A "date-time" timestamp goes on the wire as an RFC3339 instant STRING. The recorded ListTables responses carry such a string for both members in every summary row, never a number.
Emulator: emits an epoch float as a bare JSON number for createdAt and modifiedAt in every table summary row (fixture s3tables-rename-table-conflict-matrix, steps task#14 and task#15; both answered 200 as recorded, so only the serialized shape diverges).
Why: "date-time" means every generated SDK deserializes these members from a string; a bare number either fails to parse or is coerced into a different instant, so any client that reads a table's creation or modification time out of a listing gets an error or a wrong time, and the @required-member contract is satisfied in name only.
Fix: the s3tables ListTables response serializer must render TableSummary Timestamp members in the modeled date-time format, the same rendering the already-filed read paths need.
Traces: s3tables-rename-table-conflict-matrix#task#14 (representative), s3tables-rename-table-conflict-matrix#task#15.

Cause 2 of 2: the table identifier inside a summary's tableARN is not minted in the service's identifier form.
AWS: Smithy: TableSummary.tableARN targets TableARN, @required, @length 1..2048, @pattern ^(arn:aws[-a-z0-9]*:[a-z0-9]+:[-a-z0-9]*:[0-9]{12}:bucket/[a-z0-9_-]{3,63}/table/[a-zA-Z0-9-_]{1,255})$, arnReference AWS::S3Tables::Table. The model constrains the charset but not the id FORM, so for the form this is recorded behavior only: in the recorded corpus the segment after /table/ is always a canonical hyphenated 8-4-4-4-12 UUID (e.g. a recorded CreateTable response in the same corpus carries .../table/37c67843-744e-48c1-8d27-4aeac8da2079), and the ARNs a listing row reports agree with the ones CreateTable/GetTable report for the same table.
Emulator: assembles the ARN with a 32-character unhyphenated hex digest after /table/, so a summary row's tableARN does not match the recorded id form. The region, account and bucket-name segments match; only the id segment's form diverges.
Why: the table ARN is the opaque handle a client stores and re-submits (GetTable, DeleteTable and TagResource all accept it), and it is also how a client correlates a listing row with a table id obtained from somewhere else. A digest-form id makes the emulator's ARNs non-interchangeable with the ids AWS mints and breaks any client or fixture that recognizes the AWS id form inside an ARN.
Fix: the table-id minting in the s3tables state layer (the value the ARN assembler interpolates) must produce the service's identifier form; the assembler itself needs no change. The same minting feeds CreateTable and GetTable, so fixing it in state covers the summary path too.
Traces: s3tables-rename-table-conflict-matrix#task#14 (representative), s3tables-rename-table-conflict-matrix#task#15.

## Evidence
Cause 1 - fixture s3tables-rename-table-conflict-matrix step task#14, ListTables, expected status 200 / actual status 200 (no status or error-code divergence):
  $.tables[name="sim2real_tbl_one_..."].createdAt: value_mismatch (expected a shaped date-time string, got 1793723961.1053646)
  $.tables[name="sim2real_tbl_one_..."].modifiedAt: value_mismatch (expected a shaped date-time string, got 1793723961.1053646)
  $.tables[name="sim2real_tbl_two_..."].createdAt / .modifiedAt: same divergence, got 1793723961.110383
Step task#15 repeats it for the surviving table row (got 1793723961.1151705). The particular instants are per-run values and are NOT the requirement; the JSON type - date-time string versus bare number - is.

Cause 2 - the same two steps, same 200/200:
  $.tables[name="sim2real_tbl_one_..."].tableARN: value_mismatch (expected the recorded ARN form, got 'arn:aws:s3tables:us-west-2:000000000000:bucket/sim2real-probe-s3t-rename-.../table/408f337c7ae46bb21a8e991e33799730')
  $.tables[name="sim2real_tbl_two_..."].tableARN: got '.../table/b2d334abb219013ec36731aa492e4b60'
  task#15: $.tables[name="sim2real_tbl_one_..."].tableARN: got '.../table/9007474a6c3338ba8b517d193eb69391'
Only the segment after /table/ diverges in form; elsewhere in the same corpus a recorded CreateTable response has that segment as a hyphenated UUID ('.../table/37c67843-744e-48c1-8d27-4aeac8da2079'). The specific id values are per-run; the 8-4-4-4-12 hyphenated form is the requirement.

Both clusters are reported with stub_path empty and owned_by empty, i.e. neither is blocked by an unimplemented operation nor gated by an existing issue's replay key.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay fixture s3tables-rename-table-conflict-matrix.
2. At step task#14 (ListTables over the rename-conflict table bucket, after the rename matrix has created two tables), read the response body: status is 200 as recorded.
3. For each element of $.tables, inspect the JSON type of createdAt and modifiedAt - the emulator returns a bare number (epoch float), the recording an RFC3339 date-time string.
4. In the same elements, inspect the segment after /table/ in tableARN - the emulator returns a 32-character unhyphenated hex digest, the recording a hyphenated 8-4-4-4-12 UUID.
5. Replay step task#15 of the same fixture (ListTables after the conflicting rename) and observe both divergences again on the surviving summary row.
