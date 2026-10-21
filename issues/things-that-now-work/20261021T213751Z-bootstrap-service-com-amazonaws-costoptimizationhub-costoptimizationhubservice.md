Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: 09acb2538adf9fea9bce04c8d04105f42be236df

# Bootstrap the Cost Optimization Hub emulator

_Filed: 2026-10-21T21:37:51Z_
_Operation: service-com-amazonaws-costoptimizationhub-costoptimizationhubservice | Class: stub_

> _Note: this issue was identified against the code base at commit `09acb2538adf9fea9bce04c8d04105f42be236df`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every handler under `services/service-com-amazonaws-costoptimizationhub-costoptimizationhubservice/src/ops/` is an unmodified scaffold: it carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))`, i.e. HTTP 501, and the service holds no state. Nothing works end-to-end.

Implement a FIRST SLICE of 6 operations — the enrollment -> preferences -> recommendations core loop — statefully, so a realistic client script succeeds. Wire protocol is `awsJson1_0`: `POST /` with `X-Amz-Target: CostOptimizationHubService.<OperationName>` (README/OPERATIONS.md).

1. **UpdateEnrollmentStatus** — input `status` (EnrollmentStatus: Active|Inactive, required), `includeMemberAccounts` (boolean). Must PERSIST the enrollment status for the caller's account (account id taken from the request credentials/context, defaulting to the emulator's 12-digit account) together with a `createdTimestamp` set on first enrollment and a `lastUpdatedTimestamp` refreshed on every call, and persist `includeMemberAccounts`. Output `status` must be the newly stored value (echoing the input is only correct because it was stored first — the store must actually change).
2. **ListEnrollmentStatuses** — input `includeOrganizationInfo` (boolean, default false, required), `accountId` (pattern `^[0-9]{12}$`), `maxResults` (0..100), `nextToken`. Must return `items` (list<AccountEnrollmentStatus>: `accountId`, `status`, `createdTimestamp`, `lastUpdatedTimestamp`) read from the store — so an account that has never called UpdateEnrollmentStatus is absent (or `Inactive`), and one that just enrolled appears as `Active`. `includeMemberAccounts` in the output must come from the stored preference, not a constant. Honour `accountId` as a filter and `maxResults`/`nextToken` as real pagination over the stored list.
3. **UpdatePreferences** — input `savingsEstimationMode` (AfterDiscounts|BeforeDiscounts), `memberAccountDiscountVisibility` (All|None), `preferredCommitment` (PreferredCommitment{term, paymentOption}). Must merge the supplied members into the persisted per-account preferences (omitted members keep their previous value) and return the resulting stored record.
4. **GetPreferences** — no input. Must return the preferences PERSISTED by UpdatePreferences (`savingsEstimationMode`, `memberAccountDiscountVisibility`, `preferredCommitment`), not a canned record. Before any UpdatePreferences call it returns the service defaults; after one it must reflect exactly what was written.
5. **ListRecommendations** — input `filter` (Filter), `orderBy` (OrderBy), `includeAllRecommendations` (boolean, default false, required), `maxResults` (0..100), `nextToken`. Must return `items` (list<Recommendation>) from a seeded/in-store recommendation set belonging to the enrolled account, applying at least `filter.recommendationIds`, `filter.accountIds`, `filter.regions`, `filter.resourceTypes`, `filter.actionTypes`, `filter.implementationEfforts`, `filter.restartNeeded`, `filter.rollbackPossible`, and real `maxResults` + opaque `nextToken` pagination (the token must resume where the previous page ended, and the last page must omit `nextToken`). Each `Recommendation` must carry a stable `recommendationId`.
6. **GetRecommendation** — input `recommendationId` (required). Must look the id up in the SAME store ListRecommendations reads, and return the detail view (`recommendationId`, `resourceId`, `resourceArn`, `accountId`, `region`, `currentResourceType`, `recommendedResourceType`, `actionType`, `implementationEffort`, `estimatedMonthlySavings`, `estimatedMonthlyCost`, `estimatedSavingsPercentage`, `currencyCode`, `source`, `lastRefreshTimestamp`, `restartNeeded`, `rollbackPossible`, `tags`) with values consistent with the list entry for that id. An unknown id must return `ResourceNotFoundException` (HTTP 404), NOT 501 and NOT an empty 200.

Every one of these six must compute its response from persisted state. No canned bodies, no echoing the input without storing it, no random ids that are never stored.

## State Model

In `src/state.rs` (`ComAmazonawsCostoptimizationhubCostoptimizationhubserviceState`, reached via `crate::state::service_state(&*self.engine)`):

- `enrollments: HashMap<AccountId /*12-digit string*/, Enrollment>` where `Enrollment { account_id: String, status: EnrollmentStatus /*Active|Inactive*/, include_member_accounts: bool, created_timestamp: DateTime, last_updated_timestamp: DateTime }`.
- `preferences: HashMap<AccountId, Preferences>` where `Preferences { savings_estimation_mode: Option<SavingsEstimationMode>, member_account_discount_visibility: Option<MemberAccountDiscountVisibility>, preferred_commitment: Option<PreferredCommitment { term: Option<Term>, payment_option: Option<PaymentOption> }> }`.
- `recommendations: HashMap<RecommendationId /*String*/, StoredRecommendation>` plus `recommendation_order: Vec<RecommendationId>` for stable list/pagination ordering. `StoredRecommendation { recommendation_id, account_id, region, resource_id, resource_arn, current_resource_type, recommended_resource_type, current_resource_details, recommended_resource_details, current_resource_summary, recommended_resource_summary, action_type, implementation_effort, restart_needed, rollback_possible, estimated_monthly_savings, estimated_monthly_cost, estimated_savings_percentage, estimated_savings_over_cost_calculation_lookback_period, recommendation_lookback_period_in_days, cost_calculation_lookback_period_in_days, currency_code, source, last_refresh_timestamp, tags: Vec<Tag> }`.
- `list_tokens: HashMap<String /*nextToken*/, usize /*offset into the filtered order*/>` (or an offset encoded in the token) so pagination resumes deterministically.

The remaining operations (ListRecommendationSummaries, ListEfficiencyMetrics) stay stubbed at 501 for now and will be filed as individual gap issues.

## Evidence

`services/service-com-amazonaws-costoptimizationhub-costoptimizationhubservice/OPERATIONS.md` lists all 8 bound operations with their input/output members and errors (awsJson1_0, target prefix `CostOptimizationHubService.`); the field names and constraints quoted above (e.g. `recommendationId` required on GetRecommendation with `ResourceNotFoundException (HTTP 404)`, `maxResults` range 0..100, `@paginated(inputToken=nextToken, outputToken=nextToken, items=items)` on ListRecommendations/ListEnrollmentStatuses) come from it. Shape members for `AccountEnrollmentStatus` (accountId/status/createdTimestamp/lastUpdatedTimestamp), `PreferredCommitment` (term/paymentOption), `Recommendation` and `Filter` come from `smithy_query query=shape`. `src/ops/get_preferences.rs` lines 4-6 carry the `SCAFFOLDED, NOT IMPLEMENTED` marker and line 163 returns `Scaffolded::not_implemented("GetPreferences")`; all 8 op files in `src/ops/` are identical scaffolds, so no behaviour exists and no test asserts any.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK client for `cost-optimization-hub` at it.
2. Call UpdateEnrollmentStatus{status:"Active", includeMemberAccounts:true} — expect 200 with status "Active" (currently 501).
3. Call ListEnrollmentStatuses{includeOrganizationInfo:false} — expect the caller's account listed with status "Active", a createdTimestamp and a lastUpdatedTimestamp, and includeMemberAccounts true.
4. Call UpdatePreferences{savingsEstimationMode:"BeforeDiscounts", memberAccountDiscountVisibility:"None", preferredCommitment:{term:"ThreeYears", paymentOption:"AllUpfront"}} — expect the stored record back.
5. Call GetPreferences — expect exactly the values written in the previous step (proving persistence, not an echo).
6. Call ListRecommendations{includeAllRecommendations:false, maxResults:1} — expect one item with a recommendationId and a nextToken; call again with that nextToken and expect the NEXT item, not the same one.
7. Call GetRecommendation{recommendationId:<id from the list>} — expect detail fields consistent with the list entry.
8. Call GetRecommendation{recommendationId:"does-not-exist"} — expect ResourceNotFoundException with HTTP 404.
