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
#[path = "all/account_settings.rs"]
mod account_settings;
#[path = "all/capacity_providers.rs"]
mod capacity_providers;
#[path = "all/cluster_capacity_and_statistics.rs"]
mod cluster_capacity_and_statistics;
#[path = "all/cluster_mutations.rs"]
mod cluster_mutations;
#[path = "all/cluster_projection.rs"]
mod cluster_projection;
#[path = "all/cluster_tombstone.rs"]
mod cluster_tombstone;
#[path = "all/container_instance_not_found.rs"]
mod container_instance_not_found;
#[path = "all/container_instances.rs"]
mod container_instances;
#[path = "all/daemon_task_definitions.rs"]
mod daemon_task_definitions;
#[path = "all/ecs_wire.rs"]
mod ecs_wire;
#[path = "all/error_messages.rs"]
mod error_messages;
#[path = "all/list_clusters.rs"]
mod list_clusters;
#[path = "all/list_service_deployments.rs"]
mod list_service_deployments;
#[path = "all/list_services.rs"]
mod list_services;
#[path = "all/list_task_definition_families.rs"]
mod list_task_definition_families;
#[path = "all/list_task_definitions.rs"]
mod list_task_definitions;
#[path = "all/load_balancer_target_groups.rs"]
mod load_balancer_target_groups;
#[path = "all/reserved_tag_prefix.rs"]
mod reserved_tag_prefix;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/service_derived_members.rs"]
mod service_derived_members;
#[path = "all/service_lifecycle.rs"]
mod service_lifecycle;
#[path = "all/service_projection.rs"]
mod service_projection;
#[path = "all/service_validation.rs"]
mod service_validation;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/tag_set_reads.rs"]
mod tag_set_reads;
#[path = "all/tagging_writes.rs"]
mod tagging_writes;
#[path = "all/task_definition_deregistration.rs"]
mod task_definition_deregistration;
#[path = "all/task_definition_projection.rs"]
mod task_definition_projection;
#[path = "all/task_definition_reference_refusals.rs"]
mod task_definition_reference_refusals;
#[path = "all/task_definition_rendering.rs"]
mod task_definition_rendering;
#[path = "all/task_lifecycle.rs"]
mod task_lifecycle;
#[path = "all/task_provisioning.rs"]
mod task_provisioning;
