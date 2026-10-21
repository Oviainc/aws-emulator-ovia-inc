Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 57742dc737335ed8f07f6b76d23b07c0a9c73918

# Bootstrap the com-amazonaws-resiliencehubv2-ngrhservicecore emulator

_Filed: 2026-10-21T21:28:10Z_
_Operation: service-com-amazonaws-resiliencehubv2-ngrhservicecore | Class: missing_

> _Note: this issue was identified against the code base at commit `57742dc737335ed8f07f6b76d23b07c0a9c73918`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 73 operations in this crate is an unmodified scaffold: each `src/ops/*.rs` carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and returns `Err(crate::scaffolded::Scaffolded::not_implemented("<Op>"))`, i.e. HTTP 501, and the crate holds no state. Implement a FIRST, SMALL slice: the stateful lifecycle of the service's primary resource, the `Service` (AWS Resilience Hub v2 / `resiliencehub` restJson1).

### Happy path to implement (in order)

1. **CreateService** — `POST /v2/create-service`. Required input: `name` (2..60, `^[A-Za-z0-9][A-Za-z0-9_\-]{1,59}$`), `regions` (list<string>, 1..5), `permissionModel` (requires `invokerRoleName`). Optional: `description` (<=615), `associatedSystems` (<=20), `policyArn`, `dependencyDiscovery` (ENABLED|DISABLED), `reportConfiguration`, `kmsKeyId`, `devOpsAgentConfiguration`, `tags` (1..50), `clientToken` (@idempotencyToken). Output: `service` (Service, required) — must contain a freshly minted `serviceArn` of the form `arn:aws:resiliencehub:<region>:<account>:service/<uuid>`, plus `name`, `regions`, `permissionModel`, `description`, `tags`, `assessmentStatus` = `NOT_STARTED`, `openFindingsCount`/`resolvedFindingsCount` = 0, and `createdAt`/`updatedAt` timestamps. The record must be STORED, not echoed. A repeat call with the same `clientToken` must return the same stored service rather than creating a second one; a duplicate `name` must raise `ConflictException` (409).

2. **GetService** — `GET /v2/get-service?serviceArn=...`. Looks up the stored service by `serviceArn` and returns the SAME `service` body that CreateService returned (same arn, name, regions, description, tags, permissionModel, createdAt). Unknown arn ⇒ `ResourceNotFoundException` (404). Malformed/short arn (len < 31 or pattern mismatch) ⇒ `ValidationException` (400).

3. **ListServices** — `GET /v2/list-services`. Must enumerate the stored services as `serviceSummaries` (list<ServiceSummary>, required) projecting `serviceArn`, `name`, `regions`, `associatedSystems`, `policyArn`, `assessmentStatus`, `openFindingsCount`, `resolvedFindingsCount`, `createdAt`, `updatedAt` from the stored record. Honour `maxResults` (1..100, default 100) by truncating and emitting an opaque `nextToken` (1..2000) that a subsequent call resumes from, and honour the `policyArn` / `systemArn` / `assessmentStatus` / `accountId` / `ouId` / `userJourneyId` query filters (at minimum `policyArn` and `assessmentStatus`). An empty store returns an empty list, not 501.

4. **UpdateService** — `POST /v2/update-service`. Required `serviceArn`; optional `description`, `associatedSystems`, `policyArn`, `regions`, `permissionModel`, `dependencyDiscovery`, `reportConfiguration`, `devOpsAgentConfiguration`. Must apply each PRESENT member to the stored record (leaving absent members untouched), bump `updatedAt`, and return the merged `service`. A later GetService/ListServices must show the new values. Unknown arn ⇒ `ResourceNotFoundException` (404).

5. **DeleteService** — `POST /v2/delete-service`. Required `serviceArn`; output echoes `serviceArn` (required) only AFTER the record is removed from the store. A subsequent GetService for that arn ⇒ `ResourceNotFoundException` (404); ListServices must no longer include it. Deleting an unknown arn ⇒ 404.

Every one of these five must compute its response from persisted state held in the shared engine reached via `crate::state::service_state(&*self.engine)`. No canned bodies, no echoing the input back, no minting an ARN that is never stored. A `CreateService` followed by `GetService` must return what was created; a `DeleteService` must make it disappear.

## State Model

In `src/state.rs`, on `ComAmazonawsResiliencehubv2NgrhservicecoreState`:

