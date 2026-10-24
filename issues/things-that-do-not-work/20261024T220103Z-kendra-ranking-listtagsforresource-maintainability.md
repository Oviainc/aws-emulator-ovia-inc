Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 564feb857476f3247a0693cb65aa2d44e3322d3a

# The "ARN resolves to no plan" refusal is built three times in three handlers instead of once, unlike this crate's single-source not-found helper

_Filed: 2026-10-24T22:01:03Z_
_Operation: ListTagsForResource | Class: maintainability_

> _Note: this issue was identified against the code base at commit `564feb857476f3247a0693cb65aa2d44e3322d3a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate deliberately centralises the twin rule: the plan-id not-found refusal is built in exactly one place (`state::plan_not_found`), documented as "the ONE place that message is built, so every plan-lookup path states it identically". The ARN-addressed equivalent is not centralised: the identical ten-line block (resolve the ARN through `plan_by_arn`/`plan_by_arn_mut`, build `ResourceUnavailableException` with the same interpolated sentence) is written out verbatim in all three tagging handlers. Three copies of one rule can drift — a reworded sentence, a different error variant, or a changed lookup in one handler leaves the other two stating something else for the same failure, which is exactly what the `plan_not_found` comment exists to prevent. Lifting it into `state.rs` beside `plan_not_found` (e.g. one `resource_unavailable(resource_arn)` constructor) leaves every response byte-identical and every existing test unchanged.

## Evidence

The convention the crate already applies to the sibling rule — services/.../src/state.rs:48-57:
```
/// The modelled `ResourceNotFoundException` for a plan id the store does not hold —
/// the ONE place that message is built, so every plan-lookup path states it
/// identically.
...
pub fn plan_not_found() -> ResourceNotFoundException {
    ResourceNotFoundException { message: Some(PLAN_NOT_FOUND_MESSAGE.to_string()) }
}
```

Copy 1 — services/.../src/ops/list_tags_for_resource.rs:165-175:
```
        let Some(plan) = store.plan_by_arn(&input.resource_arn) else {
            return Err(ListTagsForResourceErrorModeled::ResourceUnavailableException(
                ResourceUnavailableException {
                    message: Some(format!(
                        "Resource {} was not found. Please check you have provided the correct resource information and try again.",
                        input.resource_arn
                    )),
                },
            )
            .into());
        };
```

Copy 2 — services/.../src/ops/tag_resource.rs:165-175:
```
        let Some(plan) = store.plan_by_arn_mut(&input.resource_arn) else {
            return Err(TagResourceErrorModeled::ResourceUnavailableException(
                ResourceUnavailableException {
                    message: Some(format!(
                        "Resource {} was not found. Please check you have provided the correct resource information and try again.",
                        input.resource_arn
                    )),
                },
            )
            .into());
        };
```

Copy 3 — services/.../src/ops/untag_resource.rs:162-172: the same block again, with `UntagResourceErrorModeled::ResourceUnavailableException`.

Existing tests that pin the behaviour the refactor must not change: services/.../tests/all/resource_tags.rs:289-351 (`an_arn_that_names_no_plan_is_refused_with_resource_unavailable`, five ARN classes x the three operations) and services/.../tests/all/wire_error_status.rs:174-194 (the `ResourceUnavailableException` rows of the client-error census, asserting the code and a non-empty message for all three operations).

## Steps to Reproduce

1. Open src/ops/list_tags_for_resource.rs:165, src/ops/tag_resource.rs:165 and src/ops/untag_resource.rs:162 side by side: the ARN-resolution-plus-refusal block is character-for-character the same except for the error enum and the `_mut` lookup.
2. Compare with src/state.rs:48-57, where the crate states that the sibling not-found message has exactly ONE construction site so every lookup path states it identically.
3. Change the sentence in one handler only (e.g. tag_resource.rs) and re-run `cargo test -p service-com-amazonaws-kendraranking-awskendrarerankingfrontendservice`: the suites in tests/all/resource_tags.rs and tests/all/wire_error_status.rs still pass, so nothing in the crate holds the three copies in agreement.
