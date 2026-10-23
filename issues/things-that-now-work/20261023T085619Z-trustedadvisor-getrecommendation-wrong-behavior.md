Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 5a6a50abfee118ac44c2855d86c20698e89c1a34

# pillarSpecificAggregates is never computed, so cost_optimizing recommendations report no estimated savings

_Filed: 2026-10-23T08:56:19Z_
_Operation: GetRecommendation | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `5a6a50abfee118ac44c2855d86c20698e89c1a34`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: for a recommendation whose `pillars` include `cost_optimizing`, AWS Trusted Advisor returns `recommendation.pillarSpecificAggregates.costOptimizing` with `estimatedMonthlySavings` and `estimatedPercentMonthlySavings` (both @required inside that structure) — this is the whole purpose of the member, documented as "The pillar aggregations for cost savings". Cost-reporting clients call GetRecommendation/ListRecommendations precisely to total those savings.

Actual: the handler hardcodes `pillar_specific_aggregates: None` for every recommendation, so the member is absent from every response, including for the two seeded cost_optimizing recommendations (`low-utilization-ec2-instances-recommendation`, `rds-idle-db-instances-recommendation`). The savings numbers are in fact already present in the store — each cost-pillar seed resource carries an "Estimated Monthly Savings" metadata column (72.00, 148.00 for EC2; 131.40 for RDS) — but nothing aggregates them into the modelled structure, and `crate::state::RecommendationEntry` holds no savings field at all.

The same hardcoded `None` is in the ListRecommendations summary builder (`RecommendationSummary$pillarSpecificAggregates`), so the gap is observable on both the single read and the list; fixing the state model fixes both.

## Evidence

Smithy model: `Recommendation$pillarSpecificAggregates` -> `RecommendationPillarSpecificAggregates{costOptimizing: RecommendationCostOptimizingAggregates{estimatedMonthlySavings (required, Double), estimatedPercentMonthlySavings (required, Double)}}`, doc "The pillar aggregations for cost savings"; `RecommendationSummary` carries the same member. Handler: services/service-com-amazonaws-trustedadvisor-trustedadvisor/src/ops/get_recommendation.rs, `recommendation()` builder, `pillar_specific_aggregates: None` (~line 108); same literal in src/ops/list_recommendations.rs `recommendation_summary()` (~line 131, doc comment states "the store holds no cost-savings aggregate"). State: src/state.rs `RecommendationEntry` has no savings member, while the SEED rows for the cost_optimizing checks carry an "Estimated Monthly Savings" metadata heading and per-resource values. Tests: no assertion anywhere on `pillarSpecificAggregates` (grep over tests/ returns no match), so this is uncovered.

## Steps to Reproduce

1. Boot the emulator with the Trusted Advisor service mounted and an account entitled to the API (support plan Business or better, as tests/all/recommendation_loop.rs does).
2. GET /v1/recommendations/arn:aws:trustedadvisor::<account>:recommendation/low-utilization-ec2-instances-recommendation (a cost_optimizing recommendation).
3. Observe the 200 body: `recommendation.pillars` contains `cost_optimizing` but `recommendation.pillarSpecificAggregates` is absent; AWS returns `pillarSpecificAggregates.costOptimizing.estimatedMonthlySavings` / `estimatedPercentMonthlySavings` for such a recommendation.
4. GET /v1/recommendations?pillar=cost_optimizing and observe the same member missing from every `recommendationSummaries` entry.
