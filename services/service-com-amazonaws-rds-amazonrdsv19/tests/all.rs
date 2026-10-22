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
#[path = "all/certificate_override.rs"]
mod certificate_override;
#[path = "all/db_engine_versions.rs"]
mod db_engine_versions;
#[path = "all/db_instance_deletion.rs"]
mod db_instance_deletion;
#[path = "all/db_instance_envelope.rs"]
mod db_instance_envelope;
#[path = "all/db_instances.rs"]
mod db_instances;
#[path = "all/docdb_query_surface.rs"]
mod docdb_query_surface;
#[path = "all/engine_version_families.rs"]
mod engine_version_families;
#[path = "all/existence_and_parameter_guards.rs"]
mod existence_and_parameter_guards;
#[path = "all/not_found_paths.rs"]
mod not_found_paths;
#[path = "all/option_group_options.rs"]
mod option_group_options;
#[path = "all/option_group_setting_catalog.rs"]
mod option_group_setting_catalog;
#[path = "all/option_groups.rs"]
mod option_groups;
#[path = "all/orderable_db_instance_options.rs"]
mod orderable_db_instance_options;
#[path = "all/query_catalog_reads.rs"]
mod query_catalog_reads;
#[path = "all/query_error_envelope.rs"]
mod query_error_envelope;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
