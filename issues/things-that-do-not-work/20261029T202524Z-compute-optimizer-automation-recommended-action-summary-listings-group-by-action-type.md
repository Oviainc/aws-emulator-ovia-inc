Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: compute-optimizer-automation.list-recommended-actions-and-summaries-structure compute-optimizer-automation.list-automation-rule-preview-structure
Issue-Shape-Id: com.amazonaws.computeoptimizerautomation#ComputeOptimizerAutomationService
Issue-Key: compute-optimizer-automation.list-recommended-actions-and-summaries-structure|run#11|AssertionError|assert len(aggregate_rows) == 1, (
Issue-Key: compute-optimizer-automation.list-automation-rule-preview-structure|run#11|AssertionError|assert len(aggregate_rows) == 1, (
Issue-Commit: 6bd726e14a5c3a202f9e89e1e854b8dfcc8d4492

# Recommended-action summary listings group by action type and never emit the account-wide ALL_Recommended_Action_Summary row, so summary totals do not reconcile with the action/preview listings

> _Note: this issue was identified against the code base at commit `6bd726e14a5c3a202f9e89e1e854b8dfcc8d4492`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListRecommendedActionSummaries

## Description
Two summary-listing operations of Compute Optimizer Automation build their summary rows by grouping on the recommended-action TYPE instead of returning the account-wide aggregate row that real AWS returns for the requested filter. Both are the same shape of defect but live in two different handlers, so both must be closed. 2 causes.

Cause 1 of 2: ListRecommendedActionSummaries omits the ALL_Recommended_Action_Summary aggregate row.
AWS: recorded behavior only — the Smithy model (RecommendedActionSummary.key, target String, required, documented only as "The grouping key used to categorize the recommended actions in this summary"; list RecommendedActionSummaries; totals in RecommendedActionTotal) does not enumerate the key values, so the requirement comes from the recording: ListRecommendedActionSummaries returns, for the requested filter, exactly ONE row whose `key` is the fixed literal string `ALL_Recommended_Action_Summary`, carrying the account-wide `total.recommendedActionCount`. On an account with nothing to recommend the row is still present with `recommendedActionCount` 0. The count is a value from the emulator's own state (it must equal the number of actions ListRecommendedActions returns for the same, unpaginated filter); the key is fixed text and is not derived from the request.
Emulator: ListRecommendedActionSummaries returns one row per recommended-action type present instead — the replay received `recommendedActionSummaries` with keys `['UpgradeEbsVolumeType', 'SnapshotAndDeleteUnattachedEbsVolume']` and zero rows keyed `ALL_Recommended_Action_Summary`, so the count of aggregate rows was 0 where exactly 1 is required. The HTTP status was 200 and the response was otherwise well-typed; only the grouping key and therefore the row set diverge.
Why: a client that reads the account-wide total — the documented way to learn how many recommended actions an account has without paging the full action list — finds no `ALL_Recommended_Action_Summary` row at all and either throws (key lookup on a missing row) or silently reads 0. A client that instead sums the rows it does get double-counts relative to the aggregate contract, and any per-type consumer sees type keys where AWS places one aggregate key, so the two independent reads (summaries vs ListRecommendedActions) never reconcile.
Fix: the ListRecommendedActionSummaries handler's summary-aggregation step — the code that derives summary rows and their `key` from stored recommended actions. It must emit the single fixed-key aggregate row with the state-derived count rather than a row per action type.
Traces: compute-optimizer-automation.list-recommended-actions-and-summaries-structure.

