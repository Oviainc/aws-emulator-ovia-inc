Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 617cb494a3d8f9d308b6478ef47b3df7665967e2

# ApplicationSource TagFilters and TagFilter Values accept more entries than their documented maximums

_Filed: 2026-10-26T08:43:15Z_
_Operation: CreateScalingPlan | Class: validation_

> _Note: this issue was identified against the code base at commit `617cb494a3d8f9d308b6478ef47b3df7665967e2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::validate_application_source` is the single gate every path that carries an `ApplicationSource` runs (CreateScalingPlan, UpdateScalingPlan and the `ApplicationSources` filter of DescribeScalingPlans). It checks stack-ARN/tag-filter exclusivity and the ARN format, but never the two entry counts the model documents for the tag-filter side: `ApplicationSource$TagFilters` is "A set of tags (up to 50)." and `TagFilter$Values` is "The tag values (0 to 20)." Neither list carries a modelled `@length`, so nothing before the handler enforces them, and an `ApplicationSource` with 500 tag filters, or one filter carrying 2000 tag values, is accepted and stored with HTTP 200 instead of being refused with the modelled `ValidationException`.

This is the exact rule the crate already implements for its sibling documented counts: `MAX_TARGET_TRACKING_CONFIGURATIONS` for "(up to 10)" on `ScalingInstruction$TargetTrackingConfigurations`, and `MAX_PLAN_NAMES`/`MAX_APPLICATION_SOURCES` for "(up to 10)" on the two `DescribeScalingPlans` selector lists — whose own comment states the principle being skipped here: "Each selector list is documented with its own maximum entry count, and neither carries a modelled `@length`, so both bounds are applied here — before the request is served — rather than only the one." No comment, test or doc in the crate justifies leaving the tag-filter counts unbounded.

## Evidence

Model text, verbatim — `smithy_query(query="shape",shape="ApplicationSource")`: `"TagFilters": {"target": "TagFilters", "type": "list", "doc": "<p>A set of tags (up to 50).</p>"}`; `smithy_query(query="shape",shape="TagFilter")`: `"Values": {"target": "TagValues", "type": "list", "doc": "<p>The tag values (0 to 20).</p>"}`. Neither list shape carries a length constraint: `smithy_query(query="shape",shape="TagFilters,TagValues")` returns `{"TagFilters": {"type": "list", "member": "TagFilter"}, "TagValues": {"type": "list", "member": {..."length": {"min": 1, "max": 256}...}}}` — the only length is on the VALUE string, not on either list.

The gate that skips both — services/service-com-amazonaws-autoscalingplans-anyscalescalingplannerfrontendservice/src/state.rs:348-368:
```
pub fn validate_application_source(source: &ApplicationSource) -> Result<String, String> {
    let stack_arn = named_stack_arn(source);
    if let Some(arn) = stack_arn {
        if !is_cloud_formation_stack_arn(arn) {
            return Err(format!("Invalid CloudFormation stack ARN: {arn}"));
        }
    }
    match (stack_arn.is_some(), names_tag_filters(source)) {
        (true, true) => Err(...),
        (false, false) => Err(...),
        _ => application_source_key(source).ok_or_else(...)
    }
}
```
(no count is read anywhere), reached from src/ops/create_scaling_plan.rs:164-165:
```
        let source_key = crate::state::validate_application_source(&input.application_source)
            .map_err(validation)?;
```

The sibling bounds the SAME crate does enforce — src/state.rs:381-383:
```
/// Documented bound on `ScalingInstruction$TargetTrackingConfigurations`: "The target
/// tracking configurations (up to 10)."
pub const MAX_TARGET_TRACKING_CONFIGURATIONS: usize = 10;
```
and src/ops/describe_scaling_plans.rs:170-174 + 244-249:
```
        // Each selector list is documented with its own maximum entry count, and
        // neither carries a modelled `@length`, so both bounds are applied here —
        // before the request is served — rather than only the one.
        check_entry_count("scalingPlanNames", names.len(), MAX_PLAN_NAMES)?;
        check_entry_count("applicationSources", sources.len(), MAX_APPLICATION_SOURCES)?;
...
/// `ScalingPlanNames` is documented as "The names of the scaling plans (up to 10)".
pub const MAX_PLAN_NAMES: usize = 10;
```

Test coverage: tests/all/application_source_exclusivity.rs:60-91 is the suite over tag-filter sources and its widest row carries three filters with at most three values each (lines 75-85); no test anywhere in the crate sends more than a handful of tag filters or tag values (grep for `TagFilters` finds only that suite and tests/all/scaling_plan_lifecycle.rs), so adding the two bounds leaves every existing row accepted and green.

## Steps to Reproduce

1. CreateScalingPlan with ScalingPlanName "too-many-tags", one valid ScalingInstruction, and ApplicationSource {"TagFilters": [ ... 60 distinct {"Key":"k<i>","Values":["v"]} entries ... ]}.
2. Observe HTTP 200 with ScalingPlanVersion 1, although `ApplicationSource$TagFilters` is documented "A set of tags (up to 50)." and nothing in the model constrains the list length.
3. DescribeScalingPlans and observe the plan stored with all 60 filters.
4. Repeat with a single filter carrying 50 values ({"Key":"k","Values":["v1",...,"v50"]}) — also accepted, although `TagFilter$Values` is documented "The tag values (0 to 20)."
5. For contrast, send DescribeScalingPlans with 11 ApplicationSources: that documented "(up to 10)" count IS refused with a 400 ValidationException (describe_scaling_plans.rs:173-174, 255-266).
