Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: 09acb2538adf9fea9bce04c8d04105f42be236df

# Bootstrap the Compute Optimizer emulator

_Filed: 2026-10-21T21:38:26Z_
_Operation: service-com-amazonaws-computeoptimizer-computeoptimizerservice | Class: stub_

> _Note: this issue was identified against the code base at commit `09acb2538adf9fea9bce04c8d04105f42be236df`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 28 handlers under `services/service-com-amazonaws-computeoptimizer-computeoptimizerservice/src/ops/` is an unmodified scaffold: it carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))` (HTTP 501). The service holds no state, so no client workflow works at all.

This issue scopes a SMALL first slice: Compute Optimizer's only genuinely writable resources are the account's *enrollment status* and *recommendation preferences*. Everything else (Get*Recommendations, Export*, projected metrics) is read-only analysis output that the real service derives from observed workloads. So the bootstrap happy path is the enrollment + preferences lifecycle, implemented statefully end-to-end over the `awsJson1_0` wire protocol (`POST /`, `X-Amz-Target: ComputeOptimizerService.<OperationName>`).

Implement these 6 operations, in this order:

1. **UpdateEnrollmentStatus** — input `status` (Status: Active|Failed|Inactive|Pending, required), `includeMemberAccounts` (boolean, default false, required). Must PERSIST the new status plus `includeMemberAccounts` and a `lastUpdatedTimestamp`, and return `status` + `statusReason` reflecting what was stored. Per the model's enum semantics only `Active`/`Inactive` are client-settable; `Pending`/`Failed` are server-assigned, so reject them with `InvalidParameterValueException` (HTTP 400).
2. **GetEnrollmentStatus** — no input. Must return the persisted `status`, `statusReason`, `memberAccountsEnrolled` (required boolean), `lastUpdatedTimestamp` and `numberOfMemberAccountsOptedIn` computed from the stored enrollment record. Before any UpdateEnrollmentStatus call it must report the documented default (`Inactive`, `memberAccountsEnrolled=false`), NOT a canned `Active`.
3. **PutRecommendationPreferences** — input `resourceType` (required), `scope` (Scope{name,value}), `enhancedInfrastructureMetrics`, `inferredWorkloadTypes`, `externalMetricsPreference`, `lookBackPeriod`, `utilizationPreferences`, `preferredResources`, `savingsEstimationMode`. Empty output. Must create-or-UPDATE (upsert) one stored preference record keyed by (resourceType, scope), merging the supplied fields into any existing record for that key. Reject `externalMetricsPreference` for any resourceType other than `Ec2Instance`, and reject a `preferredResources` entry with neither `includeList` nor `excludeList`, with `InvalidParameterValueException`.
4. **GetRecommendationPreferences** — input `resourceType` (required), `scope`, `nextToken`, `maxResults` (0..1000); paginated with `items=recommendationPreferencesDetails`. Must return the records actually written by step 3, filtered by `resourceType` and, when supplied, by `scope`; each `RecommendationPreferencesDetail` must carry the stored `scope`, `resourceType` and preference fields. Honour `maxResults` and round-trip an opaque `nextToken` so a second page returns the remainder and the final page omits the token.
5. **GetEffectiveRecommendationPreferences** — input `resourceArn` (required). Must RESOLVE the effective preference for that ARN from stored records by precedence (resource-level scope matching the ARN > account-level > organization-level > service default), returning `enhancedInfrastructureMetrics`, `externalMetricsPreference`, `lookBackPeriod` (default `DAYS_14` when nothing is set), `utilizationPreferences` and `preferredResources`. Not a hardcoded blob.
6. **DeleteRecommendationPreferences** — input `resourceType` (required), `scope`, `recommendationPreferenceNames` (required list). Empty output. Must remove ONLY the named preference fields from the record for that (resourceType, scope) key, deleting the record once no preference fields remain, so that a following GetRecommendationPreferences no longer reports them and GetEffectiveRecommendationPreferences falls back to the next scope / default.

Cross-cutting for this slice: when the persisted enrollment status is not `Active`, the preference operations that model `OptInRequiredException` (HTTP 403) must return it rather than succeeding — that is the one behaviour that ties the two halves of the workflow together and proves state is shared.

NEVER STUBS: each of the six must compute its response from the persisted store reached via `crate::state::service_state(&*self.engine)`. No canned responses, no echoing the input back without storing it, no generated identifiers that are never persisted. A write followed by a read must return what was written.

## State Model

In `src/state.rs` (`ComAmazonawsComputeoptimizerComputeoptimizerserviceState`):

- `enrollment: Enrollment` — single per-account record:
  - `status: Status` (default `Inactive`)
  - `status_reason: Option<String>`
  - `member_accounts_enrolled: bool` (default false; set from `includeMemberAccounts`)
  - `last_updated_timestamp: Option<DateTime>`
  - `number_of_member_accounts_opted_in: Option<i32>`
- `preferences: BTreeMap<PreferenceKey, PreferenceRecord>` where
  - `PreferenceKey { resource_type: ResourceType, scope_name: Option<ScopeName>, scope_value: Option<String> }` (a `None` scope means the account-level default)
  - `PreferenceRecord { scope: Option<Scope>, resource_type: ResourceType, enhanced_infrastructure_metrics: Option<EnhancedInfrastructureMetrics>, inferred_workload_types: Option<InferredWorkloadTypesPreference>, external_metrics_preference: Option<ExternalMetricsPreference>, look_back_period: Option<LookBackPeriodPreference>, utilization_preferences: Vec<UtilizationPreference>, preferred_resources: Vec<PreferredResource>, savings_estimation_mode: Option<SavingsEstimationMode> }`
- Pagination tokens for GetRecommendationPreferences: an opaque token encoding the index/key offset into the ordered `preferences` map (stable ordering comes from the BTreeMap key).

Out of scope for this first slice (they stay 501 and will be filed as individual gap issues): all `Get*Recommendations` / `Get*ProjectedMetrics` / `GetRecommendationSummaries` / `GetEnrollmentStatusesForOrganization` operations, and the `Export*` + `DescribeRecommendationExportJobs` family. The natural SECOND slice is ExportEC2InstanceRecommendations -> DescribeRecommendationExportJobs backed by a stored `export_jobs: BTreeMap<String, RecommendationExportJob>`.

## Evidence

`services/service-com-amazonaws-computeoptimizer-computeoptimizerservice/OPERATIONS.md` lists all 28 operations with their input/output members and errors; the field names, requiredness, defaults, enum values, `maxResults` ranges (0..1000) and `@paginated(inputToken=nextToken, outputToken=nextToken, items=recommendationPreferencesDetails)` trait quoted above are taken verbatim from it (UpdateEnrollmentStatus/GetEnrollmentStatus lines 130-141 and 194-198; Put/Get/DeleteRecommendationPreferences lines 27-31, 174-179, 188-192; GetEffectiveRecommendationPreferences lines 124-128). `README.md` documents the `awsJson1_0` + `X-Amz-Target` routing and that every generated stub answers HTTP 501 until written. `src/ops/put_recommendation_preferences.rs` is representative: it carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header (line 4) and its `handle` body ends in `Err(crate::scaffolded::Scaffolded::not_implemented("PutRecommendationPreferences"))` (line 226), binding `input`/`ctx`/`engine` only to silence unused warnings — nothing is persisted. The same marker is present in all 28 op files. The documented defaults cited (enrollment starts `Inactive`, `lookBackPeriod` defaults to `DAYS_14` when unspecified, `externalMetricsPreference` is settable only for `Ec2Instance`, deleting a preference is per-name via `recommendationPreferenceNames`) come from the model's own operation/member documentation reproduced in the stub headers (e.g. lines 63-111 of that file). No tests assert any of this today: `tests/` holds only `state_roundtrip.rs`, a compiled example of the state API.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI client at it with service `compute-optimizer`.
2. Call GetEnrollmentStatus: expect HTTP 200 with the documented default `status=Inactive`, `memberAccountsEnrolled=false`. Actual: HTTP 501 NotImplemented.
3. Call UpdateEnrollmentStatus with `status=Active`, `includeMemberAccounts=true`: expect 200 echoing the stored `status=Active`; then call GetEnrollmentStatus again and expect `status=Active`, `memberAccountsEnrolled=true` and a non-null `lastUpdatedTimestamp`.
4. Call UpdateEnrollmentStatus with `status=Pending`: expect HTTP 400 `InvalidParameterValueException` (not a success, not 501).
5. Call PutRecommendationPreferences with `resourceType=Ec2Instance`, `scope={name: AccountId, value: <account>}`, `enhancedInfrastructureMetrics=Active`, `lookBackPeriod=DAYS_32`: expect 200 and an empty body.
6. Call GetRecommendationPreferences with `resourceType=Ec2Instance`: expect 200 and one `recommendationPreferencesDetails` entry carrying exactly the scope and preference values just written.
7. Write a second preference for `resourceType=Ec2Instance` at `scope={name: ResourceArn, value: arn:aws:ec2:us-east-1:111122223333:instance/i-1234567890abcdef0}` with `enhancedInfrastructureMetrics=Inactive`, then call GetRecommendationPreferences with `maxResults=1`: expect one entry plus a `nextToken`, and the remaining entry (and no token) on the follow-up page.
8. Call GetEffectiveRecommendationPreferences with `resourceArn=arn:aws:ec2:us-east-1:111122223333:instance/i-1234567890abcdef0`: expect the resource-level record to win (`enhancedInfrastructureMetrics=Inactive`) with `lookBackPeriod=DAYS_32` inherited from the account scope.
9. Call DeleteRecommendationPreferences with `resourceType=Ec2Instance`, the ResourceArn scope and `recommendationPreferenceNames=[EnhancedInfrastructureMetrics]`: expect 200, then GetRecommendationPreferences must no longer report that field and GetEffectiveRecommendationPreferences for the same ARN must fall back to the account-level `Active`.
10. Call UpdateEnrollmentStatus with `status=Inactive`, then PutRecommendationPreferences again: expect HTTP 403 `OptInRequiredException`.
11. Restart nothing; repeat GetRecommendationPreferences — the surviving records must still be returned from the shared engine store, proving state is not per-request.
