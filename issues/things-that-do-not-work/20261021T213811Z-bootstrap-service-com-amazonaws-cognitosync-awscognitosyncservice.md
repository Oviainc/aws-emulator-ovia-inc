Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: 09acb2538adf9fea9bce04c8d04105f42be236df

# Bootstrap the Cognito Sync emulator

_Filed: 2026-10-21T21:38:11Z_
_Operation: service-com-amazonaws-cognitosync-awscognitosyncservice | Class: stub_

> _Note: this issue was identified against the code base at commit `09acb2538adf9fea9bce04c8d04105f42be236df`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 17 operations in `services/service-com-amazonaws-cognitosync-awscognitosyncservice/src/ops/` is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state, so no client can do anything with this emulator. Implement a FIRST, SMALL slice: the dataset sync loop of the primary resource (a Dataset of Records, keyed by identity pool + identity).

### Happy path to implement (in order)

1. **ListRecords** — `GET /identitypools/{IdentityPoolId}/identities/{IdentityId}/datasets/{DatasetName}/records`. On a dataset that was never written, return `DatasetExists=false`, `Count=0`, `Records=[]`, `DatasetSyncCount=0` and a freshly minted `SyncSessionToken` that the client then hands to UpdateRecords. Honor `LastSyncCount` (return only records with `SyncCount > LastSyncCount`) and `MaxResults`/`NextToken` at least to the extent of bounding and continuing the list.
2. **UpdateRecords** — `POST /identitypools/{IdentityPoolId}/identities/{IdentityId}/datasets/{DatasetName}`. Inputs: `RecordPatches` (list of `RecordPatch{Op: replace|remove, Key (required), Value, SyncCount (required), DeviceLastModifiedDate}`), `SyncSessionToken` (required), optional `DeviceId`, `ClientContext` header. Creates the dataset implicitly if absent (real Cognito Sync auto-creates datasets). For `Op=replace`, store/overwrite the record; for `Op=remove`, delete it. Bump the dataset's `DatasetSyncCount` once per successful call and set each written record's `SyncCount` to the new dataset sync count, plus `LastModifiedDate` (server now) and `LastModifiedBy` (device id or identity id). If a patch's `SyncCount` is behind the stored record's `SyncCount`, fail the whole call with `ResourceConflictException` (HTTP 409) and persist nothing — the optimistic-concurrency rule is the core semantic of this API. Output: `Records` — the records just written, with their new `SyncCount`s, read back out of state.
3. **ListRecords** (again) — must now return `DatasetExists=true`, the persisted records with the exact `Key`/`Value` written, `Count` equal to the number returned, `DatasetSyncCount` equal to the bumped value, `LastModifiedBy`, and a new `SyncSessionToken`.
4. **DescribeDataset** — `GET .../datasets/{DatasetName}`. Return the `Dataset{IdentityId, DatasetName, CreationDate, LastModifiedDate, LastModifiedBy, DataStorage, NumRecords}` computed from stored records: `NumRecords` = record count, `DataStorage` = summed byte length of keys+values. `ResourceNotFoundException` (404) for a dataset that was never written.
5. **ListDatasets** — `GET .../identities/{IdentityId}/datasets`. Return every dataset stored for that (pool, identity) as the same `Dataset` structures, with `Count` set to the number returned, and `NextToken`/`MaxResults` paging that actually bounds the page and resumes from the token.
6. **DeleteDataset** — `DELETE .../datasets/{DatasetName}`. Remove the dataset and its records from state, return the deleted `Dataset` in the `Dataset` output member, and `ResourceNotFoundException` (404) on a second delete. A following DescribeDataset must 404 and ListDatasets must no longer list it.

Every one of these must compute its response from persisted state held in the shared engine (reached via `crate::state::service_state(&*self.engine)` per the crate README). No canned bodies, no echoing the input back, no minting ids/tokens that are never stored. A write followed by a read must return exactly what was written.

## State Model

In `src/state.rs`:

