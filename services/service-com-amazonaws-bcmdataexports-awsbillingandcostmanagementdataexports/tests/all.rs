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
#[path = "all/catalog_dictionary.rs"]
mod catalog_dictionary;
#[path = "all/catalog_schema.rs"]
mod catalog_schema;
#[path = "all/export_lifecycle.rs"]
mod export_lifecycle;
#[path = "all/not_found_message.rs"]
mod not_found_message;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/table_catalog.rs"]
mod table_catalog;
#[path = "all/table_listing.rs"]
mod table_listing;
