Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 01973b2fa0798bae2c2d50e81b185c07c4a43584

# Bootstrap the EBS (EBS direct APIs) emulator

_Filed: 2026-10-21T21:58:15Z_
_Operation: service-com-amazonaws-ebs-ebs | Class: missing_

> _Note: this issue was identified against the code base at commit `01973b2fa0798bae2c2d50e81b185c07c4a43584`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 6 operations in `src/ops/` is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state, so no client can do anything with this service. Implement a FIRST, SMALL stateful slice: the EBS direct-APIs snapshot write-then-read loop, in this order.

1. **StartSnapshot** (`POST /snapshots` -> 201). Input: `VolumeSize` (long, >=1, required), optional `ParentSnapshotId`, `Tags`, `Description`, `ClientToken` (@idempotencyToken), `Encrypted`, `KmsKeyArn`, `Timeout`. Must mint a NEW snapshot id matching the modeled pattern `^snap-[0-9a-f]+$`, persist the snapshot record, and return `SnapshotId`, `Status` = `pending`, `StartTime`, `VolumeSize`, `BlockSize` (524288), `OwnerId`, plus the `Description`/`Tags`/`ParentSnapshotId`/`KmsKeyArn` that were stored. If `ParentSnapshotId` is given it must name an existing completed snapshot (else `ResourceNotFoundException`, 404) and the new snapshot must start out with a COPY of the parent's blocks. Repeating a request with the same `ClientToken` must return the same `SnapshotId` rather than creating a second snapshot.
2. **PutSnapshotBlock** (`PUT /snapshots/{SnapshotId}/blocks/{BlockIndex}` -> 201). Input: `BlockData` (@httpPayload, required), `DataLength` (required header `x-amz-Data-Length`), `Checksum` + `ChecksumAlgorithm` (`SHA256`, both required headers), optional `Progress`. Must store the block bytes under (snapshot id, block index) in the snapshot's block map, reject an unknown or already-completed `SnapshotId` with `ResourceNotFoundException`/`ValidationException`, and echo back the stored `Checksum`/`ChecksumAlgorithm` headers computed over what was persisted — not blindly reflected from the request.
3. **CompleteSnapshot** (`POST /snapshots/completion/{SnapshotId}` -> 202). Input: `ChangedBlocksCount` (required header), optional `Checksum`/`ChecksumAlgorithm`/`ChecksumAggregationMethod` (`LINEAR`). Must look up the pending snapshot, verify the number of blocks actually written matches `ChangedBlocksCount` (mismatch -> `ValidationException`, 400), flip the stored status to `completed`, and return `Status` read from that stored record. Unknown id -> `ResourceNotFoundException` (404).
4. **ListSnapshotBlocks** (`GET /snapshots/{SnapshotId}/blocks` -> 200). Must enumerate the blocks actually persisted for a COMPLETED snapshot, sorted ascending by `BlockIndex`, each item a `Block{BlockIndex, BlockToken}` where the `BlockToken` is a token this emulator can later resolve back to that block. Must honour `StartingBlockIndex` (skip lower indexes), `MaxResults` (page size) and `NextToken` (opaque continuation matching `^[A-Za-z0-9+/=]+$`), returning `NextToken` only when more blocks remain. Also return `VolumeSize`, `BlockSize` and `ExpiryTime` from the stored snapshot. Listing a `pending` snapshot id that does not exist -> `ResourceNotFoundException`.
5. **GetSnapshotBlock** (`GET /snapshots/{SnapshotId}/blocks/{BlockIndex}` -> 200). Input: `BlockToken` (required @httpQuery `blockToken`). Must resolve the token, fetch the persisted bytes for that (snapshot, index) and return them as the `BlockData` @httpPayload with `DataLength` and `Checksum`/`ChecksumAlgorithm` headers derived from those bytes. Bytes returned MUST be byte-identical to what PutSnapshotBlock stored. Unknown snapshot/index -> `ResourceNotFoundException` (404); missing/garbage `BlockToken` -> `ValidationException` (400).

`ListChangedBlocks` stays stubbed (501) for this first slice and will be filed as its own gap issue later.

NEVER STUBS: each of these five must compute its response from the persisted store reached via `crate::state::service_state(&*self.engine)` (see README.md "State"). No canned ids, no echoed checksums, no random token without storage. A StartSnapshot followed by PutSnapshotBlock, CompleteSnapshot, ListSnapshotBlocks and GetSnapshotBlock must observe each other's writes across requests.

## State Model

In `src/state.rs` on `ComAmazonawsEbsEbsState`:

