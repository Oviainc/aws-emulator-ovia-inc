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
#[path = "all/application_lifecycle.rs"]
mod application_lifecycle;
#[path = "all/pre_handler_refusals.rs"]
mod pre_handler_refusals;
#[path = "all/signed_bluinsights_url.rs"]
mod signed_bluinsights_url;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
