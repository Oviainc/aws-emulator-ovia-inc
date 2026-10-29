Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 0c9cda5ef496fef3a9fe3e02c7d9493d80d1134a

# The resolveToResourceType/resolveToResourceId pair resolution is written out three times instead of once beside its targetResource sibling

_Filed: 2026-10-28T11:52:25Z_
_Operation: ListActions | Class: maintainability_

> _Note: this issue was identified against the code base at commit `0c9cda5ef496fef3a9fe3e02c7d9493d80d1134a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Three handlers resolve the optional `resolveTo` reference with the same four-arm match: both members present -> look the id up in `store.assets` and 404 if absent; neither present -> no filter; either half alone -> InvalidRequestException with a fixed message. The three copies are character-identical, comment included, so a change to the refusal wording, the 404 resource naming, or the "half a pair identifies nothing" rule has to be made in three places and can drift. The crate already hoisted the equivalent rule for the sibling reference into one place (`crate::state::target_resource_reference`, documented as the single definition so "a resource is addressed identically whether the request states it as a structure or as a type and an id"); the `resolveTo` pair has no such helper. Hoisting it leaves every response byte-identical.

## Evidence

Copy 1 — src/ops/list_actions.rs:213-237:
  "let resolved_asset = match (input.resolve_to_resource_type.as_deref(), input.resolve_to_resource_id.as_deref()) { (Some(_), Some(asset_id)) => { if !store.assets.contains_key(asset_id) { return Err(not_found(crate::state::not_found_message(crate::state::RESOURCE_ASSET, asset_id))); } Some(asset_id) } (None, None) => None, (Some(_), None) => { return Err(invalid_request(\"resolveToResourceId is required when resolveToResourceType is specified.\")) } (None, Some(_)) => { return Err(invalid_request(\"resolveToResourceType is required when resolveToResourceId is specified.\")) } };"

Copy 2 — src/ops/list_executions.rs:217-241: identical text, same comment at 211-216.

Copy 3 — src/ops/describe_computation_model_execution_summary.rs:185-209: identical text, same comment at 180-184.

The precedent for one definition, in this crate's own state module:
  src/state.rs:2973-2986 "/// The `(TargetResourceType, ID)` pair `target_resource` names, or the refusal a structure naming no single resource draws. ... The pair this returns is the same pair the listings scoped to a target resource take, so a resource is addressed identically whether the request states it as a structure or as a type and an id.
  pub fn target_resource_reference(target_resource: &TargetResource) -> Result<(&'static str, &str), String>" — called from src/ops/execute_action.rs:180-182.

Existing tests that keep the behaviour pinned while the copies are merged: tests/all/computation_model_execution_summary.rs:636 asserts the exact message "resolveToResourceId is required when resolveToResourceType is specified."; tests/all/actions.rs:487 the_resolve_to_filter_narrows_the_listing_to_its_matches; tests/all/executions.rs:453 the_action_type_and_resolve_to_filters_narrow_the_listing_to_their_matches.

## Steps to Reproduce

1. Open src/ops/list_actions.rs:213-237, src/ops/list_executions.rs:217-241 and src/ops/describe_computation_model_execution_summary.rs:185-209 side by side: the resolveTo match, its two refusal strings and the asset 404 are the same code three times.
2. Compare with src/state.rs:2984 target_resource_reference, the single definition the crate already keeps for the sibling targetResource reference and calls from src/ops/execute_action.rs:181.
3. Change one refusal message in one copy and run the suite: only tests/all/computation_model_execution_summary.rs:636 fails, so two of the three copies are free to drift unnoticed.