- `snapshots: HashMap<String /* snap-xxxx */, Snapshot>`
- `Snapshot { snapshot_id: String, owner_id: String, status: Status /* pending | completed | error */, volume_size: i64, block_size: i32 /* 524288 */, start_time: SystemTime, expiry_time: SystemTime, description: Option<String>, tags: Vec<Tag{key, value}>, parent_snapshot_id: Option<String>, kms_key_arn: Option<String>, encrypted: bool, sse_type: SSEType, timeout_minutes: Option<i32>, blocks: BTreeMap<i32 /* BlockIndex */, BlockEntry> }`
- `BlockEntry { data: Vec<u8>, data_length: i32, checksum: String /* base64 SHA256 of data */, checksum_algorithm: ChecksumAlgorithm, block_token: String, progress: Option<i32> }`
- `client_tokens: HashMap<String /* ClientToken */, String /* SnapshotId */>` for StartSnapshot idempotency
- `block_tokens: HashMap<String /* BlockToken */, (String /* SnapshotId */, i32 /* BlockIndex */)>` so GetSnapshotBlock resolves a token handed out by ListSnapshotBlocks
- `list_page_tokens: HashMap<String /* NextToken */, (String /* SnapshotId */, i32 /* next BlockIndex */)>` for ListSnapshotBlocks pagination
- a counter/RNG for minting ids of the form `snap-` + lowercase hex

Done = an integration test that starts a snapshot, writes two blocks at different indexes, completes it with `ChangedBlocksCount=2`, lists exactly those two block indexes in ascending order with usable block tokens, gets each block back with byte-identical payload and matching `DataLength`/`Checksum`, and gets 404 `ResourceNotFoundException` for a `snap-deadbeef` that was never started.

## Evidence

`services/service-com-amazonaws-ebs-ebs/OPERATIONS.md` lists all 6 modeled operations with their routes, inputs, outputs and errors: StartSnapshot (`POST /snapshots` -> 201, outputs SnapshotId/Status/StartTime/VolumeSize/BlockSize, ClientToken is @idempotencyToken), PutSnapshotBlock (`PUT /snapshots/{SnapshotId}/blocks/{BlockIndex}` -> 201, BlockData @httpPayload, required DataLength/Checksum/ChecksumAlgorithm headers), CompleteSnapshot (`POST /snapshots/completion/{SnapshotId}` -> 202, required ChangedBlocksCount header, outputs Status one of completed|error|pending), ListSnapshotBlocks (`GET /snapshots/{SnapshotId}/blocks`, @paginated inputToken=NextToken/outputToken=NextToken, MaxResults 100..10000, StartingBlockIndex, outputs Blocks/VolumeSize/BlockSize/ExpiryTime/NextToken), GetSnapshotBlock (`GET /snapshots/{SnapshotId}/blocks/{BlockIndex}`, required BlockToken @httpQuery, outputs BlockData @httpPayload + DataLength/Checksum headers). All six declare `ResourceNotFoundException (HTTP 404)` and `ValidationException (HTTP 400)`. Protocol is `restJson1`, sigv4 name `ebs`.

All six handler files under `services/service-com-amazonaws-ebs-ebs/src/ops/` (start_snapshot.rs, put_snapshot_block.rs, complete_snapshot.rs, list_snapshot_blocks.rs, get_snapshot_block.rs, list_changed_blocks.rs) still carry the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker at line 4 and return `Err(crate::scaffolded::Scaffolded::not_implemented(..))` -> HTTP 501, so the service holds zero state. Well-established EBS direct-API semantics ground the ordering: a snapshot is created `pending`, blocks are written into it, CompleteSnapshot seals it, and only then are blocks listable/readable via block tokens.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI `ebs` client at it.
2. `aws ebs start-snapshot --volume-size 1` -> expect 201 with a fresh `SnapshotId` matching `^snap-[0-9a-f]+$`, `Status=pending`, `BlockSize=524288`; today returns HTTP 501 NotImplemented.
3. `aws ebs put-snapshot-block --snapshot-id <id> --block-index 0 --data-length 524288 --checksum <base64 sha256> --checksum-algorithm SHA256 --block-data block0.bin` and again for `--block-index 1` -> expect 201 each and the bytes persisted.
4. `aws ebs complete-snapshot --snapshot-id <id> --changed-blocks-count 2` -> expect 202 with `Status=completed`; a count that disagrees with the blocks written must give 400 ValidationException.
5. `aws ebs list-snapshot-blocks --snapshot-id <id>` -> expect exactly block indexes 0 and 1 in ascending order, each with a BlockToken, plus VolumeSize/BlockSize/ExpiryTime; re-run with `--max-results 100 --starting-block-index 1` to check bounds and NextToken handling.
6. `aws ebs get-snapshot-block --snapshot-id <id> --block-index 0 --block-token <token from list> out.bin` -> expect 200 and `out.bin` byte-identical to `block0.bin`, with matching DataLength and Checksum headers.
7. `aws ebs list-snapshot-blocks --snapshot-id snap-deadbeef` -> expect 404 ResourceNotFoundException, not 501.
