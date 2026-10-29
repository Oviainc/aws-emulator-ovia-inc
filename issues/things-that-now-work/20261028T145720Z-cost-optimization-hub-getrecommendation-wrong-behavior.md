Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 436fee4160e0c164196671a7c5bd1a9f84eddc25

# GetRecommendation 404s on ids ListRecommendations just returned to a management account

_Filed: 2026-10-28T14:57:20Z_
_Operation: GetRecommendation | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `436fee4160e0c164196671a7c5bd1a9f84eddc25`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`GetRecommendation` resolves the row it may answer with its own hand-rolled lookup — `store.visible_recommendations(caller_account_id())`, i.e. the caller's OWN account only — instead of the shared `recommendation_view` / `visible_recommendations_for` rule both list readers go through. `recommendation_view` widens the readable set to the accounts the request ADDRESSES (`addressed_accounts`), so a management account naming `filter.accountIds` is served another account's rows by `ListRecommendations` and `ListRecommendationSummaries`. Feeding one of those `recommendationId`s back into `GetRecommendation` answers 404 `ResourceNotFoundException`, because the id is not in the caller's own single-account set.

That contradicts this handler's own module doc, which states the lookup set IS the set `ListRecommendations` pages over, and it contradicts the modelled workflow ("To retrieve the recommendationId, use the ListRecommendations API"): the id the list hands out cannot be read back. The crate already owns the helper the sibling readers use, so the detail view is the one path that skips it.

## Evidence

This handler's own doc, src/ops/get_recommendation.rs:5-9: "Looks `recommendationId` up among the recommendations the calling account may read — **the same set `ListRecommendations` pages over**, so an account that is not enrolled, or whose analysis is not due yet, holds no recommendation at all — and answers with the detail view of that row, or `ResourceNotFoundException` when this account holds no such id."

But the code uses the single-account lookup, src/ops/get_recommendation.rs:211-214:
```
let row = store
    .visible_recommendations(crate::state::caller_account_id())
    .into_iter()
    .find(|row| row.recommendation_id == input.recommendation_id);
```
while the sibling reader goes through the shared rule, src/ops/list_recommendations.rs:209-212:
```
let view = state
    .lock()
    .recommendation_view(crate::state::caller_account_id(), input.filter.as_ref())
    .map_err(|_| not_an_organization_view())?;
```
and that shared rule widens the set past the caller's own account, src/state.rs:1123-1130:
```
let addressed = self.addressed_accounts(account_id, named, false);
...
rows: self
    .visible_recommendations_for(&addressed)
```
with `addressed_accounts` returning the NAMED accounts when `filter.accountIds` is populated (src/state.rs:1048-1053).

The wider set is exercised today: tests/all/account_scoped_reads.rs:314-360 asserts `ListRecommendations{"filter":{"accountIds":[MEMBER_ACTIVE]}}` answers `member_rows` to the management account — ids that `GetRecommendation` cannot resolve. The only `GetRecommendation`/list agreement test uses the caller's own rows (tests/all/cost_optimization_hub.rs:991-1039, `enrolled_router()`), so the divergence is uncovered.

## Steps to Reproduce

1. Build the management-account router with its store (tests/all/cost_optimization_hub.rs:70-79) and give a member account an enrollment plus catalog rows, as tests/all/account_scoped_reads.rs:285-288 does (`record_enrollment(&store, MEMBER_ACTIVE, "Active", true)` and `catalog_rows_for(&store, MEMBER_ACTIVE, 2)`).
2. Call `ListRecommendations{"includeAllRecommendations": true, "filter": {"accountIds": ["111122223333"]}}` — HTTP 200 with the member's rows and their `recommendationId`s.
3. Call `GetRecommendation{"recommendationId": "<one of those ids>"}` — HTTP 404 `ResourceNotFoundException`, although the list just returned the row to the same caller.
4. Expected: the detail view resolves the same rows the list reader selected for the caller (the shared `recommendation_view` / `visible_recommendations_for` rule), per src/ops/get_recommendation.rs:5-9.
