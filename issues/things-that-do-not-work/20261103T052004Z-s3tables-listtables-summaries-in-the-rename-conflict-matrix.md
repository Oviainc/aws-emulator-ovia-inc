Issue-Source: trace-sweep-divergence
Issue-Trace: s3tables-rename-table-conflict-matrix
Issue-Step: task#14
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3tables-rename-table-conflict-matrix
Issue-Shape-Id: com.amazonaws.s3tables#S3TableBuckets
Issue-Key: s3tables|ListTables|2xx->2xx|$.tables[name="sim2real_tbl_one_38aa029d"].createdAt
Issue-Key: s3tables|ListTables|2xx->2xx|$.tables[name="sim2real_tbl_one_38aa029d"].modifiedAt
Issue-Key: s3tables|ListTables|2xx->2xx|$.tables[name="sim2real_tbl_one_38aa029d"].tableARN
Issue-Key: s3tables|ListTables|2xx->2xx|$.tables[name="sim2real_tbl_two_38aa029d"].createdAt
Issue-Key: s3tables|ListTables|2xx->2xx|$.tables[name="sim2real_tbl_two_38aa029d"].modifiedAt
Issue-Key: s3tables|ListTables|2xx->2xx|$.tables[name="sim2real_tbl_two_38aa029d"].tableARN
Issue-Commit: 7e282e985d147868e376415a13aa29a1f3675b80

# s3tables ListTables summaries in the rename-conflict matrix serialize timestamps as epoch numbers and mint non-UUID table ids inside tableARN

> _Note: this issue was identified against the code base at commit `7e282e985d147868e376415a13aa29a1f3675b80`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListTables

## Description
Successful s3tables ListTables responses return the right members with the wrong wire shapes: the per-table summary timestamps are emitted as bare JSON numbers instead of the modeled date-time strings, and the identifier segment inside tableARN is a 32-character hex digest instead of the identifier form the service mints. 2 causes, both in the s3tables ListTables response serializer and in the table-id minting in state.

Three open issues carry this same family of faults for OTHER clusters - 20261103T033559Z-s3tables-list-and-get-responses-emit-epoch-timestamps.md, 20261101T230800Z-s3tables-listtables-summaries-carry-epoch-timestamps-and-unrecorded.md and 20261101T230800Z-s3tables-read-responses-diverge-timestamps-emitted-as-epoch.md - but the replay reports the two clusters below as UNOWNED (their replay keys are per-run table names that no open issue's keys match), so they are filed here and those issues are named for cross-reference.

Cause 1 of 2: ListTables summary createdAt / modifiedAt are JSON numbers, not date-time strings.
AWS: Smithy: TableSummary.createdAt and TableSummary.modifiedAt both target Timestamp with timestampFormat "date-time" and are @required (com.amazonaws.s3tables#TableSummary); ListTables output member `tables` targets TableSummaryList and is required. Recorded ListTables responses carry an RFC3339 instant string with sub-second precision for both members, never a number.
Emulator: emits an epoch float as a bare JSON number for createdAt and modifiedAt in every table summary (fixture s3tables-rename-table-conflict-matrix, steps task#14 and task#15; both answered status 200 as expected, only the serialized shape diverges).
Why: "date-time" means every generated SDK deserializes these members from a string; a JSON number either fails to parse outright or is coerced into a different instant, so any client that reads a table's creation or modification time from a listing is broken, and the required-member contract is violated in shape.
Fix: the s3tables ListTables response serializer must render Timestamp members per the modeled date-time format for table summaries, the same way the already-filed read paths must.
Traces: s3tables-rename-table-conflict-matrix#task#14 (representative), s3tables-rename-table-conflict-matrix#task#15.

Cause 2 of 2: the table identifier inside tableARN is not minted in the service's identifier form.
AWS: Smithy: TableSummary.tableARN targets TableARN, @required, length 1..2048, pattern ^(arn:aws[-a-z0-9]*:[a-z0-9]+:[-a-z0-9]*:[0-9]{12}:bucket/[a-z0-9_-]{3,63}/table/[a-zA-Z0-9-_]{1,255})$, arnReference AWS::S3Tables::Table. Recorded behavior only for the id FORM: in every recorded response the segment after /table/ is a canonical 8-4-4-4-12 hyphenated UUID (CreateTable in the same corpus records e.g. the UUID form after /table/), and the corresponding ListTables/CreateTable responses are consistent with it.
Emulator: builds the ARN with a 32-character unhyphenated hex digest after /table/, so the summary's tableARN does not match the recorded shape for the same table.
Why: the table ARN is the opaque handle a client stores and re-submits (GetTable/DeleteTable/TagResource all take it), and it is also the key by which a client correlates a listing row with a table id obtained elsewhere. A digest-form id makes the emulator's ARNs non-interchangeable with the ids AWS mints and breaks any client or test that recognizes the AWS id form inside an ARN.
Fix: the table-id minting in the s3tables state layer (the value used to assemble tableARN) must produce the service's identifier form; the ARN assembler itself then needs no change. Note the same minting feeds CreateTable/GetTable, so fixing it in state covers the summary path too.
Traces: s3tables-rename-table-conflict-matrix#task#14 (representative), s3tables-rename-table-conflict-matrix#task#15.

## Evidence
Cause 1 - fixture s3tables-rename-table-conflict-matrix step task#14, ListTables, expected status 200 / actual status 200:
  $.tables[name="sim2real_tbl_one_..."].createdAt: value_mismatch (expected a shaped date-time string, got 1793683090.7062974)
  $.tables[name="sim2real_tbl_one_..."].modifiedAt: value_mismatch (expected a shaped date-time string, got 1793683090.7062974)
  $.tables[name="sim2real_tbl_two_..."].createdAt / .modifiedAt: same, got 1793683090.7112346
Step task#15 repeats it for the surviving table (got 1793683090.7160137). The particular instants are per-run values; the JSON TYPE (date-time string vs number) is the requirement.

Cause 2 - same two steps:
  $.tables[name="sim2real_tbl_one_..."].tableARN: value_mismatch (expected the recorded ARN shape, got 'arn:aws:s3tables:us-west-2:000000000000:bucket/sim2real-probe-s3t-rename-.../table/fc598c885e7f109b53c3df6091776f85')
  $.tables[name="sim2real_tbl_two_..."].tableARN: got '.../table/643a23b0b0cb269d13f61c5c0fa2150c'
  task#15: $.tables[name="sim2real_tbl_one_..."].tableARN got '.../table/94e604843f94feff9450080d336ebc99'
The bucket name, account and region segments matched; only the segment after /table/ diverges in form. Elsewhere in the same corpus a CreateTable response records that segment as a hyphenated UUID (e.g. '.../table/37c67843-744e-48c1-8d27-4aeac8da2079'), while this emulator returns a 32-hex digest. The specific id values are per-run; the 8-4-4-4-12 form is the requirement.

No status or error-code divergence in either cluster: both steps returned 200 as recorded.

## Steps to Reproduce
1. Replay fixture s3tables-rename-table-conflict-matrix step task#14 (ListTables over the rename-conflict table bucket, after the rename matrix has created two tables).
2. In each element of $.tables, inspect the JSON type of createdAt and modifiedAt: the emulator returns a bare number, the recording a date-time string.
3. In the same elements, inspect the segment after /table/ in tableARN: the emulator returns a 32-character unhyphenated hex digest, the recording a hyphenated UUID.
4. Replay step task#15 of the same fixture (ListTables after the conflicting rename) and observe both divergences again on the surviving summary row.
