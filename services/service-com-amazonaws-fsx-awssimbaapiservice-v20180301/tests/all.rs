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
#[path = "all/constraint_refusals.rs"]
mod constraint_refusals;
#[path = "all/create_file_system_resolves_ec2_subnets.rs"]
mod create_file_system_resolves_ec2_subnets;
#[path = "all/create_file_system_validation.rs"]
mod create_file_system_validation;
#[path = "all/file_system_lifecycle.rs"]
mod file_system_lifecycle;
#[path = "all/not_found_messages.rs"]
mod not_found_messages;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
