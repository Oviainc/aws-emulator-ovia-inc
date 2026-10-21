Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 7b44f3f32d1f58d662874eb8ca0f546e296cee00

# Bootstrap the Kinesis emulator

_Filed: 2026-10-21T21:09:01Z_
_Operation: service-com-amazonaws-kinesis-kinesis-20131202 | Class: missing_

> _Note: this issue was identified against the code base at commit `7b44f3f32d1f58d662874eb8ca0f546e296cee00`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

All 45 operations in `services/service-com-amazonaws-kinesis-kinesis-20131202/src/ops/` are unmodified scaffolds: each carries `//! SCAFFOLDED, NOT IMPLEMENTED:` and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))` -> HTTP 501, and the service holds zero state. Implement a FIRST minimal slice: the produce/consume loop of the primary resource (a data stream), statefully, over the awsJson1_1 wire protocol (`POST /`, `X-Amz-Target: Kinesis_20131202.<Op>`).

## Happy path to implement (in order)

1. **CreateStream** — in: `StreamName` (required, 1..128, `^[a-zA-Z0-9_.-]+$`), `ShardCount` (>=1, default to 1 when absent), `StreamModeDetails`, `Tags`. Out: no members. Must persist a new stream record: allocate `ShardCount` shards with ids `shardId-%012d` (0-based), split the 128-bit hash key space contiguously across them, set `StreamStatus=ACTIVE` (the emulator may skip CREATING), `StreamCreationTimestamp=now`, `RetentionPeriodHours=24`, and synthesize `StreamARN` = `arn:aws:kinesis:<region>:<account>:stream/<StreamName>` so it matches the modeled pattern `^arn:aws.*:kinesis:.*:\d{12}:stream/\S+$`. A second CreateStream for the same name must fail `ResourceInUseException`.
2. **DescribeStreamSummary** — in: `StreamName` or `StreamARN`. Out: `StreamDescriptionSummary` (required) with `StreamName`, `StreamARN`, `StreamStatus`, `RetentionPeriodHours`, `StreamCreationTimestamp`, `EnhancedMonitoring` (all `@required` in the model), `OpenShardCount` computed from stored open shards. Unknown stream -> `ResourceNotFoundException`.
3. **PutRecord** — in: `Data` (blob, required), `PartitionKey`, `ExplicitHashKey`, `StreamName`/`StreamARN`. Out: `ShardId` + `SequenceNumber` (both required). Must choose the shard by hashing `PartitionKey` (MD5 of the key as a 128-bit integer) into the stored `HashKeyRange`s — or by `ExplicitHashKey` when given — append the record to that shard's log with a monotonically increasing numeric sequence number (matching `^(0|([1-9]\d{0,128}))$`) and `ApproximateArrivalTimestamp=now`, and return the real shard id and sequence number of the stored record. Unknown stream -> `ResourceNotFoundException`. (**PutRecords** may be added in the same slice: it is the same append over a batch, returning one `PutRecordsResultEntry` per input entry plus `FailedRecordCount`.)
4. **GetShardIterator** — in: `ShardId` (required), `ShardIteratorType` (required: TRIM_HORIZON | LATEST | AT_SEQUENCE_NUMBER | AFTER_SEQUENCE_NUMBER | AT_TIMESTAMP), `StartingSequenceNumber`, `Timestamp`, `StreamName`/`StreamARN`. Out: `ShardIterator` (1..512). Must mint an opaque, server-side-resolvable token that encodes (stream, shard, read position): TRIM_HORIZON = oldest stored record, LATEST = end of the shard log, AT/AFTER_SEQUENCE_NUMBER = at/after the given sequence number, AT_TIMESTAMP = first record at or after the timestamp. Unknown stream or shard -> `ResourceNotFoundException`; a `ShardIteratorType` needing a sequence number without one -> `InvalidArgumentException`.
5. **GetRecords** — in: `ShardIterator` (required), `Limit` (1..10000). Out: `Records` (required), `NextShardIterator`, `MillisBehindLatest`. Must resolve the iterator against the stored shard log and return the records actually written by step 3 (`SequenceNumber`, `Data` byte-identical, `PartitionKey`, `ApproximateArrivalTimestamp`), honouring `Limit`, then return a `NextShardIterator` positioned after the last returned record so repeated polling drains and then returns an empty `Records` list with `MillisBehindLatest=0`. An unparseable/unknown iterator -> `ExpiredIteratorException` (or `InvalidArgumentException` for a malformed one). An iterator for a deleted stream -> `ResourceNotFoundException`.
6. **ListStreams** — out: `StreamNames` (required), `HasMoreStreams` (required), optional `StreamSummaries`. Must list the names of streams actually in state, sorted, honouring `Limit`/`ExclusiveStartStreamName` at least to the extent of setting `HasMoreStreams` correctly.
7. **DeleteStream** — in: `StreamName`/`StreamARN`. Out: no members. Must remove the stream and all shard logs/iterators from state; a later DescribeStreamSummary/PutRecord/GetRecords for it must fail `ResourceNotFoundException`, and the name must disappear from ListStreams. Deleting an unknown stream -> `ResourceNotFoundException`.

Every one of these must compute its response from persisted state reached via `crate::state::service_state(&*self.engine)` — no canned bodies, no echoing the input back, no random ids that are never stored. In particular, GetRecords returning an empty list or a fabricated record while PutRecord succeeded is a failure of this issue, and so is a PutRecord `ShardId` that does not name a shard CreateStream actually allocated.

## State Model

In `src/state.rs` (the service's shared, lockable store):

- `streams: BTreeMap<String /* StreamName */, Stream>`
- `Stream { name: String, arn: String, status: StreamStatus /* ACTIVE|CREATING|DELETING|UPDATING */, stream_mode: StreamMode /* PROVISIONED|ON_DEMAND */, retention_period_hours: i32, creation_timestamp: SystemTime, encryption_type: EncryptionType /* NONE */, key_id: Option<String>, enhanced_monitoring: Vec<MetricsName>, tags: BTreeMap<String,String>, shards: Vec<Shard>, next_sequence_number: u128 }`
- `Shard { shard_id: String /* shardId-000000000000 */, parent_shard_id: Option<String>, adjacent_parent_shard_id: Option<String>, hash_key_range: (u128 /* starting */, u128 /* ending */), starting_sequence_number: u128, ending_sequence_number: Option<u128> /* None while OPEN */, records: Vec<StoredRecord> }`
- `StoredRecord { sequence_number: u128, partition_key: String, explicit_hash_key: Option<String>, data: Vec<u8>, arrival: SystemTime }`
- Shard iterators: either `iterators: HashMap<String /* opaque token */, IteratorState { stream_name: String, shard_id: String, next_index: usize, issued_at: SystemTime }>`, or a signed/encoded token carrying the same triple. Sequence numbers are allocated from `Stream::next_sequence_number` so they increase monotonically per stream and are stable across reads.

Derived values (`OpenShardCount`, `HasMoreShards`, `MillisBehindLatest`, `FailedRecordCount`, `HasMoreStreams`) are computed from these structures on each call, never stored as constants.

## Out of scope for this slice

The remaining 38 operations (channels, consumers/SubscribeToShard, resource policies, tags, encryption, SplitShard/MergeShards/UpdateShardCount, enhanced monitoring, account settings, DescribeLimits, DescribeStream's shard pagination) stay 501 and will be filed as individual gap issues later.

## Evidence

1) `services/service-com-amazonaws-kinesis-kinesis-20131202/OPERATIONS.md` lists 45 operations with protocol `awsJson1_1` / target prefix `Kinesis_20131202`, and gives the exact signatures cited above: CreateStream (L39-43: `StreamName` required, `ShardCount` range 1.., empty output), DescribeStreamSummary (L106-110: `StreamDescriptionSummary` required), GetRecords (L124-128: `Records` required, `NextShardIterator`, `MillisBehindLatest`, `ExpiredIteratorException`), GetShardIterator (L136-139: `ShardId` + `ShardIteratorType` required, `ShardIterator` out), PutRecord (L193-197: `Data` required, `ShardId` + `SequenceNumber` required out), ListStreams (L168-173: `StreamNames` + `HasMoreStreams` required), DeleteStream (L63-67). 2) `smithy_query shape=StreamDescriptionSummary,Shard,Record,HashKeyRange,SequenceNumberRange` confirms the required members the state model must be able to produce (`StreamStatus`, `RetentionPeriodHours`, `StreamCreationTimestamp`, `EnhancedMonitoring`, `OpenShardCount`; `Shard.HashKeyRange`/`SequenceNumberRange` required; `Record.SequenceNumber`/`Data` required). 3) `grep '^//! SCAFFOLDED, NOT IMPLEMENTED:' src/ops/` matches all 45 operation files (create_stream.rs:4, put_record.rs:4, get_shard_iterator.rs:4, get_records.rs:4, list_streams.rs:4, delete_stream.rs:4, ...), so nothing is implemented and no test asserts any behavior. 4) Well-established Kinesis semantics: records are placed by MD5 hash of the partition key into a shard's hash key range, sequence numbers increase per shard, and a shard iterator is consumed/advanced via `NextShardIterator`.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI Kinesis client at it.
2. CreateStream(StreamName="demo", ShardCount=1) -> expect 200 and an empty body (currently HTTP 501 NotImplemented).
3. DescribeStreamSummary(StreamName="demo") -> expect StreamDescriptionSummary with StreamName="demo", StreamStatus=ACTIVE, OpenShardCount=1, RetentionPeriodHours=24 and a StreamARN matching ^arn:aws.*:kinesis:.*:\d{12}:stream/demo$.
4. ListStreams() -> expect StreamNames contains "demo" and HasMoreStreams=false.
5. PutRecord(StreamName="demo", PartitionKey="pk-1", Data=b"hello") -> expect ShardId="shardId-000000000000" and a numeric SequenceNumber; repeat with Data=b"world" and expect a strictly greater SequenceNumber.
6. GetShardIterator(StreamName="demo", ShardId="shardId-000000000000", ShardIteratorType="TRIM_HORIZON") -> expect a non-empty ShardIterator token.
7. GetRecords(ShardIterator=<token>) -> expect exactly the two records in write order with Data byte-identical to b"hello"/b"world", PartitionKey="pk-1", the SequenceNumbers returned by PutRecord, and a NextShardIterator.
8. GetRecords(ShardIterator=<NextShardIterator>) -> expect Records=[] and MillisBehindLatest=0 (iterator drained, not replaying).
9. GetShardIterator(... ShardIteratorType="LATEST") then GetRecords -> expect Records=[]; then PutRecord a third record and GetRecords on that iterator chain -> expect only the third record.
10. DeleteStream(StreamName="demo") -> expect 200; then DescribeStreamSummary(StreamName="demo") -> expect ResourceNotFoundException, and ListStreams() no longer contains "demo".
11. DescribeStreamSummary(StreamName="never-made") and GetShardIterator on an unknown ShardId -> expect ResourceNotFoundException, not 501 and not a success.
