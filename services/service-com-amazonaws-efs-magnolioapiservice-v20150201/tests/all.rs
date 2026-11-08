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
#[path = "all/access_point_deletion.rs"]
mod access_point_deletion;
#[path = "all/access_point_lifecycle.rs"]
mod access_point_lifecycle;
#[path = "all/backup_policy.rs"]
mod backup_policy;
#[path = "all/delete_access_point.rs"]
mod delete_access_point;
#[path = "all/error_document_message.rs"]
mod error_document_message;
#[path = "all/error_message_construction.rs"]
mod error_message_construction;
#[path = "all/file_system_description.rs"]
mod file_system_description;
#[path = "all/file_system_lifecycle.rs"]
mod file_system_lifecycle;
#[path = "all/file_system_policy.rs"]
mod file_system_policy;
#[path = "all/harness.rs"]
mod harness;
#[path = "all/lifecycle_configuration.rs"]
mod lifecycle_configuration;
#[path = "all/mutation_results.rs"]
mod mutation_results;
#[path = "all/provisioned_throughput_required.rs"]
mod provisioned_throughput_required;
#[path = "all/replication_configuration.rs"]
mod replication_configuration;
#[path = "all/replication_refusals.rs"]
mod replication_refusals;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/tags.rs"]
mod tags;
#[path = "all/update_file_system.rs"]
mod update_file_system;
