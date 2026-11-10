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
#[path = "all/batch_translation_jobs.rs"]
mod batch_translation_jobs;
#[path = "all/document_translation.rs"]
mod document_translation;
#[path = "all/rejection_paths.rs"]
mod rejection_paths;
#[path = "all/resource_tag_listing.rs"]
mod resource_tag_listing;
#[path = "all/resource_tag_removal.rs"]
mod resource_tag_removal;
#[path = "all/resource_tagging.rs"]
mod resource_tagging;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/terminology_lifecycle.rs"]
mod terminology_lifecycle;
