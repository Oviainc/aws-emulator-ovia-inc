Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 772fb5ab5b7496e2df886721dbd75d9882bc460e

# A promotion's share is computed from the requested numResults, not from the items the response actually returns, so it over-promotes a short response

_Filed: 2026-10-28T21:33:32Z_
_Operation: GetRecommendations | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `772fb5ab5b7496e2df886721dbd75d9882bc460e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`GetRecommendations` sizes the promotion's share of the response from the REQUESTED `numResults` instead of from the number of entries the response actually reports. `promoted_slots(num_results, percent)` is evaluated once, before any candidate is known, from the value `effective_num_results` settled for the request; the fill loop that follows then stops at `candidates.len()` whenever the resolved catalog (minus the `RELATED_ITEMS` query item) and the request's filter leave fewer candidates than `numResults`.

When that happens, the promotion takes a far larger share of the reported list than the caller asked for: with a 4-entry candidate set, `numResults: 20` and `percentPromotedItems: 25`, the promotion gets `ceil(20 * 25 / 100) = 5` slots, so EVERY one of the 4 reported items is labelled with `promotionName` and `reason: ["Promoted item"]` — a 100% promotion for a request that asked for 25%. The handler's own step-6 doc states the rule the code breaks ("of the returned slots"), and the modelled member doc states the same thing ("The percentage of recommended items to apply the promotion to"). The only divergence the crate's comments justify in this computation is the ROUNDING direction, not the base the percentage is taken of.

Note the defect is limited to the size of the promoted share; which items are promoted (top-ranked entries matching the promotion's filter) and the labels they carry are correct.

## Evidence

The rule the handler states for itself — `services/service-com-amazonaws-personalizeruntime-amazonpersonalizeruntime/src/ops/get_recommendations.rs:35-37`:
```
//! 6. **Promote.** A `promotions` entry takes `percentPromotedItems` of the returned
//!    slots from the entries its own filter selects; each such item carries
//!    `promotionName` and the documented `reason` "Promoted item".
```
The modelled member doc, `smithy_query(query="shape", shape="Promotion")` — `Promotion$percentPromotedItems`:
```
"percentPromotedItems": {"target": "PercentPromotedItems", "range": {"min": 1, "max": 100}, "doc": "<p>The percentage of recommended items to apply the promotion to.</p>"}
```
The code that takes the percentage of the REQUESTED count instead — `src/ops/get_recommendations.rs:489-498`:
```
/// How many of `num_results` slots a promotion of `percent` percent takes.
///
/// Rounded UP, so a promotion the caller asked for is always applied to at least one
/// slot: `percentPromotedItems` is modelled `range 1..100`, so asking for a promotion
/// at all asks for a non-empty share. The rounding direction is CHOSEN — AWS
/// documents only "the percentage of recommended items to apply the promotion to".
fn promoted_slots(num_results: usize, percent: i32) -> usize {
    let percent = percent.clamp(0, 100) as usize;
    num_results.saturating_mul(percent).div_ceil(100)
}
```
(the comment justifies only the ROUNDING, not the base), fed with the request's settled count at `src/ops/get_recommendations.rs:264`:
```
        let promotion = promotion(&guard, input.promotions.as_deref(), num_results)?;
```
which comes from `src/ops/get_recommendations.rs:213-218`:
```
        let num_results = registry::effective_num_results(
            input.num_results,
            registry::DEFAULT_NUM_RESULTS,
            max_num_results,
        )
        .map_err(deny)?;
```
and the loops that show the RETURNED count is `min(num_results, candidates.len())`, not `num_results` — `src/ops/get_recommendations.rs:324-342`:
```
        if let Some(promotion) = promotion.as_ref() {
            for (position, (index, _)) in candidates.iter().enumerate() {
                if chosen.len() >= promotion.slots.min(num_results) {
                    break;
                }
                if promotion.filter.retains(&resolved.items[*index].attributes) {
                    chosen.push((position, true));
                }
            }
        }
        for position in 0..candidates.len() {
            if chosen.len() >= num_results {
                break;
            }
```
Test coverage: `tests/all/inference.rs:740-816` (`a_promotion_takes_its_documented_share_of_the_returned_items`) pins the share only for rows where the 40-entry seeded catalog makes the returned count EQUAL `numResults` — `("half of ten", 10, 50, 5)`, `("all of ten", 10, 100, 10)`, `("a quarter of four", 4, 25, 1)`, `("a tenth of eight, rounded up", 8, 10, 1)`, `("three tenths of ten", 10, 30, 3)` — and asserts `returned.len() == num_results` at `tests/all/inference.rs:771-775`, so no row exercises a response shorter than `numResults`. Those rows stay green if the share is taken of the reported entries, because returned == numResults in every one of them.

## Steps to Reproduce

1. Register a `USER_PERSONALIZATION` campaign whose Items catalog holds only 4 entries, all with `GENRE` = "Comedy" (`registry::register_campaign` with `Campaign::new(arn, "demo", RecipeType::UserPersonalization, items)`, as `tests/all/inference.rs:91-102` does).
2. Register a filter: `registry::register_filter(engine, <filter arn>, "INCLUDE ItemID WHERE Items.GENRE IN (\"Comedy\")")`.
3. POST /recommendations with {"campaignArn": <campaign>, "userId": "user-1", "numResults": 20, "promotions": [{"name": "promo1", "percentPromotedItems": 25, "filterArn": <filter arn>}]}.
4. Observed: HTTP 200 with 4 entries in `itemList` (the whole catalog), and ALL 4 carry `promotionName`: "promo1" and `reason`: ["Promoted item"] — the promotion took 100% of the returned slots, because `promoted_slots(20, 25) = 5` was computed from the requested count.
5. Expected per `src/ops/get_recommendations.rs:35-37` ("takes `percentPromotedItems` of the returned slots") and `Promotion$percentPromotedItems` ("The percentage of recommended items to apply the promotion to"): 25% of the 4 items the response reports — 1 promoted entry, the other 3 unlabelled.
6. Repeat with `numResults: 4` (so the returned count equals the requested count): exactly 1 entry is promoted, confirming the requested count is what sizes the share.
