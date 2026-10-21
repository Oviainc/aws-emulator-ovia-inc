Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 01973b2fa0798bae2c2d50e81b185c07c4a43584

# Bootstrap the DynamoDB Streams emulator

_Filed: 2026-10-21T21:58:07Z_
_Operation: service-com-amazonaws-dynamodbstreams-dynamodbstreams-20120810 | Class: missing_

> _Note: this issue was identified against the code base at commit `01973b2fa0798bae2c2d50e81b185c07c4a43584`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The whole crate is a fresh scaffold: all four handlers in `services/service-com-amazonaws-dynamodbstreams-dynamodbstreams-20120810/src/ops/` (`list_streams.rs`, `describe_stream.rs`, `get_shard_iterator.rs`, `get_records.rs`) carry the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and return `Err(crate::scaffolded::Scaffolded::not_implemented(..))` (HTTP 501), and `src/state.rs` holds `// TODO: add service-specific state fields`. No client can consume a stream.

DynamoDB Streams has no create/delete API of its own (streams are created by DynamoDB `CreateTable`/`UpdateTable` with a `StreamSpecification`), so the first slice is the stream *consumer* loop — the exact loop the KCL/Lambda poller and every tutorial uses. Implement these 4 operations, in this order, all computed from persisted state:

1. **ListStreams** — input `TableName` (optional filter, len 3..255, pattern `^[a-zA-Z0-9_.-]+$`), `Limit` (>=1), `ExclusiveStartStreamArn`. Output `Streams: [Stream{StreamArn, TableName, StreamLabel}]` plus `LastEvaluatedStreamArn` when the page was truncated by `Limit`. Must enumerate the streams actually registered in state, filtered by `TableName` when given, and return an empty `Streams` list (not an error) when there are none.
2. **DescribeStream** — input `StreamArn` (required, len 37..1024), `Limit`, `ExclusiveStartShardId`. Output `StreamDescription{StreamArn, StreamLabel, StreamStatus (ENABLED), StreamViewType (as registered: KEYS_ONLY|NEW_IMAGE|OLD_IMAGE|NEW_AND_OLD_IMAGES), CreationRequestDateTime, TableName, KeySchema, Shards: [Shard{ShardId, ParentShardId, SequenceNumberRange{StartingSequenceNumber, EndingSequenceNumber}}], LastEvaluatedShardId}`. An unknown/unregistered `StreamArn` must return `ResourceNotFoundException` (HTTP 400), not a success with an empty description. `ShardId` must be 28..65 chars and `EndingSequenceNumber` must be absent for an open (still-writable) shard.
3. **GetShardIterator** — input `StreamArn` (required), `ShardId` (required), `ShardIteratorType` (required: TRIM_HORIZON | LATEST | AT_SEQUENCE_NUMBER | AFTER_SEQUENCE_NUMBER | AT_TIMESTAMP), `SequenceNumber`, `Timestamp`. Output `ShardIterator` (len 1..2048) — an opaque token that the service RESOLVES later, i.e. it must be recorded in state against (stream, shard, cursor position, issued-at) rather than being a formatted echo of the input. Unknown stream or shard => `ResourceNotFoundException`. TRIM_HORIZON must position at the oldest retained record, LATEST just past the newest existing record.
4. **GetRecords** — input `ShardIterator` (required), `Limit` (>=1, cap 1000). Output `Records: [Record{eventID, eventName (INSERT|MODIFY|REMOVE), eventVersion, eventSource "aws:dynamodb", awsRegion, dynamodb: StreamRecord{ApproximateCreationDateTime, Keys, NewImage, OldImage, SequenceNumber, SizeBytes, StreamViewType}}]` and `NextShardIterator`. Must read records from the position the iterator token names, advance that position, honour `Limit`, return `Records: []` with a still-usable `NextShardIterator` when caught up, and return `ExpiredIteratorException` for an unknown/retired token. `NewImage`/`OldImage` must be populated per the stream's `StreamViewType` (KEYS_ONLY => neither image).

Every response must be derived from persisted state — no canned bodies, no echoed input, no randomly minted ARN/ShardId/ShardIterator that is never stored. Specifically: a `GetShardIterator` followed by two `GetRecords` calls must not re-deliver the same records, and `DescribeStream` must report the same `StreamArn`/`TableName`/`StreamViewType` that `ListStreams` returned.

A seam is needed to get records INTO the store, since this service only reads. Minimum: an internal append entry point on the state (`append_record(stream_arn, Record)`) plus registration of a stream for a table, so the DynamoDB service's `CreateTable`(StreamSpecification)/`PutItem`/`UpdateItem`/`DeleteItem` handlers can publish into it later. Full cross-service propagation from DynamoDB is NOT required for this first slice as long as the state API exists and the four operations read it correctly; wiring it is a follow-up.

## State Model

In `ComAmazonawsDynamodbstreamsDynamodbstreams20120810State`:

