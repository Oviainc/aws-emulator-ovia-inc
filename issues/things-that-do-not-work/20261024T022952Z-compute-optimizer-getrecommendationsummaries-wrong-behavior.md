Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: d5cb6525aec300e57e039668d831e20baadf847e

# Compute idleSummaries and idleSavingsOpportunity from state instead of hardcoding them absent

_Filed: 2026-10-24T02:29:52Z_
_Operation: GetRecommendationSummaries | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `d5cb6525aec300e57e039668d831e20baadf847e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`RecommendationSummary` models `idleSummaries` ("Describes the findings summary of the idle resources") and `idleSavingsOpportunity`, and `aggregatedSavingsOpportunity` is the savings opportunity aggregated across rightsizing AND idle recommendations. The handler hardcodes `idle_summaries: None` and `idle_savings_opportunity: None` for every summary it builds, and sets `aggregated_savings_opportunity` to the very same value it already put in `savings_opportunity`, so those three members carry no information about state at all.

Expected: a summary reports how many of the account's resources of that type the service found idle (one `Summary` entry per `IdleFinding` value with its count), the savings opportunity of acting on those idle findings, and an `aggregatedSavingsOpportunity` that is the rightsizing opportunity plus the idle opportunity.
Actual: the two idle members are always absent regardless of what the service holds, and `aggregatedSavingsOpportunity` is a duplicate of `savingsOpportunity` by construction — so even once idle recommendations exist in the store (see the companion GetIdleRecommendations gap, and note `tests/all/idle_recommendations.rs` records them straight into `state.idle_recommendations`), the summaries would still not reflect them.

## Evidence

Spec: `RecommendationSummary$idleSummaries` — "Describes the findings summary of the idle resources"; `RecommendationSummary$idleSavingsOpportunity`; `RecommendationSummary$aggregatedSavingsOpportunity` — "describes the savings opportunity for recommendations of a given resource type or for the recommendation option of a given resource" (rightsizing + idle), all modeled on the shape this operation returns. Handler: `src/ops/get_recommendation_summaries.rs` `summarise()` lines 324-340 — `idle_summaries: None`, `idle_savings_opportunity: None`, and `aggregated_savings_opportunity: Some(savings_opportunity(rightsizing_savings, current_monthly_cost))`, the identical call already made for `savings_opportunity`. The handler never consults `guard.idle_recommendations` even though the store holds that field. Tests: `tests/all/recommendation_summaries.rs` asserts counts, costs, risk ratings and paging but makes no assertion about idleSummaries/idleSavingsOpportunity, so nothing pins the current absence.

## Steps to Reproduce

1. Boot the compute-optimizer emulator and opt the account in with UpdateEnrollmentStatus(status=Active)
2. Arrange for the service to hold at least one idle resource recommendation for the account (the state field the idle reader serves)
3. Call GetRecommendationSummaries
4. Observe every recommendationSummaries entry omits idleSummaries and idleSavingsOpportunity, and aggregatedSavingsOpportunity is byte-identical to savingsOpportunity rather than including the idle opportunity