Cause 2 of 2: ListAutomationRulePreviewSummaries omits the ALL_Recommended_Action_Summary aggregate row.
AWS: recorded behavior only — the model (PreviewResultSummary.key, target String, required, "The key identifier for this preview result summary"; list PreviewResultSummaries; totals in RulePreviewTotal) likewise does not enumerate key values. The recording requires that for a given ruleType/recommendedActionTypes filter, ListAutomationRulePreviewSummaries returns exactly ONE row keyed with the fixed literal `ALL_Recommended_Action_Summary`, whose `total.recommendedActionCount` is the account-wide count for that filter — present with 0 on an account with no snapshot-eligible volumes — and whose summed counts equal the number of `previewResults` ListAutomationRulePreview returns for the identical filter when neither response is truncated.
Emulator: for ruleType=AccountRule, recommendedActionTypes=[SnapshotAndDeleteUnattachedEbsVolume], maxResults=100 the emulator answered 200 with `previewResultSummaries` keyed `['SnapshotAndDeleteUnattachedEbsVolume']` — a per-action-type row — and no `ALL_Recommended_Action_Summary` row, so the aggregate-row count was 0 where exactly 1 is required.
Why: the preview flow exists so a caller can size a rule's impact before activating it. Without the aggregate row the caller cannot read the filter's account-wide total, and the cross-check it is meant to perform (summaries total vs previewResults length) cannot be made; a caller keying on the fixed aggregate key gets a missing-key failure on every preview.
Fix: the ListAutomationRulePreviewSummaries handler's preview-summary aggregation — the code that derives preview summary rows and their `key`. The same fixed-key aggregate row must be produced per filter, with the count taken from the matching preview results in state.
Traces: compute-optimizer-automation.list-automation-rule-preview-structure.

Note: neither cause is covered by any open issue for this service; the nearest open issues (20261023T134046Z-compute-optimizer-automation-error-messages-not-built-from.md, 20261029T182545Z-compute-optimizer-automation-createautomationrule-validation--r3.md) concern error wording and create-time validation, not summary-row construction.

## Evidence
Cause 1 — compute-optimizer-automation.list-recommended-actions-and-summaries-structure:
  Request: ListRecommendedActionSummaries(maxResults=25) on an enrolled account (base compute-optimizer enrollment Active, automation enrollment Active).
  Expected (AWS, recorded): HTTP 200, recommendedActionSummaries = exactly one row with key == "ALL_Recommended_Action_Summary" (fixed text) and total.recommendedActionCount == number of actions ListRecommendedActions returns for the unpaginated listing (value from state).
  Actual (emulator): HTTP 200, recommendedActionSummaries keys == ['UpgradeEbsVolumeType', 'SnapshotAndDeleteUnattachedEbsVolume']; zero ALL_Recommended_Action_Summary rows.
  Assertion: AssertionError: expected exactly one ALL_Recommended_Action_Summary row, got keys ['UpgradeEbsVolumeType', 'SnapshotAndDeleteUnattachedEbsVolume']

Cause 2 — compute-optimizer-automation.list-automation-rule-preview-structure:
  Request: ListAutomationRulePreviewSummaries(ruleType="AccountRule", recommendedActionTypes=["SnapshotAndDeleteUnattachedEbsVolume"], maxResults=100), after a successful ListAutomationRulePreview with the same filter.
  Expected (AWS, recorded): HTTP 200, previewResultSummaries = exactly one row with key == "ALL_Recommended_Action_Summary" and total.recommendedActionCount == len(previewResults) for the identical filter when untruncated.
  Actual (emulator): HTTP 200, previewResultSummaries keys == ['SnapshotAndDeleteUnattachedEbsVolume']; zero ALL_Recommended_Action_Summary rows.
  Assertion: AssertionError: expected exactly one ALL_Recommended_Action_Summary row, got keys ['SnapshotAndDeleteUnattachedEbsVolume']

In both responses the status (200), member names and member types are correct; the divergence is solely the set of summary rows and the `key` value used for them.

## Steps to Reproduce
1. Point a botocore client at the emulator and ensure enrollment: compute-optimizer UpdateEnrollmentStatus(status="Active", includeMemberAccounts=False) until GetEnrollmentStatus reports Active, then compute-optimizer-automation UpdateEnrollmentConfiguration(status="Active") until GetEnrollmentConfiguration reports Active.
2. Cause 1: call compute-optimizer-automation ListRecommendedActions(maxResults=25) and ListRecommendedActionSummaries(maxResults=25). Observe that recommendedActionSummaries contains one row per recommended-action type and no row whose key is "ALL_Recommended_Action_Summary"; the account-wide total cannot be read, and no row's count equals len(recommendedActions).
3. Cause 2: call ListAutomationRulePreview(ruleType="AccountRule", recommendedActionTypes=["SnapshotAndDeleteUnattachedEbsVolume"], maxResults=100) then ListAutomationRulePreviewSummaries with the identical filter. Observe that previewResultSummaries is keyed by the action type rather than containing the single "ALL_Recommended_Action_Summary" aggregate row whose total.recommendedActionCount equals len(previewResults).
4. Replay the two checks listed in trace_ids to confirm a fix.
