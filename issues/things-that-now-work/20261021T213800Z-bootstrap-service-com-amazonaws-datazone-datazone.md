Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 09acb2538adf9fea9bce04c8d04105f42be236df

# Bootstrap the DataZone emulator

_Filed: 2026-10-21T21:38:00Z_
_Operation: service-com-amazonaws-datazone-datazone | Class: missing_

> _Note: this issue was identified against the code base at commit `09acb2538adf9fea9bce04c8d04105f42be236df`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 195 operations in `services/service-com-amazonaws-datazone-datazone/src/ops/` is an unmodified scaffold: it carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))` -> HTTP 501, and the crate holds no state. No client workflow works.

Implement a FIRST, SMALL slice: the domain -> project lifecycle, which is the entry point of every real DataZone client (you cannot touch any other resource without a domain id, and nearly every other route is nested under `/v2/domains/{domainIdentifier}`). Wire protocol is `restJson1` (SDK id `DataZone`, signing name `datazone`).

Happy path to implement, in order (fields from OPERATIONS.md):

1. **CreateDomain** — `POST /v2/domains` -> 201. Input: `name` (required), `description`, `singleSignOn`, `domainExecutionRole`, `kmsKeyIdentifier`, `tags`, `domainVersion` (V1|V2), `serviceRole`, `clientToken`. Output: `id` (must match `^dzd[-_][a-zA-Z0-9_-]{1,36}$` — generate e.g. `dzd_<random>`), `rootDomainUnitId` (matching `^[a-z0-9_\-]+$`), `name`, `description`, `arn`, `status` (set `AVAILABLE`), `portalUrl`, `tags`, `domainVersion`, `domainExecutionRole`, `serviceRole`, `singleSignOn`, `kmsKeyIdentifier`. Persist the whole record plus `createdAt`/`lastUpdatedAt`.
2. **GetDomain** — `GET /v2/domains/{identifier}` -> 200. Must return the PERSISTED domain: `id`, `name`, `description`, `arn`, `rootDomainUnitId`, `domainExecutionRole` (required), `status` (required), `portalUrl`, `createdAt`, `lastUpdatedAt`, `tags`, `domainVersion`, `serviceRole`. Unknown id -> `ResourceNotFoundException` (HTTP 404); id not matching the `dzd` pattern -> `ValidationException` (HTTP 400).
3. **ListDomains** — `GET /v2/domains` -> 200, `@paginated(inputToken=nextToken, outputToken=nextToken, items=items)`. Return one `DomainSummary` per stored domain (`id`, `name`, `arn`, `description`, `status`, `createdAt`, `lastUpdatedAt`, `managedAccountId`, `domainVersion`), honour the `status` query filter, honour `maxResults` (range 1..25) and round-trip an opaque `nextToken`.
4. **CreateProject** — `POST /v2/domains/{domainIdentifier}/projects` -> 201. Input: `name` (required, len 1..64, `^[\w -]+$`), `description`, `glossaryTerms`, `domainUnitId`, `projectProfileId`, `resourceTags`, `projectCategory`. Output: `domainId`, `id` (`^[a-zA-Z0-9_-]{1,36}$`), `name`, `description`, `projectStatus` (set `ACTIVE`), `createdBy` (required — derive from the caller identity/sigv4 principal, not a constant echo of input), `createdAt`, `lastUpdatedAt`, `glossaryTerms`, `domainUnitId`, `projectProfileId`, `resourceTags`. Unknown `domainIdentifier` -> 404 `ResourceNotFoundException`.
5. **GetProject** — `GET /v2/domains/{domainIdentifier}/projects/{identifier}` -> 200. Must return exactly what CreateProject stored (same `name`, `description`, `glossaryTerms`, `createdBy`, `createdAt`, `projectStatus`). Unknown project -> 404.
6. **ListProjects** — `GET /v2/domains/{domainIdentifier}/projects` -> 200, paginated. `items` = `ProjectSummary` list scoped to that domain only; honour the `name` and `projectCategory` query filters, `maxResults` (1..50) and `nextToken`.
7. **DeleteProject** — `DELETE /v2/domains/{domainIdentifier}/projects/{identifier}` -> 204, empty body. Removes the project; a subsequent GetProject/ListProjects must no longer see it, and a second DeleteProject on the same id -> 404 `ResourceNotFoundException`.
8. **DeleteDomain** — `DELETE /v2/domains/{identifier}` -> 202 with required `status` output (`DELETING`/`DELETED`). After it, GetDomain -> 404 and the domain disappears from ListDomains; its projects go with it.

NEVER STUBS: each of these must compute its response from persisted state. Hardcoded bodies, echoing the request back without storing it, or minting an id that is never saved are failures — a create followed by a get/list must return what was created, and a delete must be observable by the next call.

## State Model

In `src/state.rs` (`ComAmazonawsDatazoneDatazoneState`, reached via `crate::state::service_state(&*self.engine)` and mutated under its lock):

- `domains: BTreeMap<String /* domain id, "dzd_xxx" */, Domain>`
- `Domain { id, arn, name, description: Option<String>, single_sign_on: Option<SingleSignOn>, domain_execution_role: Option<String>, service_role: Option<String>, kms_key_identifier: Option<String>, status: DomainStatus, portal_url: Option<String>, domain_version: DomainVersion, root_domain_unit_id: String, tags: BTreeMap<String,String>, created_at: DateTime, last_updated_at: DateTime, projects: BTreeMap<String /* project id */, Project> }`
- `Project { id, domain_id, name, description: Option<String>, project_status: ProjectStatus, created_by: String, created_at: DateTime, last_updated_at: DateTime, glossary_terms: Vec<String>, domain_unit_id: Option<String>, project_profile_id: Option<String>, resource_tags: Vec<ResourceTag>, project_category: Option<String>, failure_reasons: Vec<ProjectDeletionError> }`
- `idempotency: BTreeMap<String /* clientToken */, String /* created resource id */>` so a repeated `CreateDomain` with the same `@idempotencyToken` returns the first domain instead of a duplicate.
- Pagination: `nextToken` is an opaque encoding of the last-returned key; listing iterates the ordered map so results are stable across pages.

All other DataZone operations (assets, glossaries, environments, subscriptions, data sources, listings, lineage, ...) stay 501 for now and will be filed as individual gap issues later.

## Evidence

`services/service-com-amazonaws-datazone-datazone/OPERATIONS.md` documents 195 restJson1 operations including CreateDomain (`POST /v2/domains` -> 201, output `id`/`rootDomainUnitId`/`status`/`arn`), GetDomain (`GET /v2/domains/{identifier}` -> 200, required `id`/`domainExecutionRole`/`status`), ListDomains (`GET /v2/domains`, @paginated, `maxResults` 1..25), CreateProject (`POST /v2/domains/{domainIdentifier}/projects` -> 201, required `domainId`/`id`/`name`/`createdBy`), GetProject (`GET .../projects/{identifier}` -> 200), ListProjects (`GET .../projects`, @paginated, `maxResults` 1..50), DeleteProject (`DELETE .../projects/{identifier}` -> 204) and DeleteDomain (`DELETE /v2/domains/{identifier}` -> 202, required `status`), each declaring `ResourceNotFoundException (HTTP 404)` and `ValidationException (HTTP 400)`. Handler evidence: `src/ops/create_domain.rs` line 4 carries `//! SCAFFOLDED, NOT IMPLEMENTED:` and line 211 returns `Err(crate::scaffolded::Scaffolded::not_implemented("CreateDomain"))`; every other file under `src/ops/` is the same generated stub, and the crate has no tests beyond `tests/state_roundtrip.rs`.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI `datazone` client at it.
2. `aws datazone create-domain --name demo-domain --domain-execution-role arn:aws:iam::123456789012:role/DZExecution` — expect 201 with a `dzd_`-prefixed `id` and `status: AVAILABLE`; currently HTTP 501 NotImplemented.
3. `aws datazone get-domain --identifier <id>` — expect 200 echoing the persisted name/description/role/status/createdAt.
4. `aws datazone list-domains` — expect the new domain in `items`; also verify `--max-results 1` paginates via `nextToken`.
5. `aws datazone create-project --domain-identifier <id> --name demo-project --description hello` — expect 201 with a project `id`, `projectStatus: ACTIVE`, populated `createdBy`.
6. `aws datazone get-project --domain-identifier <id> --identifier <projectId>` — expect the same name/description that were created.
7. `aws datazone list-projects --domain-identifier <id>` — expect exactly that project, scoped to the domain.
8. `aws datazone delete-project --domain-identifier <id> --identifier <projectId>` — expect 204; then get-project must return 404 ResourceNotFoundException and list-projects must be empty.
9. `aws datazone delete-domain --identifier <id>` — expect 202 with a `status`; then get-domain must return 404 and list-domains must not include it.
10. Negative checks: get-domain with `--identifier dzd_doesnotexist` -> 404 ResourceNotFoundException; get-domain with `--identifier not-a-domain` -> 400 ValidationException.
