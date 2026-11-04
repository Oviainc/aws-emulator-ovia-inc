Issue-Source: trace-sweep-divergence
Issue-Trace: s3tables-rename-table-conflict-matrix
Issue-Step: task#14
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3tables-rename-table-conflict-matrix s3tables-table-bucket-metrics-config s3tables-table-bucket-metrics-configuration s3tables-table-bucket-metrics-configuration-lifecycle
Issue-Shape-Id: com.amazonaws.s3tables#S3TableBuckets
Issue-Key: s3tables|ListTables|2xx->2xx|$.tables[name="sim2real_tbl_one_cf956e38"].createdAt
Issue-Key: s3tables|ListTables|2xx->2xx|$.tables[name="sim2real_tbl_one_cf956e38"].modifiedAt
Issue-Key: s3tables|ListTables|2xx->2xx|$.tables[name="sim2real_tbl_one_cf956e38"].tableARN
Issue-Key: s3tables|ListTables|2xx->2xx|$.tables[name="sim2real_tbl_two_cf956e38"].createdAt
Issue-Key: s3tables|ListTables|2xx->2xx|$.tables[name="sim2real_tbl_two_cf956e38"].modifiedAt
Issue-Key: s3tables|ListTables|2xx->2xx|$.tables[name="sim2real_tbl_two_cf956e38"].tableARN
Issue-Key: s3tables|GetTableBucketMetricsConfiguration|4xx->4xx|$.message
Issue-Key: s3tables|GetTableBucketMetricsConfiguration|2xx->2xx|$.id
Issue-Key: s3tables|GetTableBucketMetricsConfiguration|4xx->2xx|$.message
Issue-Key: s3tables|GetTableBucketMetricsConfiguration|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: s3tables|GetTableBucketMetricsConfiguration|4xx->2xx|status
Issue-Commit: 49d3116d0296255c615a3da18aeed15b4a7eb9dc

# s3tables: 5 net-new causes — ListTables summary wire shapes and table-bucket metrics configuration reads diverge

