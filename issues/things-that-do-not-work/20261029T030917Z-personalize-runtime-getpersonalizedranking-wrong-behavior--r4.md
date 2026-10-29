Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: c088b2b7fac1291fdb05737f13bee382d9ee2825

# A repeated inputList id is reported once per occurrence with the full catalog share, so a ranking's scores can sum above 1.0

_Filed: 2026-10-29T03:09:17Z_
_Operation: GetPersonalizedRanking | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `c088b2b7fac1291fdb05737f13bee382d9ee2825`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`GetPersonalizedRanking` normalizes the whole stored catalog into per-item SHARES of one unit of certainty (`scoring::normalize`), then walks `inputList` POSITION by POSITION and pushes `catalog_shares[index]` for every occurrence of an id. `InputList` carries no `@uniqueItems` in the model, so a caller may legitimately repeat an id (up to the 500-entry list the handler itself allows). When it does, the same catalog entry is reported several times, each copy carrying the FULL share, so the reported scores add up to more than one unit of certainty — e.g. an `inputList` naming every catalog id twice reports a list summing to ~2.0.

That contradicts the invariant this crate states twice in its own code and asserts on every ranking response it tests: `scoring`'s module contract ("a response that reports a subset of the catalog therefore sums to at most 1.0"), the handler's own step-4 comment ("a ranking of part of the catalog sums to at most 1.0"), and the shared test helper `assert_descending_and_bounded`, which fails any response whose scores sum above 1.0 and which `score_denominator.rs` applies to `personalizedRanking` responses.

The crate's own test rows cannot catch it: the only duplicate row repeats ONE id of a 40-entry catalog, so the oversized sum stays far below 1.0, and the per-id check in that suite reads the response through `scores_by_id`, a map keyed by `itemId`, which silently collapses the duplicate so the `total` it sums never sees the second copy. The two properties the crate pins — "each reported entry carries the share the whole-catalog call gave it" and "the reported list sums to at most 1.0" — cannot both hold once an id repeats, which is the inconsistency to resolve; the handler currently keeps the first and breaks the second.

## Evidence

The rule the crate states — `src/state/scoring.rs:98-106`:
```
/// `scores` must be the relevance of EVERY entry the resolved resource's stored
/// catalog holds for this request — the whole scored catalog, settled BEFORE any
/// member of the request that merely selects which entries come back (which ids the
/// caller listed, ...) ... a response that reports a subset of the catalog
/// therefore sums to at most 1.0
```
The same rule in the handler — `src/ops/get_personalized_ranking.rs:239-245`:
```
        // 4. Score the WHOLE stored catalog for this request, and settle each entry's
        //    share of the certainty over it. ... Re-ranking the same
        //    user over a longer list, or with a filter, therefore reports an item with
        //    the same score, and a ranking of part of the catalog sums to at most 1.0.
```
The code that breaks it — `src/ops/get_personalized_ranking.rs:262-285` (one push per `inputList` POSITION, the same `catalog_shares[index]` each time):
```
        scoring::normalize(&mut catalog_shares);
...
        for (position, item_id) in input.input_list.iter().enumerate() {
            match campaign.items.iter().position(|item| &item.item_id == item_id) {
                Some(index) => {
...
                    ranked.push((index, catalog_shares[index], position));
```
The model permits the duplicate — `smithy_query(query="shape", shape="InputList")`: `{"shape_id": "com.amazonaws.personalizeruntime#InputList", "type": "list", "member": {"target": "ItemID", "length": {"min": 0, "max": 256}}}` (no `uniqueItems`), and `GetPersonalizedRankingRequest$inputList` doc: "A list of items (by itemId) to rank. ... If you are including metadata in recommendations, the maximum is 50. Otherwise, the maximum is 500."

The invariant the crate ASSERTS on ranking responses — `tests/all/support.rs:281-286`:
```
    let total: f64 = scores.iter().sum();
    assert!(
        total <= 1.0 + 1e-9,
        "case {case}: scores must sum to at most 1.0, got {total} from {scores:?}"
    );
```
applied to `personalizedRanking` at `tests/all/score_denominator.rs:432` (`assert_descending_and_bounded(case, &scores(&response, "personalizedRanking"))`) and `:279`.

Why the suites stay green today — the only duplicate rows are tiny: `tests/all/score_denominator.rs:314-324` (`"a repeated id"`, `vec![every_id[3], every_id[3], every_id[9]]`) and `tests/all/inference.rs:915` (`("a repeated id", vec!["item-5", "item-5", "item-9"], 3)`), both against a 40-entry catalog; and the per-id reference check at `tests/all/score_denominator.rs:408,418-424` reads the response through `scores_by_id` (`tests/all/support.rs:247-252`), a `BTreeMap` keyed by `itemId`, so the second copy is collapsed before the `total` at `:436` is summed.

## Steps to Reproduce

1. Register a `PERSONALIZED_RANKING` campaign through `state::register_campaign` with a catalog whose entries carry popularity, e.g. `Campaign::new(arn, "demo", RecipeType::PersonalizedRanking, items(40))` as `tests/all/score_denominator.rs:93` does.
2. POST /personalize-ranking with `{"campaignArn": <that arn>, "userId": "user-7", "inputList": <every catalog id, listed TWICE>}` — 80 entries, well inside the 500 the handler allows and inside the model's un-`uniqueItems` list.
3. Read the response's `personalizedRanking`: each catalog id appears twice, both copies carrying the same `score` the whole-catalog ranking reports for it.
4. Sum the `score` members of the returned list: ~2.0. Feed the same list to the crate's own helper `assert_descending_and_bounded` (tests/all/support.rs:268) — it fails on "scores must sum to at most 1.0", the invariant `src/state/scoring.rs:98-106` and `src/ops/get_personalized_ranking.rs:245` both state.
5. Contrast with the existing passing row `tests/all/score_denominator.rs:314-324`, which repeats one id of forty and therefore keeps the oversized sum invisible.
