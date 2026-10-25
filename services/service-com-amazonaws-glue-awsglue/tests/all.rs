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
#[path = "all/batch_get_jobs.rs"]
mod batch_get_jobs;
#[path = "all/catalog_identity.rs"]
mod catalog_identity;
#[path = "all/catalog_mutations.rs"]
mod catalog_mutations;
#[path = "all/catalog_projections.rs"]
mod catalog_projections;
#[path = "all/crawlers.rs"]
mod crawlers;
#[path = "all/custom_entity_types.rs"]
mod custom_entity_types;
#[path = "all/data_catalog.rs"]
mod data_catalog;
#[path = "all/error_messages.rs"]
mod error_messages;
#[path = "all/etl_connections.rs"]
mod etl_connections;
#[path = "all/etl_jobs_and_connections.rs"]
mod etl_jobs_and_connections;
#[path = "all/get_table_entity_not_found.rs"]
mod get_table_entity_not_found;
#[path = "all/job_listing.rs"]
mod job_listing;
#[path = "all/job_runs.rs"]
mod job_runs;
#[path = "all/jobs.rs"]
mod jobs;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/store_capacity.rs"]
mod store_capacity;
#[path = "all/wire.rs"]
mod wire;
