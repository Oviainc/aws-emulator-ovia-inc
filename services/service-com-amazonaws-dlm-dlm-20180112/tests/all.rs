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
#[path = "all/default_policy_only_parameters.rs"]
mod default_policy_only_parameters;
#[path = "all/error_paths.rs"]
mod error_paths;
#[path = "all/execution_role_pass_role.rs"]
mod execution_role_pass_role;
#[path = "all/interval_validation.rs"]
mod interval_validation;
#[path = "all/lifecycle_policy_crud.rs"]
mod lifecycle_policy_crud;
#[path = "all/policy_read_projection.rs"]
mod policy_read_projection;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
