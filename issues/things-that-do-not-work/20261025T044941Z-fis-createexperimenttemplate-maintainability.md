Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 4244fec3063c58aa4a2476d7619962eecf74c002

# The template-admissibility refusal sequence is implemented twice (create and update), so the two copies can drift

_Filed: 2026-10-25T04:49:41Z_
_Operation: CreateExperimentTemplate | Class: maintainability_

> _Note: this issue was identified against the code base at commit `4244fec3063c58aa4a2476d7619962eecf74c002`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Which refusals judge a template record, and in which order, is written out twice: once in `CreateExperimentTemplate::handle` over the record it builds and once in `UpdateExperimentTemplate::handle` over the candidate it builds. Both call the same five state helpers in the same order (`stop_conditions_refusal`, `template_experiment_options_refusal`, `log_configuration_refusal`, `actions_quota_refusal` -> ServiceQuotaExceededException, `actions_refusal`), preceded by `role_arn_refusal`, and the update's comment states that the two MUST agree. Because the sequence is duplicated rather than expressed once, adding (or reordering) a refusal in one handler silently leaves the other accepting a template the first one refuses — the exact drift the test suite exists to catch. Extracting one `fn template_refusal(record) -> Option<...>` in `state.rs` that both handlers map into their own error enums changes no response and no test: every refusal, its order and its error type stay as they are.

## Evidence

Copy 1 — services/service-com-amazonaws-fis-faultinjectionsimulator/src/ops/create_experiment_template.rs:196-227:
```
        if let Some(refusal) = crate::state::role_arn_refusal("roleArn", Some(record.role_arn.as_str())) { ... ValidationException ... }
        if let Some(refusal) = crate::state::stop_conditions_refusal(&record.stop_conditions) { ... }
        if let Some(refusal) = crate::state::template_experiment_options_refusal(&record.experiment_options) { ... }
        if let Some(refusal) = crate::state::log_configuration_refusal(record.log_configuration.as_ref()) { ... }
        if let Some(refusal) = crate::state::actions_quota_refusal(&record.actions) { ... ServiceQuotaExceededException ... }
        if let Some(refusal) = crate::state::actions_refusal(&record.actions) { ... }
```
Copy 2 — src/ops/update_experiment_template.rs:175-223, with the agreement requirement stated in its own comment at lines 192-195 ("The change is applied to a CANDIDATE copy and judged before it is committed, so a refused update leaves the stored template exactly as it was — by the same rules, in the same order, as `CreateExperimentTemplate` applies to the template it builds."):
```
        if let Some(refusal) = crate::state::role_arn_refusal("roleArn", input.role_arn.as_deref()) { ... }
        ...
        if let Some(refusal) = crate::state::stop_conditions_refusal(&candidate.stop_conditions) { ... }
        if let Some(refusal) = crate::state::template_experiment_options_refusal(&candidate.experiment_options) { ... }
        if let Some(refusal) = crate::state::log_configuration_refusal(candidate.log_configuration.as_ref()) { ... }
        if let Some(refusal) = crate::state::actions_quota_refusal(&candidate.actions) { ... ServiceQuotaExceededException ... }
        if let Some(refusal) = crate::state::actions_refusal(&candidate.actions) { ... }
```
The helpers themselves already say the rule belongs to the TEMPLATE and not to one request shape — src/state.rs:2151-2154: `/// Taken over the STORED stop conditions rather than over one request's input shape, so /// the rule is judged the same whether the template is being created or updated` (and the same sentence for `actions_refusal` at src/state.rs:2562-2563 and `log_configuration_refusal` at src/state.rs:2199-2200), which is exactly the "one rule, two call sites" shape.

Tests that pin the behaviour a fix must leave unchanged: tests/all/experiment_template_shape_floors.rs (module doc, lines 15-18: "The rule is about the TEMPLATE, not about one request's shape, so every row below is driven through BOTH `CreateExperimentTemplate` ... and `UpdateExperimentTemplate` ..., and the two have to agree."), with `a_create_refuses_a_template_below_the_shape_floors` (line 238), `an_update_refuses_a_resulting_template_below_the_shape_floors` (line 313) and `one_member_below_a_floor_is_enough_to_refuse_an_update` (line 386); plus tests/all/control_plane_rules.rs and tests/all/update_experiment_template.rs for the role/options/action rules.

## Steps to Reproduce

1. Open src/ops/create_experiment_template.rs:196-227 and src/ops/update_experiment_template.rs:175-223 side by side: the same six refusal calls appear in the same order, differing only in the record they are taken over and the error enum they are lifted into.
2. Note src/ops/update_experiment_template.rs:192-195, which states the two must apply the same rules in the same order — a requirement nothing in the code enforces.
3. Add a hypothetical new template-level refusal (for example a `targets_refusal`) to the create handler only: `cargo test -p service-com-amazonaws-fis-faultinjectionsimulator` still passes for the update path, so the drift is invisible until a caller updates a template into the refused shape.
