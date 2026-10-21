Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 7b44f3f32d1f58d662874eb8ca0f546e296cee00

# Bootstrap the inspector2 emulator

_Filed: 2026-10-21T21:08:48Z_
_Operation: service-com-amazonaws-inspector2-inspector2 | Class: missing_

> _Note: this issue was identified against the code base at commit `7b44f3f32d1f58d662874eb8ca0f546e296cee00`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

All 81 operations in `services/service-com-amazonaws-inspector2-inspector2/src/ops/` are unmodified scaffolds: each carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))`, i.e. HTTP 501, and the service holds zero state. Nothing works end-to-end.

Implement a FIRST minimal happy-path slice — Inspector2's account-activation plus the suppression-filter lifecycle, which is the service's primary user-managed resource (restJson1, sigv4 name `inspector2`):

1. **Enable** — `POST /enable`. Input: `resourceTypes` (required, 1..5 of the ResourceScanType enum: EC2 | ECR | LAMBDA | LAMBDA_CODE | CODE_REPOSITORY), optional `accountIds` (0..100, 12-digit), optional `clientToken` (@idempotencyToken). Must record per-account enablement in state and return `accounts` (required, list<Account>: `accountId`, `status`, `resourceStatus`) with status ENABLED for each requested account (default to the caller account when `accountIds` is absent) and `failedAccounts` only for accounts that genuinely fail. Do not echo a canned list.
2. **BatchGetAccountStatus** — `POST /status/batch/get`. Input: optional `accountIds` (0..100). Must read back what Enable persisted: `accounts` (required, list<AccountState>: `accountId`, `state` {status/errorCode/errorMessage}, `resourceState` per resource type). Before Enable the account must report DISABLED, after Enable ENABLED for exactly the resource types enabled.
3. **CreateFilter** — `POST /filters/create`. Input: `action` (NONE | SUPPRESS, required), `filterCriteria` (required), `name` (1..128, required), optional `description`, `reason`, `tags`. Must mint a deterministic, well-formed filter ARN (`arn:aws:inspector2:<region>:<account>:owner/<account>/filter/<id>`, 1..128), store a Filter record, and return `arn`. Store exactly what was sent — criteria, action, description, reason, tags must survive.
4. **ListFilters** — `POST /filters/list`. Input: optional `arns`, `action`, `maxResults` (1..100), `nextToken`. Must return `filters` (required, list<Filter> with `arn`, `ownerId`, `name`, `criteria`, `action`, `createdAt`, `updatedAt`, plus `description`/`reason`/`tags` when set) computed from stored filters, honouring the `arns` and `action` filters, truncating at `maxResults` and emitting/accepting an opaque `nextToken` for the remainder.
5. **UpdateFilter** — `POST /filters/update`. Input: `filterArn` (required) plus any of `action`, `description`, `filterCriteria`, `name`, `reason`. Must mutate the stored record in place (only the supplied members), refresh `updatedAt`, return `arn`, and raise `ResourceNotFoundException` (HTTP 404) for an unknown ARN. A following ListFilters must show the new values.
6. **DeleteFilter** — `POST /filters/delete`. Input: `arn` (required). Must remove the stored filter, return `arn`, and raise `ResourceNotFoundException` (HTTP 404) when the ARN is unknown or already deleted. A following ListFilters must no longer contain it.

Every one of these six must compute its response from the shared engine state reached via `crate::state::service_state(&*self.engine)` — no canned responses, no echoing the input back without storing it, no random ARN without a stored record. Each implemented op must have its `SCAFFOLDED, NOT IMPLEMENTED` marker line deleted.

## State Model

In `ComAmazonawsInspector2Inspector2State`:

- `filters: BTreeMap<String /* filter ARN */, StoredFilter>` where `StoredFilter { arn: String, owner_id: String /* 12-digit account */, name: String, criteria: FilterCriteria, action: FilterAction, description: Option<String>, reason: Option<String>, tags: BTreeMap<String, String>, created_at: SystemTime, updated_at: SystemTime }`. Insertion-ordered/stable iteration so pagination tokens are meaningful.
- `accounts: BTreeMap<String /* 12-digit accountId */, AccountEnablement>` where `AccountEnablement { status: Status /* ENABLED | DISABLED | SUSPENDED */, resource_status: BTreeMap<ResourceScanType, Status>, enabled_at: SystemTime, client_tokens: BTreeSet<String> }` — `resource_status` is the per-resource-type map that both Enable's `Account.resourceStatus` and BatchGetAccountStatus's `AccountState.resourceState` are rendered from, and `client_tokens` makes Enable idempotent for a repeated `clientToken`.
- `next_filter_id: u64` (or a UUID source) used only to mint filter ARNs; the ARN is the map key so ARNs and records can never diverge.

Pagination tokens for ListFilters should encode the position in the stable `filters` ordering (e.g. the last returned ARN), not an index into a freshly built list.

All other 75 operations (findings, coverage, CIS scans, code security, connectors, members/delegated-admin, SBOM/report exports, encryption key, tagging) stay 501 for now and will be filed as individual gap issues.

## Evidence

`services/service-com-amazonaws-inspector2-inspector2/OPERATIONS.md` (generated from `smithy-model/com-amazonaws-inspector2-inspector2.json`): header says "81 operations", protocol `restJson1`, SDK id `Inspector2`. Routes and members quoted above come from its sections `Enable` (L217-222), `BatchGetAccountStatus` (L46-51), `CreateFilter` (L131-136), `ListFilters` (L428-434, `@paginated(inputToken=nextToken, outputToken=nextToken, items=filters)`), `UpdateFilter` (L605-610) and `DeleteFilter` (L182-187). The Filter/AccountState output shapes (required `arn`, `ownerId`, `name`, `criteria`, `action`, `createdAt`, `updatedAt`; `accountId`, `state`, `resourceState`) come from `smithy_query query=shape shape=Filter,AccountState`. A repo-wide grep for `^//! SCAFFOLDED, NOT IMPLEMENTED:` under this crate's `src/ops/` matches every one of the 81 handler files (including `enable.rs`, `batch_get_account_status.rs`, `create_filter.rs`, `list_filters.rs`, `update_filter.rs`, `delete_filter.rs`), so there is no implemented behavior and no test asserting any of it; the only test present is the generic `tests/state_roundtrip.rs`.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI client at it with service `inspector2`.
2. `aws inspector2 enable --resource-types EC2 ECR` → expect HTTP 200 with `accounts[0].status == "ENABLED"`; actual: HTTP 501 NotImplemented.
3. `aws inspector2 batch-get-account-status` → expect the account to report state ENABLED with resourceState EC2/ECR enabled (read back from what Enable stored); actual: HTTP 501.
4. `aws inspector2 create-filter --name suppress-lows --action SUPPRESS --filter-criteria '{"severity":[{"comparison":"EQUALS","value":"LOW"}]}'` → expect 200 with a well-formed filter `arn`; actual: HTTP 501.
5. `aws inspector2 list-filters` → expect exactly one Filter whose `name`, `action`, `criteria`, `createdAt`/`updatedAt` match what was created; actual: HTTP 501.
6. `aws inspector2 update-filter --filter-arn <arn> --description 'tuned'` then `list-filters` → expect the stored filter to show the new description and a bumped `updatedAt`; actual: HTTP 501.
7. `aws inspector2 delete-filter --arn <arn>` then `list-filters` → expect `filters` empty, and a second `delete-filter` with the same ARN to return ResourceNotFoundException (HTTP 404); actual: HTTP 501 for both.