- `services: BTreeMap<String /* serviceArn */, ServiceRecord>` — insertion/creation order preserved for stable list + pagination.
- `ServiceRecord { service_arn: String, name: String, description: Option<String>, associated_systems: Vec<AssociatedSystem>, policy_arn: Option<String>, regions: Vec<String>, permission_model: Option<PermissionModel>, dependency_discovery: Option<DependencyDiscoveryConfig>, report_configuration: Option<ServiceReportConfiguration>, dev_ops_agent_configuration: Option<DevOpsAgentConfigurationView>, kms_key_id: Option<String>, tags: BTreeMap<String,String>, assessment_status: AssessmentStatus /* NOT_STARTED at create */, open_findings_count: i32, resolved_findings_count: i32, account_id: String, created_at: SystemTime, updated_at: SystemTime }`
- `service_names: BTreeMap<String /* name */, String /* serviceArn */>` — for the duplicate-name `ConflictException`.
- `idempotency: BTreeMap<String /* clientToken */, String /* serviceArn */>` — for `@idempotencyToken` replay on CreateService.
- `list_tokens: BTreeMap<String /* nextToken */, usize /* offset into the services order */>` — or encode the offset/last-arn in the opaque token; either way the token must resume the same scan.

Only a `Service` → `ServiceSummary` projection helper is needed on top of this; both GetService and ListServices read the same `ServiceRecord`.

All other 68 operations (systems, user journeys, policies, tests, test runs, assertions, input sources, findings, reports, tagging, …) stay at 501 for now and will be filed as individual gap issues later. Keep this first slice small.

Done = a client script that runs CreateService → GetService → ListServices → UpdateService → GetService → DeleteService → GetService(404) end-to-end with correct, stateful bodies at every step.

## Evidence

Spec: `services/service-com-amazonaws-resiliencehubv2-ngrhservicecore/smithy-model/com-amazonaws-resiliencehubv2-ngrhservicecore.json` + the denormalized `OPERATIONS.md` in the crate root. OPERATIONS.md header: "73 operations", protocol `restJson1`, sdkId `resiliencehubv2`, sigv4 name `resiliencehub`. OPERATIONS.md lines 54-59 (CreateService: route `POST /v2/create-service`, required `name`/`regions`/`permissionModel`, output `service` (Service, required), errors incl. ConflictException 409 / ValidationException 400), 190-196 (GetService: `GET /v2/get-service`, @readonly, `serviceArn` @httpQuery required, output `service`, ResourceNotFoundException 404), 348-354 (ListServices: @paginated(inputToken=nextToken, outputToken=nextToken, items=serviceSummaries), `maxResults` range 1..100 default 100, filters systemArn/userJourneyId/ouId/accountId/assessmentStatus/policyArn), 538-543 (UpdateService: `POST /v2/update-service`, required `serviceArn`, optional mutable members, output `service`), 117-122 (DeleteService: `POST /v2/delete-service`, required `serviceArn`, output `serviceArn` required). smithy_query on shapes `Service`/`ServiceSummary`/`PermissionModel` supplied the stored field list (serviceArn, name, description, associatedSystems, policyArn, regions, permissionModel, dependencyDiscovery, reportConfiguration, kmsKeyId, tags, assessmentStatus, openFindingsCount, resolvedFindingsCount, accountId, ouId, organizationId, createdAt, updatedAt) and `PermissionModel.invokerRoleName` as required; the `Service` doc states "A service is the primary unit of resilience assessment", confirming it as the primary resource. All-stub status confirmed: every file under `src/ops/` is a scaffold — e.g. `src/ops/create_service.rs:4` carries `//! SCAFFOLDED, NOT IMPLEMENTED:` and line 181 returns `Err(crate::scaffolded::Scaffolded::not_implemented("CreateService"))` (HTTP 501). No tests exist beyond the generated `tests/state_roundtrip.rs`.

## Steps to Reproduce

1. Start the emulator and point an AWS client at it with signing name `resiliencehub`.
2. POST /v2/create-service with {"name":"checkout-svc","regions":["us-east-1"],"permissionModel":{"invokerRoleName":"ResilienceHubRole"},"description":"checkout path","tags":{"env":"dev"},"clientToken":"tok-1"} — expect 200 with service.serviceArn minted, name/regions/description/tags as sent, assessmentStatus NOT_STARTED, createdAt set. Today: HTTP 501 NotImplemented.
3. Repeat the identical POST with clientToken "tok-1" — expect the SAME serviceArn back (idempotent replay), not a second service.
4. GET /v2/get-service?serviceArn=<arn from step 2> — expect 200 with the identical service body (same arn, name, regions, description, tags, createdAt).
5. GET /v2/list-services — expect serviceSummaries containing exactly that service with its stored name/regions/assessmentStatus. Create two more services, then GET /v2/list-services?maxResults=2 and expect 2 summaries plus a nextToken; re-issue with that nextToken and expect the remaining summary with no overlap.
6. POST /v2/update-service with {"serviceArn":"<arn>","description":"updated","regions":["us-east-1","us-west-2"]} — expect 200 with the merged service; then GET /v2/get-service and expect description="updated", regions of length 2, name unchanged, updatedAt > createdAt.
7. POST /v2/delete-service with {"serviceArn":"<arn>"} — expect 200 echoing serviceArn.
8. GET /v2/get-service?serviceArn=<arn> — expect 404 ResourceNotFoundException; GET /v2/list-services must no longer list it.
9. GET /v2/get-service?serviceArn=not-an-arn — expect 400 ValidationException.
