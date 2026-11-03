Issue-Source: trace-sweep-divergence
Issue-Trace: s3tables-rename-table-conflict-matrix
Issue-Step: task#14
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3tables-rename-table-conflict-matrix s3tables-table-bucket-encryption-config
Issue-Shape-Id: com.amazonaws.s3tables#S3TableBuckets
Issue-Key: s3tables|ListTables|2xx->2xx|$.tables[name="sim2real_tbl_one_e20b9467"].createdAt
Issue-Key: s3tables|ListTables|2xx->2xx|$.tables[name="sim2real_tbl_one_e20b9467"].modifiedAt
Issue-Key: s3tables|ListTables|2xx->2xx|$.tables[name="sim2real_tbl_one_e20b9467"].tableARN
Issue-Key: s3tables|ListTables|2xx->2xx|$.tables[name="sim2real_tbl_two_e20b9467"].createdAt
Issue-Key: s3tables|ListTables|2xx->2xx|$.tables[name="sim2real_tbl_two_e20b9467"].modifiedAt
Issue-Key: s3tables|ListTables|2xx->2xx|$.tables[name="sim2real_tbl_two_e20b9467"].tableARN
Issue-Key: s3tables|GetTableBucketEncryption|2xx->2xx|$.encryptionConfiguration.kmsKeyArn
Issue-Key: s3tables|GetTableBucketEncryption|2xx->2xx|$.encryptionConfiguration.sseAlgorithm
Issue-Commit: fd8612b3bbd3230cc8e7e24a7ba646f624ffbaca

# s3tables table reads diverge: ListTables summary shapes and table-bucket encryption state

> _Note: this issue was identified against the code base at commit `fd8612b3bbd3230cc8e7e24a7ba646f624ffbaca`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListTables

## Description
Three unowned replay clusters on s3tables read paths. All three answered the recorded HTTP status, but the BODY diverges: ListTables renders table summaries with the wrong JSON types and a non-service-form table id inside tableARN, and GetTableBucketEncryption serves a configuration the bucket was never given. 3 causes; a fixer must close all three before submitting.

Related open issues carry the SAME family of faults for OTHER clusters and do NOT own these replay keys (the replay reports all three clusters as unowned): 20261103T033559Z-s3tables-list-and-get-responses-emit-epoch-timestamps.md, 20261101T230800Z-s3tables-listtables-summaries-carry-epoch-timestamps-and-unrecorded.md, 20261101T230800Z-s3tables-read-responses-diverge-timestamps-emitted-as-epoch.md, 20261103T052004Z-s3tables-listtables-summaries-in-the-rename-conflict-matrix.md and 20261103T164116Z-s3tables-listtables-table-summaries-serialize-createdat-modifiedat-as.md. They are named here for cross-reference only; the clusters below still need the fix to be confirmed by replaying the fixture steps listed under Traces.

Cause 1 of 3: ListTables summary createdAt / modifiedAt are serialized as bare JSON numbers instead of date-time strings.
AWS: Smithy: com.amazonaws.s3tables#TableSummary.createdAt and .modifiedAt both target Timestamp with timestampFormat "date-time" and are @required; ListTables output member `tables` targets TableSummaryList and is @required (ListTables GET /tables/{tableBucketARN}, 200). Recorded ListTables responses carry an RFC3339 instant string with sub-second precision for both members, never a number.
Emulator: emits an epoch float as a bare JSON number for createdAt and modifiedAt in every table summary. Status 200 as recorded; only the serialized type diverges.
Why: "date-time" means every generated SDK deserializes these members from a string. A bare number either fails to parse or is coerced into a different instant, so any client that reads a table's creation or modification time out of a listing gets an error or a wrong timestamp, and the @required contract is satisfied only in name.
Fix: the s3tables ListTables response serializer must render Timestamp members per the modeled date-time format for every table summary.
Traces: s3tables-rename-table-conflict-matrix#task#14 (representative), s3tables-rename-table-conflict-matrix#task#15.

