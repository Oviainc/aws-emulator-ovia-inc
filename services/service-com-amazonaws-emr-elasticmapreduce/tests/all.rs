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
#[path = "all/auto_scaling_policy.rs"]
mod auto_scaling_policy;
#[path = "all/block_public_access.rs"]
mod block_public_access;
#[path = "all/cluster_id_refusal_code.rs"]
mod cluster_id_refusal_code;
#[path = "all/cluster_placement.rs"]
mod cluster_placement;
#[path = "all/emr_control_plane.rs"]
mod emr_control_plane;
#[path = "all/enum_refusal_identity.rs"]
mod enum_refusal_identity;
#[path = "all/input_refusals.rs"]
mod input_refusals;
#[path = "all/instance_fleets.rs"]
mod instance_fleets;
#[path = "all/instance_groups.rs"]
mod instance_groups;
#[path = "all/instance_store_catalog.rs"]
mod instance_store_catalog;
#[path = "all/job_flow_lifecycle.rs"]
mod job_flow_lifecycle;
#[path = "all/job_flow_role.rs"]
mod job_flow_role;
#[path = "all/malformed_input_refusals.rs"]
mod malformed_input_refusals;
#[path = "all/managed_scaling_policy.rs"]
mod managed_scaling_policy;
#[path = "all/modify_instance_groups.rs"]
mod modify_instance_groups;
#[path = "all/notebook_executions.rs"]
mod notebook_executions;
#[path = "all/persistent_app_ui.rs"]
mod persistent_app_ui;
#[path = "all/refusal_messages.rs"]
mod refusal_messages;
#[path = "all/refusal_wording.rs"]
mod refusal_wording;
#[path = "all/release_label_application_names.rs"]
mod release_label_application_names;
#[path = "all/release_label_catalog.rs"]
mod release_label_catalog;
#[path = "all/release_labels.rs"]
mod release_labels;
#[path = "all/resource_not_found_code.rs"]
mod resource_not_found_code;
#[path = "all/resource_tag_removal.rs"]
mod resource_tag_removal;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/security_configuration_listing.rs"]
mod security_configuration_listing;
#[path = "all/security_configurations.rs"]
mod security_configurations;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/studio_session_mapping_listing.rs"]
mod studio_session_mapping_listing;
#[path = "all/supported_instance_types.rs"]
mod supported_instance_types;
#[path = "all/tag_character_class.rs"]
mod tag_character_class;
#[path = "all/termination_protection.rs"]
mod termination_protection;
