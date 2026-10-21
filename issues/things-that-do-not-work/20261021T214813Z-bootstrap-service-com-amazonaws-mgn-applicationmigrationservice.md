Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: a2986bf0fd3cc35fe32946cf5d2b8a785e7f69a4

# Bootstrap the mgn (Application Migration Service) emulator

_Filed: 2026-10-21T21:48:13Z_
_Operation: service-com-amazonaws-mgn-applicationmigrationservice | Class: missing_

> _Note: this issue was identified against the code base at commit `a2986bf0fd3cc35fe32946cf5d2b8a785e7f69a4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 95 handlers under `services/service-com-amazonaws-mgn-applicationmigrationservice/src/ops/` is an unmodified scaffold that carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and answers HTTP 501 `NotImplemented` while holding no state, so no client workflow works at all.

Implement a FIRST, SMALL, stateful slice: the Application resource lifecycle, which is mgn's primary user-facing resource (restJson1, SDK id `mgn`, sigv4 name `mgn`). Scope is exactly these 5 operations, in this order:

1. **InitializeService** — `POST /InitializeService -> 204`, no input/output members. Must flip a persisted per-account `initialized` flag. Until it has been called, the other four operations must return the modeled `UninitializedAccountException` (HTTP 400); after it, they must succeed. Calling it twice is fine (idempotent).
2. **CreateApplication** — `POST /CreateApplication -> 201`. Input: `name` (required, 1..256, pattern `^[^\s\x00]( *[^\s\x00])*$`), `description` (0..600), `tags` (map, 0..50), `accountID` (12 digits). Must mint a fresh `applicationID` matching `^app-[0-9a-zA-Z]{17}$` and an `arn`, store the record, and return the stored record: `applicationID`, `arn`, `name`, `description`, `isArchived=false`, `applicationAggregatedStatus` (`{lastUpdateDateTime, healthStatus, progressStatus, totalSourceServers=0}`), `creationDateTime` and `lastModifiedDateTime` as ISO8601 `...Z` strings matching the modeled pattern (19..32 chars), and `tags`. Because the operation is `@idempotent`, a repeat of the same request must not create a duplicate.
3. **ListApplications** — `POST /ListApplications -> 200`. Input: `filters` (`ListApplicationsRequestFilters` = `applicationIDs`, `isArchived`, `waveIDs`), `maxResults` (1..1000), `nextToken` (0..2048), `accountID`. Must return `items` built from the stored map (never a canned list), honour `filters.applicationIDs` / `filters.isArchived`, truncate to `maxResults`, and emit `nextToken` only when more remain — a later call with that token must resume without repeating or dropping items.
4. **UpdateApplication** — `POST /UpdateApplication -> 200`. Input: `applicationID` (required), `name`, `description`, `accountID`. Must mutate the stored record in place (only the members present in the request), bump `lastModifiedDateTime`, and return the full updated Application. Unknown `applicationID` -> `ResourceNotFoundException` (HTTP 404).
5. **DeleteApplication** — `POST /DeleteApplication -> 204`. Input: `applicationID` (required), `accountID`. Must remove the record from state; a following ListApplications must not show it and a following UpdateApplication on it must return `ResourceNotFoundException` (404). Deleting an unknown ID -> `ResourceNotFoundException` (404).

NEVER STUBS: each of the five must compute its response from persisted state. Hardcoded or echoed bodies, and minting an ID without storing the record, are failures — a create followed by a list/update must return exactly what was created, including the generated IDs and timestamps.

## State Model

In `src/state.rs` (`ComAmazonawsMgnApplicationmigrationserviceState`, reached via `crate::state::service_state(&*self.engine)`):

- `initialized_accounts: HashSet<String>` — account IDs for which InitializeService has run (default/missing `accountID` collapses to one well-known key, e.g. `"000000000000"`).
- `applications: HashMap<String /* applicationID, "app-" + 17 alnum */, Application>` where `Application { application_id: String, arn: String, name: String, description: Option<String>, is_archived: bool, aggregated_status: ApplicationAggregatedStatus { last_update_date_time: String, health_status: Option<String>, progress_status: Option<String>, total_source_servers: i64 }, creation_date_time: String /* ISO8601 Z */, last_modified_date_time: String, tags: BTreeMap<String,String>, wave_id: Option<String>, account_id: String, source_server_ids: BTreeSet<String> }`.
- `application_id_counter` (or any monotonic generator) so minted IDs are unique and pattern-valid.
- Insertion order preserved (e.g. an `app_order: Vec<String>`) so ListApplications pagination with `nextToken` is stable across calls.

Done = a client script that runs InitializeService -> CreateApplication -> ListApplications -> UpdateApplication -> ListApplications -> DeleteApplication -> ListApplications succeeds with correct, stateful bodies at every step. All other mgn operations (waves, source servers, jobs, replication/launch configuration templates, connectors, imports/exports, network migration) stay 501 and will be filed as individual gap issues later.

## Evidence

`services/service-com-amazonaws-mgn-applicationmigrationservice/OPERATIONS.md` lines 25-30 (InitializeService: `POST /InitializeService -> 204`, errors AccessDeniedException/ValidationException), 80-86 (CreateApplication: required `name`, output `applicationID` pattern `^app-[0-9a-zA-Z]{17}$`, `arn`, `isArchived`, `applicationAggregatedStatus`, `creationDateTime`/`lastModifiedDateTime`, errors ConflictException/ServiceQuotaExceededException/UninitializedAccountException), 88-94 (DeleteApplication -> 204, ResourceNotFoundException 404), 96-102 (ListApplications `@paginated(inputToken=nextToken, outputToken=nextToken, items=items)`, `maxResults` 1..1000), 134-140 (UpdateApplication, ResourceNotFoundException 404). Shapes `Application`, `ApplicationAggregatedStatus` and `ListApplicationsRequestFilters` (applicationIDs/isArchived/waveIDs) confirmed via smithy_query against `smithy-model/com-amazonaws-mgn-applicationmigrationservice.json`. Stub status confirmed: `src/ops/create_application.rs` line 4 carries `//! SCAFFOLDED, NOT IMPLEMENTED:` and the handler returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))` (HTTP 501); every file in `src/ops/` is the same scaffold. No tests exist beyond the generated `tests/state_roundtrip.rs`.

## Steps to Reproduce

1. POST /InitializeService with an empty body -> expect HTTP 204 (currently 501).
2. POST /CreateApplication with {"name":"billing-app","description":"demo","tags":{"env":"dev"}} -> expect HTTP 201 and a body containing a freshly minted applicationID matching ^app-[0-9a-zA-Z]{17}$, an arn, isArchived=false, and ISO8601 creationDateTime/lastModifiedDateTime (currently 501).
3. POST /ListApplications with {} -> expect HTTP 200 and items containing exactly the created application with the same applicationID, name, description and tags.
4. POST /UpdateApplication with {"applicationID":"<id>","description":"updated"} -> expect HTTP 200, description="updated", name unchanged, lastModifiedDateTime newer than creationDateTime.
5. POST /ListApplications with {"filters":{"applicationIDs":["<id>"]}} -> expect the one application with description="updated" (proves the update was persisted, not echoed).
6. POST /DeleteApplication with {"applicationID":"<id>"} -> expect HTTP 204; then POST /ListApplications {} -> expect items empty, and POST /UpdateApplication on the same ID -> expect ResourceNotFoundException with HTTP 404.
7. Restart-free negative check: on a fresh account that never called InitializeService, POST /ListApplications -> expect UninitializedAccountException with HTTP 400.
