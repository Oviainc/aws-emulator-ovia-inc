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
#[path = "all/aliases.rs"]
mod aliases;
#[path = "all/client_error_status.rs"]
mod client_error_status;
#[path = "all/harness.rs"]
mod harness;
#[path = "all/key_deletion_cancellation.rs"]
mod key_deletion_cancellation;
#[path = "all/key_lifecycle.rs"]
mod key_lifecycle;
#[path = "all/key_specs.rs"]
mod key_specs;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
