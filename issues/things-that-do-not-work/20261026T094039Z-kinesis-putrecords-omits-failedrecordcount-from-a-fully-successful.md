Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: firehose__glue__kinesis__s3.kds-put-records-accepts-the-stream-arn-in-place-of-the-stream-name
Issue-Key: firehose__glue__kinesis__s3.kds-put-records-accepts-the-stream-arn-in-place-of-the-stream-name|run#6|KeyError|assert response["FailedRecordCount"] == 0, (
Issue-Commit: b76b7166d3d256a1ac9f503358a3b66c42be670e

# Kinesis PutRecords omits FailedRecordCount from a fully successful batch instead of reporting 0

> _Note: this issue was identified against the code base at commit `b76b7166d3d256a1ac9f503358a3b66c42be670e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutRecords

## Description
One net-new root cause (Cause 1 of 1).

Cause 1 of 1: `PutRecords` omits the `FailedRecordCount` output member whenever no record in the batch failed, so a success response carries only `Records` and `EncryptionType`. Every AWS SDK's canonical PutRecords idiom reads `FailedRecordCount` unconditionally to decide whether a retry pass over the partial failures is needed, so the member's absence is not a cosmetic difference — it is a missing key.

AWS: Smithy (services/service-com-amazonaws-kinesis-kinesis-20131202/smithy-model/com-amazonaws-kinesis-kinesis-20131202.json) models `PutRecordsOutput$FailedRecordCount` as `PositiveIntegerObject` (range min 1), not `@required`, and documents it as "The number of unsuccessfully processed records in a PutRecords request". The model's range therefore says nothing about the all-succeeded case; recorded behavior only: real Kinesis returns the member on EVERY 200 PutRecords response, with the value `0` when every entry was stored. The behavioral check asserts exactly that (`assert response["FailedRecordCount"] == 0`) for all three reference spellings (StreamName alone, StreamARN alone, both together), which is only satisfiable if the field is always present. Status and payload otherwise match: HTTP 200, no error code.

Emulator: the handler returns `failed_record_count: (failed > 0).then_some(failed)` — i.e. `None` for a clean batch — and the comment there justifies it from the modeled `range 1..` constraint. The serializer then drops the absent member, and the SDK-level response has no `FailedRecordCount` key at all. The replayed check dies on the first arm (the StreamName write) with `KeyError: 'FailedRecordCount'`.

Why: a client that follows the documented PutRecords retry pattern crashes with a KeyError / missing-field error on a request that fully succeeded, before it can inspect `Records`. Strongly-typed SDKs surface it as an absent optional and silently treat "no count" as indistinguishable from an unparsed response, so partial-failure handling goes untested and producers that key their retry loop on the count break against the emulator while working against AWS. The emulator's own choice makes the HAPPY path the broken one, which is the worst case for a data-plane write.

Fix: the `PutRecords` output construction in the operation's handler — services/service-com-amazonaws-kinesis-kinesis-20131202/src/ops/put_records.rs — must always populate `failed_record_count` with the batch's failure count, including `Some(0)`, and the operation's output serializer must emit the member for a zero value rather than eliding it (the modeled `range 1..` must not be read as permission to omit the field). The per-entry `Records` array construction above it is correct and unchanged.

Note on scope: because the check aborts on this first assertion, the remainder of that same check is left ungraded by this run — the shard-identity assertions for the ARN/both spellings, the three-record drain ordering, and the `PutRecords` with NEITHER StreamName nor StreamARN arm (which must be refused `InvalidArgumentException` with message "Stream arn and stream name can't be empty at the same time": fixed text, no variable parts). Those are not claimed as divergences here; re-replaying the check after this fix is what grades them.

No open issue covers this: the existing kinesis-named issues all concern Kinesis Analytics routing, Kinesis Video registries, or cross-service consumers of kinesis data-plane stubs (e.g. 20261026T022914Z-events-kinesis-..., 20261026T074043Z-firehose-kinesis-s3-...), none of them the PutRecords success-response shape.

## Evidence
Failing check: firehose__glue__kinesis__s3.kds-put-records-accepts-the-stream-arn-in-place-of-the-stream-name [error]

Expected (real AWS, HTTP 200): PutRecords response contains `FailedRecordCount` on every success, valued 0 when no entry failed, alongside the `Records` array of one entry per request record.

Actual (emulator): `FailedRecordCount` is absent from the 200 response. Replay trace:

  File "check_firehose__glue__kinesis__s3_kds_put_records_accepts_the_stream_arn_in_place_of_the_stream_name.py", line 104, in run
    assert response["FailedRecordCount"] == 0, (
  KeyError: 'FailedRecordCount'

The failure occurs on the first of the three arms (`put_records(StreamName=..., Records=[...])`), so it is not specific to the ARN spelling.

Source of the behavior, services/service-com-amazonaws-kinesis-kinesis-20131202/src/ops/put_records.rs:416-422:

  Ok(PutRecordsOutput {
      records,
      // `FailedRecordCount` is modelled `range 1..`, so a batch nothing failed in
      // reports no count at all rather than a zero the member cannot carry.
      failed_record_count: (failed > 0).then_some(failed),
      encryption_type: Some(encryption_type),
  })

Model reference: PutRecordsOutput members = FailedRecordCount (PositiveIntegerObject, range 1.., optional), Records (PutRecordsResultEntryList, required, len 1..500), EncryptionType (enum KMS|NONE).

## Steps to Reproduce
1. kinesis.create_stream(StreamName=<name>, ShardCount=1, StreamModeDetails={"StreamMode": "PROVISIONED"}) and poll describe_stream_summary until StreamStatus == ACTIVE and list_shards(StreamName=<name>) succeeds.
2. resp = kinesis.put_records(StreamName=<name>, Records=[{"Data": b'{"id":"by-name"}\n', "PartitionKey": "pk"}])
3. Inspect resp: the emulator returns HTTP 200 with `Records` (one entry carrying ShardId and SequenceNumber) and `EncryptionType`, but no `FailedRecordCount` key. Real AWS returns `FailedRecordCount: 0`.
4. Repeat step 2 with StreamARN=<arn> only, and with both StreamName and StreamARN: same omission.
5. Equivalent: re-replay behavioral check firehose__glue__kinesis__s3.kds-put-records-accepts-the-stream-arn-in-place-of-the-stream-name, which raises KeyError: 'FailedRecordCount' at its first assertion.
