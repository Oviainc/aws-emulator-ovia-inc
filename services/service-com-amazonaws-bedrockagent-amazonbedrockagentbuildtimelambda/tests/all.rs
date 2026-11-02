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
#[path = "all/agent_lifecycle.rs"]
mod agent_lifecycle;
#[path = "all/data_source_lifecycle.rs"]
mod data_source_lifecycle;
#[path = "all/delete_removes_the_resource.rs"]
mod delete_removes_the_resource;
#[path = "all/flow_definition_validation.rs"]
mod flow_definition_validation;
#[path = "all/flow_lifecycle.rs"]
mod flow_lifecycle;
#[path = "all/flow_version_listing.rs"]
mod flow_version_listing;
#[path = "all/immutable_configuration_members.rs"]
mod immutable_configuration_members;
#[path = "all/ingestion_job_lifecycle.rs"]
mod ingestion_job_lifecycle;
#[path = "all/knowledge_base_lifecycle.rs"]
mod knowledge_base_lifecycle;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/update_round_trip.rs"]
mod update_round_trip;
