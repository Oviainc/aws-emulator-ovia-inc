Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: aeaddd52642475574d853bf8e089296dd7f2f86c

# GetRecommendations scores are renormalized over the returned slice, and the input the recipe type does not use still changes the ordering

_Filed: 2026-10-24T00:25:43Z_
_Operation: GetRecommendations | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `aeaddd52642475574d853bf8e089296dd7f2f86c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Two divergences in how GetRecommendations computes the response from the stored catalog.

Cause 1 of 2 — a returned item's `score` depends on `numResults`.
AWS: Amazon Personalize scores a user against the WHOLE catalog (the scoring logic `PredictedItem$score` points at, "For more information on scoring logic, see how-scores-work"): the per-user scores are normalized across all items in the catalog, so a response that returns a subset carries scores that sum to well UNDER 1, and the score reported for one item is a property of (campaign, userId, context, item) — asking for 1, 5 or 25 results reports the SAME score for the same item, and a `numResults=1` response returns a small fraction, not 1.0. The response approaches a sum of 1 only when the entire catalog is returned.
Emulator: the handler collects only the CHOSEN slots' raw scores and divides them by their own sum, so every response sums to exactly 1.0 whatever the catalog size: `numResults=1` always reports `score: 1.0`, and the top item's score differs between a `numResults=5` and a `numResults=10` call on an otherwise identical request. The fix is to make the denominator catalog-relative (normalize over the full scored candidate set, then take the top `numResults`) rather than per-response.
Note: the fixed bootstrap issue (20261022T001755Z) asked for scores "normalised to sum to ~1.0 across the returned itemList"; that instruction is what produced the per-response denominator and is the part that needs refining — this issue is about the denominator, not about the descending order, which is correct.

Cause 2 of 2 — the personalization input the resolved recipe type does not use still perturbs the result.
AWS (GetRecommendations doc, verbatim): "USER_PERSONALIZATION - userId required, itemId not used RELATED_ITEMS - itemId required, userId not used". A spurious `itemId` sent to a USER_PERSONALIZATION campaign, or a spurious `userId` sent to a RELATED_ITEMS campaign, is ignored, so the returned items and their order are unchanged.
Emulator: both members are fed unconditionally into the relevance seed for every recipe type, so adding or changing the unused member re-shuffles the whole response: two RELATED_ITEMS calls with the same `itemId` but different `userId` return different item sets/orderings, and the same for USER_PERSONALIZATION with a varying `itemId`.

## Evidence

Spec: `GetRecommendations` doc in the Smithy model ("USER_PERSONALIZATION - userId required, itemId not used RELATED_ITEMS - itemId required, userId not used") and `PredictedItem$score` ("A numeric representation of the model's certainty... For more information on scoring logic, see how-scores-work") — catalog-wide score normalization is well-established Personalize behavior (subset responses sum far below 1).
Handler: `services/service-com-amazonaws-personalizeruntime-amazonpersonalizeruntime/src/ops/get_recommendations.rs` lines 313-317 build `scores` from the chosen slots only and call `scoring::normalize`; `src/state/scoring.rs` lines 102-117 divide by the sum of exactly those scores (and spread 1.0 evenly when all are 0). Cause 2: `get_recommendations.rs` lines 254-279 pass `[&resolved.scope, user_id, item_id, &context_seed, &item.item_id]` into `scoring::relevance` for every recipe type; only the "never recommend the queried item" rule at lines 259-261 is recipe-type aware.
Tests: `tests/all/inference.rs` `assert_descending_and_normalized` asserts the sum-to-1.0-per-response behavior (it is codified, not accidental), and no test varies `numResults` to check score stability. `related_items_recommends_from_the_catalog_but_never_the_queried_item` (line 846) never sends a `userId` with a RELATED_ITEMS campaign, and `recommendations_come_from_the_stored_catalog_in_descending_score_order` never sends an `itemId` with a USER_PERSONALIZATION campaign, so cause 2 is untested.

## Steps to Reproduce

1. Make a USER_PERSONALIZATION campaign with a catalog of ~40 items visible to the runtime registry (today only the in-process `state::register_campaign` seam can do this — see the open issue about no wire path registering a campaign).
2. Call GetRecommendations with that campaignArn, userId=user-1, numResults=1. Observed: the single item reports score 1.0. Expected: a small fraction of 1 (catalog-wide normalization).
3. Call GetRecommendations with the same campaignArn/userId and numResults=5, then numResults=10, and compare the score reported for an item present in both responses. Observed: two different scores for the same item, and each response's scores sum to exactly 1.0. Expected: the same score in both responses, with the sums below 1.
4. Call GetRecommendations against the same USER_PERSONALIZATION campaign twice with userId=user-1, numResults=10, once with itemId=item-3 and once with no itemId. Observed: different item lists/orders. Expected: identical responses (itemId is not used for USER_PERSONALIZATION).
5. Call GetRecommendations against a RELATED_ITEMS campaign twice with itemId=item-3, numResults=10, once with userId=user-1 and once with userId=user-2. Observed: different item lists/orders. Expected: identical responses (userId is not used for RELATED_ITEMS).
