Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 74270ec219ed28f8a3534ea9bf74072ac231b62f

# Two different `context` maps collapse to one scoring seed, so distinct contextual metadata yields the identical ordering and scores

_Filed: 2026-10-29T14:10:16Z_
_Operation: GetRecommendations | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `74270ec219ed28f8a3534ea9bf74072ac231b62f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`scoring::context_seed` renders the request's `context` map into ONE seed part by joining `key=value` pairs with a comma. The map's KEY shape forbids both separator characters, but the VALUE shape does not: `AttributeValue` carries only `length 0..1000` and no `@pattern`, so a caller may legitimately send a value containing `,` and `=`. When it does, two different context maps render to the same string and therefore to the same hash input, so `GetRecommendations` (and the sibling `GetPersonalizedRanking`) return byte-identical scores and ordering for requests whose contextual metadata differs.

Concretely, `{"DEVICE": "mobile", "PLACE": "home"}` and `{"DEVICE": "mobile,PLACE=home"}` both render to `DEVICE=mobile,PLACE=home`.

This is the exact ambiguity the module already decided must not happen: `SEED_SEPARATOR` exists so that `["ab", "c"]` and `["a", "bc"]` cannot hash alike, and the byte was chosen because "no modelled input may contain [it] in a meaningful position". The context part is then composed with two characters the model DOES allow inside a context value, so the guarantee the separator buys between parts is given away inside this one part. The crate also asserts the property this breaks in its own suite: "contextual metadata is part of the score, so two contexts must not share one ordering".

No existing test sends a context value containing `,` or `=` (the only context rows are `{"DEVICE": "mobile"}` / `{"DEVICE": "desktop"}` in tests/all/inference.rs, and `grep -n 'context' tests/all/score_denominator.rs` returns nothing), and every score assertion in the crate compares one response against another response of the same operation, so disambiguating the rendering leaves all of them green.

## Evidence

The rule the module states for seed composition — `src/state/scoring.rs:29-32`:
```
/// Byte written between seed parts so that `["ab", "c"]` and `["a", "bc"]` hash
/// differently. ASCII unit separator, which no modelled input may contain in a
/// meaningful position.
const SEED_SEPARATOR: u8 = 0x1f;
```
The code that gives it away inside the context part — `src/state/scoring.rs:78-92`:
```
/// The request's `context` map rendered as one stable seed part.
pub fn context_seed(context: Option<&HashMap<String, String>>) -> String {
    ...
    let mut pairs: Vec<String> = context
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect();
    pairs.sort();
    pairs.join(",")
}
```
The model permits both separators inside a value — `smithy_query(query="shape", shape="Context")`: `{"type": "map", "length": {"min": 0, "max": 150}, "key": {"target": "AttributeName", "length": {"min": 0, "max": 150}, "pattern": "^[A-Za-z\\d_]+$"}, "value": {"target": "AttributeValue", "length": {"min": 0, "max": 1000}, "sensitive": true}}` — the KEY has a pattern that excludes `=` and `,`; the VALUE has no pattern at all.

The call sites that feed it into the score — `src/ops/get_recommendations.rs:282`: `let context_seed = scoring::context_seed(input.context.as_ref());` (used at `:291-299` inside `scoring::relevance(item.base_score, &[&resolved.scope, personalization, &context_seed, &item.item_id])`), and the sibling `src/ops/get_personalized_ranking.rs:250`: `let context_seed = scoring::context_seed(input.context.as_ref());` (used at `:254-264`).

The property the crate asserts and this breaks — `tests/all/inference.rs:341-345`:
```
    assert_ne!(
        mobile_ids, desktop_ids,
        "contextual metadata is part of the score, so two contexts must not share one \
         ordering; both returned {mobile_ids:?}"
    );
```
Test coverage that stays unchanged under a fix: that suite (`context_changes_the_ordering_and_an_equal_context_does_not`, tests/all/inference.rs:306) is the only place a context is sent, and its values (`mobile`, `desktop`) carry neither separator; the score suites (tests/all/score_denominator.rs, tests/all/inference.rs:1190) send no context and compare a response only against another response of the same operation.

## Steps to Reproduce

1. Register a `USER_PERSONALIZATION` campaign with a scored catalog through `state::register_campaign`, e.g. `Campaign::new(arn, "demo", RecipeType::UserPersonalization, items(40))` as tests/all/inference.rs:92 does.
2. POST /recommendations with {"campaignArn": <arn>, "userId": "user-7", "numResults": 10, "context": {"DEVICE": "mobile", "PLACE": "home"}} and record the returned `itemList` ids and scores.
3. POST /recommendations again with everything identical except {"context": {"DEVICE": "mobile,PLACE=home"}} — one key, whose value contains the `,` and `=` that `AttributeValue` permits.
4. Compare: both responses carry the same ids in the same order with the same scores, because `context_seed` rendered both maps as the string `DEVICE=mobile,PLACE=home`.
5. Contrast with the pair the crate's own test requires to differ ({"DEVICE": "mobile"} vs {"DEVICE": "desktop"}, tests/all/inference.rs:323-345): those do produce different orderings, so the collision is specific to values carrying the separators the renderer uses.
6. Repeat the same two requests against POST /personalize-ranking with a PERSONALIZED_RANKING campaign and an `inputList` — the same collision occurs through src/ops/get_personalized_ranking.rs:250.
