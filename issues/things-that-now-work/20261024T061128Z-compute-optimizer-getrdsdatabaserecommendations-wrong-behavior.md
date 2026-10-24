Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: b4572519ed5f295b6edc2ad3f0f0ced732a31b98

# Apply the resolved savingsEstimationMode preference to RDS recommendations instead of discarding it

_Filed: 2026-10-24T06:11:28Z_
_Operation: GetRDSDatabaseRecommendations | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `b4572519ed5f295b6edc2ad3f0f0ced732a31b98`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: the savings-estimation-mode preference an account activates with `PutRecommendationPreferences(savingsEstimationMode=AfterDiscounts)` is observable on an RDS recommendation — `RDSDBInstanceRecommendationOption$savingsOpportunityAfterDiscounts` / `RDSDBStorageRecommendationOption$savingsOpportunityAfterDiscounts` are reported, and `RDSEffectiveRecommendationPreferences$savingsEstimationMode` ("Describes the savings estimation mode preference applied for calculating savings opportunity for DB instances") reflects the mode in force.

Actual: `recommendation_for()` resolves the account's effective preferences (`store.effective_preferences_for(..)`, which does compute `savings_estimation_mode`) and then uses only `enhanced_infrastructure_metrics` and `look_back_period` from it. Both option kinds are built with `savings_opportunity_after_discounts: None` unconditionally, and `savingsEstimationMode` is hardcoded to `RDSSavingsEstimationMode { source: SAVINGS_ESTIMATION_SOURCE }` regardless of the stored preference. Nothing the client writes for `savingsEstimationMode` on `RdsDBInstance` (which `PutRecommendationPreferences` accepts — the member is not in the handler's resource-type restriction table) changes any byte of the RDS response, so the preference round-trip is unobservable here.

Same class, same operation: the instance option's `projectedUtilizationMetrics` is always `None` and its `performanceRisk` always the modeled default `0`, so the recommended DB instance class is reported with no projection and no risk at all, where AWS reports both.

## Evidence

Spec: `RDSDBInstanceRecommendationOption$savingsOpportunityAfterDiscounts` and `$projectedUtilizationMetrics` and `$performanceRisk` (range 0..4) are modeled members of the option this operation returns; `RDSEffectiveRecommendationPreferences$savingsEstimationMode` documents the applied preference; `PutRecommendationPreferencesRequest$savingsEstimationMode` — "Specify the AfterDiscounts status to activate the preference" — places no resource-type restriction, and the handler's restriction table in `src/ops/put_recommendation_preferences.rs` does not list it, so an RdsDBInstance-scoped preference IS accepted and stored. Handler: `src/ops/get_rds_database_recommendations.rs` `recommendation_for()` — `let effective = store.effective_preferences_for(..)` then only `effective.enhanced_infrastructure_metrics` / `effective.look_back_period` are read; `savings_opportunity_after_discounts: None` in both the instance and the storage option; `savings_estimation_mode: Some(RDSSavingsEstimationMode { source: Some(crate::state::SAVINGS_ESTIMATION_SOURCE..) })`; `projected_utilization_metrics: None`, `performance_risk: 0.0`. Contrast `src/ops/get_auto_scaling_group_recommendations.rs`, which gates `savings_opportunity_after_discounts` on `preferences.after_discounts()` — the resolution exists and is used there. Tests: `tests/all/rds_database_recommendations.rs` contains no assertion on `savingsOpportunityAfterDiscounts` or `savingsEstimationMode` (grep finds none), so nothing pins the current behaviour.

## Steps to Reproduce

1. Boot the emulator; call UpdateEnrollmentStatus(status=Active).
2. Call PutRecommendationPreferences(resourceType='RdsDBInstance', scope={'name':'AccountId','value':<caller account>}, savingsEstimationMode='AfterDiscounts') and observe 200.
3. Call GetRecommendationPreferences(resourceType='RdsDBInstance', scope=same) and confirm the preference is stored.
4. Call GetRDSDatabaseRecommendations and inspect `rdsDBRecommendations[0]`.
5. Observe `instanceRecommendationOptions[0].savingsOpportunityAfterDiscounts` absent, `storageRecommendationOptions[0].savingsOpportunityAfterDiscounts` absent, and `effectiveRecommendationPreferences.savingsEstimationMode` identical to the response taken before the preference was written — i.e. the activated preference has no observable effect.
6. Also observe `instanceRecommendationOptions[0]` carries no `projectedUtilizationMetrics` and `performanceRisk: 0`.