Cause 2 of 3: the table identifier inside a summary's tableARN is a 32-character hex digest, not the identifier form the service mints.
AWS: Smithy: TableSummary.tableARN targets TableARN, @required, length 1..2048, pattern ^(arn:aws[-a-z0-9]*:[a-z0-9]+:[-a-z0-9]*:[0-9]{12}:bucket/[a-z0-9_-]{3,63}/table/[a-zA-Z0-9-_]{1,255})$, arnReference AWS::S3Tables::Table. The id FORM is recorded behavior only: in the recordings the segment after /table/ is a canonical hyphenated 8-4-4-4-12 UUID (a CreateTable response in the same corpus records it as '.../table/37c67843-744e-48c1-8d27-4aeac8da2079').
Emulator: assembles the ARN with an unhyphenated 32-hex digest after /table/, so a listing row's tableARN does not have the recorded id form. The account, region and bucket segments match.
Why: the table ARN is the opaque handle a client stores and re-submits (GetTable, DeleteTable, TagResource all accept it) and the key by which a listing row is correlated with an id obtained elsewhere. A digest-form id makes the emulator's ARNs non-interchangeable with the ids AWS mints and breaks any client or test that recognizes the AWS id form inside an ARN.
Fix: the table-id minting in the s3tables state layer (the value the ARN assembler interpolates after /table/) must produce the service's identifier form; the same minting feeds CreateTable/GetTable, so fixing it in state covers the summary path.
Traces: s3tables-rename-table-conflict-matrix#task#14 (representative), s3tables-rename-table-conflict-matrix#task#15.

Cause 3 of 3: GetTableBucketEncryption reports a default AES256 configuration instead of the table bucket's configured SSE-KMS configuration.
AWS: Smithy: GetTableBucketEncryption (GET /buckets/{tableBucketARN}/encryption, 200) returns @required encryptionConfiguration : EncryptionConfiguration, whose @required sseAlgorithm is the SSEAlgorithm enum [AES256, aws:kms] and whose kmsKeyArn (length 1..2048, pattern ^(arn:aws[-a-z0-9]*:kms:[-a-z0-9]*:[0-9]{12}:key/.+)$) is documented as "required only when sseAlgorithm is set to aws:kms". A table bucket's encryption configuration is supplied by CreateTableBucket.encryptionConfiguration or PutTableBucketEncryption.encryptionConfiguration (both target the same EncryptionConfiguration shape), and the read must echo the configuration in force for that bucket. The recording shows exactly that: sseAlgorithm 'aws:kms' together with the KMS key ARN that was configured for the bucket.
Emulator: answers 200 with encryptionConfiguration.sseAlgorithm = 'AES256' and no kmsKeyArn member at all, i.e. a fixed S3-managed-key default, for a bucket whose recorded configuration is SSE-KMS with a customer key. The configured algorithm/key is therefore either not persisted when the bucket's encryption is set, or not read back by this handler.
Why: this is a silent security-posture misreport. A client that reads the bucket's encryption to verify SSE-KMS is in force, to discover which KMS key to grant the S3 Tables maintenance principal, or to gate a compliance check, is told the bucket uses S3-managed keys and is given no key ARN — so it either fails the check against a correctly configured bucket or concludes KMS encryption is absent when it is not.
Hypothesis: PutTableBucketEncryption is an unimplemented 501 scaffold stub in this build (open issues 20261101T233620Z-s3tables-puttablebucketencryption-stub.md and 20261102T065133Z-s3tables-encryption-metrics-replication-put-operations-answer-501.md), which would be upstream of the read for configurations set after creation; that does not cover the read path itself, which must serve the bucket's stored configuration (including one supplied at CreateTableBucket time) rather than a hard-coded AES256 default.
Fix: the s3tables table-bucket state must store the encryptionConfiguration supplied at bucket creation and by the encryption Put, and the GetTableBucketEncryption handler must serialize that stored configuration — emitting kmsKeyArn whenever sseAlgorithm is aws:kms — instead of a constant.
Traces: s3tables-table-bucket-encryption-config#task#5.

