Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 2543c0eeec68e797eed0a2f109c688ea6582e1e3

# Bootstrap the Personalize Runtime emulator

_Filed: 2026-10-22T00:17:55Z_
_Operation: service-com-amazonaws-personalizeruntime-amazonpersonalizeruntime | Class: missing_

> _Note: this issue was identified against the code base at commit `2543c0eeec68e797eed0a2f109c688ea6582e1e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every handler under `services/service-com-amazonaws-personalizeruntime-amazonpersonalizeruntime/src/ops/` is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state. This service models exactly three operations (all of them inference/runtime calls over `restJson1`), so the bootstrap slice is the whole runtime surface plus the backing registry needed to make it stateful.

### Happy path to implement (in order)

1. **GetRecommendations** — `POST /recommendations`. Input: `campaignArn` OR `recommenderArn` (both `len 0..256`, pattern `^arn:([a-z\d-]+):personalize:.*:.*:.+$`), `userId`, `itemId`, `numResults` (required, `range 0..`, declared default 0 — AWS treats absent/0 as 25), `context`, `filterArn`, `filterValues`, `promotions` (`len 0..1`, each `Promotion{name, percentPromotedItems 1..100, filterArn, filterValues}`), `metadataColumns` (`len 0..1`, key `ITEMS`). Output: `itemList` (list of `PredictedItem{itemId, score, promotionName, metadata, reason}`) sorted descending by `score`, and a `recommendationId`. Must resolve the ARN against the registry (404 `ResourceNotFoundException` for an unknown campaign/recommender/filter) and must return exactly `numResults` items drawn from the stored item catalog, deterministically derived from the stored catalog + `userId`/`itemId` — not a hardcoded list.
2. **GetPersonalizedRanking** — `POST /personalize-ranking`. Input: `campaignArn`, `inputList`, `userId` (all three `@required`), plus `context`, `filterArn`, `filterValues`, `metadataColumns`. Output: `personalizedRanking` (list of `PredictedItem`) that is a PERMUTATION of `inputList` — same cardinality, same item IDs, re-ordered by a computed score descending — plus `recommendationId`. Items in `inputList` that are not in the campaign's catalog must still be returned (AWS ranks them last with a low score), and an unknown `campaignArn` must be 404.
3. **GetActionRecommendations** — `POST /action-recommendations`. Input: `campaignArn`, `userId`, `numResults` (required), `filterArn`, `filterValues`. Output: `actionList` (list of `PredictedAction{actionId, score}`) descending by score, plus `recommendationId`.
4. **Registry + seeding seam** — the three runtime calls are read-only against control-plane resources owned by the sibling `personalize` service (campaigns, recommenders, filters, Items/Actions datasets). Bootstrap needs a concrete in-crate state store for those resources plus a way to populate it (shared engine state keyed by ARN, seeded at startup/test fixture or shared with the `personalize` crate), so that a client can register a campaign and then get recommendations from it.

### Non-negotiables

- Every response must be COMPUTED from persisted state: the item/action IDs returned must come from the stored catalog for the resolved campaign/recommender, scores must be derived (and strictly non-increasing across the returned list), and `recommendationId` must be a freshly generated opaque ID per call. No canned item lists, no echoing the request, no random IDs without a backing store.
- `filterArn` must actually filter: an unknown filter ARN is `ResourceNotFoundException` (404); a known filter's INCLUDE/EXCLUDE expression with `filterValues` must remove items from the result set, and the result must still be padded up to `numResults` from the catalog (AWS's popular-item placeholder behaviour) where possible.
- `promotions` must be honoured on GetRecommendations: `percentPromotedItems` of the returned items come from the promotion's filter, and each such item carries `promotionName`.
- `metadataColumns` (key `ITEMS`, max 10 column names) must populate `PredictedItem.metadata` from the stored item attributes; when absent, `metadata` must be omitted entirely.
- Modeled constraint violations (ARN pattern, `numResults` range, map length bounds) must be refused with 400 `InvalidInputException`/`ValidationException`, never 501 and never a silent 200.

### State Model

- `PersonalizeRuntimeState { campaigns: HashMap<String /*campaignArn*/, Campaign>, recommenders: HashMap<String /*recommenderArn*/, Recommender>, filters: HashMap<String /*filterArn*/, Filter>, recommendation_ids: Vec<String> /*issued IDs, for traceability*/ }`
- `Campaign { arn, name, solution_version_arn, recipe_type: UserPersonalization | RelatedItems | PersonalizedRanking | ActionRecommendation, metadata_enabled: bool, items: Vec<Item>, actions: Vec<Action> }`
- `Recommender { arn, name, use_case, metadata_enabled: bool, items: Vec<Item> }`
- `Item { item_id, base_score: f64, attributes: HashMap<String /*column*/, String /*value*/> }`
- `Action { action_id, base_score: f64 }`
- `Filter { arn, expression: String, referenced_params: Vec<String> }`

Scoring for a request = deterministic function of (`item.base_score`, `userId`/`itemId`, `context`) so repeated identical calls return the same ordering (the operations are `@idempotent` in the model), with scores normalised to sum to ~1.0 across the returned `itemList` as AWS does.

Nothing is left out of scope: this service has only these three operations, so the bootstrap slice IS the service.

## Evidence

`services/service-com-amazonaws-personalizeruntime-amazonpersonalizeruntime/OPERATIONS.md` lists all 3 operations (`GetActionRecommendations`, `GetPersonalizedRanking`, `GetRecommendations`) with routes `POST /action-recommendations`, `POST /personalize-ranking`, `POST /recommendations`, protocol `restJson1`, signing name `personalize`, and errors `InvalidInputException (400)` / `ResourceNotFoundException (404)`. `src/ops/get_recommendations.rs` lines 4-6 carry the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and line 200 returns `Err(crate::scaffolded::Scaffolded::not_implemented("GetRecommendations"))`; `get_personalized_ranking.rs` and `get_action_recommendations.rs` are the same generated stub. Output shapes from the Smithy model: `PredictedItem{itemId, score (double), promotionName, metadata (sensitive map), reason}`, `PredictedAction{actionId, score}`, `Promotion{name, percentPromotedItems (1..100), filterArn, filterValues}`. Well-established AWS behaviour grounds the ordering (`itemList` "sorted in descending order by prediction score", per the modeled doc on `GetRecommendations.itemList`) and the permutation property of `personalizedRanking`.

## Steps to Reproduce

1. Start the emulator and seed/register a campaign ARN `arn:aws:personalize:us-east-1:123456789012:campaign/demo` backed by a USER_PERSONALIZATION recipe with an item catalog of e.g. 100 items, plus an ACTION_RECOMMENDATION campaign and one filter ARN.
2. Call GetRecommendations with `campaignArn` = the demo campaign, `userId` = "user-1", `numResults` = 10. Expect HTTP 200 with `itemList` of exactly 10 `PredictedItem`s whose `itemId`s all come from the seeded catalog, `score` values strictly non-increasing, and a non-empty `recommendationId`. Today: HTTP 501 NotImplemented.
3. Repeat the identical GetRecommendations call and verify the same ordering is returned (operation is `@idempotent`), with a fresh `recommendationId`.
4. Call GetRecommendations again with `metadataColumns` = {"ITEMS": ["genre"]} and verify each returned item carries `metadata.genre` taken from the seeded item attributes; omit `metadataColumns` and verify `metadata` is absent.
5. Call GetRecommendations with `promotions` = [{name: "promo1", percentPromotedItems: 50, filterArn: <seeded filter>}] and `numResults` = 10; verify ~5 returned items carry `promotionName` = "promo1".
6. Call GetPersonalizedRanking with the demo `campaignArn`, `userId` = "user-1", and `inputList` = ["item-5","item-42","item-7"]; expect 200 with `personalizedRanking` containing exactly those three item IDs, reordered by descending score.
7. Call GetActionRecommendations with the action campaign ARN, `userId` = "user-1", `numResults` = 3; expect 200 with 3 `PredictedAction`s from the seeded action catalog, descending by score.
8. Call each of the three operations with `campaignArn` = `arn:aws:personalize:us-east-1:123456789012:campaign/does-not-exist`; expect HTTP 404 `ResourceNotFoundException`, not 501 and not a 200.
9. Call GetRecommendations with `campaignArn` = "not-an-arn" (violates the modeled pattern); expect HTTP 400, not 501 and not a 200.
