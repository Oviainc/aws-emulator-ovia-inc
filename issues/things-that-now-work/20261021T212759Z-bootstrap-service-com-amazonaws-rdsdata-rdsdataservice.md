Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 57742dc737335ed8f07f6b76d23b07c0a9c73918

# Bootstrap the RDS Data Service (rds-data) emulator

_Filed: 2026-10-21T21:27:59Z_
_Operation: service-com-amazonaws-rdsdata-rdsdataservice | Class: missing_

> _Note: this issue was identified against the code base at commit `57742dc737335ed8f07f6b76d23b07c0a9c73918`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 6 operations in `src/ops/` is an unmodified scaffold: each carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))`, i.e. HTTP 501, and the service holds zero state. Nothing works end to end.

Implement a FIRST SLICE of 5 operations (restJson1, sigv4 name `rds-data`) that make a realistic Data API client script work against a small in-memory SQL engine. The remaining operation (`ExecuteSql`, deprecated) stays 501 and will be filed separately.

### 1. ExecuteStatement — `POST /Execute`
Input: `resourceArn` (required), `secretArn` (required), `sql` (required), optional `database`, `schema`, `parameters` (list<SqlParameter>{name, value: Field union, typeHint}), `transactionId`, `includeResultMetadata` (default false), `continueAfterTimeout` (default false), `resultSetOptions`, `formatRecordsAs` (JSON|NONE).
Output: `records` (list<list<Field>>), `columnMetadata` (list<ColumnMetadata>), `numberOfRecordsUpdated` (long, default 0), `generatedFields` (list<Field>), `formattedRecords` (string).
Must actually interpret the `sql` against persisted state for the minimal set a bootstrap client uses: `CREATE TABLE`, `INSERT`, `SELECT`, `UPDATE`, `DELETE`, `DROP TABLE`. Named parameters (`:name`) must be bound from `parameters` by `name`, and the `Field` union member used on the way in determines the stored value type and the member used on the way out (`stringValue`/`longValue`/`doubleValue`/`booleanValue`/`isNull`/`blobValue`). DML must report the real affected-row count in `numberOfRecordsUpdated` (0 for DDL and SELECT); SELECT must return the rows actually stored, in insertion order, and must NOT set `numberOfRecordsUpdated`. `columnMetadata` (at minimum `name`, `label`, `typeName`, `nullable`) is returned ONLY when `includeResultMetadata` is true. When `formatRecordsAs=JSON`, `records` and `columnMetadata` must be empty and `formattedRecords` must carry the result set as a JSON array of objects. With no `transactionId` the statement auto-commits; with one, it runs inside that transaction and its writes must not be visible to a non-transactional reader until commit.

### 2. BeginTransaction — `POST /BeginTransaction`
Input: `resourceArn`, `secretArn` (required), optional `database`, `schema`. Output: `transactionId` — a freshly generated opaque id (<=192 chars) that is STORED along with the resourceArn/secretArn/database/schema it was opened for, not merely returned.

### 3. CommitTransaction — `POST /CommitTransaction`
Input: `resourceArn`, `secretArn`, `transactionId` (all required). Output: `transactionStatus` (e.g. `"Transaction Committed"`). Must apply the transaction's buffered writes to the committed database, remove the transaction, and make the writes visible to a subsequent non-transactional `ExecuteStatement` SELECT. An unknown or already-finished `transactionId` must fail with `TransactionNotFoundException` (HTTP 404), not 200 and not 501.

### 4. RollbackTransaction — `POST /RollbackTransaction`
Same input. Output: `transactionStatus` (e.g. `"Transaction Rolled Back"`). Must DISCARD the transaction's buffered writes — a later SELECT must show the pre-transaction rows — and remove the transaction. Unknown id => `TransactionNotFoundException` (404).

### 5. BatchExecuteStatement — `POST /BatchExecute`
Input: `resourceArn`, `secretArn`, `sql` (required), optional `database`, `schema`, `parameterSets` (list<list<SqlParameter>>), `transactionId`. Output: `updateResults` — EXACTLY one `UpdateResult` per entry in `parameterSets`, in order, each produced by re-running `sql` with that parameter set against persisted state (so N parameter sets of an INSERT leave N rows that a following SELECT returns). A `transactionId` that is not open must fail with `TransactionNotFoundException` (404).

All five must compute their responses from the persisted store reached via `crate::state::service_state(&*self.engine)` — no canned, echoed, or hardcoded bodies, and no generated transaction id that is not stored. A 200 that persists nothing is worse than the current honest 501.

## State Model

In `src/state.rs`, on `ComAmazonawsRdsdataRdsdataserviceState`:

- `clusters: HashMap<String /* resourceArn */, Cluster>`
- `Cluster { databases: HashMap<String /* database name, "" = default */, Database>, transactions: HashMap<String /* transactionId */, Transaction> }`
- `Database { tables: HashMap<String /* lowercased table name */, Table> }`
- `Table { name: String, columns: Vec<Column>, rows: Vec<Vec<Value>>, next_auto_increment: i64 }`
- `Column { name: String, type_name: String /* e.g. INT, VARCHAR, BOOLEAN, DOUBLE */, nullable: bool, auto_increment: bool }`
- `Value` enum mirroring the `Field` union: `Null | Bool(bool) | Long(i64) | Double(f64) | String(String) | Blob(Vec<u8>)` — with conversion both ways so what goes in through `parameters` comes back out through `records` in the same union member.
- `Transaction { id: String, resource_arn: String, secret_arn: String, database: Option<String>, schema: Option<String>, created_at: SystemTime, pending: Database /* copy-on-write snapshot applied on commit, dropped on rollback */ }`

Done when a client can run: BeginTransaction -> ExecuteStatement(CREATE TABLE, txn) -> BatchExecuteStatement(INSERT with 3 parameterSets, txn) -> CommitTransaction -> ExecuteStatement(SELECT with includeResultMetadata=true, no txn) returning the 3 rows with correct column metadata -> BeginTransaction -> ExecuteStatement(DELETE, txn) -> RollbackTransaction -> SELECT still returning 3 rows.

## Evidence

`services/service-com-amazonaws-rdsdata-rdsdataservice/OPERATIONS.md` lists all 6 operations with routes, input/output members and errors (restJson1, sdkId `RDS Data`, sigv4 `rds-data`): BatchExecuteStatement `POST /BatchExecute`, BeginTransaction `POST /BeginTransaction`, CommitTransaction `POST /CommitTransaction`, ExecuteSql `POST /ExecuteSql` (@deprecated), ExecuteStatement `POST /Execute`, RollbackTransaction `POST /RollbackTransaction`. `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:'` matches all 6 files in `src/ops/`; `src/ops/execute_statement.rs:246` is `Err(crate::scaffolded::Scaffolded::not_implemented("ExecuteStatement"))`. Field/SqlParameter/ColumnMetadata/UpdateResult member lists taken from the Smithy model (`Field` is a union of isNull/booleanValue/longValue/doubleValue/stringValue/blobValue/arrayValue; `UpdateResult` carries `generatedFields`). ExecuteStatement's model doc states that a call without `transactionId` auto-commits, that `formattedRecords` is only present when `formatRecordsAs=JSON` and that `columnMetadata`/`records` are blank in that case; `TransactionNotFoundException` (HTTP 404, "The transaction ID wasn't found") is modeled on ExecuteStatement, BatchExecuteStatement, CommitTransaction and RollbackTransaction. No tests exist beyond the generated `tests/state_roundtrip.rs`.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK rds-data client at it with resourceArn=arn:aws:rds:us-east-1:000000000000:cluster:demo and secretArn=arn:aws:secretsmanager:us-east-1:000000000000:secret:demo
2. BeginTransaction -> expect 200 with a non-empty transactionId (today: HTTP 501 NotImplemented)
3. ExecuteStatement sql='CREATE TABLE pets (id INT, name VARCHAR(32))' with that transactionId -> expect 200, numberOfRecordsUpdated=0
4. BatchExecuteStatement sql='INSERT INTO pets (id, name) VALUES (:id, :name)' with 3 parameterSets and that transactionId -> expect 200 with updateResults of length 3
5. CommitTransaction with that transactionId -> expect 200 with transactionStatus set
6. ExecuteStatement sql='SELECT id, name FROM pets' with includeResultMetadata=true and NO transactionId -> expect the 3 committed rows in records (longValue/stringValue) plus columnMetadata for id and name
7. BeginTransaction again, ExecuteStatement sql='DELETE FROM pets' inside it (expect numberOfRecordsUpdated=3), then RollbackTransaction -> re-run the SELECT and expect the 3 rows still present
8. CommitTransaction with transactionId='does-not-exist' -> expect HTTP 404 TransactionNotFoundException
