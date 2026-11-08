Issue-Severity: CRITICAL
Issue-Gap-Class: wrong-behavior
Issue-Commit: bf25df16afda3c80a3b12bb65197c738522b5a3b

# Bootstrap the service-com-amazonaws-continuum-continuumcontrolplane emulator

_Filed: 2026-11-06T18:49:15Z_
_Operation: service-com-amazonaws-continuum-continuumcontrolplane | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `bf25df16afda3c80a3b12bb65197c738522b5a3b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 33 operations in this crate is an untouched scaffold answering HTTP 501 `NotImplemented` and holding no state, so no client can do anything with the service. Implement a FIRST SLICE: the stateful lifecycle of the primary resource, the **Environment** (`/environments`), which is also the parent path element of every code-finding/issue/scan subresource.

Scope — these 5 operations, in order, each computing its response from persisted state:

1. **CreateEnvironment** (`POST /environments -> 201`). Input: `name` (required, pattern `^[a-zA-Z0-9][a-zA-Z0-9 _.\-]{0,254}$`), optional `configuration` (CreateEnvironmentConfiguration{codeConfiguration}). Must mint a fresh `environmentId` matching `^[a-z0-9]+$` (len 1..64) and an `environmentArn` matching `^arn:[a-z0-9-]+:continuum:[a-z0-9-]+:[0-9]{12}:environments/[a-z0-9]+$` built from the request's partition/region/account, store the record, and return `environmentId`, `environmentArn`, `status` = `ACTIVE`, `createdAt`. Two creates must yield two DIFFERENT ids both retrievable afterwards.
2. **GetEnvironment** (`GET /environments/{environmentId} -> 200`). Must look the id up in the store and return the STORED `environmentId`, `environmentArn`, `name`, `status`, `statusUpdatedAt`, `createdAt`, `updatedAt` and the required `configuration` (EnvironmentConfigurationDetails) derived from what CreateEnvironment/UpdateEnvironment persisted. Unknown id -> `ResourceNotFoundException` (404), not 501 and not a zeroed 200.
3. **UpdateEnvironment** (`PATCH /environments/{environmentId} -> 200`). Optional `name` and `configuration`; per the model, omitted members preserve the stored values. Must mutate the stored record in place, bump `updatedAt`/`statusUpdatedAt`, and return the merged record (`environmentId`, `environmentArn`, `name`, `status`, `statusUpdatedAt`, `createdAt`, `configuration`). A following GetEnvironment must show the new name and the previously stored configuration where nothing was sent.
4. **ListEnvironments** (`GET /environments -> 200`, `@paginated(inputToken=nextToken, outputToken=nextToken, items=items)`). Must return one `EnvironmentListEntry{environmentId, environmentArn, name, status, createdAt}` per LIVE stored environment, honour `maxResults` (1..100) by truncating, and emit `nextToken` only when more remain; replaying that token must continue from where the previous page stopped without repeating or dropping entries. Deleted environments must not appear.
5. **DeleteEnvironment** (`DELETE /environments/{environmentId} -> 204`, `@idempotent`). Must remove the stored record (and any subresources keyed under it) and return an empty 204 body. A following GetEnvironment must answer `ResourceNotFoundException`, and the id must be gone from ListEnvironments.

No canned, hardcoded or echoed responses: every field above must be read back out of the store, and ids/ARNs must be minted and KEPT (a random id that is not stored is a failure). Everything else in the crate (collectors, integrations, code findings/issues/scans/artifacts, resource policies) stays 501 and will be filed as individual gap issues later.

## State Model

In `src/state.rs` (`ComAmazonawsContinuumContinuumcontrolplaneState`, reached from handlers via `crate::state::service_state(&*self.engine)` and its `lock()` guard):

- `environments: BTreeMap<String /* environmentId */, Environment>` — BTreeMap so ListEnvironments has a stable, resumable order for `nextToken`.
- `struct Environment { environment_id: String, environment_arn: String, name: String, status: EnvironmentStatus /* ACTIVE */, status_message: Option<String>, status_updated_at: SystemTime, created_at: SystemTime, updated_at: SystemTime, configuration: EnvironmentConfig }`
- `struct EnvironmentConfig { code_configuration: Option<CodeConfig> }` — what CreateEnvironmentConfiguration/UpdateEnvironmentConfiguration carried, rendered back out as EnvironmentConfigurationDetails{codeConfiguration} by Get/Update.
- `next_environment_seq: u64` — monotonic counter used to mint `^[a-z0-9]+$` ids (e.g. lowercase-hex of the counter), so ids are unique and reproducible.

Pagination tokens for ListEnvironments are derived from the last returned `environmentId` (the BTreeMap key to resume after), not from an index into a snapshot.

## Evidence

All 33 handlers are untouched scaffolds, e.g. `services/service-com-amazonaws-continuum-continuumcontrolplane/src/ops/create_environment.rs:4`: `//! SCAFFOLDED, NOT IMPLEMENTED: this operation answers HTTP 501 `NotImplemented` and` (same marker present in get_environment.rs:4, update_environment.rs:4, list_environments.rs:4, delete_environment.rs:4 and the other 28 files).

