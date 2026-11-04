Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 34e6e58e29d15417fc6ff05c31f4f08593fa6d9a

# Bootstrap the Athena emulator

_Filed: 2026-10-21T21:18:34Z_
_Operation: service-com-amazonaws-athena-amazonathena | Class: missing_

> _Note: this issue was identified against the code base at commit `34e6e58e29d15417fc6ff05c31f4f08593fa6d9a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 70 operations in `services/service-com-amazonaws-athena-amazonathena/src/ops/` is an unmodified scaffold: it carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))` => HTTP 501 `NotImplemented`, holding zero state. There is no working emulator for Athena at all.

Implement a FIRST, SMALL slice: the query-execution lifecycle, which is Athena's primary resource and core workflow. Wire protocol is `awsJson1_1`, `POST /` with `X-Amz-Target: AmazonAthena.<OperationName>` (OPERATIONS.md "Wire protocol").

### Happy path to implement (in order)

1. **StartQueryExecution** — input `QueryString` (required, 1..262144), optional `ClientRequestToken` (@idempotencyToken, 32..128), `QueryExecutionContext` (Database/Catalog), `ResultConfiguration` (OutputLocation), `WorkGroup`, `ExecutionParameters`. Output `QueryExecutionId` (1..128, `^\S+$`). Must MINT a new id, PERSIST a QueryExecution record (query text, workgroup — default `primary` when absent, context, result configuration, submission timestamp, initial status), and return the stored id. Honour `@idempotent`/`ClientRequestToken`: the same token must return the SAME `QueryExecutionId` without creating a second execution (model doc: "If another StartQueryExecution request is received, the same response is returned and another query is not created").
2. **GetQueryExecution** — input `QueryExecutionId` (required). Output `QueryExecution` built FROM the stored record: `QueryExecutionId`, `Query` (byte-for-byte the submitted `QueryString`), `StatementType`, `QueryExecutionContext`, `ResultConfiguration` (including the resolved `OutputLocation`), `WorkGroup`, `Status{State, SubmissionDateTime, CompletionDateTime, StateChangeReason?}`, `Statistics`. An unknown id must fail with the modelled `InvalidRequestException` (HTTP 400), not a 200 with an empty body.
3. **GetQueryResults** — input `QueryExecutionId` (required), `MaxResults` (1..1000), `NextToken` (1..1024). Output `ResultSet{ResultSetMetadata{ColumnInfo...}, Rows}` plus `NextToken` when more rows remain. Rows must come from the result set stored for THAT execution (a deterministic synthesized result set is acceptable — the emulator does not run SQL — but it must be stored at execution time and returned identically on repeat calls). Calling it for an execution whose state is not `SUCCEEDED` must raise `InvalidRequestException` (well-established Athena behaviour: "Query has not yet finished"). Paging must respect `MaxResults` and resume from `NextToken`.
4. **ListQueryExecutions** — input `WorkGroup`, `MaxResults` (0..50), `NextToken`. Output `QueryExecutionIds` + `NextToken`. Must list the ids actually persisted, filtered by workgroup, most-recent-first, honouring `MaxResults`/`NextToken` (no echoing, no unbounded dump).
5. **StopQueryExecution** — input `QueryExecutionId` (required), empty output. Must transition the stored record's `Status.State` to `CANCELLED` and set `CompletionDateTime`, observable via a subsequent GetQueryExecution. Unknown id => `InvalidRequestException`.
6. **BatchGetQueryExecution** — input `QueryExecutionIds` (1..50). Output `QueryExecutions` for ids that exist and `UnprocessedQueryExecutionIds` (with `ErrorCode`/`ErrorMessage`) for ids that do not — the partition must be computed from state.

State transitions: a newly started execution goes `QUEUED` -> `RUNNING` -> `SUCCEEDED` (settling to `SUCCEEDED` promptly is fine, but the stored `Status` must be real and terminal states must stick). A default `primary` workgroup should exist so the path works without CreateWorkGroup.

Every response must be computed from persisted state: no canned bodies, no echoing the input back, no minting an id that is never stored.

## State Model

In `src/state.rs` (`ComAmazonawsAthenaAmazonathenaState`, reached via `crate::state::service_state(&*self.engine)`):

