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
#[path = "all/account_attributes.rs"]
mod account_attributes;
#[path = "all/batch_modify_cluster_snapshots.rs"]
mod batch_modify_cluster_snapshots;
#[path = "all/cluster_iam_roles.rs"]
mod cluster_iam_roles;
#[path = "all/cluster_lifecycle.rs"]
mod cluster_lifecycle;
#[path = "all/cluster_parameter_groups.rs"]
mod cluster_parameter_groups;
#[path = "all/cluster_parameters.rs"]
mod cluster_parameters;
#[path = "all/cluster_security_groups.rs"]
mod cluster_security_groups;
#[path = "all/cluster_structure.rs"]
mod cluster_structure;
#[path = "all/cluster_versions.rs"]
mod cluster_versions;
#[path = "all/default_cluster_parameters.rs"]
mod default_cluster_parameters;
#[path = "all/error_envelope.rs"]
mod error_envelope;
#[path = "all/existence_preconditions.rs"]
mod existence_preconditions;
#[path = "all/hsm_configurations.rs"]
mod hsm_configurations;
#[path = "all/maintenance_tracks.rs"]
mod maintenance_tracks;
#[path = "all/modify_cluster_parameter_group.rs"]
mod modify_cluster_parameter_group;
#[path = "all/modify_cluster_snapshot.rs"]
mod modify_cluster_snapshot;
#[path = "all/orderable_cluster_options.rs"]
mod orderable_cluster_options;
#[path = "all/qev2_idc_applications.rs"]
mod qev2_idc_applications;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/reset_cluster_parameter_group.rs"]
mod reset_cluster_parameter_group;
#[path = "all/snapshot_access.rs"]
mod snapshot_access;
#[path = "all/snapshot_restore.rs"]
mod snapshot_restore;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/static_catalog.rs"]
mod static_catalog;
#[path = "all/storage_summary.rs"]
mod storage_summary;
#[path = "all/usage_limits.rs"]
mod usage_limits;
