Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 9fd9433686ca60725b6b64638320351edab7ca3b

# Bootstrap the Timestream Write emulator

_Filed: 2026-10-21T21:53:19Z_
_Operation: service-com-amazonaws-timestreamwrite-timestream-20181101 | Class: missing_

> _Note: this issue was identified against the code base at commit `9fd9433686ca60725b6b64638320351edab7ca3b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 19 operations in `services/service-com-amazonaws-timestreamwrite-timestream-20181101/src/ops/` is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state, so no client can do anything with this emulator. Implement a FIRST, SMALL slice: the database/table/ingest lifecycle, backed by real persisted state in `ComAmazonawsTimestreamwriteTimestream20181101State` (reached via `crate::state::service_state(&*self.engine)`).

Wire protocol (from README/OPERATIONS.md): `awsJson1_0`, `POST /` with `X-Amz-Target: Timestream_20181101.<OperationName>`.

### Happy path to implement, in order

1. **CreateDatabase** — input `DatabaseName` (required, pattern `^[a-zA-Z0-9_.-]+$`), optional `KmsKeyId`, `Tags`. Insert a new database into state and return `Database` populated from what was stored: `Arn` (`arn:aws:timestream:<region>:<account>:database/<DatabaseName>`), `DatabaseName`, `TableCount` = 0, `KmsKeyId` (default a synthetic key id when the caller omits it), `CreationTime`, `LastUpdatedTime`. A second create of the same name must fail with `ConflictException` (HTTP 409).
2. **CreateTable** — input `DatabaseName` + `TableName` (both required), optional `RetentionProperties`, `MagneticStoreWriteProperties`, `Schema`, `Tags`. Must fail `ResourceNotFoundException` (404) when the database does not exist, and `ConflictException` (409) when the table name already exists in that database. Store the table and return `Table` with `Arn` (`.../database/<db>/table/<table>`), `TableName`, `DatabaseName`, `TableStatus` = `ACTIVE`, the `RetentionProperties` as stored (defaulting to the service defaults 6 hours memory / 73000 days magnetic when absent), `MagneticStoreWriteProperties`, `Schema`, `CreationTime`, `LastUpdatedTime`. Creating a table must bump the owning database's `TableCount`.
3. **WriteRecords** — input `DatabaseName`, `TableName` (required), `Records` (required, 1..100), optional `CommonAttributes`. Resolve the table from state (404 `ResourceNotFoundException` if the database or table is absent), merge `CommonAttributes` into each record, append the resulting records to the table's record buffer, and return `RecordsIngested` computed from the number actually stored: `Total` = records accepted, `MemoryStore` = that same count, `MagneticStore` = 0. Do NOT return a hardcoded count.
4. **DescribeTable** — input `DatabaseName`, `TableName`. Return the `Table` exactly as persisted by CreateTable (same ARN, status, retention properties, timestamps); `ResourceNotFoundException` (404) when absent.
5. **ListTables** — input optional `DatabaseName`, `NextToken`, `MaxResults` (1..20). Return `Tables` read from state (all tables of the named database, or all tables when `DatabaseName` is omitted), honoring `MaxResults` and emitting/consuming an opaque `NextToken` so a paging client terminates. `ResourceNotFoundException` when the named database does not exist.
6. **DeleteTable** / **DeleteDatabase** — remove the stored table (decrementing the database's `TableCount`) / remove the database. Empty output body. `ResourceNotFoundException` (404) on a second delete, and on a subsequent `DescribeTable` / `DescribeDatabase`.

Every response above must be computed from persisted state — no canned bodies, no echoing the input back, and no generating an ARN/TaskId without storing the corresponding object. A create followed by a describe in a later request must return what was created, and a delete must be observable by the next reader.

## State Model

In `src/state.rs`, hold the service state behind the shared engine lock:

- `databases: BTreeMap<String /* DatabaseName */, DatabaseEntry>` — ordered so `ListDatabases`/`ListTables` paginate deterministically.
- `DatabaseEntry { arn: String, database_name: String, kms_key_id: String, creation_time: SystemTime, last_updated_time: SystemTime, tags: Vec<Tag>, tables: BTreeMap<String /* TableName */, TableEntry> }` — `TableCount` in the wire `Database` is derived as `tables.len()`, never stored separately.
- `TableEntry { arn: String, table_name: String, database_name: String, status: TableStatus /* ACTIVE */, retention_properties: RetentionProperties { memory_store_retention_period_in_hours, magnetic_store_retention_period_in_days }, magnetic_store_write_properties: Option<MagneticStoreWriteProperties>, schema: Option<Schema>, creation_time: SystemTime, last_updated_time: SystemTime, tags: Vec<Tag>, records: Vec<StoredRecord> }`.
- `StoredRecord { dimensions: Vec<Dimension>, measure_name: String, measure_value: Option<String>, measure_value_type: Option<MeasureValueType>, measure_values: Vec<MeasureValue>, time: String, time_unit: TimeUnit, version: i64 /* default 1 */, ingested_at: SystemTime }` — appended by WriteRecords and counted into `RecordsIngested`.
- `tags_by_arn` may be derived from the `tags` fields above; an explicit `BTreeMap<String /* ResourceARN */, Vec<Tag>>` is acceptable if the tagging ops are wired later.

Out of scope for this first slice (they stay 501 and will be filed as individual gap issues): `UpdateDatabase`, `UpdateTable`, `ListDatabases` (optional stretch — trivially the same pattern as `ListTables`), `DescribeEndpoints`, `TagResource`, `UntagResource`, `ListTagsForResource`, and the whole batch-load family (`CreateBatchLoadTask`, `DescribeBatchLoadTask`, `ListBatchLoadTasks`, `ResumeBatchLoadTask`).

Done = a realistic integration test drives the 6 operations above over `awsJson1_0` and every response body is correct and stateful, not merely 2xx.

## Evidence

All 19 handler files under `services/service-com-amazonaws-timestreamwrite-timestream-20181101/src/ops/` carry the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker (confirmed by grep: create_database.rs, create_table.rs, write_records.rs, describe_table.rs, list_tables.rs, delete_table.rs, delete_database.rs, plus the other 12), i.e. the whole service answers HTTP 501 and persists nothing. Contracts cited from the crate's `OPERATIONS.md`: CreateDatabase (`DatabaseName` required / output `Database`; `ConflictException` 409), CreateTable (`DatabaseName`+`TableName` required / output `Table`; `ResourceNotFoundException` 404, `ConflictException` 409), WriteRecords (`Records` required len 1..100, `CommonAttributes` / output `RecordsIngested`; `ResourceNotFoundException` 404), DescribeTable, ListTables (`@paginated` NextToken/MaxResults 1..20), DeleteTable, DeleteDatabase. Field lists for `Database` (Arn, DatabaseName, TableCount, KmsKeyId, CreationTime, LastUpdatedTime), `Table` (Arn, TableName, DatabaseName, TableStatus, RetentionProperties, CreationTime, LastUpdatedTime, MagneticStoreWriteProperties, Schema), `RecordsIngested` (Total, MemoryStore, MagneticStore) and `RetentionProperties` (both members @required, ranges 1..8766 / 1..73000) come from the Smithy model shapes. Wire protocol `awsJson1_0` with target prefix `Timestream_20181101` per OPERATIONS.md 'Wire protocol'. No tests exist beyond the generated `tests/state_roundtrip.rs`.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK `timestream-write` client at it (endpoint override; signing name `timestream`).
2. CreateDatabase with DatabaseName=`demo_db` — expect 200 and a `Database` with DatabaseName=`demo_db`, TableCount=0, a `database/demo_db` ARN and a CreationTime. Today: HTTP 501 NotImplemented.
3. CreateDatabase with DatabaseName=`demo_db` again — expect `ConflictException` (HTTP 409).
4. CreateTable with DatabaseName=`demo_db`, TableName=`demo_table`, RetentionProperties{MemoryStoreRetentionPeriodInHours=24, MagneticStoreRetentionPeriodInDays=7} — expect 200, TableStatus=`ACTIVE`, ARN `.../database/demo_db/table/demo_table`.
5. CreateTable with DatabaseName=`missing_db` — expect `ResourceNotFoundException` (HTTP 404).
6. WriteRecords into `demo_db`/`demo_table` with CommonAttributes{Dimensions:[{Name:host,Value:h1}]} and 3 Records (MeasureName=cpu, MeasureValue=`42.0`, MeasureValueType=DOUBLE, Time=<epoch ms>, TimeUnit=MILLISECONDS) — expect 200 with RecordsIngested{Total=3, MemoryStore=3, MagneticStore=0}.
7. DescribeTable `demo_db`/`demo_table` — expect the SAME ARN, TableStatus and RetentionProperties (24 hours / 7 days) that CreateTable returned.
8. ListTables with DatabaseName=`demo_db`, MaxResults=1 — expect 1 table plus a NextToken; follow the token to exhaustion and expect the full set with no NextToken on the last page.
9. DescribeDatabase `demo_db` — expect TableCount=1.
10. DeleteTable `demo_db`/`demo_table` then DescribeTable again — expect `ResourceNotFoundException` (HTTP 404).
11. DeleteDatabase `demo_db` then DescribeDatabase again — expect `ResourceNotFoundException` (HTTP 404).