- `query_executions: HashMap<String /*QueryExecutionId*/, QueryExecutionRecord>`
  - `id: String`
  - `query_string: String`
  - `statement_type: StatementType` (DML/DDL/UTILITY, derived from the SQL's leading keyword)
  - `work_group: String` (default `"primary"`)
  - `context: Option<QueryExecutionContext>` (`database`, `catalog`)
  - `result_configuration: Option<ResultConfiguration>` (`output_location`, `encryption_configuration`)
  - `execution_parameters: Vec<String>`
  - `status: QueryStatus { state, state_change_reason, submission_date_time, completion_date_time }`
  - `statistics: QueryStats { engine_execution_time_millis, data_scanned_in_bytes, total_execution_time_in_millis }`
  - `result_set: StoredResultSet { column_info: Vec<ColumnInfo>, rows: Vec<Row> }`
- `query_execution_order: Vec<String>` — insertion order for ListQueryExecutions (iterate reversed for newest-first)
- `idempotency_tokens: HashMap<String /*ClientRequestToken*/, String /*QueryExecutionId*/>`
- `work_groups: HashMap<String, WorkGroupRecord { name, state, description, configuration, creation_time }>` — pre-seeded with `primary`
- `result_page_tokens: HashMap<String /*NextToken*/, (String /*execution id*/, usize /*row offset*/)>` or an opaque offset encoding, so GetQueryResults/ListQueryExecutions paging is resumable

All other operations (notebooks, sessions, calculations, capacity reservations, data catalogs, prepared statements, named queries, tagging) stay 501 for now and will be filed as individual gap issues.

Done = a client can StartQueryExecution, poll GetQueryExecution until `SUCCEEDED`, GetQueryResults and read back the stored rows/columns, see the id in ListQueryExecutions, StopQueryExecution on a second execution and observe `CANCELLED`, and BatchGetQueryExecution partition known vs unknown ids — all with correct bodies, not just 2xx.

## Evidence

All 70 handler files under `services/service-com-amazonaws-athena-amazonathena/src/ops/` are scaffolds; e.g. `src/ops/start_query_execution.rs:4` has `//! SCAFFOLDED, NOT IMPLEMENTED:` and line 193 returns `Err(crate::scaffolded::Scaffolded::not_implemented("StartQueryExecution"))` (HTTP 501), with `let _ = (&input, ctx, &self.engine);` proving no state is touched. Contracts cited from OPERATIONS.md: StartQueryExecution (lines 385-390), GetQueryExecution (201-205), GetQueryResults (207-212, @paginated), ListQueryExecutions (337-342, @paginated, MaxResults 0..50), StopQueryExecution (404-409), BatchGetQueryExecution (39-43), CreateWorkGroup/GetWorkGroup (90-94, 250-254). Shape members for `QueryExecution`, `QueryExecutionStatus`, `ResultSet` and `WorkGroup` taken from the Smithy model via smithy_query. Idempotency semantics quoted from the model's own `ClientRequestToken` doc (reproduced in `start_query_execution.rs:59-68`). "Query has not yet finished" for GetQueryResults on a non-SUCCEEDED execution is well-established Athena behaviour. No tests assert any of this: the crate ships only `tests/state_roundtrip.rs`, a state-API example.

## Steps to Reproduce

1. POST / with X-Amz-Target: AmazonAthena.StartQueryExecution and body {"QueryString":"SELECT 1","ResultConfiguration":{"OutputLocation":"s3://bucket/res/"}} — expect 200 with a QueryExecutionId; currently 501 NotImplemented.
2. Repeat the same call with an identical ClientRequestToken — expect the SAME QueryExecutionId and no second execution.
3. POST AmazonAthena.GetQueryExecution with that id — expect QueryExecution.Query == "SELECT 1", WorkGroup == "primary", ResultConfiguration.OutputLocation == "s3://bucket/res/", Status.State eventually SUCCEEDED with SubmissionDateTime/CompletionDateTime set.
4. POST AmazonAthena.GetQueryResults with that id and MaxResults=1 — expect a ResultSet with ResultSetMetadata.ColumnInfo and one row, plus a NextToken; follow the NextToken and expect the remaining rows, identical on repeat calls.
5. POST AmazonAthena.GetQueryResults for a freshly started (not yet SUCCEEDED) execution — expect InvalidRequestException (HTTP 400).
6. POST AmazonAthena.ListQueryExecutions with {"WorkGroup":"primary","MaxResults":1} — expect the newest id and a NextToken paging to the older ones.
7. Start a second query, POST AmazonAthena.StopQueryExecution for it, then GetQueryExecution — expect Status.State == CANCELLED with CompletionDateTime set.
8. POST AmazonAthena.BatchGetQueryExecution with [<real id>, "does-not-exist"] — expect one entry in QueryExecutions and one in UnprocessedQueryExecutionIds.
9. POST AmazonAthena.GetQueryExecution with an unknown id — expect InvalidRequestException (HTTP 400), not a hollow 200.
