Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: a2986bf0fd3cc35fe32946cf5d2b8a785e7f69a4

# Bootstrap the Migration Hub Refactor Spaces emulator

_Filed: 2026-10-21T21:47:55Z_
_Operation: service-com-amazonaws-migrationhubrefactorspaces-refactorspaces | Class: stub_

> _Note: this issue was identified against the code base at commit `a2986bf0fd3cc35fe32946cf5d2b8a785e7f69a4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 24 operations in `services/service-com-amazonaws-migrationhubrefactorspaces-refactorspaces/src/ops/` is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state, so no client can do anything with this service. This issue defines the FIRST, minimal happy-path slice: the environment/application lifecycle of Refactor Spaces (restJson1, sigv4 signing name `refactor-spaces`).

### Happy path to implement (in order)

1. **CreateEnvironment** — `POST /environments`. Input: `Name` (required, 3..63, must not start with `env-`), `NetworkFabricType` (required, `NONE`|`TRANSIT_GATEWAY`), optional `Description`, `Tags`, `ClientToken`. Must mint a fresh `EnvironmentId` matching `^env-[0-9A-Za-z]{10}$`, build `Arn` of the form `arn:aws:refactor-spaces:<region>:<account-id>:environment/<EnvironmentId>`, store the record, and return `Name`, `Arn`, `EnvironmentId`, `Description`, `NetworkFabricType`, `OwnerAccountId`, `State` (`ACTIVE` is acceptable for the emulator; `CREATING` only if a later Get transitions it), `Tags`, `CreatedTime`, `LastUpdatedTime` — all read back out of the stored record, not echoed from the request body.
2. **GetEnvironment** — `GET /environments/{EnvironmentIdentifier}`. Must look the environment up by id and return exactly what CreateEnvironment persisted (same `Arn`, `EnvironmentId`, `Name`, `NetworkFabricType`, `Tags`, `CreatedTime`). Unknown id → `ResourceNotFoundException` (HTTP 404), not 200 and not 501.
3. **ListEnvironments** — `GET /environments`. Must return one `EnvironmentSummary` per stored environment (id, arn, name, state, network fabric type, timestamps), honour `maxResults` (1..100) by truncating, and emit/accept an opaque `NextToken` so paging through all environments yields each one exactly once. An empty store returns an empty list, not an error.
4. **CreateApplication** — `POST /environments/{EnvironmentIdentifier}/applications`. Input: `Name` (required, must not start with `app-`), `VpcId` (required, `^vpc-...$`), `ProxyType` (required, `API_GATEWAY`), optional `ApiGatewayProxy`, `Tags`, `ClientToken`. Must reject a non-existent `EnvironmentIdentifier` with `ResourceNotFoundException` (404), mint `ApplicationId` matching `^app-[0-9A-Za-z]{10}$`, store the application under its parent environment, and return the stored `Arn`, `ApplicationId`, `EnvironmentId`, `VpcId`, `ProxyType`, `State`, `Tags`, `CreatedTime`.
5. **ListApplications** — `GET /environments/{EnvironmentIdentifier}/applications`. Must return only the applications belonging to that environment (scoping matters: applications created in env A must not appear under env B), with the same `maxResults`/`NextToken` behaviour as ListEnvironments, and 404 for an unknown environment.
6. **DeleteEnvironment** — `DELETE /environments/{EnvironmentIdentifier}`. Must remove the environment from the store and return `Name`, `Arn`, `EnvironmentId`, `State` (`DELETING`), `LastUpdatedTime` from the record being removed. After it succeeds, GetEnvironment on that id must return `ResourceNotFoundException` (404) and ListEnvironments must no longer include it. Deleting an unknown id → 404.

No canned, hardcoded, or request-echoed responses: each of the six must compute its response from the persisted store, so that a create is observable by every later read and a delete is observable as absence. Identifier generation without storage (random id, nothing saved) counts as a failure of this issue.

Minimum input validation for the slice: reject a missing required member and an identifier that does not match its modeled pattern (`^env-[0-9A-Za-z]{10}$`, `^app-[0-9A-Za-z]{10}$`) with `ValidationException` (HTTP 400) rather than 404 or 500.

## State Model

A single service-level store behind the existing state lock, e.g.:

- `RefactorSpacesState { environments: HashMap<String /* env-XXXXXXXXXX */, Environment>, resource_policies: HashMap<String /* resource ARN */, String /* policy JSON */> }` — `resource_policies` may stay empty in this slice (Put/Get/DeleteResourcePolicy remain stubbed).
- `Environment { environment_id: String, name: String, arn: String, description: Option<String>, network_fabric_type: String, transit_gateway_id: Option<String>, owner_account_id: String, state: String, tags: BTreeMap<String,String>, created_time: DateTime, last_updated_time: DateTime, applications: HashMap<String /* app-XXXXXXXXXX */, Application>, vpcs: BTreeMap<String /* vpc id */, EnvironmentVpc> }`
- `Application { application_id: String, environment_id: String, name: String, arn: String, vpc_id: String, proxy_type: String, api_gateway_proxy: Option<ApiGatewayProxy>, state: String, tags: BTreeMap<String,String>, created_time: DateTime, last_updated_time: DateTime, services: HashMap<String /* svc-XXXXXXXXXX */, Service>, routes: HashMap<String /* rte-XXXXXXXXXX */, Route> }`
- `Service` and `Route` structs may be declared now (fields mirroring CreateService / CreateRoute outputs) but left unpopulated — their handlers are out of scope for this first slice.
- Insertion order must be preserved (or a deterministic sort key kept) so list pagination is stable across calls; `NextToken` should encode the index/last-seen id of the previous page.

### Out of scope (stay 501, filed separately)

CreateService, CreateRoute, GetService, GetRoute, ListServices, ListRoutes, ListEnvironmentVpcs, UpdateRoute, DeleteService, DeleteRoute, DeleteApplication, Put/Get/DeleteResourcePolicy, TagResource, UntagResource, ListTagsForResource.

### Done when

An integration test drives steps 1-6 against the emulator and every assertion on the response bodies (not just the status code) holds: the ids/ARNs round-trip, the application is listed only under its own environment, and the environment is gone after deletion.

## Evidence

OPERATIONS.md for this crate documents the routes, required members and output members cited above: CreateEnvironment (`POST /environments`, required `Name`+`NetworkFabricType`, outputs `EnvironmentId` `^env-[0-9A-Za-z]{10}$`, `Arn`, `State`, `CreatedTime`), GetEnvironment (`GET /environments/{EnvironmentIdentifier}`, @readonly, declares `ResourceNotFoundException` 404), ListEnvironments (@paginated inputToken=NextToken/outputToken=NextToken/items=EnvironmentSummaryList, `MaxResults` range 1..100), CreateApplication (`POST /environments/{EnvironmentIdentifier}/applications`, required `Name`/`VpcId`/`ProxyType`, outputs `ApplicationId` `^app-[0-9A-Za-z]{10}$`, declares `ResourceNotFoundException` 404 for a bad parent env), ListApplications (@paginated, items=ApplicationSummaryList, env-scoped path), DeleteEnvironment (`DELETE /environments/{EnvironmentIdentifier}`, @idempotent, outputs `State`/`LastUpdatedTime`). Also: `grep -l '^//! SCAFFOLDED, NOT IMPLEMENTED:' src/ops/*.rs` matches all 24 operation files in this crate, each returning `crate::scaffolded::Scaffolded::not_implemented(..)` (HTTP 501), so there is no state store and no test coverage for any of them yet. ValidationException (400) and ResourceNotFoundException (404) are modeled on all six operations in the slice.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI client for `refactor-spaces` at it.
2. CreateEnvironment with Name="demo-env", NetworkFabricType="NONE", Tags={"team":"migration"} — expect 200 with a fresh EnvironmentId matching ^env-[0-9A-Za-z]{10}$ and an Arn; actual: HTTP 501 NotImplemented.
3. GetEnvironment with that EnvironmentIdentifier — expect 200 echoing the persisted Name, Arn, NetworkFabricType and Tags.
4. ListEnvironments — expect the new environment present exactly once; repeat with maxResults=1 and follow NextToken to confirm paging covers every environment without duplicates.
5. CreateApplication under that environment with Name="demo-app", VpcId="vpc-0123456789abcdef0", ProxyType="API_GATEWAY" — expect 200 with ApplicationId matching ^app-[0-9A-Za-z]{10}$ and EnvironmentId equal to the parent.
6. ListApplications for that environment — expect exactly the one application; ListApplications for a second, different environment — expect an empty list (no cross-environment leakage).
7. CreateApplication under EnvironmentIdentifier="env-0000000000" (never created) — expect ResourceNotFoundException (HTTP 404).
8. DeleteEnvironment on the demo environment — expect 200 with State=DELETING; then GetEnvironment on it — expect ResourceNotFoundException (HTTP 404) and ListEnvironments no longer lists it.
