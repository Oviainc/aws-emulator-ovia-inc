Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 34e6e58e29d15417fc6ff05c31f4f08593fa6d9a

# Bootstrap the trustedadvisor emulator

_Filed: 2026-10-21T21:18:34Z_
_Operation: service-com-amazonaws-trustedadvisor-trustedadvisor | Class: missing_

> _Note: this issue was identified against the code base at commit `34e6e58e29d15417fc6ff05c31f4f08593fa6d9a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 12 handlers in `services/service-com-amazonaws-trustedadvisor-trustedadvisor/src/ops/` is an unmodified scaffold: each carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))` (HTTP 501), and `src/state.rs` holds no Trusted Advisor data. The emulator therefore has no behaviour at all.

Implement a FIRST, SMALL slice: the account-level recommendation loop (restJson1, sigv4 name `trustedadvisor`). Scope = 5 operations, in order:

1. **ListChecks** — `GET /v1/checks`. Returns `checkSummaries` (required) + `nextToken`. Each `CheckSummary` must carry `id`, `arn` (`arn:aws:trustedadvisor:::check/<id>`), `name`, `description`, `pillars`, `awsServices`, `source`, `metadata` — all `@required` — from the seeded catalog. Honour the query filters `pillar`, `awsService`, `source` and the `maxResults`/`nextToken` pagination contract (`@paginated(items=checkSummaries)`).
2. **ListRecommendations** — `GET /v1/recommendations`. Returns `recommendationSummaries` (required) + `nextToken`, one `RecommendationSummary` per stored recommendation with `id`, `type`, `status`, `pillars`, `source`, `name`, `resourcesAggregates{okCount,warningCount,errorCode/errorCount}` and `arn` (`arn:aws:trustedadvisor::<account-id>:recommendation/<id>`), plus `checkArn`, `lifecycleStage`, `lastUpdatedAt`. Filters to honour: `type`, `status`, `pillar`, `awsService`, `source`, `checkIdentifier`, `afterLastUpdatedAt`, `beforeLastUpdatedAt`, with `maxResults`/`nextToken`.
3. **GetRecommendation** — `GET /v1/recommendations/{recommendationIdentifier}`. Look the ARN up in the store and return the full `Recommendation` (summary fields + `description`, `createdAt`, `updateReason`, `updateReasonCode`, `resolvedAt`). Unknown-but-well-formed ARN -> `ResourceNotFoundException` (404); ARN failing `len 20..200` / `^arn:[\w-]+:trustedadvisor::\d{12}:recommendation\/[\w-]+$` -> `ValidationException` (400).
4. **ListRecommendationResources** — `GET /v1/recommendations/{recommendationIdentifier}/resources`. Returns `recommendationResourceSummaries` (required) + `nextToken`, each with `id`, `arn` (`arn:aws:trustedadvisor::<acct>:recommendation-resource/<rec-id>/<res-id>`), `awsResourceId`, `regionCode`, `status`, `metadata`, `lastUpdatedAt`, `exclusionStatus` (default `included`), `recommendationArn`. Honour `status`, `exclusionStatus`, `regionCode` filters and pagination; unknown recommendation -> 404.
5. **UpdateRecommendationLifecycle** — `PUT /v1/recommendations/{recommendationIdentifier}/lifecycle`, empty output, 200. Must PERSIST `lifecycleStage` (one of `in_progress | pending_response | dismissed | resolved`), `updateReason`, `updateReasonCode` and bump `lastUpdatedAt` (and set `resolvedAt` when the stage becomes `resolved`) on the stored recommendation, so a following GetRecommendation/ListRecommendations reflects the change. Unknown ARN -> `ResourceNotFoundException`; lifecycle updates are only valid for `type = priority` recommendations in real Trusted Advisor, so a `standard` recommendation must fail with `ConflictException` (409); `updateReason` shorter than 10 chars or an unmodeled enum value -> `ValidationException` (400).

Stretch (same slice, cheap once state exists): **BatchUpdateRecommendationResourceExclusion** — `PUT /v1/batch-update-recommendation-resource-exclusion`, input `recommendationResourceExclusions` (1..25 of `{arn, isExcluded}`), output `batchUpdateRecommendationResourceExclusionErrors` (required, may be empty). Each accepted entry flips the stored resource's `exclusionStatus` to `excluded`/`included` (observable via ListRecommendationResources' `exclusionStatus` filter and `resourcesAggregates.excludedCount`); unknown ARNs come back as per-entry `{arn, errorCode, errorMessage}` rather than a top-level error.

