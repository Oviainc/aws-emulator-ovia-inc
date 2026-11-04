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
#[path = "all/data_catalog_lifecycle.rs"]
mod data_catalog_lifecycle;

#[path = "all/query_execution_lifecycle.rs"]
mod query_execution_lifecycle;

#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;

#[path = "all/table_metadata.rs"]
mod table_metadata;

#[path = "all/work_group_lifecycle.rs"]
mod work_group_lifecycle;
