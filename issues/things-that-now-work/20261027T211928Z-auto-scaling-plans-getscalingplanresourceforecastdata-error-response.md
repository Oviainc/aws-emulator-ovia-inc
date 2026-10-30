Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: c0eb4295fc6918b9011f996ea325c28045b54b65

# An off-enum ServiceNamespace/ScalableDimension on the forecast read is refused with the resource sentence instead of the crate's single off-enum sentence

_Filed: 2026-10-27T21:19:28Z_
_Operation: GetScalingPlanResourceForecastData | Class: error-response_

> _Note: this issue was identified against the code base at commit `c0eb4295fc6918b9011f996ea325c28045b54b65`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::one_of` is the crate's single owner of the refusal for "a value the model's own enum does not list", and its doc says so explicitly: it is `pub` so that EVERY handler refuses an off-enum value with the same sentence. The write paths follow that rule for `ServiceNamespace` and `ScalableDimension` (`validate_scaling_instruction` calls `one_of` for both members before anything else).

`GetScalingPlanResourceForecastData` carries the same two members, but `validate_forecast_resource` never asks `one_of` about them: it compares each against the single documented valid value and, on any mismatch, answers `UNSUPPORTED_RESOURCE` ("Unsupported service namespace, resource type or scalable dimension"). That sentence is the crate's wording for a well-formed triple that does not describe a scalable resource; it is now also the answer for a member value the model's enum does not contain at all. So one and the same off-enum value — e.g. `ServiceNamespace: "bogus"` — is refused with two different sentences depending on which operation received it, which is exactly what the `one_of` doc claims cannot happen. The enum-set refusal, which names the member and the accepted values, is the only one that tells the caller the value is not a modelled enum member; the resource sentence names neither the member nor the value.

(The generated input types carry these members as `String`, and the forecast handler itself calls `one_of` for `ForecastDataType` on the line above — so the handler is the place that owns the enum check here, not the framework.)

## Evidence

The rule, stated in the crate — services/service-com-amazonaws-autoscalingplans-anyscalescalingplannerfrontendservice/src/state.rs:1401-1414:
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

The sibling (write) path obeys it for these two members — src/state.rs:1027-1036:
```
    one_of(
        &at("ServiceNamespace"),
        &instruction.service_namespace,
        SERVICE_NAMESPACE_VALUES,
    )?;
    one_of(
        &at("ScalableDimension"),
        &instruction.scalable_dimension,
        SCALABLE_DIMENSION_VALUES,
    )?;
```

The forecast read does not — src/ops/get_scaling_plan_resource_forecast_data.rs:377-386:
```
fn validate_forecast_resource(
    input: &GetScalingPlanResourceForecastDataInput,
) -> Result<(), String> {
    if input.service_namespace != FORECAST_SERVICE_NAMESPACE
        || input.scalable_dimension != FORECAST_SCALABLE_DIMENSION
    {
        return Err(crate::state::UNSUPPORTED_RESOURCE.to_string());
    }
    crate::state::validate_resource_id(&input.resource_id, &input.scalable_dimension)
}
```
while the same handler does use `one_of` for its other enum member — same file:208-213:
```
        crate::state::one_of(
            "ForecastDataType",
            &input.forecast_data_type,
            FORECAST_DATA_TYPE_VALUES,
        )
        .map_err(validation)?;
```
and `UNSUPPORTED_RESOURCE` is documented as the sentence for a resource triple, not for an off-enum member — src/state.rs:846-855: "How the service refuses a `(ServiceNamespace, ResourceId, ScalableDimension)` triple that does not describe one scalable resource".

Test coverage: tests/all/forecast_data.rs and tests/all/scaling_instruction_validation.rs drive the off-enum refusals for the instruction members (the `one_of` sentence) and the unsupported-triple refusal for the forecast read; no test asserts the forecast read's message for an off-enum `ServiceNamespace`/`ScalableDimension`, so the divergence is unpinned.

## Steps to Reproduce

1. CreateScalingPlan with one instruction whose ServiceNamespace is `bogus` and observe the ValidationException message: `ScalingInstructions[0].ServiceNamespace: Value 'bogus' failed to satisfy constraint: Member must satisfy enum value set: [autoscaling, dynamodb, ec2, ecs, rds]`.
2. Call GetScalingPlanResourceForecastData with `ServiceNamespace: "bogus"` (plus any valid ScalingPlanName/ScalingPlanVersion 1, ResourceId `autoScalingGroup/my-asg`, ScalableDimension `autoscaling:autoScalingGroup:DesiredCapacity`, ForecastDataType `CapacityForecast`, and an in-range StartTime/EndTime).
3. Observe the ValidationException message is `Unsupported service namespace, resource type or scalable dimension` — it names neither the member nor the rejected value, instead of the single off-enum sentence `crate::state::one_of` exists to give every handler.
4. Repeat with `ScalableDimension: "bogus"` for the same result.