Because Trusted Advisor has no create API, the store must be SEEDED at engine init with a small deterministic catalog (a few checks, and for each a recommendation with a handful of resources). Every response above must be computed from that persisted state — no canned bodies, no echoing input, no random ARN that is not stored. A lifecycle update followed by a read must show the new stage.

## State Model

In `src/state.rs`, on `ComAmazonawsTrustedadvisorTrustedadvisorState`:

- `checks: BTreeMap<String /*check id*/, Check>` where `Check { id, arn, name, description, pillars: Vec<RecommendationPillar>, aws_services: Vec<String>, source: RecommendationSource, metadata: BTreeMap<String,String>, resource_arn_queryable: bool, aws_resource_types: Vec<String>, check_granularity: Option<String>, recommendation_id: Option<String> }`
- `recommendations: BTreeMap<String /*recommendation ARN*/, RecommendationEntry>` where `RecommendationEntry { id, arn, name, description, check_arn: Option<String>, r#type: RecommendationType, status: RecommendationStatus, lifecycle_stage: Option<RecommendationLifecycleStage>, pillars: Vec<RecommendationPillar>, source: RecommendationSource, aws_services: Vec<String>, created_at, last_updated_at, resolved_at: Option<..>, update_reason: Option<String>, update_reason_code: Option<UpdateRecommendationLifecycleStageReasonCode>, resource_arns: Vec<String> }`
- `recommendation_resources: BTreeMap<String /*resource ARN*/, ResourceEntry>` where `ResourceEntry { id, arn, recommendation_arn, aws_resource_id, region_code, status: ResourceStatus, metadata: BTreeMap<String,String>, last_updated_at, exclusion_status: ExclusionStatus }`
- `resourcesAggregates` for a recommendation is DERIVED on read by counting its `ResourceEntry` statuses (`okCount`/`warningCount`/`errorCount`) and `excludedCount` — never a stored constant.
- Pagination: `nextToken` is an opaque cursor over the sorted key order (e.g. base64 of the last-returned key), validated on input and rejected with `ValidationException` when malformed.

The remaining operations (GetOrganizationRecommendation, ListOrganizationRecommendations, ListOrganizationRecommendationAccounts, ListOrganizationRecommendationResources, ListRecommendationsForResource, UpdateOrganizationRecommendationLifecycle) stay 501 for now and will be filed as individual gap issues.

Done = an integration test drives steps 1-5 end-to-end against one engine instance and asserts response bodies, not just 2xx: the recommendation ARN from ListRecommendations resolves in GetRecommendation, its resources list matches the aggregates, and the lifecycle update is visible on the next read.

## Evidence

OPERATIONS.md (crate root) documents all 12 operations, their restJson1 routes, required input/output members and declared errors — e.g. `GET /v1/recommendations/{recommendationIdentifier}` -> output `recommendation`, errors ResourceNotFoundException (404)/ValidationException (400); `PUT /v1/recommendations/{recommendationIdentifier}/lifecycle` -> required `lifecycleStage`, errors include ConflictException (409). Smithy model shapes Recommendation, RecommendationSummary, CheckSummary, RecommendationResourceSummary, RecommendationResourceExclusion and RecommendationResourcesAggregates give the required members and ARN patterns cited above. All 12 files under `src/ops/` are scaffolds returning `Scaffolded::not_implemented` (HTTP 501) per README.md's description of the generated stub, and no Trusted Advisor state structures exist in `src/state.rs`.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI `trustedadvisor` client at it.
2. `aws trustedadvisor list-checks --max-results 2` — expect a non-empty `checkSummaries` with required id/arn/name/description/pillars/awsServices/source/metadata and a `nextToken` when more remain; today: HTTP 501 NotImplemented.
3. `aws trustedadvisor list-recommendations` — expect `recommendationSummaries` whose `checkArn` values match the checks returned above.
4. `aws trustedadvisor get-recommendation --recommendation-identifier <arn from previous step>` — expect the full Recommendation with resourcesAggregates matching the resource list.
5. `aws trustedadvisor list-recommendation-resources --recommendation-identifier <same arn>` — expect resource summaries with exclusionStatus `included`.
6. `aws trustedadvisor update-recommendation-lifecycle --recommendation-identifier <same arn> --lifecycle-stage resolved --update-reason 'fixed in prod' --update-reason-code other` — expect 200 and empty body.
7. Re-run get-recommendation — expect `lifecycleStage=resolved`, the stored `updateReason`/`updateReasonCode`, a bumped `lastUpdatedAt` and a set `resolvedAt`.
8. Call get-recommendation with `arn:aws:trustedadvisor::123456789012:recommendation/does-not-exist` — expect ResourceNotFoundException (404), and with `not-an-arn` — expect ValidationException (400).
