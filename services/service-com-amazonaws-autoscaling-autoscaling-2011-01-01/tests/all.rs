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
#[path = "all/auto_scaling_group_lifecycle.rs"]
mod auto_scaling_group_lifecycle;
#[path = "all/error_envelope.rs"]
mod error_envelope;
#[path = "all/launch_configurations_are_unavailable.rs"]
mod launch_configurations_are_unavailable;
#[path = "all/reference_data_catalogs.rs"]
mod reference_data_catalogs;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
// The shared harness the suites above drive the service through: the awsQuery POST
// and the XML readers, defined once.
#[path = "all/support.rs"]
mod support;
