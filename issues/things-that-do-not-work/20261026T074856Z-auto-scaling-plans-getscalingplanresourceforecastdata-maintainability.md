Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 653ea257673c095df18531588ef6bfc9ad1ec3b6

# The "configures predictive scaling" rule is implemented twice in this crate

_Filed: 2026-10-26T07:48:55Z_
_Operation: GetScalingPlanResourceForecastData | Class: maintainability_

> _Note: this issue was identified against the code base at commit `653ea257673c095df18531588ef6bfc9ad1ec3b6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The rule that decides whether a scaling instruction configures predictive scaling — a load metric specification of either kind being present — is written out twice in this crate: once as the private helper `configures_predictive_scaling` in `src/state.rs`, used by the validation path shared by CreateScalingPlan and UpdateScalingPlan, and again inline in `ResourceForecast::of` in the forecast handler, which recomputes the same disjunction with its own local binding. The two copies can drift: if the model's notion of "configuring predictive scaling" is ever refined on one side, the write path would refuse configurations the forecast path still forecasts (or vice versa). Making the existing `state::configures_predictive_scaling` helper `pub` and calling it from `ResourceForecast::of` leaves every response byte-identical and every existing test passing, because both copies evaluate the same two `Option` members.

## Evidence

Copy 1 — services/service-com-amazonaws-autoscalingplans-anyscalescalingplannerfrontendservice/src/state.rs:812-815:
```
fn configures_predictive_scaling(instruction: &ScalingInstruction) -> bool {
    instruction.predefined_load_metric_specification.is_some()
        || instruction.customized_load_metric_specification.is_some()
}
```
Copy 2 — services/service-com-amazonaws-autoscalingplans-anyscalescalingplannerfrontendservice/src/ops/get_scaling_plan_resource_forecast_data.rs:654-661:
```
    fn of(instruction: &ScalingInstruction) -> Option<Self> {
        let configures_predictive_scaling = instruction
            .predefined_load_metric_specification
            .is_some()
            || instruction.customized_load_metric_specification.is_some();
        if !configures_predictive_scaling {
            return None;
        }
```
Both cite the same model sentence (state.rs:807-809 and get_scaling_plan_resource_forecast_data.rs:650-652 quote "This parameter or a CustomizedLoadMetricSpecification is required when configuring predictive scaling, and cannot be used otherwise").
Existing tests covering both copies: tests/all/forecast_data.rs:840 `a_plan_configuring_no_predictive_scaling_reports_an_empty_forecast` (the forecast copy), and tests/all/scaling_mode_conditional_members.rs:249 `create_refuses_a_conditional_member_the_instructions_modes_do_not_allow` / :288 `update_refuses_the_same_conditional_members_create_refuses` (the state.rs copy). Both must stay green and unchanged.

## Steps to Reproduce

1. Open src/state.rs and read lines 812-815 (`configures_predictive_scaling`)
2. Open src/ops/get_scaling_plan_resource_forecast_data.rs and read lines 654-661 (`ResourceForecast::of`)
3. Observe the same two-member disjunction is implemented independently in both places, with no call from the second to the first
