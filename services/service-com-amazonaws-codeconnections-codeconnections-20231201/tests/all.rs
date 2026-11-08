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
#[path = "all/absent_resource_answers.rs"]
mod absent_resource_answers;
#[path = "all/connection_lifecycle.rs"]
mod connection_lifecycle;
#[path = "all/connection_reads.rs"]
mod connection_reads;
#[path = "all/git_sync.rs"]
mod git_sync;
#[path = "all/host_association.rs"]
mod host_association;
#[path = "all/host_deletion.rs"]
mod host_deletion;
#[path = "all/host_update.rs"]
mod host_update;
#[path = "all/list_hosts.rs"]
mod list_hosts;
#[path = "all/list_repository_links.rs"]
mod list_repository_links;
#[path = "all/list_repository_sync_definitions.rs"]
mod list_repository_sync_definitions;
#[path = "all/list_sync_configurations.rs"]
mod list_sync_configurations;
#[path = "all/modeled_input_refusals.rs"]
mod modeled_input_refusals;
#[path = "all/refusal_messages.rs"]
mod refusal_messages;
#[path = "all/repository_link_lifecycle.rs"]
mod repository_link_lifecycle;
#[path = "all/resource_sync_status.rs"]
mod resource_sync_status;
#[path = "all/resource_tagging.rs"]
mod resource_tagging;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/sync_blocker_summary.rs"]
mod sync_blocker_summary;
#[path = "all/target_prefix_forms.rs"]
mod target_prefix_forms;
#[path = "all/target_probe.rs"]
mod target_probe;
#[path = "all/validation_message_rules.rs"]
mod validation_message_rules;
#[path = "all/wire_target_routing.rs"]
mod wire_target_routing;
