Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3tables.table-bucket-metrics-configuration-toggle-server-enforced
Issue-Shape-Id: com.amazonaws.s3tables#S3TableBuckets
Issue-Key: s3tables.table-bucket-metrics-configuration-toggle-server-enforced|run#13|AssertionError|assert final["id"] == last_id, (
Issue-Commit: 6739bb5760215e5968a004f0fc2bacf6f12cc28a

# s3tables: the table-bucket metrics configuration id is re-minted on every PutTableBucketMetricsConfiguration instead of being the bucket's fixed whole-bucket id

> _Note: this issue was identified against the code base at commit `6739bb5760215e5968a004f0fc2bacf6f12cc28a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetTableBucketMetricsConfiguration

## Description
One net-new root cause in the s3tables table-bucket metrics configuration path, grounded in the behavioral check s3tables.table-bucket-metrics-configuration-toggle-server-enforced. Every other failing check in this suite run was reported as ALREADY FILED (blocked by an unimplemented scaffold stub) or OWNED by an open issue, so this is the only surviving cluster.

Cross-reference: the open WIRE issue 20261104T092732Z-s3tables-5-net-new-causes-listtables-summary-wire.md records the same root cause as its 'Cause 4 of 5' (the id is a minted digest rather than the service's fixed whole-bucket id). That issue carries only wire replay keys (s3tables|GetTableBucketMetricsConfiguration|2xx->2xx|$.id) and no behavioral key, so the replay reports this behavioral check as unowned; this entry adds the behavioral binding and the additional stability requirement the check asserts across put/delete cycles. It does not supersede that issue.

Cause 1 of 1: GetTableBucketMetricsConfiguration reports a freshly minted per-put identifier, so the metrics configuration id is neither bucket-stable nor AWS's value.
AWS: Smithy (services/service-com-amazonaws-s3tables-s3tablebuckets/smithy-model/com-amazonaws-s3tables-s3tablebuckets.json) models PutTableBucketMetricsConfiguration as PUT /buckets/{tableBucketARN}/metrics -> 204, @idempotent, whose only input member is the tableBucketARN httpLabel — the put carries NO per-configuration settings, so there is nothing per-call for an id to be derived from. GetTableBucketMetricsConfiguration is GET /buckets/{tableBucketARN}/metrics -> 200 with required tableBucketARN and `id` (String, 'The unique identifier of the metrics configuration'). The model fixes no id VALUE, so the value is recorded behavior only: S3 Tables maintains a single whole-bucket metrics configuration per table bucket and reports it with the fixed, bucket-independent id `EntireBucket`. Because the id is a property of that one configuration and not of a call, it is identical on every read of the same bucket and is unchanged by a delete/re-put cycle. DeleteTableBucketMetricsConfiguration (DELETE, 204, @idempotent) removes the configuration; a read afterwards refuses 404 / NotFoundException / 'No such metric configuration exists' (that refusal path now behaves correctly — the check passed all three delete cycles' refusal assertions).
Emulator: the id is minted per put. Over three put/get/delete cycles on one table bucket the emulator answered 200 with a different 32-hex-character value each time, and the final put after the last delete produced yet another id: the check's last read got '4c70d0c7-8594-4313-9359-38de628d45fb' where the preceding cycle had reported '7e307b87-7baf-4b3c-810b-1e3f42a27f05' (both per-run values; neither is the requirement, and neither is `EntireBucket`). tableBucketARN echoes correctly, the status is 200, and the metrics-off 404 after each delete is correct — only $.id diverges.
Why: the id is the handle a client uses to name the configuration it enabled. A client that recognizes or re-submits the well-known whole-bucket id sees an unrecognized opaque value; worse, a client that caches the id and compares it after toggling metrics off and on concludes the configuration was replaced, because the emulator's id is a per-call artifact rather than a stable property of the bucket. Idempotency of the modeled @idempotent put is therefore observably violated.
Fix: the metrics-configuration state written by the PutTableBucketMetricsConfiguration handler and read back by the GetTableBucketMetricsConfiguration handler — the single whole-bucket configuration must carry AWS's fixed id, and a put over an existing (or previously deleted) configuration must not re-mint it.
Traces: behavioral check s3tables.table-bucket-metrics-configuration-toggle-server-enforced (final assertion, after three put/delete cycles). The wire issue named above pins the same value divergence at s3tables-table-bucket-metrics-config#task#4.

## Evidence
Behavioral suite simparity-behavioral-aws-v1 replayed against the emulator built from this checkout.

[mismatch] s3tables.table-bucket-metrics-configuration-toggle-server-enforced
  File "check_s3tables_table_bucket_metrics_configuration_toggle_server_enforced.py", line 60, in run
    assert final["id"] == last_id
  AssertionError: re-enabling after a delete produced a different id ('4c70d0c7-8594-4313-9359-38de628d45fb') than the prior cycle's ('7e307b87-7baf-4b3c-810b-1e3f42a27f05') -- the metrics configuration id is expected to be a stable property of the bucket, not re-minted per Put

What the check established BEFORE that assertion (all passed, so these paths are correct today): for each of three cycles, PutTableBucketMetricsConfiguration(tableBucketARN) then GetTableBucketMetricsConfiguration returned 200 with tableBucketARN echoed; DeleteTableBucketMetricsConfiguration then GetTableBucketMetricsConfiguration refused with code NotFoundException and message exactly 'No such metric configuration exists' (wholly fixed text, no interpolated bucket name or ARN).

Expected vs actual on the id:
  Expected (AWS, recorded behavior): $.id is the fixed whole-bucket identifier 'EntireBucket' — bucket-independent fixed text, identical on every read of the bucket and unchanged across delete/re-put.
  Actual (emulator): $.id is a newly minted opaque value on every put; two reads of the SAME bucket separated by a delete and a re-put returned two different ids.
The specific id strings quoted above are per-run values and are NOT the requirement; the requirement is the fixed value and its stability across puts.

Code the diverging value comes from (cited by the open wire issue 20261104T092732Z-s3tables-5-net-new-causes-listtables-summary-wire.md, which records the same root cause):
  services/service-com-amazonaws-s3tables-s3tablebuckets/src/ops/put_table_bucket_metrics_configuration.rs (mint_metrics_configuration_id)
  services/service-com-amazonaws-s3tables-s3tablebuckets/src/ops/get_table_bucket_metrics_configuration.rs (id echoed from state)

## Steps to Reproduce
1. CreateTableBucket(name=<unique name>) and keep the returned arn.
2. Repeat three times: PutTableBucketMetricsConfiguration(tableBucketARN=<arn>) (expect 204); GetTableBucketMetricsConfiguration(tableBucketARN=<arn>) and record $.id; DeleteTableBucketMetricsConfiguration(tableBucketARN=<arn>); GetTableBucketMetricsConfiguration(tableBucketARN=<arn>) and observe 404 / NotFoundException / 'No such metric configuration exists'.
3. PutTableBucketMetricsConfiguration(tableBucketARN=<arn>) once more, then GetTableBucketMetricsConfiguration(tableBucketARN=<arn>).
4. Observe that every recorded $.id differs from the previous one, and that none of them is AWS's fixed whole-bucket id 'EntireBucket'. On AWS all four reads return the same id.
5. Re-replay to confirm the fix: behavioral check s3tables.table-bucket-metrics-configuration-toggle-server-enforced.
