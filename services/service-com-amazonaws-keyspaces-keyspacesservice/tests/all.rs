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
#[path = "all/column_types_and_auto_scaling.rs"]
mod column_types_and_auto_scaling;
#[path = "all/get_keyspace.rs"]
mod get_keyspace;
#[path = "all/keyspace_table_lifecycle.rs"]
mod keyspace_table_lifecycle;
#[path = "all/list_keyspaces.rs"]
mod list_keyspaces;
#[path = "all/list_types.rs"]
mod list_types;
#[path = "all/resource_tagging.rs"]
mod resource_tagging;
#[path = "all/service_managed_catalog.rs"]
mod service_managed_catalog;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/type_lifecycle.rs"]
mod type_lifecycle;
#[path = "all/update_table.rs"]
mod update_table;