- `datasets: HashMap<(IdentityPoolId, IdentityId, DatasetName), DatasetEntry>`
- `DatasetEntry { creation_date: Timestamp, last_modified_date: Timestamp, last_modified_by: Option<String>, sync_count: i64, records: BTreeMap<String /*Key*/, RecordEntry> }` — `BTreeMap` so list order is deterministic across calls.
- `RecordEntry { value: Option<String>, sync_count: i64, last_modified_date: Timestamp, last_modified_by: Option<String>, device_last_modified_date: Option<Timestamp> }`
- `sync_sessions: HashMap<String /*SyncSessionToken*/, (IdentityPoolId, IdentityId, DatasetName)>` — tokens minted by ListRecords and validated by UpdateRecords.
- Derived, never stored twice: `Dataset.NumRecords` = `records.len()`, `Dataset.DataStorage` = sum of `key.len() + value.len()`, `ListRecords.Count` = number of records in the page, `DatasetSyncCount` = `DatasetEntry.sync_count`.

The remaining 11 operations (BulkPublish, GetBulkPublishDetails, Get/SetCognitoEvents, Get/SetIdentityPoolConfiguration, DescribeIdentityPoolUsage, DescribeIdentityUsage, ListIdentityPoolUsage, RegisterDevice, SubscribeToDataset, UnsubscribeFromDataset) stay scaffolded at 501 for now and will be filed as individual gap issues.

**Done when:** a client script can run the workflow in Steps to reproduce end to end, with record keys/values, `SyncCount`s, `NumRecords`, `Count` and the 404-after-delete all correct — not merely 2xx.

## Evidence

`grep -l '^//! SCAFFOLDED, NOT IMPLEMENTED:' services/service-com-amazonaws-cognitosync-awscognitosyncservice/src/ops/` matches all 17 operation files (update_records.rs, list_records.rs, describe_dataset.rs, list_datasets.rs, delete_dataset.rs, ...), each returning `Err(crate::scaffolded::Scaffolded::not_implemented(..))` = HTTP 501 with zero persisted state. Contracts are from the crate's OPERATIONS.md (restJson1, sigv4 name `cognito-sync`): UpdateRecords route/inputs (`RecordPatches`, required `SyncSessionToken`, `ResourceConflictException` 409) at OPERATIONS.md §UpdateRecords; ListRecords outputs (`Records`, `Count`, `DatasetSyncCount`, `DatasetExists`, `SyncSessionToken`, `LastSyncCount`/`MaxResults`/`NextToken` query params) at §ListRecords; DescribeDataset/ListDatasets/DeleteDataset `Dataset` output and `ResourceNotFoundException` 404 at §DescribeDataset, §ListDatasets, §DeleteDataset. Field lists for `Dataset{IdentityId, DatasetName, CreationDate, LastModifiedDate, LastModifiedBy, DataStorage, NumRecords}`, `Record{Key, Value, SyncCount, LastModifiedDate, LastModifiedBy, DeviceLastModifiedDate}` and `RecordPatch{Op, Key, Value, SyncCount, DeviceLastModifiedDate}` come from smithy_query on the model; the Dataset doc string states datasets "are automatically created if they don't exist", which grounds implicit creation in UpdateRecords.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI cognito-sync client at it.
2. ListRecords(IdentityPoolId='us-east-1:00000000-0000-0000-0000-000000000001', IdentityId='us-east-1:00000000-0000-0000-0000-0000000000a1', DatasetName='mygame') -> expect 200 with DatasetExists=false, Count=0, Records=[], and a SyncSessionToken (today: HTTP 501).
3. UpdateRecords(same pool/identity/dataset, SyncSessionToken=<token from step 2>, DeviceId='device-1', RecordPatches=[{Op:'replace', Key:'highScore', Value:'42', SyncCount:0}]) -> expect 200 and Records containing Key='highScore', Value='42', SyncCount=1.
4. ListRecords again -> expect DatasetExists=true, Count=1, DatasetSyncCount=1, the record with Value='42', and a new SyncSessionToken.
5. UpdateRecords with a stale patch ({Op:'replace', Key:'highScore', Value:'7', SyncCount:0}) -> expect ResourceConflictException (HTTP 409) and ListRecords still showing Value='42'.
6. DescribeDataset(...,'mygame') -> expect Dataset with DatasetName='mygame', NumRecords=1, non-zero DataStorage, LastModifiedBy='device-1'.
7. ListDatasets(pool, identity) -> expect Count=1 and 'mygame' present; repeat with MaxResults=1 to confirm paging/NextToken are honored.
8. DeleteDataset(...,'mygame') -> expect 200 with the deleted Dataset echoed from state; then DescribeDataset -> ResourceNotFoundException (404) and ListDatasets -> Count=0.
