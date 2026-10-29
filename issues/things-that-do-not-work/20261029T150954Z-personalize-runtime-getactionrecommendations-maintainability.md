Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 7b6bbea9d5777d5e3200cc7b9f4654c9e364acc7

# The recipe-type-to-operation mapping is implemented four times: three handler gates re-derive what RecipeType::operation() already owns

_Filed: 2026-10-29T15:09:54Z_
_Operation: GetActionRecommendations | Class: maintainability_

> _Note: this issue was identified against the code base at commit `7b6bbea9d5777d5e3200cc7b9f4654c9e364acc7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate states the rule "which runtime operation serves a campaign of this recipe type" exactly once, as `RecipeType::operation()` in `src/state.rs`. Every one of the three handlers then re-derives that same mapping as its own hand-written predicate instead of asking the helper:

* `GetActionRecommendations` gates on `campaign.recipe_type != RecipeType::PersonalizedActions`
* `GetPersonalizedRanking` gates on `campaign.recipe_type != RecipeType::PersonalizedRanking`
* `GetRecommendations` gates on `matches!(campaign.recipe_type, RecipeType::PersonalizedRanking | RecipeType::PersonalizedActions)`

Each of those three predicates is exactly `recipe_type.operation() != <this operation's name>`, so the mapping exists in four places that can drift. The `GetRecommendations` copy is the one that can drift SILENTLY: it is a NEGATIVE enumeration of the two recipe types it refuses, so adding a recipe type to `RecipeType` forces `operation()` to be updated (its `match` is exhaustive and the compiler demands the new arm) while `matches!(.., PersonalizedRanking | PersonalizedActions)` keeps compiling and silently makes `GetRecommendations` SERVE a recipe type that `operation()` says another operation serves. The compiler cannot catch that, because a `matches!` pattern list is not exhaustiveness-checked.

The refusal MESSAGES are built separately from the gates (`GetRecommendations` already calls `campaign.recipe_type.operation()` for its message; the other two format their own expected recipe name), so routing every gate through the one helper changes no response byte and no existing assertion.

## Evidence

The single owner of the rule — `services/service-com-amazonaws-personalizeruntime-amazonpersonalizeruntime/src/state.rs:129-136`:
```
    /// The runtime operation that serves a campaign of this recipe type.
    pub fn operation(self) -> &'static str {
        match self {
            Self::UserPersonalization | Self::RelatedItems => "GetRecommendations",
            Self::PersonalizedRanking => "GetPersonalizedRanking",
            Self::PersonalizedActions => "GetActionRecommendations",
        }
    }
```

Copy 1 — `src/ops/get_action_recommendations.rs:193-201`:
```
        if campaign.recipe_type != RecipeType::PersonalizedActions {
            return Err(invalid(format!(
                "Campaign {} deploys a solution version trained with the {} recipe type; this \
                 campaign must deploy a solution version trained with a {} recipe.",
```

Copy 2 — `src/ops/get_personalized_ranking.rs:215-224`:
```
        if campaign.recipe_type != RecipeType::PersonalizedRanking {
            return Err(invalid(format!(
                "Campaign {} deploys a solution version trained with the {} recipe type; the \
                 solution backing a GetPersonalizedRanking campaign must have been created using \
                 a recipe of type {}.",
```

Copy 3 (the negative enumeration) — `src/ops/get_recommendations.rs:409-419`:
```
            if matches!(
                campaign.recipe_type,
                RecipeType::PersonalizedRanking | RecipeType::PersonalizedActions
            ) {
                return Err(invalid(format!(
                    "Campaign {arn} deploys a solution version trained with the {} recipe type; \
                     use the {} API.",
                    campaign.recipe_type.as_str(),
                    campaign.recipe_type.operation()
                )));
```
Note that this same handler already calls `campaign.recipe_type.operation()` on the very next line for its message — the helper is in scope and in use, but the branch above it re-states the mapping instead of using it.

Test coverage that pins current behaviour and stays unchanged under a dedup (the gates decide WHETHER to refuse, the formats decide the TEXT, and neither changes):
* `tests/all/inference.rs:1385-1425` — `an_operation_refuses_a_campaign_another_operation_serves`, with the comment "// Each recipe type is served by exactly one of the three operations." and all four wrong (operation, recipe type) rows asserting `(400, "InvalidInputException")`.
* `tests/all/request_only_refusal_order.rs:161-165, 210-214, 247-251` — pins each of the three refusal messages verbatim (e.g. "PERSONALIZED_RANKING recipe type; use the GetPersonalizedRanking API.").
* `tests/all/optional_input_refusal_order.rs:169-177, 213-220` and `tests/all/resolution_precedes_optional_input.rs:243-268` — pin that the wrong-recipe-type refusal wins over the missing-personalization-input refusal.

## Steps to Reproduce

1. Read `RecipeType::operation()` at services/service-com-amazonaws-personalizeruntime-amazonpersonalizeruntime/src/state.rs:129-136 — the one place that says which operation serves which recipe type.
2. Read the three recipe-type gates: src/ops/get_action_recommendations.rs:193, src/ops/get_personalized_ranking.rs:215, src/ops/get_recommendations.rs:409-412. Each is the same mapping re-spelled as a literal predicate; each is equivalent to `campaign.recipe_type.operation() != <Self::NAME>`.
3. Observe the drift path: add a hypothetical fifth `RecipeType` variant. `operation()`'s `match` fails to compile until the new arm is added (the rule's owner is forced to stay correct), but `matches!(campaign.recipe_type, RecipeType::PersonalizedRanking | RecipeType::PersonalizedActions)` in get_recommendations.rs:409-412 keeps compiling and now admits the new recipe type into GetRecommendations, contradicting whatever `operation()` was updated to say.
4. Confirm a fix is response-neutral: the three refusal strings are produced by the `format!` calls below each gate, not by the gate predicate, so replacing the predicates with the helper leaves tests/all/inference.rs:1385-1425, tests/all/request_only_refusal_order.rs:161-251, tests/all/optional_input_refusal_order.rs:169-220 and tests/all/resolution_precedes_optional_input.rs:243-268 asserting exactly what they assert today.
