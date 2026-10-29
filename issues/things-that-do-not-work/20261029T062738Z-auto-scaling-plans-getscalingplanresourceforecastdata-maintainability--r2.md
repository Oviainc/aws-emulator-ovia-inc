Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: cc767c9c860d2029dd9667f4e637f1644e38e746

# The scalable-resource identity triple is spelled out a second time in the forecast handler instead of using the crate's single definition

_Filed: 2026-10-29T06:27:38Z_
_Operation: GetScalingPlanResourceForecastData | Class: maintainability_

> _Note: this issue was identified against the code base at commit `cc767c9c860d2029dd9667f4e637f1644e38e746`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate has ONE definition of the identity of the scalable resource a scaling instruction applies to — the `(ServiceNamespace, ResourceId, ScalableDimension)` triple in `crate::state` — and three of its four users go through it: `validate_scaling_instructions` (one instruction per resource), `merge_scaling_instructions` (pairing a requested instruction with the one in force) and `resource_identity_key` (the order and the `NextToken` of `DescribeScalingPlanResources`). The fourth user, `GetScalingPlanResourceForecastData::instruction_for`, hand-rolls the same triple comparison member by member, while its own doc cites the same model sentence and calls it "the same (ServiceNamespace, ResourceId, ScalableDimension) triple". `state::resource_identity` is private, so the handler cannot reach the shared definition at all. The two copies can therefore drift: any change to how this service decides that an instruction names a given resource (and the triple is built from caller text under no modelled pattern beyond `^[\u0020-\uD7FF…]*$`) has to be made in two places, and a change made in `state` alone would silently leave the forecast read matching by the old rule — a forecast issued for the wrong instruction, or no forecast for a resource the plan does cover. This crate has already fixed the identical shape of duplication once: `state::configures_predictive_scaling` carries the comment "Public because the forecast path asks the same question" (issue 20261026T074856Z, fixed). Fix: expose the existing `state::resource_identity` and have `instruction_for` compare it against the request's triple. No response and no existing test changes.

## Evidence

Copy 1 — the crate's single definition, `src/state.rs:1038-1044` and `1068-1075`:
```
/// The identity of the one scalable resource a scaling instruction applies to.
///
/// "Each scaling instruction applies to one resource" (`ScalingInstruction` doc), and
/// that resource is named by the `(ServiceNamespace, ResourceId, ScalableDimension)`
/// triple — so this is the identity [`validate_scaling_instructions`] holds one
/// instruction per, and the one the update merge matches on.
pub type ResourceIdentity<'a> = (&'a str, &'a str, &'a str);
...
fn resource_identity(instruction: &ScalingInstruction) -> ResourceIdentity<'_> {
    (
        instruction.service_namespace.as_str(),
        instruction.resource_id.as_str(),
        instruction.scalable_dimension.as_str(),
    )
}
```
(note: not `pub`, so no handler can call it; its users are `state.rs:997`, `state.rs:1002`, `state.rs:1059` and `state.rs:1141`)

Copy 2 — the same rule written out again, `src/ops/get_scaling_plan_resource_forecast_data.rs:550-566`:
```
/// The stored scaling instruction a forecast for the requested resource is issued from.
///
/// "Each scaling instruction applies to one resource"
/// (`smithy_query(query="doc",shape="ScalingInstruction")`), identified by the same
/// `(ServiceNamespace, ResourceId, ScalableDimension)` triple this request carries — so
/// the forecast of a resource is the forecast of the instruction naming it, and a plan
/// holding no instruction for it has no forecast to report.
fn instruction_for<'a>(
    plan: &'a crate::state::ScalingPlanRecord,
    input: &GetScalingPlanResourceForecastDataInput,
) -> Option<&'a ScalingInstruction> {
    plan.scaling_instructions.iter().find(|instruction| {
        instruction.service_namespace == input.service_namespace
            && instruction.resource_id == input.resource_id
            && instruction.scalable_dimension == input.scalable_dimension
    })
}
```

Precedent in this same crate for folding such a copy back, `src/state.rs:1401-1406`:
```
/// Public because the forecast path asks the same question: an instruction configuring
/// no predictive scaling has no forecast to report.
pub fn configures_predictive_scaling(instruction: &ScalingInstruction) -> bool {
```

Existing tests that pin both sides of the behaviour and must stay green unchanged:
- `tests/all/forecast_data.rs:870` — case "a plan that covers another resource" (the forecast of a resource the plan does not instruct)
- `tests/all/forecast_datapoints.rs:659` — "case a plan that instructs another resource / {data_type}: nothing instructs …"
- `tests/all/scaling_instruction_validation.rs` — one instruction per scalable resource (the `state` copy)
- `tests/all/scaling_plan_resource_paging.rs` — the identity-keyed walk built on `resource_identity_key`
- `tests/all/state_roundtrip.rs`

## Steps to Reproduce

1. Open `services/service-com-amazonaws-autoscalingplans-anyscalescalingplannerfrontendservice/src/state.rs` and read lines 1038-1075: `resource_identity` is the crate's single definition of "which scalable resource this instruction applies to", and it is private.
2. Grep its users: `state.rs:997`/`1002` (update merge), `state.rs:1059` (`resource_identity_key`, used by DescribeScalingPlanResources for ordering and its NextToken) and `state.rs:1141` (one-instruction-per-resource validation).
3. Open `src/ops/get_scaling_plan_resource_forecast_data.rs:557-566`: `instruction_for` compares `service_namespace`, `resource_id` and `scalable_dimension` member by member instead of going through that definition.
4. Confirm the drift risk: change the identity rule in `state::resource_identity` (for example by trimming or case-folding a part) and observe that DescribeScalingPlanResources, the update merge and instruction uniqueness follow the new rule while the forecast read keeps matching by the old one.
