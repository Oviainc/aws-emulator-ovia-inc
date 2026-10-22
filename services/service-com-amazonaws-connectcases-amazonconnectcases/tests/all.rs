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
#[path = "all/case_event_configuration.rs"]
mod case_event_configuration;
#[path = "all/case_lifecycle.rs"]
mod case_lifecycle;
#[path = "all/field_listing.rs"]
mod field_listing;
#[path = "all/field_options.rs"]
mod field_options;
#[path = "all/layout_lifecycle.rs"]
mod layout_lifecycle;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/template_listing.rs"]
mod template_listing;
