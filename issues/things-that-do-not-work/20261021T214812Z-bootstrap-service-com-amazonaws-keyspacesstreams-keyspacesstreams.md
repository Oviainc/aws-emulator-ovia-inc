Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: a2986bf0fd3cc35fe32946cf5d2b8a785e7f69a4

# Bootstrap the KeyspacesStreams emulator

_Filed: 2026-10-21T21:48:12Z_
_Operation: service-com-amazonaws-keyspacesstreams-keyspacesstreams | Class: stub_

> _Note: this issue was identified against the code base at commit `a2986bf0fd3cc35fe32946cf5d2b8a785e7f69a4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 4 operations this crate binds is an unmodified scaffold answering HTTP 501 `NotImplemented`, so no client can do anything with the service. Implement the full consumer read loop of the service's primary resource (a change-data-capture stream for a Keyspaces table), statefully, in this order:

1. **ListStreams** — input `keyspaceName`, `tableName`, `maxResults`, `nextToken`; output `streams` (list of `Stream{streamArn, keyspaceName, tableName, streamLabel}`) + `nextToken`. Must enumerate the streams actually held in state, filtered by `keyspaceName`/`tableName` when supplied, truncated to `maxResults` with a real opaque `nextToken` that resumes where the previous page stopped (model marks it `@paginated(inputToken=nextToken, outputToken=nextToken, items=streams)`).
2. **GetStream** — input `streamArn` (required), `maxResults`, `shardFilter`, `nextToken`; output the stored stream's `streamArn`, `streamLabel`, `streamStatus` (ENABLED/…), `streamViewType` (KEYS_ONLY | NEW_AND_OLD_IMAGES | NEW_IMAGE | OLD_IMAGE), `creationRequestDateTime`, `keyspaceName`, `tableName`, `shards` (list of `Shard{shardId, sequenceNumberRange{startingSequenceNumber, endingSequenceNumber}, parentShardIds}`) + `nextToken`. Values must come from the stored stream record, not be echoed from the request or hardcoded. An ARN with no stream in state → `ResourceNotFoundException` (404).
3. **GetShardIterator** — input `streamArn` (required), `shardId` (required), `shardIteratorType` (required: TRIM_HORIZON | LATEST | AT_SEQUENCE_NUMBER | AFTER_SEQUENCE_NUMBER), `sequenceNumber`; output `shardIterator`. Must mint an iterator token that encodes (streamArn, shardId, resolved position) and register it in state. Unknown stream/shard → `ResourceNotFoundException`; `AT_SEQUENCE_NUMBER`/`AFTER_SEQUENCE_NUMBER` without `sequenceNumber`, or a `sequenceNumber` outside the shard's range → `ValidationException` (400).
4. **GetRecords** — input `shardIterator` (required), `maxResults`; output `changeRecords` (list of `Record{eventVersion, createdAt, origin, partitionKeys, clusteringKeys, newImage, oldImage, sequenceNumber}`), `nextShardIterator`, `iteratorDescription{iteratorPosition}`. Must resolve the iterator to its stored position, return at most `maxResults` records from that shard **in sequence-number order starting at that position**, and return a *new* `nextShardIterator` advanced past the records returned so a repeat poll does not re-deliver them. When the cursor is at the end of the shard's records, return an empty `changeRecords` with `iteratorPosition = AT_TIP` and a still-valid `nextShardIterator`; when records remain, `BEHIND_TIP`. An unknown/expired iterator → `ResourceNotFoundException`.

None of these may be canned, echoed or random-without-storage: each response field must be computed from persisted state, and a second `GetRecords` with the returned `nextShardIterator` must observably continue (not restart) the read. Because this service has no `CreateStream` API (streams are created by Keyspaces/CDC-enabled tables), the bootstrap also needs a seeding path so state can be populated for tests — a stream + shard + record seed on the service state (e.g. created when a Keyspaces table with CDC is created, or a test-only seeding helper on the state struct). Pagination tokens must be opaque strings within the modeled length bounds (80..3000) and must be rejected with `ValidationException` when not issued by this emulator.

## State Model

In `src/state.rs` (`ComAmazonawsKeyspacesstreamsKeyspacesstreamsState`), reachable via `crate::state::service_state(&*self.engine)`:

- `streams: BTreeMap<String /* streamArn */, StreamEntry>`
  - `StreamEntry { stream_arn: String, stream_label: String, keyspace_name: String, table_name: String, stream_status: StreamStatus, stream_view_type: StreamViewType, creation_request_date_time: SystemTime, shards: Vec<ShardEntry> }`
- `ShardEntry { shard_id: String /* 28..65 chars */, parent_shard_ids: Vec<String>, starting_sequence_number: String, ending_sequence_number: Option<String> /* None = open shard */, records: Vec<RecordEntry> /* kept sorted by sequence_number */ }`
- `RecordEntry { sequence_number: String /* 21..48 chars, monotonic within the shard */, event_version: String, created_at: SystemTime, origin: OriginType, partition_keys: BTreeMap<String, KeyspacesCell>, clustering_keys: BTreeMap<String, KeyspacesCell>, new_image: Option<KeyspacesRow>, old_image: Option<KeyspacesRow> }`
- `iterators: HashMap<String /* opaque shardIterator token */, IteratorEntry>`
  - `IteratorEntry { stream_arn: String, shard_id: String, next_index: usize /* cursor into ShardEntry.records */, created_at: SystemTime }`
- `page_tokens: HashMap<String /* opaque nextToken */, PageCursor { kind: Streams | Shards, stream_arn: Option<String>, keyspace_name: Option<String>, table_name: Option<String>, offset: usize }>`
- a monotonic counter used to mint sequence numbers, shard iterators and page tokens so each is unique and non-guessable.

Anything outside this loop (e.g. shardFilter types beyond the basic child/parent selection) may stay 501 and will be filed as individual gap issues later.

## Evidence

`services/service-com-amazonaws-keyspacesstreams-keyspacesstreams/OPERATIONS.md` lists exactly 4 bound operations (GetRecords, GetShardIterator, GetStream, ListStreams), their required/optional input members, output members and the 5 modeled errors (AccessDeniedException 403, InternalServerException 500, ResourceNotFoundException 404, ThrottlingException 429, ValidationException 400), plus the `@paginated(nextToken/shards)` and `@paginated(nextToken/streams)` traits on GetStream and ListStreams. Shape detail for `Stream`, `Shard`, `SequenceNumberRange`, `Record`, `IteratorDescription` and `ShardFilter` came from `smithy_query` against `smithy-model/com-amazonaws-keyspacesstreams-keyspacesstreams.json` (IteratorDescription docs define AT_TIP = no more records now, BEHIND_TIP = more may be available). `grep -n 'SCAFFOLDED, NOT IMPLEMENTED' src/ops/` matches all four handler files — `get_records.rs`, `get_shard_iterator.rs`, `get_stream.rs`, `list_streams.rs` — each still returning `Err(crate::scaffolded::Scaffolded::not_implemented(..))` (HTTP 501). Protocol is `awsJson1_0` via `POST /` with `X-Amz-Target: KeyspacesStreams.<Operation>` (README/OPERATIONS.md).

## Steps to Reproduce

1. Seed state with one CDC stream for keyspace `mykeyspace`, table `mytable`, holding one open shard with 3 change records.
2. POST / with X-Amz-Target: KeyspacesStreams.ListStreams and body {"keyspaceName":"mykeyspace"} — expect 200 with `streams[0].streamArn`/`keyspaceName`/`tableName`/`streamLabel` for the seeded stream (currently HTTP 501).
3. POST / with X-Amz-Target: KeyspacesStreams.GetStream and body {"streamArn":"<arn from step 2>"} — expect 200 with the stored streamLabel, streamStatus, streamViewType, creationRequestDateTime, keyspaceName, tableName and a `shards` list containing the seeded shardId and its sequenceNumberRange.
4. POST / with X-Amz-Target: KeyspacesStreams.GetShardIterator and body {"streamArn":"<arn>","shardId":"<shardId from step 3>","shardIteratorType":"TRIM_HORIZON"} — expect 200 with a non-empty `shardIterator`.
5. POST / with X-Amz-Target: KeyspacesStreams.GetRecords and body {"shardIterator":"<iterator>","maxResults":2} — expect 200 with the first 2 seeded records in sequence order, `iteratorDescription.iteratorPosition = BEHIND_TIP`, and a `nextShardIterator` different from the input iterator.
6. Repeat GetRecords with the returned `nextShardIterator` — expect only the 3rd record (no re-delivery of the first two), then a further poll returning an empty `changeRecords` with `iteratorPosition = AT_TIP`.
7. Call GetStream with an unknown streamArn and GetRecords with a fabricated shardIterator — expect `ResourceNotFoundException` (HTTP 404), and GetShardIterator with shardIteratorType `AT_SEQUENCE_NUMBER` and no `sequenceNumber` — expect `ValidationException` (HTTP 400).
