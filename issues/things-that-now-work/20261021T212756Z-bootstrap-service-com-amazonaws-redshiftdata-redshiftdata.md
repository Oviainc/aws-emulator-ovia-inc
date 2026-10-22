Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 57742dc737335ed8f07f6b76d23b07c0a9c73918

# Bootstrap the Redshift Data API emulator

_Filed: 2026-10-21T21:27:56Z_
_Operation: service-com-amazonaws-redshiftdata-redshiftdata | Class: missing_

> _Note: this issue was identified against the code base at commit `57742dc737335ed8f07f6b76d23b07c0a9c73918`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 12 operations in this crate is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state, so no client workflow works at all. Implement a FIRST, SMALL stateful slice: the core statement lifecycle of the Redshift Data API.

Happy path to implement, in order (wire protocol `awsJson1_1`, `POST /` with `X-Amz-Target: RedshiftData.<Operation>`):

1. **ExecuteStatement** — input `Sql` (required), plus one connection target (`ClusterIdentifier` + `DbUser`/`SecretArn`, or `WorkgroupName`) and `Database`; optional `StatementName`, `Parameters` (list<SqlParameter{name,value}>), `WithEvent`, `ClientToken`, `ResultFormat` (CSV|JSON), `SessionId`/`SessionKeepAliveSeconds`. Must mint a new `Id` matching the modeled UUID pattern `^[a-z0-9]{8}(-[a-z0-9]{4}){3}-[a-z0-9]{12}(:\d{0,2})?$`, persist a statement record, and return `Id`, `CreatedAt`, `Database`, `DbUser`/`SecretArn`, `ClusterIdentifier`/`WorkgroupName`, `Status` (SUBMITTED, advancing to FINISHED), `HasResultSet`.
2. **DescribeStatement** — input `Id` (required). Must look the statement up in state and return the stored `Id`, `QueryString`, `Status`, `CreatedAt`, `UpdatedAt`, `Duration`, `RedshiftPid`, `HasResultSet`, `ResultRows`, `ResultSize`, `RedshiftQueryId`, `Database`, `DbUser`/`SecretArn`, `ClusterIdentifier`/`WorkgroupName`, `QueryParameters`, `ResultFormat`, `SessionId`, `ExecutionMode`, and `Error` when the statement failed. Unknown `Id` → `ResourceNotFoundException` (HTTP 404).
3. **GetStatementResult** — input `Id` (required), `NextToken`. Must return the rows actually produced by the statement identified by `Id`: `Records` (list of list<Field> unions: `stringValue`/`longValue`/`doubleValue`/`booleanValue`/`isNull`/`blobValue`), `ColumnMetadata` (name/label/typeName/nullable/length/...), `TotalNumRows`, and `NextToken` only when more rows remain. Statements with no result set (e.g. DDL/DML) must behave consistently with the `HasResultSet` flag recorded at execution; unknown `Id` → `ResourceNotFoundException`.
4. **ListStatements** — input `MaxResults` (0..100), `NextToken`, and filters `StatementName`, `Status` (ABORTED|ALL|FAILED|FINISHED|PICKED|STARTED|SUBMITTED), `RoleLevel`, `Database`, `ClusterIdentifier`, `WorkgroupName`. Must enumerate persisted statements newest-first, honour the filters, cap the page at `MaxResults` and return an opaque `NextToken` that resumes exactly where the previous page stopped. `Statements` entries are `StatementData{Id, QueryString, QueryStrings, SecretArn, Status, StatementName, CreatedAt, UpdatedAt, QueryParameters, IsBatchStatement, ResultFormat, SessionId}`.
5. **CancelStatement** — input `Id` (required). Must transition a still-running (SUBMITTED/PICKED/STARTED) persisted statement to ABORTED, return `Status: true`, and make that new status observable to a following DescribeStatement. Unknown `Id` → `ResourceNotFoundException` (404); a statement already FINISHED must not be reported as cancelled.

Every one of these MUST compute its response from persisted state. No canned bodies, no echoing the request back, no minting an `Id` that is never stored. A statement created by ExecuteStatement must be visible to DescribeStatement, GetStatementResult, ListStatements and CancelStatement in the same process, and a cancellation must be visible to the next read.

SQL execution itself can start minimal: a tiny in-memory evaluator is enough — recognise `SELECT`-shaped statements that produce rows (including simple literal projections such as `SELECT 1`), and treat CREATE/INSERT/UPDATE/DELETE as zero-row statements with `HasResultSet=false` while updating `ResultRows`. What matters for this slice is that the result returned by GetStatementResult is derived from the statement that was actually submitted, and that `ColumnMetadata` matches the shape of `Records`.

Out of scope for this first slice (stay 501, filed separately later): BatchExecuteStatement, GetStatementResultV2, ListDatabases, ListSchemas, ListTables, DescribeTable, ListSessions.

## State Model

