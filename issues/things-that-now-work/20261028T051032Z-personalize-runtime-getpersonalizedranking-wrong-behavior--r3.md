Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: d9dd770f9e2574e0600226bd25d044e51daa0c38

# GetPersonalizedRanking omits the metadata member for an inputList entry the catalog never held, bypassing the one helper that owns the metadata rule

_Filed: 2026-10-28T05:10:32Z_
_Operation: GetPersonalizedRanking | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `d9dd770f9e2574e0600226bd25d044e51daa0c38`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

When a `GetPersonalizedRanking` request asks for `metadataColumns`, the handler serves `PredictedItem$metadata` through `crate::state::metadata_for` for every entry the campaign's stored Items catalog holds, but the branch that appends an `inputList` id the catalog never held hardcodes `metadata: None` instead of calling that helper.

`metadata_for` is the one place this crate decides what `None` MEANS: `None` is reserved for "the request asked for no metadata", in which case the member is omitted entirely; an entry that carries none of the REQUESTED columns still gets a map (the helper only leaves the absent columns out of it). So one response to one request that did ask for metadata comes back shaped two different ways: catalog entries carry a `metadata` object (possibly empty) and appended entries carry no `metadata` member at all, which per the crate's own rule tells the client that no metadata was requested. A client cannot distinguish "metadata was not requested" from "this entry has none".

The same handler already treats such an entry as "an entry with no stored attributes" a few lines earlier: it builds `no_attributes` (an empty attribute map) precisely so the request's filter can decide on it through the shared evaluator. The metadata path is the one place that does not reuse that treatment, and `GetRecommendations` routes every entry it reports through `metadata_for`. The defect is the bypassed helper, not a new rule: feeding the appended entry's empty attribute set through `metadata_for` with the already-computed `metadata_columns` is what the rest of the crate does.

The path is completely untested: no test in the crate sends `metadataColumns` to `/personalize-ranking` and inspects a response body.

## Evidence

The rule the helper owns — `services/service-com-amazonaws-personalizeruntime-amazonpersonalizeruntime/src/state.rs:659-665`:
```
/// The `PredictedItem$metadata` map for one stored item, or `None` when the request
/// asked for no metadata — in which case the member is omitted entirely.
///
/// A requested column the item itself does not carry is left out of the map rather
/// than reported empty: the value is absent in the stored Items dataset.
pub fn metadata_for(item: &Item, columns: Option<&[String]>) -> Option<HashMap<String, String>> {
    let columns = columns?;
```

The same rule restated in this handler's own doc — `src/ops/get_personalized_ranking.rs:34-35`:
```
//! 5. `metadataColumns` is served from the stored item attributes; with no
//!    `metadataColumns` the `metadata` member is omitted entirely.
```

The path that honours it — `src/ops/get_personalized_ranking.rs:295-307`:
```
        let mut personalized_ranking: Vec<PredictedItem> = ranked
            .iter()
            .map(|(index, score, _)| {
                let item = &campaign.items[*index];
                PredictedItem {
                    item_id: Some(item.item_id.clone()),
                    score: Some(*score),
                    promotion_name: None,
                    metadata: registry::metadata_for(item, metadata_columns.as_deref()),
```

The path that breaks it — `src/ops/get_personalized_ranking.rs:311-317`:
```
        personalized_ranking.extend(appended.into_iter().map(|item_id| PredictedItem {
            item_id: Some(item_id),
            score: Some(0.0),
            promotion_name: None,
            metadata: None,
            reason: None,
        }));
```

The empty attribute set this very handler already builds for the same entries — `src/ops/get_personalized_ranking.rs:256-260`:
```
        // 5. Report the entries `inputList` names, and collect the ones the catalog
        //    does not hold. An entry with no catalog record carries no stored
        //    attributes, so the request's filter decides on an empty attribute set: an
        //    INCLUDE filter cannot keep it, an EXCLUDE filter cannot remove it.
        let no_attributes: BTreeMap<String, String> = BTreeMap::new();
```

The sibling handler that routes every reported entry through the helper — `src/ops/get_recommendations.rs:339-347`:
```
                PredictedItem {
                    item_id: Some(item.item_id.clone()),
                    score: Some(score),
                    promotion_name: ...,
                    metadata: registry::metadata_for(item, metadata_columns.as_deref()),
```

Test coverage leaves it open: the only suite that inspects `metadata` in a response body drives `/recommendations` — `tests/all/inference.rs:354-409` (`metadata_columns_are_served_from_the_stored_item_attributes`, `emulator.ok(RECOMMENDATIONS, json!({ ... "metadataColumns": { "ITEMS": columns } }))` at `tests/all/inference.rs:367-377`), and the one `GetPersonalizedRanking` test that sends `metadataColumns` expects a REFUSAL, never a body — `tests/all/refusal_wording.rs:268-277` (`body["metadataColumns"] = json!({ "ITEMS": ["GENRE"] });` … `emulator.refusal(PERSONALIZE_RANKING, body)`). A grep for `metadata` across `tests/` finds no other `/personalize-ranking` row.

## Steps to Reproduce

1. Register a PERSONALIZED_RANKING campaign with metadata in recommendations enabled over an Items catalog that carries a GENRE column: `registry::register_campaign(engine, Campaign::new(arn, "demo", RecipeType::PersonalizedRanking, items(40)).with_metadata_enabled())` (as `tests/all/inference.rs:91-102` does).
2. POST /personalize-ranking with {"campaignArn": <campaign>, "userId": "user-4", "inputList": ["item-5", "no-such-item"], "metadataColumns": {"ITEMS": ["GENRE"]}}.
3. Read the two `personalizedRanking` entries. Observed: the `item-5` entry carries "metadata": {"GENRE": ...}; the `no-such-item` entry carries NO `metadata` member at all, which `src/state.rs:659-660` and `src/ops/get_personalized_ranking.rs:34-35` both define as "the request asked for no metadata".
4. Compare with the analogous GetRecommendations case — an item stored with none of the requested columns (e.g. register a catalog entry via `Item::new("item-x", 1.0)` with no attributes and ask for {"ITEMS": ["GENRE"]}): that entry comes back with the `metadata` member PRESENT, because every reported entry goes through `metadata_for` (`src/ops/get_recommendations.rs:345`). Expected: the appended entry is reported the same way, by passing its (empty) attribute set — the `no_attributes` map the handler already builds at line 260 — through `registry::metadata_for`.
