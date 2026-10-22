Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 7b44f3f32d1f58d662874eb8ca0f546e296cee00

# Bootstrap the firehose emulator

_Filed: 2026-10-21T21:08:13Z_
_Operation: service-com-amazonaws-firehose-firehose-20150804 | Class: missing_

> _Note: this issue was identified against the code base at commit `7b44f3f32d1f58d662874eb8ca0f546e296cee00`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 12 operations in this crate is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state, so no Firehose client can do anything. Implement a first, SMALL stateful slice: the delivery-stream lifecycle plus record ingestion, in this order.

1. **CreateDeliveryStream** — input `DeliveryStreamName` (required, 1..64, `^[a-zA-Z0-9_.-]+$`), `DeliveryStreamType` (default `DirectPut`; one of DatabaseAsSource | DirectPut | KinesisStreamAsSource | MSKAsSource), and the destination configs (at minimum honour `S3DestinationConfiguration` / `ExtendedS3DestinationConfiguration`), plus `Tags`. Store a new stream record and return the generated `DeliveryStreamARN` in the modeled form `arn:aws:firehose:<region>:<account>:deliverystream/<name>`. Creating a name that already exists must fail with `ResourceInUseException` (HTTP 400).
2. **DescribeDeliveryStream** — input `DeliveryStreamName` (required); must return the stored `DeliveryStreamDescription` with `DeliveryStreamName`, `DeliveryStreamARN`, `DeliveryStreamStatus` (`ACTIVE` once created), `DeliveryStreamType`, `VersionId`, `CreateTimestamp`, `LastUpdateTimestamp`, `Destinations` (each with its assigned `DestinationId` and the description built from the config supplied at create time) and `HasMoreDestinations`. An unknown name must fail with `ResourceNotFoundException` (HTTP 400).
3. **ListDeliveryStreams** — must return `DeliveryStreamNames` from the store (names only, sorted) and `HasMoreDeliveryStreams`, honouring `Limit` (1..10000), `ExclusiveStartDeliveryStreamName` and the optional `DeliveryStreamType` filter. Never a hardcoded/empty list when streams exist.
4. **PutRecord** — input `DeliveryStreamName` (required) and `Record` (required, `Data` blob 0..1024000). Append the record to the target stream's buffer and return a freshly generated unique `RecordId` (required, len >= 1). Unknown stream name must fail with `ResourceNotFoundException`.
5. **PutRecordBatch** — input `Records` (1..500, required). Append all records and return `FailedPutCount` (0 on the happy path) and a `RequestResponses` list with one `PutRecordBatchResponseEntry` per input record, each carrying its own distinct `RecordId`, positionally aligned with the input list. Unknown stream name must fail with `ResourceNotFoundException`; an empty or >500 record list must be rejected with `InvalidArgumentException`.
6. **DeleteDeliveryStream** — input `DeliveryStreamName` (required), `AllowForceDelete`. Remove the stream from the store so a subsequent DescribeDeliveryStream returns `ResourceNotFoundException` and ListDeliveryStreams no longer shows the name. Deleting an unknown name returns `ResourceNotFoundException`.

Every one of these must compute its response from persisted state reached through `crate::state::service_state(&*self.engine)` — no canned responses, no echoing the input back, no generating an ARN or RecordId that is never stored. A create followed by a describe must return exactly what was created; records put must be observable in the stream's buffer (count/ordering) by subsequent operations and by any later-implemented delivery logic.

## State Model

In `src/state.rs` (the per-service state held by the shared engine):

