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
#[path = "all/batch_get_view.rs"]
mod batch_get_view;
#[path = "all/index_and_view_lifecycle.rs"]
mod index_and_view_lifecycle;
#[path = "all/index_state_and_resource_tags.rs"]
mod index_state_and_resource_tags;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
