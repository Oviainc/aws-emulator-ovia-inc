Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: b4c888ef8ded8aa913bb7b3b9f51a492e4dac1e5

# The TargetTrackingConfigurations entry-count refusal bypasses the crate's single documented-entry-count wording

_Filed: 2026-10-27T01:55:55Z_
_Operation: CreateScalingPlan | Class: error-response_

> _Note: this issue was identified against the code base at commit `b4c888ef8ded8aa913bb7b3b9f51a492e4dac1e5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::entry_count_message` is documented as "the single place the whole crate words" the refusal of a request list member that holds more entries than its own documentation allows, and `check_entry_count` is the predicate built on it — used for `applicationSource.tagFilters`, `applicationSource.tagFilters[i].values`, `scalingPlanNames` and `applicationSources`.

`ScalingInstruction$TargetTrackingConfigurations` is exactly such a member: its documentation states a maximum entry count ("The target tracking configurations (up to 10).", captured as `MAX_TARGET_TRACKING_CONFIGURATIONS`) and the list shape carries no modelled `@length` (`smithy_query(query="shape",shape="TargetTrackingConfigurations")` returns a bare `list` of `TargetTrackingConfiguration`), so the bound is the handler's to apply. But `validate_target_tracking_configurations` words that same refusal a SECOND time, inline and differently: it omits the "1 validation error detected: Value at '…'" prefix the helper renders and uses its own "Value of length N" form. The comment claiming a single place is therefore false, and the two copies of one rule can drift: a client (or a future bound added to another list) sees two different renderings of the same documented-entry-count violation on one service.

## Evidence

The crate's stated single source of truth:
```
/// A list member whose documentation states a maximum entry count is bounded by it even
/// when the list shape carries no modelled `@length`, so nothing before the handler
/// applies it: this is the single place the whole crate words that refusal. `member` is
/// the member's wire path, as AWS's validation failures name it, and `max` the count its
/// own documentation states.
pub fn entry_count_message(member: &str, max: usize) -> String {
    format!(
        "1 validation error detected: Value at '{member}' failed to satisfy constraint: \
         Member must have length less than or equal to {max}"
    )
}
```
(src/state.rs:513-527, with `check_entry_count` at src/state.rs:529-536).

The second, divergent copy of the same rule, reached from `CreateScalingPlan`/`UpdateScalingPlan` via `validate_scaling_instructions`:
```
if configurations.len() > MAX_TARGET_TRACKING_CONFIGURATIONS {
    return Err(format!(
        "{member}: Value of length {} failed to satisfy constraint: Member must have \
         length less than or equal to {MAX_TARGET_TRACKING_CONFIGURATIONS}",
        configurations.len()
    ));
}
```
(src/state.rs:1002-1008), on a member whose documented bound is recorded in the same file:
```
/// Documented bound on `ScalingInstruction$TargetTrackingConfigurations`: "The target
/// tracking configurations (up to 10)."
pub const MAX_TARGET_TRACKING_CONFIGURATIONS: usize = 10;
```
(src/state.rs:594-596).

Callers of the helper, for comparison: `check_entry_count("applicationSource.tagFilters", filters.len(), MAX_TAG_FILTERS)?` (src/state.rs:550) and `check_entry_count("scalingPlanNames", names.len(), MAX_PLAN_NAMES)?` / `check_entry_count("applicationSources", sources.len(), MAX_APPLICATION_SOURCES)?` (src/ops/describe_scaling_plans.rs:173-174), whose wording is pinned by tests/all/tag_filter_entry_bounds.rs:53-54 and tests/all/describe_scaling_plans_selector_bounds.rs:75-76 (`"1 validation error detected: Value at '{wire_member}' failed to satisfy constraint: Member must have length less than or equal to {bound}"`). The TargetTrackingConfigurations case is exercised by tests/all/scaling_instruction_validation.rs:186-191 ("one target tracking configuration past the documented bound") but only its code is asserted — `assert_eq!((status, code.as_str()), (400, "ValidationException"), ...)` (tests/all/scaling_instruction_validation.rs:588-592) — so no test pins its divergent text.

## Steps to Reproduce

1. CreateScalingPlan with an ApplicationSource carrying 51 TagFilters → 400 ValidationException with "1 validation error detected: Value at 'applicationSource.tagFilters' failed to satisfy constraint: Member must have length less than or equal to 50".
2. CreateScalingPlan with one valid ScalingInstruction whose TargetTrackingConfigurations holds 11 entries → 400 ValidationException with "ScalingInstructions[0].TargetTrackingConfigurations: Value of length 11 failed to satisfy constraint: Member must have length less than or equal to 10".
3. Compare the two messages: the same documented-entry-count rule is worded in two different formats, contradicting the `entry_count_message` doc at src/state.rs:518 that calls itself "the single place the whole crate words that refusal".
