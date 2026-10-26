Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: ba10b2e9c791e1406ed6cd1db28496a7b9239da3

# State the "which Regions are opt targets" rule once instead of copying the same match into EnableRegion and DisableRegion

_Filed: 2026-10-24T23:52:58Z_
_Operation: EnableRegion | Class: maintainability_

> _Note: this issue was identified against the code base at commit `ba10b2e9c791e1406ed6cd1db28496a7b9239da3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The rule that decides whether a `RegionName` can be the target of a Region opt call — unknown Region code → `unknown_region()` ValidationException, catalog-default Region → `region_not_opt_target()` ValidationException, otherwise proceed — is written out twice, branch for branch, in `enable_region.rs` and `disable_region.rs`. The same pair of handlers also each carry an identical, admittedly unreachable `RegionOptState::EnabledByDefault` arm that repeats the second refusal a third and fourth time.

The two copies can drift: a change to which statuses count as opt targets (or to which refusal each case earns) has to be made in both files, and nothing in the crate ties them together. The error payloads already live in one place (`state::unknown_region`, `state::region_not_opt_target`), so one helper in `state.rs` that answers "is this Region an opt target, and if not which refusal does it earn" would leave every response byte-identical and every existing test passing.

## Evidence

Copy 1 — services/.../src/ops/enable_region.rs:202-217:
```
let region_name = input.region_name;
match crate::state::region_kind(&region_name) {
    None => {
        return Err(EnableRegionErrorModeled::ValidationException(
            crate::state::unknown_region(&region_name),
        )
        .into())
    }
    Some(RegionKind::EnabledByDefault) => {
        return Err(EnableRegionErrorModeled::ValidationException(
            crate::state::region_not_opt_target(&region_name),
        )
        .into())
    }
    Some(RegionKind::OptIn) => {}
}
```
Copy 2 — services/.../src/ops/disable_region.rs:203-218 is the same fifteen lines with `DisableRegionErrorModeled` substituted:
```
let region_name = input.region_name;
match crate::state::region_kind(&region_name) {
    None => {
        return Err(DisableRegionErrorModeled::ValidationException(
            crate::state::unknown_region(&region_name),
...
    Some(RegionKind::EnabledByDefault) => {
        return Err(DisableRegionErrorModeled::ValidationException(
            crate::state::region_not_opt_target(&region_name),
```
The duplicated unreachable arm: enable_region.rs:252-259 and disable_region.rs:253-260, both "// Not reachable: the catalog check above already refused a default-enabled Region" followed by the same `region_not_opt_target` refusal.

Tests that pin the behaviour and must stay unchanged: tests/all/region_opt_in.rs:396 `a_value_that_is_not_a_region_code_is_refused_by_every_region_operation` and tests/all/region_opt_in.rs:209 `a_default_enabled_region_reports_enabled_by_default_and_is_not_an_opt_target`.

## Steps to Reproduce

1. Open services/service-com-amazonaws-account-account/src/ops/enable_region.rs:202-217 and services/service-com-amazonaws-account-account/src/ops/disable_region.rs:203-218 side by side: the region-opt-target rule is the same match, duplicated.
2. Note the second duplication of the same refusal in the unreachable `RegionOptState::EnabledByDefault` arms (enable_region.rs:252-259, disable_region.rs:253-260).
3. Run `cargo test -p service-com-amazonaws-account-account region_opt_in` to see the tests that cover both copies and must keep passing unchanged after the rule is stated once in `state.rs`.
