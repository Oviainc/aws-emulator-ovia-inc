Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 68144f2b1a9e2dba602611483c97527f7042e8b8

# An off-enum CustomizedLoadMetricSpecification.Statistic is refused with the "only valid value is Sum" sentence instead of the crate's single off-enum sentence

_Filed: 2026-10-28T22:22:46Z_
_Operation: CreateScalingPlan | Class: error-response_

> _Note: this issue was identified against the code base at commit `68144f2b1a9e2dba602611483c97527f7042e8b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::one_of` is this crate's single owner of the refusal for "a value the model's own enum does not list", and its own doc states that it is `pub` precisely so that EVERY handler refuses an off-enum value with the same sentence. The write paths that validate a scaling instruction obey that rule for every other enum-typed member they touch — `ServiceNamespace`, `ScalableDimension`, `PredefinedScalingMetricType`, `PredefinedLoadMetricType`, `PredictiveScalingMode`, `PredictiveScalingMaxCapacityBehavior`, `ScalingPolicyUpdateBehavior`, and — the direct sibling of the member at fault — `CustomizedScalingMetricSpecification.Statistic`, which is refused through `one_of(..., METRIC_STATISTIC_VALUES)`.

`CustomizedLoadMetricSpecification.Statistic` targets the same modelled `MetricStatistic` enum, but `validate_predictive_scaling` never asks `one_of` about it: it compares the raw string against the single documented valid value and answers `"<member>.Statistic: the only valid value is Sum"` for ANY mismatch. That sentence is the crate's wording for an on-enum statistic the member's own documentation excludes (`Average`, `Maximum`, …); it is now also the answer for a value the model's enum does not contain at all (e.g. `"Bogus"`). So one and the same off-enum value is refused with two different sentences depending on which of the two neighbouring customized-metric specifications carried it — in the SAME request, in the SAME helper — which is exactly what the `one_of` doc claims cannot happen. The enum-set sentence is the only one that tells the caller the value is not a modelled enum member: it names the member, the rejected value and the accepted set, none of which the "only valid value is Sum" sentence carries for an unmodelled value.

The generated input type carries the member as a plain `String` (`shared_types.rs:96: pub statistic: String`), so the handler is the place that owns the enum check here, not the framework — the same reason the crate re-checks the other enum members by hand.

This is a different member and a different code path from the already-open forecast-read issue (20261027T211928Z, which covers `GetScalingPlanResourceForecastData`'s `ServiceNamespace`/`ScalableDimension` being refused with `UNSUPPORTED_RESOURCE`): this one is on the two write paths' shared scaling-instruction validation.

## Evidence

The rule, stated in this crate — services/service-com-amazonaws-autoscalingplans-anyscalescalingplannerfrontendservice/src/state.rs:1484-1497:
```
/// Refuse a value the model's own enum does not list, naming the member and the values
/// it accepts.
///
/// `pub` so every handler refuses an off-enum value with the same sentence.
pub fn one_of(member: &str, value: &str, allowed: &[&str]) -> Result<(), String> {
    if allowed.contains(&value) {
        return Ok(());
    }
    Err(format!(
        "{member}: Value '{value}' failed to satisfy constraint: Member must satisfy enum \
         value set: [{}]",
        allowed.join(", ")
    ))
}
```

The direct sibling member obeys it — state.rs:1287-1291 (inside `validate_target_tracking_configuration`):
```
        (None, Some(customized)) => one_of(
            &format!("{at}.CustomizedScalingMetricSpecification.Statistic"),
            &customized.statistic,
            METRIC_STATISTIC_VALUES,
        )?,
```

The load-metric side does not — state.rs:1421-1429 (inside `validate_predictive_scaling`):
```
    if let Some(specification) = customized {
        // "The statistic of the metric. The only valid value is Sum."
        if specification.statistic != LOAD_METRIC_STATISTIC {
            return Err(format!(
                "{}.Statistic: the only valid value is {LOAD_METRIC_STATISTIC}",
                at("CustomizedLoadMetricSpecification")
            ));
        }
    }
```

Both members target the same modelled enum — `smithy_query(query="shape",shape="CustomizedLoadMetricSpecification,CustomizedScalingMetricSpecification")`:
- `CustomizedLoadMetricSpecification`: `"Statistic": {"target": "MetricStatistic", "required": true, "type": "enum", "doc": "<p>The statistic of the metric. The only valid value is <code>Sum</code>.</p>"}`
- `CustomizedScalingMetricSpecification`: `"Statistic": {"target": "MetricStatistic", "required": true, "type": "enum", "doc": "<p>The statistic of the metric.</p>"}`

and the enum's value list is mirrored in the crate — src/ops/types/shared_types.rs:139:
```
pub const METRIC_STATISTIC_VALUES: &[&str] = &["Average", "Maximum", "Minimum", "SampleCount", "Sum"];
```

The path is reached from `CreateScalingPlan` first — src/ops/create_scaling_plan.rs:169-170:
```
        crate::state::validate_scaling_instructions(&input.scaling_instructions)
            .map_err(validation)?;
```
(and identically from `UpdateScalingPlan` — src/ops/update_scaling_plan.rs:193-195).

Test coverage: the only row that drives a bad load statistic sends an ON-enum value — tests/all/scaling_instruction_validation.rs:306-312:
```
        (
            "a customized load metric whose statistic is not Sum",
            json!([with(&[(
                "CustomizedLoadMetricSpecification",
                json!({ "MetricName": "my-load", "Namespace": "MyApp", "Statistic": "Average" })
            )])]),
        ),
```
so checking the enum set FIRST and keeping the Sum sentence for an on-enum value leaves that row — and every other test — unchanged; no test asserts the message for an off-enum load statistic, so the divergence is unpinned.

## Steps to Reproduce

1. POST / with `X-Amz-Target: AnyScaleScalingPlannerFrontendService.CreateScalingPlan` and a body whose single scaling instruction carries `"CustomizedScalingMetricSpecification": {"MetricName":"m","Namespace":"MyApp","Statistic":"Bogus"}` inside its `TargetTrackingConfigurations`. Observe the 400 `ValidationException` message: `ScalingInstructions[0].TargetTrackingConfigurations[0].CustomizedScalingMetricSpecification.Statistic: Value 'Bogus' failed to satisfy constraint: Member must satisfy enum value set: [Average, Maximum, Minimum, SampleCount, Sum]`.
2. Repeat the same create, this time with a valid predefined scaling metric configuration and `"CustomizedLoadMetricSpecification": {"MetricName":"my-load","Namespace":"MyApp","Statistic":"Bogus"}` on the instruction.
3. Observe the 400 `ValidationException` message is `ScalingInstructions[0].CustomizedLoadMetricSpecification.Statistic: the only valid value is Sum` — it neither names the rejected value nor says that the value is outside the modelled `MetricStatistic` set, so the same off-enum value on the same enum is refused with two different sentences in one request shape.
4. Send the on-enum-but-invalid value `"Statistic":"Average"` on the customized LOAD metric and observe the same "only valid value is Sum" sentence — the sentence that is correct for this case and that the off-enum case borrows.
