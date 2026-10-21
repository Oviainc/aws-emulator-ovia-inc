Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 57742dc737335ed8f07f6b76d23b07c0a9c73918

# Bootstrap the Resource Groups Tagging API emulator

_Filed: 2026-10-21T21:27:55Z_
_Operation: service-com-amazonaws-resourcegroupstaggingapi-resourcegroupstaggingapi-20170126 | Class: missing_

> _Note: this issue was identified against the code base at commit `57742dc737335ed8f07f6b76d23b07c0a9c73918`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every operation in `services/service-com-amazonaws-resourcegroupstaggingapi-resourcegroupstaggingapi-20170126/src/ops/` is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state. Implement a FIRST happy-path slice: the tag read/write loop over a persisted ARN -> tags store.

Wire protocol (from README/OPERATIONS.md): `awsJson1_1`, `POST /` with `X-Amz-Target: ResourceGroupsTaggingAPI_20170126.<OperationName>`.

### Happy path (implement in this order)

1. **TagResources** — input `ResourceARNList` (list<string>, 1..20, required) and `Tags` (map<string,string>, 1..50, required). Merge each key/value into the stored tag map for every ARN in the list (upsert: existing key is overwritten, other keys preserved). Output `FailedResourcesMap` (map ARN -> FailureInfo{StatusCode, ErrorCode, ErrorMessage}) — empty map on full success. An ARN that fails (e.g. unparseable/unsupported) must appear in `FailedResourcesMap` with HTTP 200 overall, not as a top-level error.
2. **GetResources** — input `PaginationToken`, `TagFilters` (list<TagFilter{Key, Values}>, 0..50), `ResourceTypeFilters`, `ResourceARNList` (1..100), `ResourcesPerPage`, `TagsPerPage`, `IncludeComplianceDetails`, `ExcludeCompliantResources`. Output `ResourceTagMappingList` (list<ResourceTagMapping{ResourceARN, Tags, ComplianceDetails}>) and `PaginationToken`. Must return exactly the ARNs/tags previously written by TagResources, filtered: `TagFilters` entries AND-ed across keys, with a filter's `Values` list OR-ed (empty `Values` = key present with any value); `ResourceTypeFilters` matched against the service/resource-type segments of the ARN; `ResourceARNList` restricting to those ARNs. `ResourcesPerPage` caps the page and a non-empty `PaginationToken` must be echoed only when more results remain, and resuming with it must return the remainder without repeats.
3. **GetTagKeys** — output `TagKeys`, the distinct, sorted set of keys across ALL stored resources, derived from state (not canned), plus `PaginationToken` when truncated.
4. **GetTagValues** — input `Key` (1..128, required); output `TagValues`, the distinct values recorded for that key across stored resources (empty list, HTTP 200, for a key nobody used), plus `PaginationToken` when truncated.
5. **UntagResources** — input `ResourceARNList` (1..20, required) and `TagKeys` (1..50, required). Remove exactly those keys from each ARN's tag map, leaving other keys intact; output `FailedResourcesMap` (empty on success). Removing all keys must make the resource's `Tags` empty in a subsequent GetResources, and a removed key must disappear from GetTagKeys/GetTagValues once no resource uses it.

Every one of these five must compute its response from the persisted store — NO canned responses, NO echoing the request back, NO random IDs without storage. In particular GetResources/GetTagKeys/GetTagValues must be pure projections of what TagResources/UntagResources wrote, and must reflect UntagResources deletions immediately.

### State Model

- `TaggingState` (behind the service state lock reached via `crate::state::service_state`):
  - `resources: BTreeMap<String /* ResourceARN */, ResourceEntry>`
  - `pagination: HashMap<String /* token */, PageCursor>` (optional for the first slice; a cursor may instead encode the last-returned ARN)
