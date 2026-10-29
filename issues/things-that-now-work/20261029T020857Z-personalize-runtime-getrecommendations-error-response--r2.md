Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: f3551fb51efeca87d6f65db01e533e63f3785794

# The request-only clauses of metadataColumns and promotions are evaluated after the resource is resolved, so a malformed request is told its campaign does not exist

_Filed: 2026-10-29T02:08:57Z_
_Operation: GetRecommendations | Class: error-response_

> _Note: this issue was identified against the code base at commit `f3551fb51efeca87d6f65db01e533e63f3785794`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate states one precedence rule for every inference operation: a refusal whose every input comes from the REQUEST is settled BEFORE the campaign/recommender is resolved, so one malformed request earns one refusal whatever the registry holds. GetRecommendations implements that for `numResults` (step 1, before `resolve()`), and GetPersonalizedRanking for `inputList`/`userId`. Two further refusals that need nothing but the request are still evaluated AFTER resolution:

1. `metadataColumns` — `registry::requested_metadata_columns` is called at get_recommendations.rs:249, after `resolve()` at line 221. That helper enforces FOUR rules, and the first two (the map key must be `ITEMS`, at most 10 columns) are pure facts about the request; only `metadata_enabled` and "column is in the catalog" need the resolved resource. The comment directly above the call asserts the opposite ("the one part of `metadataColumns` that needs it"). The same ordering exists in the sibling GetPersonalizedRanking (helper called at get_personalized_ranking.rs:224, after `guard.campaign(...)` at line 210).
2. `promotions` — `promotion(...)` is called at get_recommendations.rs:264, after resolution. Its missing-member refusals (`promotions[0].name is required.`, `promotions[0].percentPromotedItems is required.`, `promotions[0].filterArn is required.`) are built from the request alone.

Consequence: a request carrying `metadataColumns: {"INTERACTIONS": ["GENRE"]}` (or 11 columns), or a `promotions[0]` with no `name`, against an unregistered campaign is answered `"<arn> does not exist or is not active yet."` — a retryable, resource-shaped refusal — instead of the clause naming the member the caller got wrong, while the identical shape of mistake on `numResults` is answered with the member clause. Likewise, against a campaign of the wrong recipe type the recipe-type refusal wins over the member clause. This is the same precedence defect that was already fixed for `numResults` (issue 20261028T072007Z), left behind on the other two request-only members; it is NOT that issue re-filed — the `numResults` hoist is in place at lines 203-218, these two call sites are not.

No existing test pins the current order: the metadataColumns and promotion refusals are only exercised against a REGISTERED campaign and assert status+code only, so hoisting the request-only clauses in front of `resolve()` leaves every assertion unchanged.

## Evidence

The rule, stated by this crate: tests/all/request_only_refusal_order.rs:5-13 — "The rule, over every inference operation: a refusal whose every input comes from the REQUEST — the value the caller sent, measured against a documented bound or against a member the model marks `@required` — is settled BEFORE the request's campaign or recommender is resolved, so one malformed request earns one refusal whatever the registry holds."

The side that honours it: src/ops/get_recommendations.rs:203-218 — "are facts about the REQUEST ... It is therefore settled BEFORE anything is resolved, and a `numResults` past the bound is refused whatever the resource turns out to be." then `effective_num_results(...)` before `resolve(` at line 221.

The side that breaks it: src/ops/get_recommendations.rs:246-254 — "// 4. Which metadata columns come back, validated against the RESOLVED resource\n//    and its catalog — the one part of `metadataColumns` that needs it" followed by `registry::requested_metadata_columns(...)`. But src/state.rs:619-637, inside that helper, refuses two request-only things first:
  `if dataset != METADATA_DATASET_ITEMS { return Err(Denied::invalid(format!("Value '{dataset}' at 'metadataColumns' failed to satisfy constraint: the map key must be {METADATA_DATASET_ITEMS}")));`
  `if requested.len() > MAX_METADATA_COLUMNS { return Err(Denied::invalid(format!("... at most {MAX_METADATA_COLUMNS} columns can be provided" ...`
and only then (src/state.rs:638) consults the resource: `if !metadata_enabled {`.
Those two bounds are documented request facts, quoted in src/state.rs:59-67: "The only `metadataColumns` map key the service accepts: \"The map key is `ITEMS`\"" and "\"The maximum number of columns you can provide is 10.\" (`GetRecommendationsRequest$metadataColumns` doc.)".

Same for promotions: src/ops/get_recommendations.rs:264 `let promotion = promotion(&guard, input.promotions.as_deref(), num_results)?;` (after resolution) whose refusals are request-only — src/ops/get_recommendations.rs:463-471: `return Err(invalid("promotions[0].name is required."))` / `"promotions[0].percentPromotedItems is required."` / `"promotions[0].filterArn is required."`, documented at lines 452-454 as "A promotion without a name, a percentage or a filter defines no promoted subset at all, which is a refusal naming the member."

Sibling with the same ordering: src/ops/get_personalized_ranking.rs:210 `let campaign = guard.campaign(&input.campaign_arn).map_err(deny)?.clone();` then :222-229 "// 3. Metadata, validated against the resolved campaign and its catalog." `registry::requested_metadata_columns(...)`.

Test coverage that stays green under a fix: tests/all/inference.rs:412-470 (`metadata_requests_outside_the_documented_rules_are_refused`) uses `CAMPAIGN_USER`/`CAMPAIGN_RELATED` (registered) and asserts only `(status, code) == (400, "InvalidInputException")`; tests/all/inference.rs:819-854 (`a_promotion_that_defines_no_subset_is_refused`) uses `CAMPAIGN_USER` and asserts only status+code.

## Steps to Reproduce

1. Boot the emulator with an empty registry (no campaign registered through `state::register_campaign`).
2. POST /recommendations with {"campaignArn": "arn:aws:personalize:us-east-1:123456789012:campaign/never-created", "userId": "user-1", "metadataColumns": {"INTERACTIONS": ["GENRE"]}}. Observed: 400 InvalidInputException "arn:...:campaign/never-created does not exist or is not active yet." Expected (by the crate's own request-only-first rule, as applied to numResults): the clause naming the metadataColumns map key.
3. Repeat with {"ITEMS": [11 column names]} instead — same resource message, although the 10-column cap is a request-only bound checked in state.rs:631 before any resource field is read.
4. POST /recommendations with the same unregistered campaignArn plus {"promotions": [{"percentPromotedItems": 50, "filterArn": "arn:aws:personalize:us-east-1:123456789012:filter/any"}]} (no name). Observed: the unresolvable-campaign message; expected: "promotions[0].name is required."
5. Contrast with the member the rule is already implemented for: the same unregistered campaignArn plus "numResults": 501 answers "Value '501' at 'numResults' failed to satisfy constraint: Member must have value less than or equal to 500" (pinned by tests/all/request_only_refusal_order.rs).
