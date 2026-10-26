Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: a4b61295d097ae4da0c45de1d4f9e67ffc2c86fd

# The request-body half of the [Default policies only] parameter list is implemented twice

_Filed: 2026-10-25T16:29:43Z_
_Operation: UpdateLifecyclePolicy | Class: maintainability_

> _Note: this issue was identified against the code base at commit `a4b61295d097ae4da0c45de1d4f9e67ffc2c86fd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The rule "which request-body members are the `[Default policies only]` parameters" is written out twice in this crate: once in `create_lifecycle_policy.rs` and once, character for character, in `update_lifecycle_policy.rs`. Both functions build the same `crate::state::DefaultPolicyOnlyParameters` from the same six body members and both hard-code `resource_type: false`.

The structure half of the rule already lives in exactly one place (`DefaultPolicyOnlyParameters::in_policy_details`), and so does the wire-name list (`DefaultPolicyOnlyParameters::stated_names`) — whose doc comment states that single-place intent. The body half is the only copy kept twice, and both copies feed two refusals each (`default_policy_only_parameters_refused` and `configuration_stated_twice`), so a member added to or removed from one copy silently makes the create and the update refuse different sets. The crate already has the idiom for collapsing this: `src/service.rs` declares the `DlmRequest` trait with one impl per input shape to read the one member two operations share.

A fix is a single trait (or helper) in `state.rs` with one impl per input shape; every response and every existing assertion stays byte-identical.

## Evidence

Copy 1 — `services/service-com-amazonaws-dlm-dlm-20180112/src/ops/create_lifecycle_policy.rs:432-444`: "fn default_policy_only_body_parameters(input: &CreateLifecyclePolicyInput) -> crate::state::DefaultPolicyOnlyParameters { crate::state::DefaultPolicyOnlyParameters { resource_type: false, create_interval: input.create_interval.is_some(), retain_interval: input.retain_interval.is_some(), copy_tags: input.copy_tags.is_some(), cross_region_copy_targets: input.cross_region_copy_targets.is_some(), extend_deletion: input.extend_deletion.is_some(), exclusions: input.exclusions.is_some(), } }"

Copy 2 — `services/service-com-amazonaws-dlm-dlm-20180112/src/ops/update_lifecycle_policy.rs:377-389`: the identical body for `&UpdateLifecyclePolicyInput`, under a doc that already states the intent it misses: "The ONE place this operation maps its request-body members onto that set, so the refusal of a default-only parameter and the ... rule read the same list — as `create_lifecycle_policy.rs` does for the create's own input." (update_lifecycle_policy.rs:370-373)

The single-place halves it should join — `src/state.rs:930-943` (`in_policy_details`) and `src/state.rs:945-962` (`stated_names`, documented "This is the ONE list of their wire names", src/state.rs:914-916). Existing idiom for the collapse — `src/service.rs:164-186` (`trait DlmRequest` with one impl per input shape).

Test coverage that must stay unchanged: `tests/all/default_policy_only_parameters.rs` (the create and update refusals, incl. the `ResourceType` rows at lines 194-204), `tests/all/default_policy_configuration_precedence.rs` (body/PolicyDetails places on both create and update), `tests/all/interval_validation.rs` (the body-stated cadence rows).

## Steps to Reproduce

1. Read services/service-com-amazonaws-dlm-dlm-20180112/src/ops/create_lifecycle_policy.rs:432-444
2. Read services/service-com-amazonaws-dlm-dlm-20180112/src/ops/update_lifecycle_policy.rs:377-389 and compare: the same six-member mapping plus `resource_type: false`
3. Note that the structure half (state.rs:930-943) and the wire-name half (state.rs:945-962) are each written once, so only the body half can drift between the create and the update
4. Run the suite before and after collapsing the two copies: tests/all/default_policy_only_parameters.rs, tests/all/default_policy_configuration_precedence.rs and tests/all/interval_validation.rs must pass unchanged
