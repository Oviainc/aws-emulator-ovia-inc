Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 804429c0e5267423598a5b7dbc8d0525cc046b43

# The item prediction-score seed is composed twice: GetRecommendations and GetPersonalizedRanking each spell out the (scope, personalization, context, itemId) tuple scoring::relevance is called with

_Filed: 2026-10-28T02:35:59Z_
_Operation: GetRecommendations | Class: maintainability_

> _Note: this issue was identified against the code base at commit `804429c0e5267423598a5b7dbc8d0525cc046b43`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::scoring` owns ONE rule for what an item's prediction score is a function of, and states it in its own module doc: the stored `base_score` plus "the request's personalization inputs — `userId` / `itemId` and `context`". The helper `scoring::relevance(base_score, parts)` deliberately takes the seed `parts` from its caller, so the composition of that tuple — which parts, in which order — is the actual rule. That composition is written out TWICE in this crate's own handler code, once in `get_recommendations.rs` and once in `get_personalized_ranking.rs`, as the same four-element array `[<resolved resource scope>, <personalization input>, <context seed>, <item id>]`.

Because the two copies are per-operation, they can drift silently and no test can notice: the two operations serve DISJOINT campaign recipe types (`GetRecommendations` refuses a `PERSONALIZED_RANKING` campaign at `get_recommendations.rs:392-402`, `GetPersonalizedRanking` refuses everything else at `get_personalized_ranking.rs:197-206`), so no request can ever score the same (campaign, user, context, item) tuple through both paths, and every existing score assertion compares one operation's response against another response of the SAME operation. Reorder, add or drop a seed part in one handler and the other keeps scoring the identical request differently, with every suite still green. `get_action_recommendations.rs:210-213` is a third hand-written copy of the same composition, with the one member its operation does not model (`context`) left out.

The de-duplication is a single helper in `scoring` that takes the scope, the personalization input, the context seed and the entry id and builds the parts array once, called from all three handlers. The parts passed stay identical, so every score, every ordering and every response body is byte-for-byte unchanged and no existing test changes.

## Evidence

The rule, stated once in the helper's own module — `services/service-com-amazonaws-personalizeruntime-amazonpersonalizeruntime/src/state/scoring.rs:9-13`:
```
//! one stored catalog entry for one request must be a PURE FUNCTION of
//!
//! * the entry's stored `base_score` (its popularity in the stored catalog), and
//! * the request's personalization inputs — `userId` / `itemId` and `context`.
```
and `src/state/scoring.rs:68-76`, which hands the composition of that tuple to the caller:
```
/// The prediction score of a catalog entry whose stored popularity is `base_score`
/// for the request described by `parts`.
pub fn relevance(base_score: f64, parts: &[&str]) -> f64 {
```

Copy 1 — `src/ops/get_recommendations.rs:275-284`:
```
            .map(|(index, item)| {
                let score = scoring::relevance(
                    item.base_score,
                    &[
                        &resolved.scope,
                        personalization,
                        &context_seed,
                        &item.item_id,
                    ],
                );
```

Copy 2 — `src/ops/get_personalized_ranking.rs:242-252`:
```
            .map(|item| {
                scoring::relevance(
                    item.base_score,
                    &[
                        &campaign.arn,
                        &input.user_id,
                        &context_seed,
                        &item.item_id,
                    ],
                )
            })
```

The same composition a third time, minus the member that operation does not model — `src/ops/get_action_recommendations.rs:210-213`:
```
                let score = scoring::relevance(
                    action.base_score,
                    &[&campaign.arn, user_id, &action.action_id],
                );
```

Why drift cannot be caught today — the two Items copies are unreachable from one another: `src/ops/get_recommendations.rs:392-402` ("Campaign {arn} deploys a solution version trained with the {} recipe type; use the {} API." for `RecipeType::PersonalizedRanking`) and `src/ops/get_personalized_ranking.rs:197-206` ("the solution backing a GetPersonalizedRanking campaign must have been created using a recipe of type {}").

Test coverage that must stay unchanged by the de-duplication (all of it compares responses of one operation against other responses of that same operation, never a recomputed score): `tests/all/inference.rs:178` (`recommendations_come_from_the_stored_catalog_in_descending_score_order`), `tests/all/inference.rs:266` (`each_user_sees_an_ordering_of_its_own_over_the_same_catalog`), `tests/all/inference.rs:306` (`context_changes_the_ordering_and_an_equal_context_does_not`), `tests/all/inference.rs:902` (`personalized_ranking_is_a_permutation_of_the_input_list`), `tests/all/inference.rs:1190` (`an_item_score_does_not_change_with_the_requested_number_of_results`), `tests/all/score_denominator.rs:254` (`a_ranking_score_does_not_change_with_the_input_list_or_the_filter`, whose reference at `:263-273` is itself another `/personalize-ranking` response), and the tie-break suite `tests/all/tie_break_order.rs:1-16`.

## Steps to Reproduce

1. Open `src/state/scoring.rs:68-76`: `relevance` takes the seed `parts` from its caller, so the tuple composition — not the helper — is the rule that decides an item's score.
2. Grep the handlers for the call: `grep -n 'scoring::relevance' src/ops/*.rs` returns `get_action_recommendations.rs:210`, `get_personalized_ranking.rs:243` and `get_recommendations.rs:276` — three hand-written parts arrays, two of them (the Items ones) element-for-element the same rule.
3. Compare `src/ops/get_recommendations.rs:278-283` with `src/ops/get_personalized_ranking.rs:245-250`: both spell out `[<resource scope>, <personalization input>, <context seed>, <item id>]`.
4. Confirm the drift is invisible: edit one copy only (e.g. swap the context seed and the item id in `get_personalized_ranking.rs`) and run the crate's tests — every suite still passes, because `src/ops/get_recommendations.rs:392-402` and `src/ops/get_personalized_ranking.rs:197-206` keep the two operations on disjoint campaigns and no test recomputes a score outside the response it came from.
5. Fix: add one `scoring` helper that builds the seed from (scope, personalization input, context seed, entry id) and call it from all three handlers, passing the same parts; responses and every test listed above stay unchanged.
