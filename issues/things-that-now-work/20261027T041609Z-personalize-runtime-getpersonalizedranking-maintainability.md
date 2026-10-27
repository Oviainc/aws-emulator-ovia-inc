Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 11fd98189f90040dc28179001c4adbbf7cfc6f77

# The score-descending comparator is implemented twice: GetPersonalizedRanking hand-rolls what scoring::by_score_desc already owns

_Filed: 2026-10-27T04:16:09Z_
_Operation: GetPersonalizedRanking | Class: maintainability_

> _Note: this issue was identified against the code base at commit `11fd98189f90040dc28179001c4adbbf7cfc6f77`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate has one helper for ordering a scored list, `crate::state::scoring::by_score_desc`, and its own doc declares it to be the ordering rule of ALL THREE operations ("Order two scored entries the way every one of these operations reports them"). Two of the three handlers call it (`get_recommendations.rs`, `get_action_recommendations.rs`). `GetPersonalizedRanking` does not: it writes the same computation out again inline — reverse `partial_cmp` on the score with `unwrap_or(Ordering::Equal)` for incomparable values, `then_with(...)` for the tie-break — and differs from the helper only in WHICH key breaks a tie (`inputList` position instead of item id).

So the "order by score descending, treating incomparable scores as equal" rule exists in two copies that can drift: a change to the helper (e.g. how an incomparable score is ordered) silently does not reach `GetPersonalizedRanking`, and the helper's doc already overstates its own reach by claiming it is how "every one of these operations" orders entries while one of them does not use it. The divergence is observable on a catalog whose entries carry no popularity — every share is then exactly 0.0, so EVERY pair is a tie and the two code paths order by different keys (id ascending vs. `inputList` position) — and `tests/all/score_denominator.rs:581` already exercises exactly that flat catalog while asserting only that the scores are 0.0, never the order.

The fix is a single shared score-descending comparator parameterised by the tie-break key, called from all three handlers, so each operation keeps the tie-break its own comment states. Every response stays byte-identical and no existing test changes.

## Evidence

The helper and the reach its own doc claims — `services/service-com-amazonaws-personalizeruntime-amazonpersonalizeruntime/src/state/scoring.rs:141-150`:
```
/// Order two scored entries the way every one of these operations reports them:
/// by score descending, then by id ascending so that equal scores still produce ONE
/// stable order for an idempotent call.
pub fn by_score_desc(left: (&str, f64), right: (&str, f64)) -> std::cmp::Ordering {
    right
        .1
        .partial_cmp(&left.1)
        .unwrap_or(std::cmp::Ordering::Equal)
        .then_with(|| left.0.cmp(right.0))
}
```

The second copy of the same computation — `src/ops/get_personalized_ranking.rs:290-298`:
```
        // Score descending; `inputList` order decides ties, so an equal pair keeps the
        // order the caller sent and the ranking stays stable across identical calls.
        ranked.sort_by(|left, right| {
            right
                .1
                .partial_cmp(&left.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| left.2.cmp(&right.2))
        });
```

The two siblings that DO call the helper — `src/ops/get_recommendations.rs:302-307`:
```
        candidates.sort_by(|(left, left_score), (right, right_score)| {
            scoring::by_score_desc(
                (&resolved.items[*left].item_id, *left_score),
                (&resolved.items[*right].item_id, *right_score),
            )
        });
```
and `src/ops/get_action_recommendations.rs:231-236`:
```
        ranked.sort_by(|(left, left_score), (right, right_score)| {
            scoring::by_score_desc(
                (left.action_id.as_str(), *left_score),
                (right.action_id.as_str(), *right_score),
            )
        });
```

Test coverage that must stay unchanged by the de-duplication: `tests/all/inference.rs:902` (`personalized_ranking_is_a_permutation_of_the_input_list`), `tests/all/inference.rs:178` (`recommendations_come_from_the_stored_catalog_in_descending_score_order`), `tests/all/inference.rs:1024` (`action_recommendations_come_from_the_stored_action_catalog`), and the shared ordering assertion `tests/all/support.rs:268-286` (`assert_descending_and_bounded`: "scores must not increase") invoked from `tests/all/score_denominator.rs:432` and `:518`. The flat-catalog suite that ties every score without pinning the tie order: `tests/all/score_denominator.rs:580-666` (`a_catalog_with_no_popularity_reports_no_certainty`, asserting only `score == 0.0` per entry).

## Steps to Reproduce

1. Open `src/state/scoring.rs:141-150` and read `by_score_desc`'s doc: it declares itself the ordering "every one of these operations" uses.
2. Grep the crate for its call sites: `grep -n 'by_score_desc' src/ops/*.rs` returns only `get_recommendations.rs:303` and `get_action_recommendations.rs:232` — `get_personalized_ranking.rs` never calls it.
3. Open `src/ops/get_personalized_ranking.rs:292-298` and compare it with the helper body: the score-descending half (reverse `partial_cmp` plus `unwrap_or(Ordering::Equal)`) is character-for-character the same; only the `then_with` key differs.
4. Observe the drift risk and the observable split: register a PERSONALIZED_RANKING campaign whose items all carry `base_score: 0.0` (as `tests/all/score_denominator.rs:191-199` builds) and POST /personalize-ranking with an `inputList` in reverse catalog order — every score ties at 0.0, so the response order comes from this inline comparator's `inputList` key, while the same tie under `GetRecommendations` would come from the helper's id key.
5. Fix: add one shared score-descending comparator in `scoring` that takes the tie-break key, and call it from all three handlers; responses and the tests listed above stay unchanged.