- `ResourceEntry { arn: String, tags: BTreeMap<String /* key */, String /* value */>, resource_type: String /* derived from the ARN: "service:resource-type" */, compliance: Option<ComplianceDetails> }`
- `PageCursor { last_arn: String, filter_fingerprint: String }` — used to resume GetResources/GetTagKeys/GetTagValues; an unknown/stale token should surface `PaginationTokenExpiredException` (HTTP 400).
- ARN parsing helper: split `arn:partition:service:region:account:resource` so `ResourceTypeFilters` can match on `service` or `service:resource-type`.

Use `BTreeMap` so key/ARN ordering is deterministic across calls and pagination is stable.

### Validation to apply in the slice

- `TagResources`/`UntagResources`: required `ResourceARNList` and `Tags`/`TagKeys` present and within the modeled 1..20 / 1..50 bounds; malformed ARN -> entry in `FailedResourcesMap` with `ErrorCode=InvalidParameterException`, `StatusCode=400`.
- `GetTagValues`: `Key` required, 1..128.
- Expired/unknown pagination token -> `PaginationTokenExpiredException` (HTTP 400) for GetResources/GetTagKeys/GetTagValues.

### Out of scope for this first slice (stay 501, filed later)

GetComplianceSummary, StartReportCreation, DescribeReportCreation, ListRequiredTags.

## Evidence

OPERATIONS.md for this crate lists all 9 operations with their input/output members: TagResources (`ResourceARNList` required 1..20, `Tags` required 1..50 -> `FailedResourcesMap`), GetResources (`TagFilters`, `ResourceARNList`, `ResourcesPerPage`, `PaginationToken` -> `ResourceTagMappingList`, `PaginationToken`, @paginated items=ResourceTagMappingList), GetTagKeys (-> `TagKeys`), GetTagValues (`Key` required -> `TagValues`), UntagResources (`ResourceARNList` + `TagKeys` required -> `FailedResourcesMap`). Shapes `ResourceTagMapping{ResourceARN, Tags, ComplianceDetails}`, `TagFilter{Key, Values}` and `FailureInfo{StatusCode, ErrorCode, ErrorMessage}` come from the Smithy model via smithy_query. `src/ops/tag_resources.rs` (representative of all handlers) carries `//! SCAFFOLDED, NOT IMPLEMENTED:` and its `handle` body ends in `Err(crate::scaffolded::Scaffolded::not_implemented("TagResources"))`, i.e. HTTP 501 with zero persisted state. PaginationTokenExpiredException (HTTP 400) is modeled on the three read operations.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI `resourcegroupstaggingapi` client at it.
2. `tag-resources --resource-arn-list arn:aws:s3:::bucket-a arn:aws:sqs:us-east-1:123456789012:queue-a --tags Env=prod,Team=core` -> expect HTTP 200 with an empty `FailedResourcesMap` (currently 501).
3. `get-resources` with no filters -> expect both ARNs returned in `ResourceTagMappingList`, each with Tags [{Env,prod},{Team,core}].
4. `get-resources --tag-filters Key=Env,Values=prod` -> expect both ARNs; `Key=Env,Values=dev` -> expect an empty list.
5. `get-resources --resource-type-filters s3` -> expect only `arn:aws:s3:::bucket-a`.
6. `get-tag-keys` -> expect TagKeys [Env, Team]; `get-tag-values --key Env` -> expect TagValues [prod].
7. `tag-resources --resource-arn-list arn:aws:s3:::bucket-a --tags Env=staging` then `get-resources --resource-arn-list arn:aws:s3:::bucket-a` -> expect Env=staging with Team=core preserved (upsert, not replace).
8. `untag-resources --resource-arn-list arn:aws:s3:::bucket-a --tag-keys Env` -> HTTP 200, empty FailedResourcesMap; `get-resources --resource-arn-list arn:aws:s3:::bucket-a` -> expect only Team=core remaining.
9. `get-resources --resources-per-page 1` -> expect 1 result plus a non-empty `PaginationToken`; call again with that token -> expect the remaining resource and no repeats.
10. `get-resources --pagination-token bogus` -> expect `PaginationTokenExpiredException` (HTTP 400), not 200 or 501.
