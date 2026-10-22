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
#[path = "all/abort_environment_update.rs"]
mod abort_environment_update;
#[path = "all/application_versions.rs"]
mod application_versions;
#[path = "all/dns_availability.rs"]
mod dns_availability;
#[path = "all/environment_operations.rs"]
mod environment_operations;
// Not a suite of its own: the name the suites below import the shared harness
// by. Declared here because every file under `tests/all/` must be.
#[path = "all/harness.rs"]
mod harness;
#[path = "all/lifecycle.rs"]
mod lifecycle;
#[path = "all/operations_role.rs"]
mod operations_role;
#[path = "all/platform_branches.rs"]
mod platform_branches;
#[path = "all/solution_stacks.rs"]
mod solution_stacks;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/swap_environment_cnames.rs"]
mod swap_environment_cnames;