- `RedshiftDataState` held in the service's shared state (behind the usual lock):
  - `statements: IndexMap<String /* Id */, Statement>` — insertion-ordered so ListStatements can page newest-first deterministically.
  - `client_tokens: HashMap<String /* ClientToken */, String /* Id */>` — for idempotent ExecuteStatement replays.
- `Statement { id: String /* UUID pattern */, sql: String, statement_name: Option<String>, database: Option<String>, db_user: Option<String>, secret_arn: Option<String>, cluster_identifier: Option<String>, workgroup_name: Option<String>, status: StatusString /* SUBMITTED|PICKED|STARTED|FINISHED|FAILED|ABORTED */, error: Option<String>, created_at: SystemTime, updated_at: SystemTime, duration_nanos: i64, redshift_pid: i64, redshift_query_id: i64, has_result_set: bool, result_rows: i64, result_size: i64, with_event: bool, result_format: ResultFormat /* CSV|JSON, default JSON */, parameters: Vec<SqlParameter{name, value}>, session_id: Option<String>, execution_mode: ExecutionMode, is_batch: bool, sub_statements: Vec<SubStatement>, result: Option<ResultSet> }`
- `ResultSet { column_metadata: Vec<ColumnMetadata{name, label, type_name, nullable, length, precision, scale, is_signed, is_case_sensitive, is_currency, schema_name, table_name, column_default}>, rows: Vec<Vec<Field>> }` where `Field` is the modeled union (`isNull|booleanValue|longValue|doubleValue|stringValue|blobValue`).
- Pagination cursor for ListStatements/GetStatementResult: an opaque token encoding the offset into `statements` / `ResultSet.rows`, validated and rejected with `ValidationException` when malformed.

Done = a client script can run ExecuteStatement → DescribeStatement (sees its own SQL and a terminal status) → GetStatementResult (gets the rows for that statement) → ListStatements (finds the statement, respects MaxResults/NextToken) → CancelStatement (observable status change), with every response assembled from the stored `Statement`.

## Evidence

All 12 handler files under `services/service-com-amazonaws-redshiftdata-redshiftdata/src/ops/` carry the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and return `Err(crate::scaffolded::Scaffolded::not_implemented(..))` → HTTP 501 with zero stored state (confirmed by grep over the whole ops directory: batch_execute_statement, cancel_statement, describe_statement, describe_table, execute_statement, get_statement_result, get_statement_result_v2, list_databases, list_schemas, list_sessions, list_statements, list_tables).

Field names, requiredness, enum value sets, constraints and errors above are taken from the crate's generated `OPERATIONS.md` (ExecuteStatement, DescribeStatement, GetStatementResult, ListStatements, CancelStatement sections: `Sql` required; `Id` required and UUID-patterned; `MaxResults` range 0..100; `Status` enums; `ResourceNotFoundException` HTTP 404 and `ValidationException` HTTP 400 declared on each), plus `smithy_query` for the `Field` union, `ColumnMetadata`, `StatementData` and `SqlParameter` member lists. Wire protocol `awsJson1_1` with `X-Amz-Target: RedshiftData.<Operation>` and sigv4 name `redshift-data` per OPERATIONS.md. The asynchronous submit-then-poll-then-fetch-results lifecycle (ExecuteStatement returns an Id; DescribeStatement polls Status; GetStatementResult retrieves rows once FINISHED; CancelStatement aborts a running statement) is the documented, well-established Redshift Data API usage pattern.

## Steps to Reproduce

1. POST / with X-Amz-Target: RedshiftData.ExecuteStatement and body {"ClusterIdentifier":"my-cluster","Database":"dev","DbUser":"awsuser","Sql":"SELECT 1","StatementName":"boot"} — expect 200 with a UUID-shaped Id, CreatedAt and a Status; today: HTTP 501 NotImplemented
2. POST / with X-Amz-Target: RedshiftData.DescribeStatement and {"Id":"<Id from step 1>"} — expect 200 echoing the PERSISTED statement (QueryString "SELECT 1", Database "dev", terminal Status FINISHED, HasResultSet true, CreatedAt/UpdatedAt); today: 501
3. POST / with X-Amz-Target: RedshiftData.GetStatementResult and {"Id":"<Id>"} — expect 200 with Records=[[{"longValue":1}]], matching ColumnMetadata and TotalNumRows=1; today: 501
4. POST / with X-Amz-Target: RedshiftData.ListStatements and {"MaxResults":1} — expect the statement from step 1 in Statements with its Id/StatementName/Status, and a NextToken only when more statements exist; today: 501
5. POST / with X-Amz-Target: RedshiftData.ExecuteStatement for a long-running statement, then X-Amz-Target: RedshiftData.CancelStatement with {"Id":"<Id>"} — expect 200 {"Status":true} and a following DescribeStatement reporting Status ABORTED; today: 501
6. POST / with X-Amz-Target: RedshiftData.DescribeStatement and {"Id":"00000000-0000-0000-0000-000000000000"} — expect HTTP 404 ResourceNotFoundException; today: 501