> _Note: this issue was identified against the code base at commit `49d3116d0296255c615a3da18aeed15b4a7eb9dc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListTables, GetTableBucketMetricsConfiguration

## Description
Five net-new root causes in the s3tables emulator, all reported as UNOWNED clusters by the wire replay of trace set simparity-wire-aws-v1. Two live in the ListTables table-summary projection/serializer and the table-id minting in state; three live in the table-bucket metrics configuration path (the GetTableBucketMetricsConfiguration handler and the metrics state a put/delete owns). A fixer must close ALL FIVE before submitting.

Cross-reference: the ListTables family is already carried for OTHER clusters by the open issues 20261103T052004Z-s3tables-listtables-summaries-in-the-rename-conflict-matrix.md, 20261103T164116Z-s3tables-listtables-table-summaries-serialize-createdat-modifiedat-as.md, 20261103T033559Z-s3tables-list-and-get-responses-emit-epoch-timestamps.md and 20261101T230800Z-s3tables-listtables-summaries-carry-epoch-timestamps-and-unrecorded.md; the replay still reports the two clusters below as unowned because those issues' replay keys embed per-run table names that no longer match, so the keys are re-bound here. Cause 3 is the same root cause as the open BEHAVIORAL issue 20261104T070242Z-s3tables-gettablebucketmetricsconfiguration-returns-the-wrong-notfoundexception-message-for.md, which carries no wire replay key; this entry adds the wire binding. Cause 5's immediate trigger is the Delete stub filed as 20261101T233620Z-s3tables-deletetablebucketmetricsconfiguration-stub.md — the read-side requirement is recorded here because no open issue carries it.

Cause 1 of 5: ListTables table summaries serialize createdAt / modifiedAt as bare JSON numbers instead of modeled date-time strings.
AWS: Smithy (services/service-com-amazonaws-s3tables-s3tablebuckets/smithy-model/com-amazonaws-s3tables-s3tablebuckets.json) models ListTables as GET /tables/{tableBucketARN} -> 200 with required output member `tables` (TableSummaryList); com.amazonaws.s3tables#TableSummary declares createdAt and modifiedAt as @required Timestamp members carrying timestampFormat "date-time". Recorded ListTables responses accordingly carry an RFC3339 instant string with sub-second precision for both members, never a number.
Emulator: every table summary renders createdAt and modifiedAt as an epoch float emitted as a bare JSON number (replay: expected a date-time string, got 1793784193.6553676 / 1793784193.6609676 on task#14 and 1793784193.6664438 on task#15). The HTTP status is correct (200 expected, 200 actual) — only the serialized JSON type diverges.
Why: "date-time" is what every generated SDK deserializes these members from, so a number either fails to parse or is coerced to a different instant; any client that reads a table's creation or modification time out of a listing is broken, and the @required contract is satisfied in name only.
Fix: the timestamp rendering in the s3tables ListTables response serializer / table-summary projection, so Timestamp members are emitted per the modeled date-time format.
Traces: s3tables-rename-table-conflict-matrix#task#14 (representative), s3tables-rename-table-conflict-matrix#task#15.

Cause 2 of 5: the table identifier inside tableARN is a 32-character hex digest, not the identifier form the service mints.
AWS: Smithy: TableSummary.tableARN targets TableARN, @required, length 1..2048, pattern ^(arn:aws[-a-z0-9]*:[a-z0-9]+:[-a-z0-9]*:[0-9]{12}:bucket/[a-z0-9_-]{3,63}/table/[a-zA-Z0-9-_]{1,255})$, arnReference AWS::S3Tables::Table. The model does not fix the id FORM, so that part is recorded behavior only: in the recordings the segment after /table/ is a canonical hyphenated 8-4-4-4-12 UUID (a CreateTable response in the same corpus records e.g. .../table/37c67843-744e-48c1-8d27-4aeac8da2079).
Emulator: assembles the ARN with a 32-character unhyphenated hex digest after /table/ (replay task#14: got arn:aws:s3tables:us-west-2:000000000000:bucket/<bucket>/table/6469dd4771fc226552707223178fea58 and .../table/ab1db072dc8ce18a7a99a754588f4c49; task#15: .../table/fc2b5725bbf32fb2133d280fe794db48). Account, region and bucket segments match; only the id form diverges. The specific id values are per-run and are NOT the requirement — the identifier FORM is.
Why: the table ARN is the opaque handle a client stores and re-submits (GetTable / DeleteTable / TagResource all accept it) and the key by which it correlates a listing row with an id obtained elsewhere. A digest-form id makes the emulator's ARNs non-interchangeable with AWS's and breaks any client that recognizes the id form inside an ARN.
Fix: the table-id minting in the s3tables state layer (the value fed into tableARN assembly); the ARN assembler itself needs no change, and the same minting feeds CreateTable/GetTable so one fix covers the summary path.
Traces: s3tables-rename-table-conflict-matrix#task#14 (representative), s3tables-rename-table-conflict-matrix#task#15.

Cause 3 of 5: GetTableBucketMetricsConfiguration's not-found message for a bucket with no metrics configuration is emulator-authored text.
AWS: Smithy models GetTableBucketMetricsConfiguration as GET /buckets/{tableBucketARN}/metrics -> 200 with errors BadRequestException(400), ConflictException(409), ForbiddenException(403), InternalServerErrorException(500), NotFoundException(404 — "The request was rejected because the specified resource could not be found"), TooManyRequestsException(429). The model fixes no message string, so the message is recorded behavior only: on a table bucket that has no metrics configuration AWS refuses the read with HTTP 404, error code NotFoundException and the message `No such metric configuration exists`. That message is WHOLLY FIXED TEXT — it interpolates no bucket name, ARN or id from the request or state.
Emulator: status (404) and code (NotFoundException) are right, but the message is `The specified metrics configuration does not exist.` (different wording, trailing period), read from crate::state::METRICS_CONFIGURATION_NOT_FOUND_MESSAGE in the absent-configuration branch of the handler (services/service-com-amazonaws-s3tables-s3tablebuckets/src/ops/get_table_bucket_metrics_configuration.rs:175-179).
Why: s3tables has a single NotFoundException shape, so the message is the only discriminator between "no such table bucket" and "no such metric configuration". A client that distinguishes "metrics were never enabled" from other 404s mis-classifies the refusal and treats a normal metrics-off state as an unexpected failure.
Fix: the error construction in the GetTableBucketMetricsConfiguration handler's absent-configuration branch and the shared message constant it reads — AWS's fixed text, kept distinct from the bucket-not-found message used by the preceding lookup.
Traces: s3tables-table-bucket-metrics-config#task#2 (representative; the same cluster covers s3tables-table-bucket-metrics-configuration#task#2 and s3tables-table-bucket-metrics-configuration-lifecycle#task#2).

Cause 4 of 5: the metrics configuration id reported after a put is a minted digest instead of the service's fixed whole-bucket configuration id.
AWS: Smithy: GetTableBucketMetricsConfigurationResponse carries required tableBucketARN and `id` (String — "The unique identifier of the metrics configuration"); PutTableBucketMetricsConfiguration is PUT /buckets/{tableBucketARN}/metrics -> 204 and @idempotent, carrying no per-configuration settings. The model fixes no id VALUE, so this is recorded behavior only: after a put, the read answers 200 with id `EntireBucket` — fixed text that is bucket-independent, naming the single whole-bucket metrics configuration S3 Tables maintains.
Emulator: answers 200 with an id minted per put — a 32-character hex digest (replay task#4: expected 'EntireBucket', got '7ecd9003c73365cc3abc48032b3247db'). The value comes from store.mint_metrics_configuration_id() in src/ops/put_table_bucket_metrics_configuration.rs:199-212 and is read back at src/ops/get_table_bucket_metrics_configuration.rs:181-186. The particular digest is per-run; the fixed id is the requirement.
Why: the id is how a client names the configuration it enabled; code and tooling that recognize or re-submit the well-known whole-bucket id see an unrecognized opaque value, and the configuration's identity is neither stable nor meaningful across the emulator and AWS.
Fix: the metrics-configuration state the put writes (and therefore the id the Get reports) — the single whole-bucket configuration must carry AWS's fixed id rather than a minted value.
Traces: s3tables-table-bucket-metrics-config#task#4 (representative; the same cluster covers s3tables-table-bucket-metrics-configuration#task#4 and s3tables-table-bucket-metrics-configuration-lifecycle#task#4).

Cause 5 of 5: the metrics configuration survives its deletion, so the read after a delete answers 200 instead of the recorded 404.
AWS: Smithy models DeleteTableBucketMetricsConfiguration as DELETE /buckets/{tableBucketARN}/metrics -> 204, @idempotent. Recorded behavior: once the configuration is removed, GetTableBucketMetricsConfiguration on the same bucket refuses with HTTP 404, error code NotFoundException, x-amzn-errortype NotFoundException, and the fixed message `No such metric configuration exists` — the same metrics-off refusal as cause 3.
Emulator: the read after the delete answers HTTP 200 with the configuration still present (replay task#6: expected status 404 with $.message 'No such metric configuration exists' and the NotFoundException errortype header, got 200 and no error body at all) — the bucket's metrics state is never cleared. The immediate trigger is that the Delete handler is still an unimplemented scaffold answering 501 (src/ops/delete_table_bucket_metrics_configuration.rs:186), which is filed as 20261101T233620Z-s3tables-deletetablebucketmetricsconfiguration-stub.md; what no open issue records, and what this cause pins, is the READ-side requirement the recording asserts: after the configuration is gone the Get must refuse with 404 / NotFoundException / that fixed message.
Why: a client cannot turn metrics off — a delete followed by a read still reports metrics enabled, so cleanup, idempotency and "is metrics on?" probes all observe state AWS no longer has.
Fix: the metrics state cleared by the DeleteTableBucketMetricsConfiguration handler (clearing the bucket's metrics record), so the Get's absent-configuration branch (cause 3) is the path taken afterwards.
Traces: s3tables-table-bucket-metrics-config#task#6 (representative; the same cluster covers s3tables-table-bucket-metrics-configuration#task#6 and s3tables-table-bucket-metrics-configuration-lifecycle#task#6).

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator up; /__admin/health 200). All five clusters below were reported with an EMPTY owned_by and an EMPTY stub_path, i.e. net-new.

Cause 1 + Cause 2 — fixture s3tables-rename-table-conflict-matrix, ListTables, expected 200 / actual 200:
  step task#14:
    $.tables[name="sim2real_tbl_one_..."].createdAt: value_mismatch (expected a date-time string, got 1793784193.6553676)
    $.tables[name="sim2real_tbl_one_..."].modifiedAt: value_mismatch (expected a date-time string, got 1793784193.6553676)
    $.tables[name="sim2real_tbl_one_..."].tableARN: value_mismatch (got '...bucket/sim2real-probe-s3t-rename-.../table/6469dd4771fc226552707223178fea58')
    $.tables[name="sim2real_tbl_two_..."].createdAt / .modifiedAt: same, got 1793784193.6609676
    $.tables[name="sim2real_tbl_two_..."].tableARN: got '.../table/ab1db072dc8ce18a7a99a754588f4c49'
  step task#15 (after the conflicting rename): same two divergences on the surviving row, got 1793784193.6664438 and '.../table/fc2b5725bbf32fb2133d280fe794db48'.
  Requirement: JSON TYPE (date-time string, not number) and ARN id FORM (hyphenated 8-4-4-4-12 UUID, as recorded elsewhere in this corpus: '.../table/37c67843-744e-48c1-8d27-4aeac8da2079'). The instants and ids themselves are per-run values.

Cause 3 — fixture s3tables-table-bucket-metrics-config step task#2, GetTableBucketMetricsConfiguration on a bucket with no metrics configuration, expected 404 / actual 404:
  Expected (AWS):   HTTP 404, x-amzn-errortype NotFoundException, message "No such metric configuration exists"   <- wholly fixed text
  Actual (emulator): HTTP 404, NotFoundException, message "The specified metrics configuration does not exist."
  Cluster also affects s3tables-table-bucket-metrics-configuration#task#2 and s3tables-table-bucket-metrics-configuration-lifecycle#task#2.

Cause 4 — same fixture step task#4, GetTableBucketMetricsConfiguration after a put, expected 200 / actual 200:
  $.id: value_mismatch (expected 'EntireBucket', got '7ecd9003c73365cc3abc48032b3247db')
  tableBucketARN matched. Cluster also affects s3tables-table-bucket-metrics-configuration#task#4 and s3tables-table-bucket-metrics-configuration-lifecycle#task#4.

Cause 5 — same fixture step task#6, GetTableBucketMetricsConfiguration after the configuration was deleted:
  status: expected 404, got 200
  $.message: missing (expected 'No such metric configuration exists', got None)
  $headers.x-amzn-errortype: missing (expected 'NotFoundException:http://internal.amazon.com/coral/com.amazon.s3tablebuckets/', got None)
  Cluster also affects s3tables-table-bucket-metrics-configuration#task#6 and s3tables-table-bucket-metrics-configuration-lifecycle#task#6.

Code the diverging values come from:
  services/service-com-amazonaws-s3tables-s3tablebuckets/src/ops/get_table_bucket_metrics_configuration.rs:175-179 (message constant) and :181-186 (id echoed from state)
  services/service-com-amazonaws-s3tables-s3tablebuckets/src/ops/put_table_bucket_metrics_configuration.rs:199-212 (mint_metrics_configuration_id)
  services/service-com-amazonaws-s3tables-s3tablebuckets/src/ops/delete_table_bucket_metrics_configuration.rs:186 (unimplemented, 501 — state never cleared)

## Steps to Reproduce
1. Replay fixture s3tables-rename-table-conflict-matrix step task#14 (ListTables over the rename-conflict table bucket after the matrix created two tables). In each element of $.tables inspect the JSON type of createdAt / modifiedAt (emulator: bare number; recording: date-time string) and the segment after /table/ in tableARN (emulator: 32-char hex digest; recording: hyphenated UUID). Replay step task#15 and observe both again on the surviving row. [causes 1 and 2]
2. Replay fixture s3tables-table-bucket-metrics-config step task#2: CreateTableBucket, then GetTableBucketMetricsConfiguration(tableBucketARN=<arn>) with no metrics configuration set. Observe 404 / NotFoundException with message 'The specified metrics configuration does not exist.'; AWS answers 404 / NotFoundException with 'No such metric configuration exists'. [cause 3]
3. Same fixture step task#4: PutTableBucketMetricsConfiguration(tableBucketARN=<arn>) (204), then GetTableBucketMetricsConfiguration(tableBucketARN=<arn>). Observe 200 with $.id set to a 32-char hex digest; AWS returns $.id = 'EntireBucket'. [cause 4]
4. Same fixture step task#6: DeleteTableBucketMetricsConfiguration(tableBucketARN=<arn>), then GetTableBucketMetricsConfiguration(tableBucketARN=<arn>). Observe 200 with the configuration still reported; AWS answers 404 / NotFoundException / 'No such metric configuration exists'. (The delete itself currently answers 501 — see the stub issue named in the description.) [cause 5]
5. Re-replay to confirm the fix: s3tables-rename-table-conflict-matrix#task#14, s3tables-rename-table-conflict-matrix#task#15, s3tables-table-bucket-metrics-config#task#2, s3tables-table-bucket-metrics-config#task#4, s3tables-table-bucket-metrics-config#task#6 (and the sibling fixtures s3tables-table-bucket-metrics-configuration / s3tables-table-bucket-metrics-configuration-lifecycle at the same step keys).
