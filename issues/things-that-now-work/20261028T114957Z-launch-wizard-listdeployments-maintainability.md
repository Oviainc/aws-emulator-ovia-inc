Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 0c9cda5ef496fef3a9fe3e02c7d9493d80d1134a

# The unsupported-filter-name guard and its refusal are implemented twice, once per list handler

_Filed: 2026-10-28T11:49:57Z_
_Operation: ListDeployments | Class: maintainability_

> _Note: this issue was identified against the code base at commit `0c9cda5ef496fef3a9fe3e02c7d9493d80d1134a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The rule "a filter whose name is not one of the modelled filter-key values is refused with a ValidationException naming the supported names" is written out twice, once in each list handler that takes filters, against a different enum-values constant. The two copies are character-for-character the same logic and the same sentence, so a change to the rule (or to the sentence) has to be made in two places and the copies can drift. `crate::state` is the designated home for exactly this kind of shared service logic, and the pair rule these two loops sit next to is already hoisted there (`state::filter_pair_refusal`), so one helper taking the values slice would leave both responses byte-identical.

## Evidence

state.rs:5-10 names the single home for shared logic:
```
//! This module owns everything the operation handlers share: ... and the small projection/pagination helpers the
//! handlers in `ops/` call. `lib.rs` is generated, so a service cannot add a module
//! beside this one — shared service logic lives here rather than being copied into
//! each operation file.
```
Copy 1, src/ops/list_deployments.rs:171-188:
```
            let name = filter.name.as_deref().unwrap_or_default();
            if !DEPLOYMENT_FILTER_KEY_VALUES.contains(&name) {
                return Err(ListDeploymentsErrorModeled::ValidationException(
                    ValidationException { message: Some(format!(
                            "'{name}' is not a supported filter name; supported names are {}.",
                            DEPLOYMENT_FILTER_KEY_VALUES.join(", ")
                        )), }) .into());
```
Copy 2, src/ops/list_deployment_pattern_versions.rs:180-197:
```
            let name = filter.name.as_str();
            if !DEPLOYMENT_PATTERN_VERSION_FILTER_KEY_VALUES.contains(&name) {
                return Err(ListDeploymentPatternVersionsErrorModeled::ValidationException(
                    ValidationException { message: Some(format!(
                            "'{name}' is not a supported filter name; supported names are {}.",
                            DEPLOYMENT_PATTERN_VERSION_FILTER_KEY_VALUES.join(", ")
                        )), }) .into());
```
The adjacent half of the same rule is already shared: both handlers call `crate::state::filter_pair_refusal("filters", ...)` (list_deployments.rs:158-163, list_deployment_pattern_versions.rs:167-172), defined once at state.rs:2183.

Existing tests that must stay unchanged: tests/all/list_deployments_filter_pairs.rs, tests/all/list_deployment_pattern_versions.rs (the enum-value refusal row at :441-454) and tests/all/validation_messages.rs.

## Steps to Reproduce

1. Read src/ops/list_deployments.rs:171-188 and src/ops/list_deployment_pattern_versions.rs:180-197 side by side: the same containment check and the same refusal sentence, differing only in which modelled values constant is consulted.
2. Note that the neighbouring pair rule is hoisted once into state.rs:2183 (`filter_pair_refusal`) and called from both handlers, while this rule is not.
3. A single `state` helper taking the supported-values slice would produce the identical message for both operations, leaving every response and the tests above unchanged.
