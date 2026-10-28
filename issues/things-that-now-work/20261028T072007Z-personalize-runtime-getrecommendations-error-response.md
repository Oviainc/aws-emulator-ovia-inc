Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: c18a0019e5dd212e394ad1114dc50f0b0a8a5ba1

# GetRecommendations evaluates the request-only numResults bound after resolving the resource, so the refusal names the resource instead of the violated bound

_Filed: 2026-10-28T07:20:07Z_
_Operation: GetRecommendations | Class: error-response_

> _Note: this issue was identified against the code base at commit `c18a0019e5dd212e394ad1114dc50f0b0a8a5ba1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`GetRecommendations` computes its `numResults` bound and checks `numResults` against it only AFTER `resolve()` has looked the campaign/recommender up in the registry, after the recipe-type check, after the personalization-input check and after `requested_metadata_columns` has validated the columns against the resolved resource. Every input to that check (`input.num_results`, `DEFAULT_NUM_RESULTS`, and `metadata_bounded_max(asks_for_metadata(input.metadata_columns))`) is a function of the REQUEST alone, and the crate states — twice — that such a bound is settled before any resource is resolved. The sibling handler `GetPersonalizedRanking` implements exactly that order for the very same doc sentence (its `inputList` bound), and has a test pinning it with a stated reason; `GetRecommendations` is the handler that skips it.

Observable effect: a request that violates the request-only bound AND names an ARN the registry does not hold is refused for the ARN, never for the bound, so the two sibling operations answer the same class of malformed request with different refusals. (`GetActionRecommendations` has the same shape: `effective_num_results` is called at `src/ops/get_action_recommendations.rs:187-192`, after the campaign lookup at line 171.)

The fix is a reordering of existing code — the `num_results` computation only needs the request, so it can be hoisted above `resolve()`, exactly as `GetPersonalizedRanking` hoists its `inputList` check. `requested_metadata_columns` must stay where it is, since it genuinely depends on the resolved resource.

## Evidence

The rule the crate states, over the shared helper — `services/service-com-amazonaws-personalizeruntime-amazonpersonalizeruntime/src/state.rs:568-575`:
```
/// Whether the request asks for metadata in recommendations at all.
///
/// This reads the REQUEST only — it is the condition in "If you are including metadata
/// in recommendations, the maximum is 50. Otherwise, the maximum is 500."
/// (`GetPersonalizedRankingRequest$inputList` / `GetRecommendationsRequest$numResults`
/// docs), which decides a bound before any resource is resolved.
pub fn asks_for_metadata(metadata_columns: Option<&HashMap<String, Vec<String>>>) -> bool {
```

The sibling handler that honours it — `src/ops/get_personalized_ranking.rs:184-193`:
```
        // 1. `inputList`'s length is a fact about the REQUEST — "If you are including
        //    metadata in recommendations, the maximum is 50. Otherwise, the maximum is
        //    500." — so it is checked BEFORE the campaign is resolved: a list past the
        //    bound is refused whatever the campaign turns out to be.
        let max_input_list = registry::metadata_bounded_max(registry::asks_for_metadata(
            input.metadata_columns.as_ref(),
        ));
        if input.input_list.len() > max_input_list {
            return Err(deny(registry::too_long("inputList", max_input_list)));
        }
```

The handler that breaks it — `src/ops/get_recommendations.rs:198-203` resolves FIRST:
```
        // 1. Resolve the campaign or recommender this request recommends from.
        let resolved = resolve(
            &guard,
            registry::present(input.campaign_arn.as_deref()),
            registry::present(input.recommender_arn.as_deref()),
        )?;
```
and only then, at `src/ops/get_recommendations.rs:224-243`, decides and applies the request-only bound — while its own comment claims the bound is request-only and shared with `GetPersonalizedRanking`:
```
        // 3. How much metadata was asked for, which is also what bounds `numResults`.
        //    The BOUND is keyed off the request alone — the same predicate
        //    `GetPersonalizedRanking` bounds `inputList` with, so one request means one
        //    bound in both operations — while the columns themselves are validated
        //    against the resolved resource and its catalog.
        let max_num_results = registry::metadata_bounded_max(registry::asks_for_metadata(
            input.metadata_columns.as_ref(),
        ));
        ...
        let num_results = registry::effective_num_results(
            input.num_results,
            registry::DEFAULT_NUM_RESULTS,
            max_num_results,
        )
        .map_err(deny)?;
```

Test coverage: the sibling order is pinned with its reason — `tests/all/refusal_wording.rs:238-243` (`an_input_list_past_its_documented_maximum_is_refused_before_the_campaign_resolves`): "The bound is a fact about the REQUEST, so it is answered without resolving anything: at the bound the request gets as far as the (unresolvable) campaign, one past it the length is what is reported", asserting `Length of 'inputList' cannot be greater than {max}` against `UNREGISTERED_CAMPAIGN` (`tests/all/refusal_wording.rs:268-292`). The `GetRecommendations` equivalent is untested: `tests/all/inference.rs:1558-1572` (`num_results_past_the_documented_maximum_is_refused`) always sends a REGISTERED campaign (`CAMPAIGN_USER` / `CAMPAIGN_ACTIONS`), so no existing test pins the current order.

## Steps to Reproduce

1. Seed an emulator with no campaign registered (or use `tests/all/refusal_wording.rs`'s `UNREGISTERED_CAMPAIGN`).
2. POST /personalize-ranking with {"campaignArn": "arn:aws:personalize:us-east-1:123456789012:campaign/never-created", "userId": "user-1", "inputList": [<501 ids>]} — observe the request-only bound wins: message `Length of 'inputList' cannot be greater than 500`.
3. POST /recommendations with {"campaignArn": "arn:aws:personalize:us-east-1:123456789012:campaign/never-created", "userId": "user-1", "numResults": 501} — observe the message is `arn:...:campaign/never-created does not exist or is not active yet.`, i.e. the request-only `numResults` bound is never reported even though it depends on nothing but the request.
4. Same with a RESOLVABLE campaign of the wrong recipe type, or a RELATED_ITEMS campaign and no `itemId`, plus `numResults: 501`: the resource-dependent refusal still wins over the request-only bound.
