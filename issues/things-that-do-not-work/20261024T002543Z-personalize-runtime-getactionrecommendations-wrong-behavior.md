Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: aeaddd52642475574d853bf8e089296dd7f2f86c

# GetActionRecommendations renormalizes action scores over the truncated list, so an action's score changes with numResults

_Filed: 2026-10-24T00:25:43Z_
_Operation: GetActionRecommendations | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `aeaddd52642475574d853bf8e089296dd7f2f86c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: the `score` of a recommended action is a property of the (campaign, userId, action) triple — the per-action prediction the PERSONALIZED_ACTIONS solution version produces (`PredictedAction$score`: "The score of the recommended action. For information about action scores, see How action recommendation scoring works"). Asking for fewer results returns the same top actions WITH THE SAME SCORES; a `numResults=1` response does not report 1.0 for the single action returned just because it is the only one asked for.

Actual: the handler scores every stored action, sorts descending, TRUNCATES to `numResults`, and only then rescales the surviving scores so they sum to exactly 1.0. The reported score is therefore a function of how many results the caller asked for: `numResults=1` always reports `score: 1.0`; the same action reports different scores in a `numResults=3` and a `numResults=10` call on an otherwise identical request; and every response sums to exactly 1.0 regardless of how many actions the campaign's catalog holds. The fix is to make the denominator independent of the request's `numResults` (normalize over the full scored, filter-surviving action set, then truncate).

## Evidence

Spec: `PredictedAction$score` doc in the Smithy model points at AWS's "How action recommendation scoring works"; a per-action prediction cannot depend on the caller's page size. `GetActionRecommendations$numResults` doc: "The number of results to return. The default is 5. The maximum is 100." — it bounds how many actions come back, not what each one scores.
Handler: `services/service-com-amazonaws-personalizeruntime-amazonpersonalizeruntime/src/ops/get_action_recommendations.rs` lines 202-222 — `ranked.truncate(num_results)` immediately followed by `scoring::normalize(&mut scores)`; `src/state/scoring.rs` lines 102-117 divide by the sum of exactly the retained scores (and spread 1.0 evenly when every score is 0).
Tests: `tests/all/inference.rs` `action_recommendations_come_from_the_stored_action_catalog` (line 1013) calls `assert_descending_and_normalized` for each `numResults` row, so it asserts the per-response sum-to-1.0 behavior and never compares one action's score across two different `numResults` values — the gap is uncovered.

## Steps to Reproduce

1. Make a PERSONALIZED_ACTIONS campaign with an action catalog of ~10 actions visible to the runtime registry (today only the in-process `state::register_campaign` seam can do this).
2. Call GetActionRecommendations with that campaignArn, userId=user-1, numResults=1. Observed: the single action reports score 1.0. Expected: its own prediction score, not 1.0 by construction.
3. Call GetActionRecommendations with the same campaignArn/userId and numResults=3, then numResults=10, and compare the score reported for an action present in both responses. Observed: two different scores for the same action; each response's scores sum to exactly 1.0. Expected: the same score for that action in both responses.
