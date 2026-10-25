Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 9ff0151e93167cf00d59909edc96bc1d4e3529be

# Enforce the documented ten-entry bound on DescribeScalingPlans ApplicationSources

_Filed: 2026-10-25T10:44:02Z_
_Operation: DescribeScalingPlans | Class: validation_

> _Note: this issue was identified against the code base at commit `9ff0151e93167cf00d59909edc96bc1d4e3529be`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DescribeScalingPlansRequest$ApplicationSources` is documented with the same "(up to 10)" bound as `ScalingPlanNames`, and neither list carries a modelled `@length` trait (so the framework enforces neither). The handler derives `MAX_PLAN_NAMES = 10` from the `ScalingPlanNames` sentence and refuses an eleventh name with `ValidationException`, but it applies no count check at all to `ApplicationSources`: it only walks the list validating each entry's shape. An eleven-entry (or thousand-entry) `ApplicationSources` filter is therefore accepted with HTTP 200, while the identically documented eleven-entry `ScalingPlanNames` filter is refused — one of the two documented "up to 10" bounds is implemented and the sibling one, in the same `handle` body, is not. No comment, test or doc in the crate states a reason for serving the unbounded source list.

## Evidence

Smithy (quoted verbatim): `smithy_query(query="doc",shape="DescribeScalingPlansRequest",member="ApplicationSources")` -> "<p>The sources for the applications (up to 10). If you specify scaling plan names, you cannot specify application sources.</p>"; `smithy_query(query="shape",shape="ApplicationSources")` -> `{"type": "list", "member": "ApplicationSource"}` (no `length` constraint, so nothing enforces it before the handler).

Handler, the bound that IS enforced — services/.../src/ops/describe_scaling_plans.rs:170-176:
```
        // "The names of the scaling plans (up to 10)."
        if names.len() > MAX_PLAN_NAMES {
            return Err(validation(format!(
                "1 validation error detected: Value at 'scalingPlanNames' failed to satisfy \
                 constraint: Member must have length less than or equal to {MAX_PLAN_NAMES}"
            )));
        }
```
with `/// `ScalingPlanNames` is documented as "The names of the scaling plans (up to 10)".` / `const MAX_PLAN_NAMES: usize = 10;` (describe_scaling_plans.rs:246-247).

Handler, the sibling bound that is NOT enforced — describe_scaling_plans.rs:205-208, the only place `sources` is inspected:
```
        for source in &sources {
            let key = crate::state::validate_application_source(source).map_err(validation)?;
            wanted_source_keys.insert(key);
        }
```
(each entry's shape is checked; `sources.len()` never is). The crate's own doc copy of the sentence sits in the same file at describe_scaling_plans.rs:53: "* `ApplicationSources` (ApplicationSources) — The sources for the applications (up to 10)."

Test coverage: tests/all/scaling_plan_lifecycle.rs:1126-1169 is the "documented combination rules" table and contains the row `("more than ten names", json!({ "ScalingPlanNames": eleven }))` (line 1138) asserting a 400 ValidationException, plus rows for malformed single sources (lines 1142-1161) — but no row supplying more than ten `ApplicationSources`. tests/all/error_message_wording.rs:247-250 likewise only has `("eleven names beside a source", ...)`. The over-ten source case is uncovered.

## Steps to Reproduce

1. Create two scaling plans over distinct CloudFormation stack ARNs and let them settle.
2. Call DescribeScalingPlans with `ScalingPlanNames` holding 11 valid names: observe 400 ValidationException ("Member must have length less than or equal to 10").
3. Call DescribeScalingPlans with `ApplicationSources` holding 11 well-formed stack sources (e.g. `arn:aws:cloudformation:us-east-1:000000000000:stack/s-0/1` … `stack/s-10/1`) and no other member.
4. Observe HTTP 200 with a filtered `ScalingPlans` list instead of the 400 ValidationException the documented "(up to 10)" bound calls for.
