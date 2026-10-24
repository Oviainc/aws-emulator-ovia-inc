Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: b4572519ed5f295b6edc2ad3f0f0ced732a31b98

# Report a savings opportunity on Amazon ECS service recommendation options instead of omitting every savings member

_Filed: 2026-10-24T06:11:28Z_
_Operation: GetECSServiceRecommendations | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `b4572519ed5f295b6edc2ad3f0f0ced732a31b98`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: an `ECSServiceRecommendation`'s recommendation option carries `savingsOpportunity` (and, with the savings-estimation-mode preference in force, `savingsOpportunityAfterDiscounts`), and `effectiveRecommendationPreferences.savingsEstimationMode` names the pricing source the estimate came from. An ECS service on Fargate is billed from a public per-vCPU-hour and per-GB-hour price, so a recommendation that moves the task's CPU/memory sizing has a computable monthly saving — which is the whole point of a Compute Optimizer recommendation.

Actual: the handler hardcodes `savings_opportunity: None`, `savings_opportunity_after_discounts: None` and `savings_estimation_mode: None` on every option it builds, with the justification "this emulator holds no pricing". That is not true of the rest of the crate: `GetEBSVolumeRecommendations` prices every volume from `volume_monthly_cost`, `GetLambdaFunctionRecommendations` from `LAMBDA_PRICE_PER_GB_SECOND`, and `GetAutoScalingGroupRecommendations` from the stored on-demand hourly prices — all three report both savings members. So a client that reads ECS savings to rank optimisation work gets a recommendation with no money in it, while the sibling readers of the same service return figures.

The option's `containerRecommendations` is likewise always absent, which is a second symptom of the same "nothing computed for this option" class.

## Evidence

Spec: `ECSServiceRecommendationOption$savingsOpportunity` and `$savingsOpportunityAfterDiscounts` are modeled on the shape this operation returns, as is `ECSEffectiveRecommendationPreferences$savingsEstimationMode` ("Describes the savings estimation mode used for calculating savings opportunity"); Compute Optimizer's documented purpose for an ECS service recommendation is the cost/performance trade-off of a different task sizing. Handler: `src/ops/get_ecs_service_recommendations.rs`, in `analyse()`'s `ECSServiceRecommendationOption` literal — `savings_opportunity: None`, `savings_opportunity_after_discounts: None`, `container_recommendations: None`, and the `ECSEffectiveRecommendationPreferences { savings_estimation_mode: None, .. }` just below it. Contrast `src/state.rs` `ObservedVolume::to_recommendation` (EBS) and `ObservedFunction::to_recommendation` (Lambda), both of which compute both savings members from a public price table. Tests: `tests/all/ecs_service_recommendations.rs` lines ~361-367 currently ASSERT the absence (`option["savingsOpportunity"].is_null() && option["savingsOpportunityAfterDiscounts"].is_null()`), so the test pins the current wrong behaviour and must be updated with the fix.

## Steps to Reproduce

1. Boot the emulator; call UpdateEnrollmentStatus(status=Active).
2. Create an Amazon ECS cluster and a Fargate service through the ECS emulator, and publish CPU/memory metrics for it so Compute Optimizer can analyse it.
3. Call GetECSServiceRecommendations with no filters and inspect `ecsServiceRecommendations[0].serviceRecommendationOptions[0]`.
4. Observe `savingsOpportunity` and `savingsOpportunityAfterDiscounts` absent and `effectiveRecommendationPreferences.savingsEstimationMode` absent, where AWS reports a monthly saving and the pricing source for the same recommendation.
5. For contrast, call GetEBSVolumeRecommendations or GetLambdaFunctionRecommendations on the same emulator and observe both savings members populated.
