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
#[path = "all/account_gate_and_not_found.rs"]
mod account_gate_and_not_found;
#[path = "all/builtin_readiness_rules.rs"]
mod builtin_readiness_rules;
#[path = "all/cell_lifecycle.rs"]
mod cell_lifecycle;
#[path = "all/cross_account_authorizations.rs"]
mod cross_account_authorizations;
#[path = "all/readiness_check_lifecycle.rs"]
mod readiness_check_lifecycle;
#[path = "all/recovery_group_lifecycle.rs"]
mod recovery_group_lifecycle;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
