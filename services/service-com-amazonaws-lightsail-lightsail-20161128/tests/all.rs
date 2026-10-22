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
#[path = "all/bucket_bundles.rs"]
mod bucket_bundles;
#[path = "all/container_service_lifecycle.rs"]
mod container_service_lifecycle;
#[path = "all/disk_block_storage.rs"]
mod disk_block_storage;
#[path = "all/domain_entry_recordset.rs"]
mod domain_entry_recordset;
#[path = "all/domain_zone.rs"]
mod domain_zone;
#[path = "all/instance_error_envelope.rs"]
mod instance_error_envelope;
#[path = "all/instance_lifecycle.rs"]
mod instance_lifecycle;
#[path = "all/region_catalog.rs"]
mod region_catalog;
#[path = "all/relational_database_blueprints.rs"]
mod relational_database_blueprints;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
