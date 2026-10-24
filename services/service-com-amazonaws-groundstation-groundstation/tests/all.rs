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
#[path = "all/config_defaults_and_references.rs"]
mod config_defaults_and_references;
#[path = "all/config_listing.rs"]
mod config_listing;
#[path = "all/contact_versions.rs"]
mod contact_versions;
#[path = "all/dataflow_endpoint_group_listing.rs"]
mod dataflow_endpoint_group_listing;
#[path = "all/dataflow_endpoint_groups.rs"]
mod dataflow_endpoint_groups;
#[path = "all/ephemeris_catalog.rs"]
mod ephemeris_catalog;
#[path = "all/lifecycle_writes.rs"]
mod lifecycle_writes;
#[path = "all/minute_usage.rs"]
mod minute_usage;
#[path = "all/not_found_messages.rs"]
mod not_found_messages;
#[path = "all/request_validation.rs"]
mod request_validation;
#[path = "all/resource_tagging.rs"]
mod resource_tagging;
#[path = "all/s3_recording_buckets.rs"]
mod s3_recording_buckets;
#[path = "all/satellite_catalog.rs"]
mod satellite_catalog;
#[path = "all/scheduling_loop.rs"]
mod scheduling_loop;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/tag_rules.rs"]
mod tag_rules;
