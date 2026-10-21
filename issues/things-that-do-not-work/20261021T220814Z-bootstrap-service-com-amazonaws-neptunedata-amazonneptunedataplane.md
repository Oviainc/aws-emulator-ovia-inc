Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: 95c91931c960f7f391e4448ce7f3ebc67d41fa17

# Bootstrap the neptunedata (Amazon Neptune data plane) emulator

_Filed: 2026-10-21T22:08:14Z_
_Operation: service-com-amazonaws-neptunedata-amazonneptunedataplane | Class: stub_

> _Note: this issue was identified against the code base at commit `95c91931c960f7f391e4448ce7f3ebc67d41fa17`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 43 operations in this crate is an unmodified scaffold: each `src/ops/*.rs` carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))`, i.e. HTTP 501, while holding zero state. No client workflow works at all.

This issue defines the FIRST, SMALL slice to implement: the Gremlin query lifecycle, which is the primary data-plane workflow of neptunedata (wire protocol `restJson1`, sigv4 signing name `neptune-db`). Implement these 5 operations, in this order, each computing its response from persisted state:

1. **GetEngineStatus** — `GET /status`, no input. Return the stored engine record: `status` ("healthy"), `startTime`, `dbEngineVersion`, `role` ("writer"), `dfeQueryEngine`, `gremlin`/`sparql`/`opencypher` (`QueryLanguageVersion`), `labMode`, `rollingBackTrxCount`, `settings`. The values must come from the engine record created when the state is initialised (fixed per-process `startTime`, stable across calls), not re-fabricated per request.
2. **ExecuteGremlinQuery** — `POST /gremlin`, input `gremlinQuery` (required), `serializer` (header `accept`). Allocate a `queryId`, insert a query record into the store (query string, accepted timestamp, eval stats), and return `requestId` (= the allocated id), `status` (`GremlinQueryStatusAttributes{message:"200 OK", code:200, attributes}`), `result` and `meta` documents. A missing/empty `gremlinQuery` must produce `MissingParameterException` (400), not a 200.
3. **ListGremlinQueries** — `GET /gremlin/status`, input `includeWaiting`. Return `acceptedQueryCount`, `runningQueryCount` and `queries` (list of `GremlinQueryStatus{queryId, queryString, queryEvalStats}`) derived by walking the stored query records — a query executed in step 2 must appear here with its real query string, and counts must match the list contents. `includeWaiting=false` must exclude records still in the waiting/accepted-but-not-running state.
4. **GetGremlinQueryStatus** — `GET /gremlin/status/{queryId}`. Look the record up by id and return `queryId`, `queryString`, `queryEvalStats` (`waited`, `elapsed`, `cancelled`, `subqueries`). An unknown `queryId` must return a modeled error (e.g. `InvalidParameterException`/`BadRequestException`, 400), never an empty 200.
5. **CancelGremlinQuery** — `DELETE /gremlin/status/{queryId}`. Mutate the stored record to cancelled and return `status`. After this, GetGremlinQueryStatus for that id must report `queryEvalStats.cancelled = true` and ListGremlinQueries must no longer count it as running. The op is `@idempotent`: a second cancel of the same id must not fail differently.

Hard requirements:
- No canned, echoed or hardcoded responses. `requestId`/`queryId` must be allocated AND stored; every read must be served by looking the record up in the shared store reached via `crate::state::service_state(&*self.engine)` (never a freshly constructed engine).
- State must survive across requests: execute → list → get → cancel → get must observe one evolving record.
- Required-member and label validation must come before any state mutation.

Everything else (SPARQL/openCypher execution, bulk loader `StartLoaderJob`/`GetLoaderJobStatus`/`ListLoaderJobs`/`CancelLoaderJob`, the ML job and endpoint families, graph summaries, streams, statistics management, `ExecuteFastReset`) stays at 501 for now and will be filed as individual gap issues later. Keep this first slice small.

## State Model

In `src/state.rs` (`ComAmazonawsNeptunedataAmazonneptunedataplaneState`):

- `engine_status: EngineStatusRecord` — `status: String` ("healthy"), `start_time: String` (RFC-1123/ISO, set once at state init), `db_engine_version: String`, `role: String`, `dfe_query_engine: String`, `gremlin_version: String`, `sparql_version: String`, `opencypher_version: String`, `lab_mode: BTreeMap<String,String>`, `rolling_back_trx_count: i32`, `settings: BTreeMap<String,String>`.
- `gremlin_queries: BTreeMap<String /*queryId*/, GremlinQueryRecord>` where `GremlinQueryRecord { query_id: String, query_string: String, serializer: Option<String>, accepted_at: SystemTime, state: QueryState /* Waiting | Running | Done | Cancelled */, waited_ms: i32, elapsed_ms: i32, cancelled: bool, subqueries: u32 }`.
- `next_query_seq: u64` — monotonic counter used to mint `queryId`/`requestId` values (UUID-shaped strings are fine, but they MUST be inserted into `gremlin_queries`).
- (reserved for the next slice, not required here) `open_cypher_queries`, `loader_jobs`, `ml_jobs` maps keyed the same way.

Done = an integration test that runs GetEngineStatus → ExecuteGremlinQuery → ListGremlinQueries → GetGremlinQueryStatus → CancelGremlinQuery → GetGremlinQueryStatus and asserts the stored query string, the query id continuity, the counts, and the flip of `cancelled` to true — with correct bodies, not merely 2xx.

## Evidence

OPERATIONS.md (crate root, rendered from smithy-model/com-amazonaws-neptunedata-amazonneptunedataplane.json): `ExecuteGremlinQuery` POST /gremlin, input `gremlinQuery` (required) + `serializer` (@httpHeader accept), output `requestId`/`status`(GremlinQueryStatusAttributes)/`result`/`meta`; `ListGremlinQueries` GET /gremlin/status, input `includeWaiting`, output `acceptedQueryCount`/`runningQueryCount`/`queries` (list<GremlinQueryStatus>); `GetGremlinQueryStatus` GET /gremlin/status/{queryId} (@readonly), output `queryId`/`queryString`/`queryEvalStats`; `CancelGremlinQuery` DELETE /gremlin/status/{queryId} (@idempotent), output `status`; `GetEngineStatus` GET /status (@readonly) with the full status member list. Shapes via smithy_query: GremlinQueryStatusAttributes{message, code, attributes}, GremlinQueryStatus{queryId, queryString, queryEvalStats}, QueryEvalStats{waited, elapsed, cancelled, subqueries}. All-stub census: `grep -rn 'SCAFFOLDED, NOT IMPLEMENTED' services/service-com-amazonaws-neptunedata-amazonneptunedataplane/src/ops` matches every operation file, including execute_gremlin_query.rs, list_gremlin_queries.rs, get_gremlin_query_status.rs, cancel_gremlin_query.rs and (via the status route) the engine-status stub. No tests assert any of this behaviour (only the generated tests/state_roundtrip.rs exists).

## Steps to Reproduce

1. Start the emulator and point an AWS SDK neptunedata client at it (sigv4 signing name `neptune-db`).
2. Call GetEngineStatus (GET /status) — expect 200 with status "healthy" and a stable startTime; actual: HTTP 501 NotImplemented.
3. Call ExecuteGremlinQuery with gremlinQuery = "g.V().limit(1)" — expect 200 with a non-empty requestId and status.code 200; actual: 501.
4. Call ListGremlinQueries — expect the query from the previous step present in `queries` with its exact queryString and counts matching the list length; actual: 501.
5. Call GetGremlinQueryStatus with that queryId — expect 200 echoing the stored queryString plus queryEvalStats; actual: 501.
6. Call CancelGremlinQuery with that queryId — expect 200 with a status string; then call GetGremlinQueryStatus again and expect queryEvalStats.cancelled = true; actual: 501 for both.
7. Call GetGremlinQueryStatus with a made-up queryId — expect a modeled 400 error, not an empty 200.