## Evidence
Replay of corpus simparity-wire-aws-v1 against the emulator built from this checkout (emulator up, 59 fixtures replayed). All three clusters below are reported with verdict=mismatch, stub_path empty and owned_by empty (unowned).

Cause 1 + Cause 2 - cluster service=s3tables|ListTables|status=200->200, fixture s3tables-rename-table-conflict-matrix step task#14, expected status 200 / actual status 200:
  $.tables[name="sim2real_tbl_one_..."].createdAt: value_mismatch (expected a shaped date-time string, got 1793733289.3494613)
  $.tables[name="sim2real_tbl_one_..."].modifiedAt: value_mismatch (expected a shaped date-time string, got 1793733289.3494613)
  $.tables[name="sim2real_tbl_one_..."].tableARN: value_mismatch (expected the recorded ARN shape, got 'arn:aws:s3tables:us-west-2:000000000000:bucket/sim2real-probe-s3t-rename-.../table/01b753218893605f10d24da9e8a12e0c')
  $.tables[name="sim2real_tbl_two_..."].createdAt / .modifiedAt: same divergence, got 1793733289.3535202
  $.tables[name="sim2real_tbl_two_..."].tableARN: got '.../table/cf1ecad16f1fce67c596c409d30f2e81'
Second cluster, step task#15 of the same fixture, expected 200 / actual 200: the surviving summary row repeats both divergences (createdAt/modifiedAt got 1793733289.3573287; tableARN got '.../table/26709e13a3928cac1330ea2c961538a2').
The instants and the particular ids are per-run values and are NOT the requirement: the requirements are the JSON type (date-time string, not number) and the id FORM after /table/ (hyphenated 8-4-4-4-12, as a CreateTable recording in the same corpus shows: '.../table/37c67843-744e-48c1-8d27-4aeac8da2079'). No status, error code or error message is involved - both steps returned 200 as recorded.

Cause 3 - cluster service=s3tables|GetTableBucketEncryption|status=200->200, fixture s3tables-table-bucket-encryption-config step task#5, expected status 200 / actual status 200:
  $.encryptionConfiguration.sseAlgorithm: value_mismatch (expected 'aws:kms', got 'AES256')
  $.encryptionConfiguration.kmsKeyArn: missing (expected the configured KMS key ARN, got None)
The recorded key ARN value ('arn:aws:kms:us-west-2:000000000000:key/<key-id>') is a per-run value; the requirement is that the read echoes the bucket's configured sseAlgorithm and, for aws:kms, the configured kmsKeyArn. No status or error-code divergence: the emulator answered 200.

## Steps to Reproduce
1. Replay fixture s3tables-rename-table-conflict-matrix step task#14 (ListTables over the rename-conflict table bucket after the rename matrix has created two tables). For each element of $.tables, inspect the JSON type of createdAt and modifiedAt: the emulator returns a bare number, the recording an RFC3339 date-time string.
2. In the same elements, inspect the segment after /table/ in tableARN: the emulator returns an unhyphenated 32-character hex digest, the recording a hyphenated UUID.
3. Replay step task#15 of the same fixture (ListTables after the conflicting rename) and observe both divergences again on the surviving summary row.
4. Replay fixture s3tables-table-bucket-encryption-config step task#5 (GetTableBucketEncryption on the table bucket whose encryption is configured as SSE-KMS with a customer KMS key earlier in the same fixture). The emulator answers 200 with encryptionConfiguration.sseAlgorithm = 'AES256' and no kmsKeyArn; the recording has sseAlgorithm 'aws:kms' together with the configured key ARN.
