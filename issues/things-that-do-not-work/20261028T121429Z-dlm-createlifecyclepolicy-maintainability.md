Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 84725303c518fb402f10cf931cddd35e44c00d9a

# Fold the duplicated `[Default policies only]` refusal composition into one state.rs helper

_Filed: 2026-10-28T12:14:29Z_
_Operation: CreateLifecyclePolicy | Class: maintainability_

> _Note: this issue was identified against the code base at commit `84725303c518fb402f10cf931cddd35e44c00d9a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The crate centralises every cross-member rule of this service in `state.rs` (the member NAME list in `DefaultPolicyOnlyParameters`, the message builders `default_policy_only_parameters_refused` / `configuration_stated_twice`, and whole rules such as `Cadence::violation`, `schedule_rule_violation`, `immutable_configuration_violation`). Two rules are the exception: their COMPOSITION — which halves to read, in which order, with which prefix, and when to refuse — is written out a second time, once in `CreateLifecyclePolicy::handle` and once in `UpdateLifecyclePolicy::handle`.

1. The `[Default policies only]` refusal for a policy that is not a default policy: `body.stated_names("")` extended with `DefaultPolicyOnlyParameters::in_policy_details(details).stated_names(POLICY_DETAILS_PREFIX)`, refused when non-empty. Written identically in both handlers; only the kind predicate (`!is_default` vs `!policy.default_policy`) and the error enum variant differ.
2. The "either in the request body, or in the PolicyDetails request structure, but not both" refusal: `body.stated_names("")` + `input.policy_details.is_some()` + `configuration_stated_twice(..)`, again written out in both handlers (the create additionally prepends `DEFAULT_POLICY_MEMBER`).

Because the composition lives in the handlers rather than beside the member list it reads, the two copies can drift: a change to which halves are consulted, to the prefix a structure-stated member is reported under, or to the order the names are emitted in has to be made twice, and a change made in only one place silently makes the create and the update refuse different sets for the same request — the exact failure `state.rs` says this rule must not have. The fix is one `state.rs` function returning the composed `Vec<String>` (the halves and the prefix), called from both handlers, which the crate already does for every other rule of this family. No response and no existing test changes: the refusals, their messages, their `Code` and their `RequiredParameters`/`MutuallyExclusiveParameters` payloads are produced by the same `state.rs` builders either way.

## Evidence

The crate states the intent that this be ONE rule — `services/service-com-amazonaws-dlm-dlm-20180112/src/state.rs:1059-1067`:
```
/// `CreateLifecyclePolicyRequest` and `UpdateLifecyclePolicyRequest` declare the
/// same body members as the alternative to that structure, so WHICH members they
/// are is one rule for both: this is the ONE place the request-body half of
/// [`DefaultPolicyOnlyParameters`] is read, beside
/// [`DefaultPolicyOnlyParameters::in_policy_details`] for the structure half.
/// Every refusal built from either half ... therefore names the same set on every operation
/// that takes it.
```

Copy 1 of the composition — `src/ops/create_lifecycle_policy.rs:235-249`:
```
        if !is_default {
            let mut refused = stated_in_body.stated_names("");
            refused.extend(
                crate::state::DefaultPolicyOnlyParameters::in_policy_details(
                    input.policy_details.as_ref(),
                )
                .stated_names(crate::state::POLICY_DETAILS_PREFIX),
            );
            if !refused.is_empty() {
                return Err(CreateLifecyclePolicyErrorModeled::InvalidRequestException(
                    crate::state::default_policy_only_parameters_refused(&refused),
                )
                .into());
            }
        }
```

Copy 2, the same lines again — `src/ops/update_lifecycle_policy.rs:235-251`:
```
        if !policy.default_policy {
            let mut refused = input.default_policy_only_body_parameters().stated_names("");
            refused.extend(
                crate::state::DefaultPolicyOnlyParameters::in_policy_details(
                    input.policy_details.as_ref(),
                )
                .stated_names(crate::state::POLICY_DETAILS_PREFIX),
            );
            if !refused.is_empty() {
                return Err(
                    UpdateLifecyclePolicyErrorModeled::InvalidRequestException(
                        crate::state::default_policy_only_parameters_refused(&refused),
                    )
                    .into(),
                );
            }
        }
```

The second duplicated composition — `src/ops/create_lifecycle_policy.rs:207-228`:
```
        let stated_in_body = input.default_policy_only_body_parameters();
        if is_default && input.policy_details.is_some() {
            let mut duplicated = stated_in_body.stated_names("");
            ...
            if !duplicated.is_empty() {
                return Err(CreateLifecyclePolicyErrorModeled::InvalidRequestException(
                    crate::state::configuration_stated_twice(&duplicated),
                )
                .into());
            }
        }
```
against `src/ops/update_lifecycle_policy.rs:260-270`:
```
        if policy.default_policy && input.policy_details.is_some() {
            let duplicated = input.default_policy_only_body_parameters().stated_names("");
            if !duplicated.is_empty() {
                return Err(
                    UpdateLifecyclePolicyErrorModeled::InvalidRequestException(
                        crate::state::configuration_stated_twice(&duplicated),
                    )
                    .into(),
                );
            }
        }
```

Test coverage that must stay unchanged (both copies are pinned on both operations): `tests/all/default_policy_only_parameters.rs:405` `a_create_refuses_default_only_parameters_for_a_custom_policy`, `tests/all/default_policy_only_parameters.rs:490` `an_update_refuses_default_only_parameters_for_a_custom_policy`, and `tests/all/default_policy_configuration_precedence.rs:565` `an_update_may_not_state_the_configuration_twice` (which asserts the status, the `Code`, the `MutuallyExclusiveParameters` naming `PolicyDetails`, and that the stored configuration is untouched), plus `tests/all/default_policy_only_body_parameter_set.rs`.

## Steps to Reproduce

1. Read services/service-com-amazonaws-dlm-dlm-20180112/src/state.rs:1056-1109 — the member list and the request-body half of the `[Default policies only]` parameter set are declared once, with a doc stating the rule is "one rule for both" operations
2. Read src/ops/create_lifecycle_policy.rs:235-249 and src/ops/update_lifecycle_policy.rs:235-251 — the composition of the two halves (body names, then details names under POLICY_DETAILS_PREFIX) and the refusal condition are written out twice, byte-for-byte apart from the kind predicate and the error variant
3. Read src/ops/create_lifecycle_policy.rs:207-228 and src/ops/update_lifecycle_policy.rs:260-270 — the "stated in both places" composition is likewise written out twice
4. Observe that every other cross-member rule of this service (Cadence::violation, schedule_rule_violation, immutable_configuration_violation, omitted_configuration_members) is a single state.rs function called from both handlers, so these two are the only rules whose copies can drift
5. Confirm the fix is behaviour-preserving by running cargo test -p service-com-amazonaws-dlm-dlm-20180112 (default_policy_only_parameters.rs and default_policy_configuration_precedence.rs pin both copies on both operations)