- `streams: BTreeMap<String /* DeliveryStreamName */, DeliveryStream>` — primary index, name-keyed; BTreeMap so ListDeliveryStreams ordering/`ExclusiveStartDeliveryStreamName` paging is deterministic.
- `DeliveryStream { name: String, arn: String, status: DeliveryStreamStatus /* ACTIVE */, stream_type: DeliveryStreamType, version_id: u64 /* rendered as decimal string for VersionId / CurrentDeliveryStreamVersionId */, create_timestamp: SystemTime, last_update_timestamp: SystemTime, source: Option<SourceDescription>, destinations: Vec<Destination>, encryption: Option<DeliveryStreamEncryptionConfiguration>, tags: BTreeMap<String, String>, records: Vec<StoredRecord>, next_destination_id: u64 }`
- `Destination { destination_id: String /* "destinationId-00000000000<n>" */, config: DestinationConfigKind }` where `DestinationConfigKind` is an enum over the modeled destination variants (S3/ExtendedS3/Redshift/Elasticsearch/Splunk/HttpEndpoint/Snowflake/Iceberg/OpenSearchServerless) so DescribeDeliveryStream can render the matching `*DestinationDescription` member.
- `StoredRecord { record_id: String /* unique, len >= 1 */, data: Vec<u8>, received_at: SystemTime }` — appended by PutRecord/PutRecordBatch.
- A monotonic `record_counter: u64` (service-level) used to mint unique `RecordId`s.

The remaining operations (UpdateDestination, TagDeliveryStream, UntagDeliveryStream, ListTagsForDeliveryStream, StartDeliveryStreamEncryption, StopDeliveryStreamEncryption) stay 501 for now and are tracked as separate gap issues — but the state model above already carries the `tags`, `encryption` and `version_id` fields they will need.

Done = a realistic integration test drives create -> describe -> list -> put-record -> put-record-batch -> delete -> describe-gone end to end against the awsJson1_1 wire protocol (`POST /`, `X-Amz-Target: Firehose_20150804.<Op>`), with response bodies asserted field by field, not just 2xx.

## Evidence

`services/service-com-amazonaws-firehose-firehose-20150804/OPERATIONS.md` lists all 12 bound operations with their input/output members and errors (CreateDeliveryStream -> `DeliveryStreamARN`; DescribeDeliveryStream -> required `DeliveryStreamDescription`; ListDeliveryStreams -> required `DeliveryStreamNames` + `HasMoreDeliveryStreams`; PutRecord -> required `RecordId`; PutRecordBatch -> required `FailedPutCount` + `RequestResponses`; DeleteDeliveryStream -> empty output; errors `ResourceInUseException`, `ResourceNotFoundException`, `InvalidArgumentException` all HTTP 400). `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' src/ops/` matches all 12 handler files (create_delivery_stream.rs, describe_delivery_stream.rs, list_delivery_streams.rs, put_record.rs, put_record_batch.rs, delete_delivery_stream.rs, update_destination.rs, tag_delivery_stream.rs, untag_delivery_stream.rs, list_tags_for_delivery_stream.rs, start_delivery_stream_encryption.rs, stop_delivery_stream_encryption.rs), each returning `Err(crate::scaffolded::Scaffolded::not_implemented(..))` = HTTP 501. The `DeliveryStreamDescription`, `DestinationDescription`, `Record` and `PutRecordBatchResponseEntry` field lists in the State Model come from the Smithy model (`smithy_query query=shape`). README.md confirms the awsJson1_1 wire protocol and `Firehose_20150804` target prefix.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI firehose client at it.
2. CreateDeliveryStream with DeliveryStreamName="demo-stream", DeliveryStreamType="DirectPut" and an S3DestinationConfiguration — expect 200 with a DeliveryStreamARN ending in `deliverystream/demo-stream` (currently 501).
3. CreateDeliveryStream again with the same name — expect ResourceInUseException (HTTP 400).
4. DescribeDeliveryStream DeliveryStreamName="demo-stream" — expect DeliveryStreamDescription with status ACTIVE, matching ARN/type, VersionId "1", CreateTimestamp set, and one entry in Destinations carrying the S3 config supplied at create time.
5. ListDeliveryStreams — expect DeliveryStreamNames to contain "demo-stream" and HasMoreDeliveryStreams=false.
6. PutRecord DeliveryStreamName="demo-stream", Record.Data=base64("hello") — expect 200 with a non-empty RecordId.
7. PutRecordBatch with 3 records — expect FailedPutCount=0 and RequestResponses of length 3, each with a distinct RecordId.
8. PutRecord against DeliveryStreamName="no-such-stream" — expect ResourceNotFoundException (HTTP 400).
9. DeleteDeliveryStream DeliveryStreamName="demo-stream" — expect 200, then DescribeDeliveryStream for the same name returns ResourceNotFoundException and ListDeliveryStreams no longer lists it.
