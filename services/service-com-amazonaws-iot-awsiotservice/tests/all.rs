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
#[path = "all/command_registry.rs"]
mod command_registry;
#[path = "all/describe_endpoint.rs"]
mod describe_endpoint;
#[path = "all/indexing_configuration.rs"]
mod indexing_configuration;
#[path = "all/job_execution_read.rs"]
mod job_execution_read;
#[path = "all/job_targeting.rs"]
mod job_targeting;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/thing_group_hierarchy.rs"]
mod thing_group_hierarchy;
#[path = "all/thing_listing.rs"]
mod thing_listing;
#[path = "all/thing_registry_lifecycle.rs"]
mod thing_registry_lifecycle;
#[path = "all/topic_rule_listing.rs"]
mod topic_rule_listing;
#[path = "all/topic_rule_registry.rs"]
mod topic_rule_registry;
