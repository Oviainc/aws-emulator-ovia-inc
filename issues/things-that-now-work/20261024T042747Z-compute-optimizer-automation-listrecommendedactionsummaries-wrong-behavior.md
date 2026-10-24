Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: be3ab9eb2c0ab303301f9b8637a58c2763937db6

# ListRecommendedActionSummaries returns one invented aggregate key instead of grouped summaries

_Filed: 2026-10-24T04:27:47Z_
_Operation: ListRecommendedActionSummaries | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `be3ab9eb2c0ab303301f9b8637a58c2763937db6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The handler always answers with EXACTLY ONE `RecommendedActionSummary` whose `key` is the hardcoded literal `"ALL_Recommended_Action_Summary"` and whose `total` is the sum over every matching action. No grouping is performed, so the paginated member (`maxResults` 1..100, `nextToken`) can never carry more than one row and a client reading `key` off the wire sees a string the service does not emit.

Expected: the response is a list of GROUPED summaries — `RecommendedActionSummary$key` is documented as "The grouping key used to categorize the recommended actions in this summary" and the shape itself as "Summary information about recommended actions, grouped by specific criteria with totals and counts". The natural (and, in this same service, already-implemented) grouping is per `recommendedActionType`: `ListAutomationRulePreviewSummaries` groups its `PreviewResultSummary` rows by `recommendedActionType` with per-group counts and savings. The operation being `@paginated` over `recommendedActionSummaries` with a 100-row page bound only makes sense for a multi-row, grouped result.

Actual: one row, invented key, one global total; per-action-type counts and savings are unavailable, and the grouping dimension a client asked about is lost.

## Evidence

Spec/model: `RecommendedActionSummary` = `{key (required, "The grouping key used to categorize the recommended actions in this summary"), total (required RecommendedActionTotal)}`, shape doc "Summary information about recommended actions, grouped by specific criteria with totals and counts" (smithy_query shape=RecommendedActionSummary); `ListRecommendedActionSummaries` is `@paginated(items=recommendedActionSummaries)` with `maxResults` range 1..100 (smithy_query operation=ListRecommendedActionSummaries).

Handler: src/ops/list_recommended_action_summaries.rs:260-271 builds `vec![RecommendedActionSummary { key: ALL_RECOMMENDED_ACTION_SUMMARY_KEY, total: aggregate.into_total() }]`; the key constant is declared at line 312 with a comment conceding the model enumerates no key set. Compare src/ops/list_automation_rule_preview_summaries.rs:306-351, which groups the same inventory by `recommended_action_type` into one `PreviewResultSummary` per type.

Test coverage: tests/all/recommended_action_summaries.rs:174 (`the_account_wide_aggregate_totals_every_stored_action`) and :669 (`one_page_holds_the_aggregate_and_a_foreign_token_is_refused`) assert the single-row aggregate, i.e. the current shape is pinned, not merely untested; no test asserts a per-action-type grouping.

## Steps to Reproduce

1. Enroll the caller: `UpdateEnrollmentConfiguration` with `status=Active`.
2. Have an inventory holding recommended actions of two different `recommendedActionType`s (e.g. `UpgradeEbsVolumeType` and `SnapshotAndDeleteUnattachedEbsVolume`), each with `estimatedMonthlySavings`.
3. Call `ListRecommendedActionSummaries` with no filters.
4. Expected: one summary per grouping key (e.g. one per `recommendedActionType`), each with its own `recommendedActionCount` and `estimatedMonthlySavings`.
5. Actual: a single summary with `key = "ALL_Recommended_Action_Summary"` whose total covers both types, and `nextToken` is never issued whatever `maxResults` is sent.
