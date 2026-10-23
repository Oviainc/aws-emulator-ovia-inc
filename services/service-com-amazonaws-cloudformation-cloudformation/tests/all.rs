// SPDX-License-Identifier: CC0-1.0

//! Single integration-test target for this crate.
//!
//! Put a new suite at `tests/all/<name>.rs` and declare it below, keeping the list
//! sorted.
//!
//! A suite that is not declared here compiles nowhere and its tests never run. A
//! suite left at the top level of `tests/` fails the build.

// `tests/all.rs` is a test-target CRATE ROOT, so a bare `mod` resolves to a SIBLING
// (`tests/state_roundtrip.rs`) — which the layout check refuses. The `#[path]` is what
// points it into `tests/all/`.
#[path = "all/agentcore_stack_outputs.rs"]
mod agentcore_stack_outputs;
#[path = "all/callback_signature.rs"]
mod callback_signature;
#[path = "all/change_set_lifecycle.rs"]
mod change_set_lifecycle;
#[path = "all/change_set_preview.rs"]
mod change_set_preview;
#[path = "all/drift_and_resource_reads.rs"]
mod drift_and_resource_reads;
#[path = "all/ecs_elbv2_stack_identifiers.rs"]
mod ecs_elbv2_stack_identifiers;
// An EMPTY, moved-out suite: its rows now live in `all/nested_resource_outputs.rs` and
// `all/provisioned_resources.rs`. Declared because the layout check refuses an undeclared
// file under `tests/all/`, and this fixer's tool set can write a file but not remove one;
// the next change to touch this directory should delete both lines and the file.
#[path = "all/ecs_elbv2_stack_outputs.rs"]
mod ecs_elbv2_stack_outputs;
#[path = "all/error_envelope.rs"]
mod error_envelope;
#[path = "all/extension_registry.rs"]
mod extension_registry;
#[path = "all/generated_physical_names.rs"]
mod generated_physical_names;
#[path = "all/nested_resource_outputs.rs"]
mod nested_resource_outputs;
#[path = "all/provisioned_resources.rs"]
mod provisioned_resources;
#[path = "all/published_type_schema.rs"]
mod published_type_schema;
#[path = "all/resolved_properties.rs"]
mod resolved_properties;
#[path = "all/scratch.rs"]
mod scratch;
#[path = "all/stack_lifecycle.rs"]
mod stack_lifecycle;
#[path = "all/stack_output_resolution.rs"]
mod stack_output_resolution;
#[path = "all/stack_outputs.rs"]
mod stack_outputs;
#[path = "all/stack_reported_members.rs"]
mod stack_reported_members;
#[path = "all/stack_resource_detail.rs"]
mod stack_resource_detail;
#[path = "all/stack_resource_providers.rs"]
mod stack_resource_providers;
#[path = "all/stack_resource_provisioning.rs"]
mod stack_resource_provisioning;
#[path = "all/stack_resources.rs"]
mod stack_resources;
#[path = "all/stack_role_trust.rs"]
mod stack_role_trust;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/template_outputs.rs"]
mod template_outputs;
#[path = "all/unprovisioned_resources.rs"]
mod unprovisioned_resources;
#[path = "all/unrecognized_resource_types.rs"]
mod unrecognized_resource_types;
#[path = "all/validate_template.rs"]
mod validate_template;
