Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 768bbe4b7ea7973f1ee2efc8bf0f22146be54ed5

# GetPersonalizedRanking renormalizes scores over the surviving subset, so an item's score depends on the rest of inputList and on filterArn

_Filed: 2026-10-24T02:09:25Z_
_Operation: GetPersonalizedRanking | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `768bbe4b7ea7973f1ee2efc8bf0f22146be54ed5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: a `PredictedItem$score` in `personalizedRanking` is the model's certainty for the (campaign, userId, context, item) tuple — the per-item prediction the PERSONALIZED_RANKING solution version produces. Re-ranking the same user over a larger or smaller `inputList`, or applying a filter that removes some of the list, must not change the score reported for an item that appears in both responses; nor may the emulator invent certainty for a catalog whose entries carry no popularity.

Actual: the handler collects the raw relevance scores of exactly the entries that (a) were found in the campaign's stored Items catalog and (b) survived `filterArn`, and then divides each by the sum of that subset (`scoring::normalize`). Four observable consequences:
1. `inputList` composition changes a score: ranking [A,B] reports A at ~0.5 and ranking [A,B,C,...,Z] reports A at a far smaller value, for the same campaign/userId/context. A single-entry `inputList` always reports `score: 1.0`.
2. `filterArn` changes a score: a filter that removes half the list shrinks the denominator, so the surviving items' scores rise even though the per-item prediction for that user is unchanged.
3. Entries the catalog does not hold are appended with `score: 0.0` and are excluded from the denominator, so whether the response's scores sum to 1.0 at all depends on how many of the caller's ids were unknown — the sum is 1.0 over the known subset only, and 0.0 when every id is unknown.
4. When every surviving entry's stored `base_score` is 0, `scoring::normalize` spreads 1.0 evenly (1/N per item) instead of reporting no certainty, so a catalog with no popularity still produces confident-looking scores.

The fix is to make the denominator independent of the request's `inputList` composition and `filterArn` (score/normalize over the full scored catalog, then report the requested items), as the sibling issues for GetRecommendations and GetActionRecommendations ask for the same operations' denominators. The descending ORDER the handler produces is correct and is not what this issue is about.

## Evidence

Spec: `PredictedItem$score` in the Smithy model — "A numeric representation of the model's certainty that the item will be the next user selection. For more information on scoring logic, see how-scores-work" — a per-item model certainty cannot be a function of which other ids the caller happened to put in `inputList` or of which filter was applied; `GetPersonalizedRankingRequest$filterArn` is documented as a filter that "include[s] items or exclude[s] items from recommendations", i.e. it changes WHICH items come back, not what each one scores. The same catalog-relative-denominator expectation is already recorded for this service's other two operations (open issues 20261024T002543Z-personalize-runtime-getrecommendations-wrong-behavior and ...-getactionrecommendations-wrong-behavior); GetPersonalizedRanking carries the identical defect and has no issue of this class.
Handler: `services/service-com-amazonaws-personalizeruntime-amazonpersonalizeruntime/src/ops/get_personalized_ranking.rs` lines 236-278 — entries missing from the catalog are pushed to `appended`, filter-removed entries are `continue`d, and only the remaining `ranked` scores are passed to `scoring::normalize`; lines 297-303 append the unknown ids with `score: Some(0.0)` after normalization. `src/state/scoring.rs` lines 102-117 divide by the sum of exactly those retained scores and spread `1.0 / len` when the total is 0.
Tests: `tests/all/inference.rs` `personalized_ranking_is_a_permutation_of_the_input_list` (line 891) and `personalized_ranking_drops_the_items_its_filter_excludes` (line 975) call `support::assert_descending_and_normalized`, which asserts the per-response sum-to-1.0 behavior; no test compares the score reported for one item across two different `inputList` sizes or across a filtered vs unfiltered call, so the gap is uncovered and is in fact codified by the current assertions.

## Steps to Reproduce

1. Make a PERSONALIZED_RANKING campaign with a stored Items catalog of ~40 entries visible to the runtime registry (today only the in-process `state::register_campaign` seam can do this — see the open issue about no wire path registering a campaign).
2. Call GetPersonalizedRanking with that campaignArn, userId=user-1 and inputList=[item-1, item-2]. Note the score reported for item-1. Observed: roughly 0.5 (and exactly 1.0 if inputList holds a single id). Expected: item-1's own prediction score for user-1.
3. Call GetPersonalizedRanking with the same campaignArn/userId and inputList=[item-1, item-2, ..., item-20], and compare item-1's score with step 2. Observed: a different, much smaller score for the same item; each response's scores sum to exactly 1.0. Expected: the same score for item-1 in both responses.
4. Repeat the 20-id call with a filterArn whose expression removes about half the listed items. Observed: the surviving items' scores are scaled up so the response again sums to 1.0. Expected: the surviving items keep the scores they had without the filter.
5. Call GetPersonalizedRanking with an inputList of ids the campaign's catalog does not hold at all. Observed: every item is returned with score 0.0 (sum 0.0), while a mixed list sums to 1.0 over only the known ids. Expected: a consistent scoring scheme that does not depend on how many ids were unknown.
6. Register a campaign whose catalog entries all carry base_score 0 and rank any two of its items. Observed: each reported score is 1/N. Expected: no invented certainty.
