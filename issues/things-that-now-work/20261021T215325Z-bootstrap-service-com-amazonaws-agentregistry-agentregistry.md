Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: 9fd9433686ca60725b6b64638320351edab7ca3b

# Bootstrap the Agent Registry emulator

_Filed: 2026-10-21T21:53:25Z_
_Operation: service-com-amazonaws-agentregistry-agentregistry | Class: stub_

> _Note: this issue was identified against the code base at commit `9fd9433686ca60725b6b64638320351edab7ca3b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The whole crate is scaffold: all three bound operations (`src/ops/list_discoverable_registry_records.rs`, `src/ops/batch_get_discoverable_registry_record.rs`, `src/ops/search_discoverable_registry_records.rs`) carry the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and answer HTTP 501 `NotImplemented`, and `src/state.rs` holds no registry data. Implement a first stateful slice covering the service's entire discovery read path.

This service models NO mutating operation (the registry/record authoring API lives outside this model), so the happy path is a *read* loop over a seeded store: the store must be populated once (seed fixture / test helper on the state struct), and every response must be computed from that store — never canned, never echoed from the request.

### Happy path (implement in this order, restJson1, sigv4 signing name `agent-registry`)

1. **ListDiscoverableRegistryRecords** — `POST /registries/{registryId}/discoverable-records-list`. Input: `registryId` (@httpLabel, required; accepts bare 12-16 char id OR full `arn:aws:agent-registry:<region>:<acct>:registry/<id>`), `maxResults`, `nextToken`, `filters` (max 10 `RegistryRecordFilter{name: recordType|descriptorType, values[1..10]}`). Output: `registryRecords` (list<DiscoverableRegistryRecordSummary>, required) + `nextToken`. Must resolve `registryId` in the store (404 `ResourceNotFoundException` when absent), return ONLY records whose `status == APPROVED` (per the model doc: `REJECTED`/`DEPRECATED`/`DRAFT`/`PENDING_APPROVAL` records are not discoverable), apply `filters` conjunctively, cap the page at `maxResults`, and emit an opaque `nextToken` that resumes exactly where the previous page stopped (no duplicate/skipped records, no token echo). Summaries must NOT include `descriptors` but must include `descriptorTypes` (the descriptor entry keys of the record).
2. **BatchGetDiscoverableRegistryRecord** — `POST /discoverable-records-batch`. Input: `entries` (list<`RegistryRecordsEntry{registryId, recordIds[1..100]}`>, required). Output: `registryRecords` (list<RegistryRecordSummary>, required) AND `errors` (list<`BatchGetDiscoverableRegistryRecordError{registryId, recordId, errorCode, message}`>, required). Partial success is the core behavior: each requested (registryId, recordId) pair that resolves to an APPROVED record yields a full `RegistryRecordSummary` (including `descriptors`, `recordVersion`, `customMetadata`); each pair that does not resolve yields an entry in `errors` with `errorCode = RESOURCE_NOT_FOUND` and the `recordId` echoed back in the SAME format the caller supplied (bare id or ARN). A missing record must NOT fail the whole call. Both output lists are required — serialize them even when empty.
3. **SearchDiscoverableRegistryRecords** — `POST /discoverable-records-search`. Input: `searchQuery` (len 0..256, required), `registryIds` (required), `maxResults` (default 10, required), `filters` (document). Output: `registryRecords` (list<RegistryRecordSummary>, required). Must search across the registries named in `registryIds` (404 for an unknown registry), match `searchQuery` case-insensitively against stored `name`, `displayName` and `description`, return only APPROVED records, and truncate to `maxResults` (defaulting to 10 when omitted). An empty `searchQuery` matches all discoverable records in the given registries. Results must be the stored summaries, not fabricated ones — a record returned here must be byte-identical to the one `BatchGetDiscoverableRegistryRecord` returns for its `recordId`.

### Cross-operation invariants
- A record visible in `ListDiscoverableRegistryRecords` must be retrievable by its `recordId` via `BatchGetDiscoverableRegistryRecord` and findable by its `name` via `SearchDiscoverableRegistryRecords`.
- `registryArn` / `recordArn` must be derived consistently from the stored registry id, record id, region and account (`arn:aws:agent-registry:<region>:<acct>:registry/<registryId>` and `.../registry/<registryId>/record/<recordId>`), satisfying the model's length/pattern constraints.
- `createdAt` / `updatedAt` serialize as RFC3339 `date-time`.
- Input violating a modeled constraint (`registryId` pattern, `searchQuery` > 256 chars, `recordIds` empty or > 100, `filters` > 10) returns 400 `ValidationException`, not a 500 or a silent success.

### State Model
In `src/state.rs`:
- `AgentRegistryState { registries: BTreeMap<String /* registryId */, Registry>, account_id: String, region: String }`
- `Registry { registry_id: String, registry_arn: String, records: BTreeMap<String /* recordId, 12 chars */, RegistryRecord> }`
- `RegistryRecord { record_id, record_arn, registry_arn, name, display_name: Option<String>, description: Option<String>, record_type: RecordType, record_version: String, status: RegistryRecordStatus, descriptors: Descriptors /* key -> descriptor */, custom_metadata: Option<Document>, created_at: DateTime, updated_at: DateTime }`
- `descriptorTypes` for the list summary is derived from `descriptors`' keys — stored once, never hardcoded per response.
- Pagination: `nextToken` encodes the last returned `recordId` (or an index into the stable `BTreeMap` iteration order) so pages are stable and resumable; ordering is the deterministic `BTreeMap` order.
- Seeding: a constructor/helper (used by tests and any fixture loader) inserts registries and records so reads have something real to return; all three handlers reach this store through `crate::state::service_state(&*self.engine)` and must share it across requests.

Done = an integration test that seeds one registry with several records (mixed `APPROVED` / `DEPRECATED`, mixed `recordType`) and then drives List (with paging and a `recordType` filter) -> BatchGet (one hit + one missing record id) -> Search (by name substring) passes with correct bodies, not merely 2xx.

## Evidence

OPERATIONS.md for `service-com-amazonaws-agentregistry-agentregistry` lists exactly 3 operations with their routes, required input/output members and errors: BatchGetDiscoverableRegistryRecord (`POST /discoverable-records-batch`, out `registryRecords` + `errors` both required), ListDiscoverableRegistryRecords (`POST /registries/{registryId}/discoverable-records-list`, @paginated(inputToken=nextToken, outputToken=nextToken, items=registryRecords), @readonly), SearchDiscoverableRegistryRecords (`POST /discoverable-records-search`, `maxResults` default 10). `grep -n 'SCAFFOLDED, NOT IMPLEMENTED' src/ops/` matches all three handler files, i.e. the service answers 501 for every call and holds zero state. Smithy model shapes confirm the field sets and semantics cited above: `DiscoverableRegistryRecordSummary` (no `descriptors`, has `descriptorTypes`), `RegistryRecordSummary` (has `descriptors`, `recordVersion`, `customMetadata`), `RegistryRecordsEntry{registryId, recordIds 1..100}`, `RegistryRecordFilter{name in [recordType, descriptorType], values 1..10}`, `BatchGetDiscoverableRegistryRecordError{registryId, recordId, errorCode in [RESOURCE_NOT_FOUND, ACCESS_DENIED, INTERNAL_ERROR], message}` with the doc stating the recordId is "echoed from the request in the same format that you supplied (ARN or record ID)", and `RegistryRecordStatus` whose doc states REJECTED/DEPRECATED records are not discoverable and APPROVED records are. No tests exist beyond the generated `tests/state_roundtrip.rs`.

## Steps to Reproduce

1. Start the emulator with the Agent Registry service mounted and seed one registry (id `abc123def456`) holding three records: `rec000000001` (name `payments-mcp`, recordType MCP, status APPROVED, descriptors keyed `mcp`), `rec000000002` (name `support-agent`, recordType AGENT, status APPROVED), `rec000000003` (name `old-agent`, recordType AGENT, status DEPRECATED).
2. POST /registries/abc123def456/discoverable-records-list with `{"maxResults":1}` — expect 200 with exactly 1 `registryRecords` entry carrying registryArn/recordArn/recordId/name/recordType/recordVersion/status=APPROVED/createdAt/updatedAt/descriptorTypes, plus a non-null `nextToken`. Today: HTTP 501 NotImplemented.
3. Repeat the call with that `nextToken` — expect the SECOND approved record (no repeat of page 1) and no `nextToken` on the final page; the DEPRECATED record `rec000000003` must never appear.
4. POST /registries/abc123def456/discoverable-records-list with `{"filters":[{"name":"recordType","values":["MCP"]}]}` — expect only `payments-mcp`.
5. POST /discoverable-records-batch with `{"entries":[{"registryId":"abc123def456","recordIds":["rec000000001","zzz999999999"]}]}` — expect 200, `registryRecords` containing the full `payments-mcp` summary (with `descriptors`), and `errors` containing one entry `{registryId:"abc123def456", recordId:"zzz999999999", errorCode:"RESOURCE_NOT_FOUND"}`.
6. POST /discoverable-records-search with `{"searchQuery":"payments","registryIds":["abc123def456"],"maxResults":10}` — expect 200 with the `payments-mcp` record, identical to what BatchGet returned for `rec000000001`.
7. POST /registries/abc123def456/discoverable-records-list for an unknown registry id `000000000000` — expect 404 `ResourceNotFoundException`; POST /discoverable-records-search with a 300-character `searchQuery` — expect 400 `ValidationException`.
