Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: bed9f8e2d85c054efb957ff95014081869f003aa

# DescribeScalingPlans silently matches nothing for a ScalingPlanVersion other than 1 instead of refusing the member

_Filed: 2026-10-27T01:42:21Z_
_Operation: DescribeScalingPlans | Class: validation_

> _Note: this issue was identified against the code base at commit `bed9f8e2d85c054efb957ff95014081869f003aa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DescribeScalingPlansRequest$ScalingPlanVersion` is documented with exactly one valid value ("Currently, the only valid value is 1"), and this crate already owns a shared helper that turns that sentence into a refusal — `crate::state::validate_plan_version` — which both sibling reads call (`DescribeScalingPlanResources` and `GetScalingPlanResourceForecastData`). `DescribeScalingPlans` is the one request shape carrying the member that never calls it: it enforces only the member's SECOND documented sentence ("If you specify a scaling plan version, you must also specify a scaling plan name") and then folds the version into the match predicate, so `{"ScalingPlanNames":["held plan"],"ScalingPlanVersion":2}` answers HTTP 200 with `ScalingPlans: []`.

That is exactly the answer the helper's own doc declares unacceptable for an operation in this position: "a value other than that one is a request this service refuses on the member itself, before any lookup, rather than a lookup that quietly matches nothing. Where the operation also models no not-found shape, refusing the member is the ONLY answer that tells a caller its request was malformed: an empty collection would report a version that cannot exist as merely holding nothing." `DescribeScalingPlans` models no not-found shape either (its errors are `ConcurrentUpdateException`, `InternalServiceException`, `InvalidNextTokenException`, `ValidationException`), so a caller that mistypes the version is told "you have no such plan" rather than "that version cannot exist" — and the three operations that carry the member now disagree three ways about the same member (refuse on the member / `ObjectNotFoundException` on the mutators / silent empty page here).

This is a different gap from the already-fixed DescribeScalingPlans validation issue, which was about the ten-entry `ApplicationSources` bound.

## Evidence

Model text (`smithy_query(query="doc",shape="DescribeScalingPlansRequest",member="ScalingPlanVersion")`): "The version number of the scaling plan. Currently, the only valid value is 1." followed by the note "If you specify a scaling plan version, you must also specify a scaling plan name." The same sentence is reproduced in the handler's own generated contract header — services/.../src/ops/describe_scaling_plans.rs:60-62:
```
//! * `ScalingPlanVersion` (ScalingPlanVersion) — The version number of the scaling plan.
//!   Currently, the only valid value is 1. If you specify a scaling plan version, you must
//!   also specify a scaling plan name.
```

The handler enforces only the second sentence — describe_scaling_plans.rs:175-180:
```
        // "If you specify a scaling plan version, you must also specify a scaling plan name."
        if input.scaling_plan_version.is_some() && names.is_empty() {
            return Err(validation(
                "ScalingPlanVersion can only be specified together with ScalingPlanNames",
            ));
        }
```
and then turns the version into a silent filter — describe_scaling_plans.rs:208-214:
```
        let matching = state.scaling_plans.values().filter(|plan| {
            if !wanted_names.is_empty() {
                return wanted_names.contains(plan.name.as_str())
                    && input
                        .scaling_plan_version
                        .is_none_or(|version| version == plan.version);
            }
```
No call to `crate::state::validate_plan_version` exists anywhere in this file (the only `validate_*` calls are `validate_application_source` at :204 and `effective_max_results` at :182).

The sibling read does the opposite with the identical member — src/ops/describe_scaling_plan_resources.rs:171-174:
```
        // `ScalingPlanVersion` documents exactly one valid value, so any other version is
        // a request this service refuses on the member rather than a lookup that misses —
        // the same rule the forecast read applies to its own copy of this member.
        crate::state::validate_plan_version(input.scaling_plan_version).map_err(validation)?;
```
and the shared helper's doc states the rule as covering every request shape that carries the member — src/state.rs:1282-1298:
```
/// Validate a `ScalingPlanVersion` a caller wrote, on an operation that identifies ONE
/// plan by it and models no not-found error.
///
/// The member documents exactly one valid value ([`ONLY_VERSION`]) — "The version number
/// of the scaling plan. Currently, the only valid value is 1", on every request shape that
/// carries it — so a value other than that one is a request this service refuses on the
/// member itself, before any lookup, rather than a lookup that quietly matches nothing.
/// Where the operation also models no not-found shape, refusing the member is the ONLY
/// answer that tells a caller its request was malformed: an empty collection would report
/// a version that cannot exist as merely holding nothing.
///
/// `pub` so every such operation refuses the member with the same sentence.
```

Test coverage: nothing pins the current behaviour. tests/all/plan_version_member_validation.rs drives its version table only over `READS`, which is defined as the other two reads — tests/all/absent_plan_reads.rs:47-50:
```
pub(crate) const READS: [(&str, &str); 2] = [
    ("DescribeScalingPlanResources", "ScalingPlanResources"),
    ("GetScalingPlanResourceForecastData", "Datapoints"),
];
```
Every `DescribeScalingPlans` case in tests/all/scaling_plan_lifecycle.rs:1150-1201 and tests/all/describe_scaling_plans_selector_bounds.rs uses `"ScalingPlanVersion": 1` or omits the member (the only version row is `("version without a name", json!({ "ScalingPlanVersion": 1 }))` at scaling_plan_lifecycle.rs:1158), so no assertion expects a 200 for version != 1.

## Steps to Reproduce

1. CreateScalingPlan with ScalingPlanName "held plan", a valid ApplicationSource and one valid ScalingInstruction — HTTP 200, ScalingPlanVersion 1.
2. Call DescribeScalingPlans with {"ScalingPlanNames":["held plan"],"ScalingPlanVersion":2}.
3. Observe HTTP 200 with "ScalingPlans": [] — the version was used as a filter (describe_scaling_plans.rs:208-214) instead of being refused on the member.
4. Call DescribeScalingPlanResources with {"ScalingPlanName":"held plan","ScalingPlanVersion":2} and observe HTTP 400 ValidationException naming ScalingPlanVersion, the value and the only valid value (describe_scaling_plan_resources.rs:174 via state.rs:1298) — the two reads disagree about the same member.
5. Repeat step 2 with version 0, -1 and 4294967296: each is a 200 empty page.