- `streams: BTreeMap<String /* StreamArn */, Stream>` — insertion/ARN-ordered so `ListStreams` paging is stable.
- `Stream { stream_arn: String, table_name: String, stream_label: String /* ISO8601 creation timestamp */, stream_status: StreamStatus /* ENABLED */, stream_view_type: StreamViewType, creation_request_date_time: SystemTime, key_schema: Vec<KeySchemaElement{attribute_name, key_type}>, shards: Vec<Shard> }`
- `Shard { shard_id: String /* e.g. "shardId-00000001700000000000-abcdef12", 28..65 chars */, parent_shard_id: Option<String>, starting_sequence_number: String, ending_sequence_number: Option<String> /* None while open */, records: Vec<StoredRecord> }` — records appended in arrival order; sequence numbers monotonically increasing, 21..40 digits.
- `StoredRecord { event_id: String, event_name: OperationType, event_version: String /* "1.1" */, event_source: String /* "aws:dynamodb" */, aws_region: String, sequence_number: String, approximate_creation_date_time: SystemTime, keys: AttributeMap, new_image: Option<AttributeMap>, old_image: Option<AttributeMap>, size_bytes: i64 }`
- `iterators: HashMap<String /* opaque ShardIterator token */, IteratorState { stream_arn: String, shard_id: String, next_index: usize /* or next sequence number */, issued_at: SystemTime }>` — `GetRecords` looks the token up here; absent token => `ExpiredIteratorException`. Each `GetRecords` mints the next token (or retires it and returns `NextShardIterator: null` for a closed shard that has been drained).
- `next_sequence_number: u64` — monotonic counter backing sequence-number generation.

Operations outside this slice: there are none — these four are the entire modeled surface, so this bootstrap is the service.

## Evidence

Spec: `OPERATIONS.md` for this crate lists exactly 4 operations with these inputs/outputs/errors — DescribeStream(`StreamArn` required len 37..1024, `Limit`, `ExclusiveStartShardId`, `ShardFilter` -> `StreamDescription`; errors InternalServerError 500, ResourceNotFoundException 400), GetRecords(`ShardIterator` required len 1..2048, `Limit` -> `Records`, `NextShardIterator`; errors ExpiredIteratorException/LimitExceededException/ResourceNotFoundException/TrimmedDataAccessException 400), GetShardIterator(`StreamArn`, `ShardId`, `ShardIteratorType` one of AFTER_SEQUENCE_NUMBER|AT_SEQUENCE_NUMBER|AT_TIMESTAMP|LATEST|TRIM_HORIZON all required, `SequenceNumber`, `Timestamp` -> `ShardIterator`), ListStreams(`TableName`, `Limit`, `ExclusiveStartStreamArn` -> `Streams`, `LastEvaluatedStreamArn`). Protocol is `awsJson1_0`, `POST /` with `X-Amz-Target: DynamoDBStreams_20120810.<Op>` (README.md + OPERATIONS.md 'Wire protocol').

Shape members quoted above come from the Smithy model via smithy_query on `StreamDescription`, `Stream`, `Shard`, `SequenceNumberRange`, `Record`, `StreamRecord`.

Implementation: `src/ops/get_records.rs:4` carries `//! SCAFFOLDED, NOT IMPLEMENTED:` and line 190 returns `Err(crate::scaffolded::Scaffolded::not_implemented("GetRecords"))`; the same holds for `describe_stream.rs`, `get_shard_iterator.rs`, `list_streams.rs`. `src/state.rs:8` is still `// TODO: add service-specific state fields`, so no stream, shard, record or iterator can be persisted. No tests exist beyond the generated `tests/state_roundtrip.rs` example.

Well-known semantics grounding the behavioural requirements: a shard iterator is opaque and expires 15 minutes after issue (ExpiredIteratorException, per GetRecords doc in the model), GetRecords returns at most 1000 records / 1 MB and returns an empty list rather than an error when caught up (GetRecords doc), and `eventSource` is `aws:dynamodb` (Record.eventSource doc).

## Steps to Reproduce

1. Start the emulator with this service mounted.
2. Register a stream-enabled table in state (via the internal append/registration seam, or via DynamoDB CreateTable with StreamSpecification once wired) and write 3 items so 3 INSERT records exist.
3. POST / with X-Amz-Target: DynamoDBStreams_20120810.ListStreams and body {"TableName":"<table>"} — expect 200 and one Stream entry whose StreamArn/TableName match the registered table (today: 501).
4. POST DescribeStream with that StreamArn — expect 200 with StreamDescription.StreamStatus=ENABLED, the table's TableName/KeySchema/StreamViewType, and at least one Shard carrying a SequenceNumberRange with no EndingSequenceNumber.
5. POST GetShardIterator with {StreamArn, ShardId from step 4, ShardIteratorType: "TRIM_HORIZON"} — expect 200 with a non-empty opaque ShardIterator.
6. POST GetRecords with that ShardIterator and Limit 2 — expect 200, exactly 2 Records in insertion order with eventName INSERT, eventSource "aws:dynamodb", dynamodb.Keys populated, strictly increasing SequenceNumbers, plus a NextShardIterator.
7. POST GetRecords again with the returned NextShardIterator — expect only the 3rd record (no re-delivery of the first two), then an empty Records list with a usable NextShardIterator on a further call.
8. POST DescribeStream with a StreamArn that was never registered — expect HTTP 400 ResourceNotFoundException, not a 200.
9. POST GetRecords with a fabricated ShardIterator string — expect HTTP 400 ExpiredIteratorException, not a 200 with empty Records.
