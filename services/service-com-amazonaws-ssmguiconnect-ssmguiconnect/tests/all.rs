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
#[path = "all/connection_recording_preferences.rs"]
mod connection_recording_preferences;
#[path = "all/jit_node_access_gate_and_error_messages.rs"]
mod jit_node_access_gate_and_error_messages;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
