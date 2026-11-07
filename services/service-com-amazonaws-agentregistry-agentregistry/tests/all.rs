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
#[path = "all/discovery_read_authorization.rs"]
mod discovery_read_authorization;
#[path = "all/discovery_read_path.rs"]
mod discovery_read_path;
#[path = "all/registry_read_refusals.rs"]
mod registry_read_refusals;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