Contract quoted from `services/service-com-amazonaws-continuum-continuumcontrolplane/OPERATIONS.md`:
- :105-107 CreateEnvironment — "**Route:** `POST /environments -> 201`"; "**Input:** `name` (string, len 1..255, pattern ^[a-zA-Z0-9][a-zA-Z0-9 _.\-]{0,254}$, required), `configuration` (CreateEnvironmentConfiguration)"; "**Output:** `environmentId` ... `environmentArn` ... `status` (EnvironmentStatus, one of: ACTIVE, required), `createdAt` (timestamp, required)".
- :112-116 GetEnvironment — "**Route:** `GET /environments/{environmentId} -> 200`"; output includes "`configuration` (EnvironmentConfigurationDetails, required)" and "`updatedAt` (timestamp, required)"; errors include "`ResourceNotFoundException (HTTP 404)`".
- :120-124 UpdateEnvironment — "**Route:** `PATCH /environments/{environmentId} -> 200`", "**Traits:** @idempotent".
- :136-139 ListEnvironments — "**Traits:** @paginated(inputToken=nextToken, outputToken=nextToken, items=items), @readonly"; "**Input:** `maxResults` (integer, range 1..100, @httpQuery(maxResults)), `nextToken` (string, len 1..2048, @httpQuery(nextToken))"; "**Output:** `items` (list<EnvironmentListEntry>, required), `nextToken` (string, len 1..2048)".
- :128-132 DeleteEnvironment — "**Route:** `DELETE /environments/{environmentId} -> 204`", "**Traits:** @idempotent".

Smithy model (`smithy_query`): `UpdateEnvironmentConfiguration` doc — "Type-specific changes to an environment. Omitted members preserve stored values." `EnvironmentConfigurationDetails` doc — "Type-specific configuration and state returned by GetEnvironment and UpdateEnvironment." `EnvironmentListEntry` doc — "Summary of an environment returned in list responses.", with required members `environmentId`, `environmentArn`, `name`, `status`, `createdAt`.

The crate README states the scaffold contract: `services/service-com-amazonaws-continuum-continuumcontrolplane/README.md:36-40` — "Every generated stub starts out **scaffolded, not implemented**: ... it answers **HTTP 501 `NotImplemented`** until you write it."

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI client for the `continuum` service (restJson1, sigv4 name `continuum`) at it.
2. POST /environments with {"name":"demo-env"} — expect 201 with a fresh `environmentId` matching ^[a-z0-9]+$, an `environmentArn` ending in `:environments/<that id>`, `status`="ACTIVE" and a `createdAt`. Today: HTTP 501 NotImplemented.
3. GET /environments/<environmentId> — expect 200 echoing the SAME id/ARN, name "demo-env", status ACTIVE, createdAt/updatedAt/statusUpdatedAt and a `configuration` object.
4. PATCH /environments/<environmentId> with {"name":"demo-env-2"} — expect 200 with name "demo-env-2", the original `createdAt` unchanged, and the stored configuration preserved; then GET again and see the new name persisted.
5. POST /environments with {"name":"second-env"}, then GET /environments?maxResults=1 — expect exactly 1 item plus a `nextToken`; GET /environments?maxResults=1&nextToken=<token> — expect the OTHER environment and no duplicate.
6. DELETE /environments/<environmentId> — expect 204 with an empty body; GET /environments/<environmentId> — expect 404 ResourceNotFoundException; GET /environments — expect only "second-env" remaining.
