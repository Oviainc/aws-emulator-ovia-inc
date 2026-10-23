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
#[path = "all/approval_team_lifecycle.rs"]
mod approval_team_lifecycle;
#[path = "all/authorization_gate.rs"]
mod authorization_gate;
#[path = "all/inactive_team_version_deletion.rs"]
mod inactive_team_version_deletion;
#[path = "all/policy_catalog.rs"]
mod policy_catalog;
#[path = "all/policy_versions.rs"]
mod policy_versions;
#[path = "all/resource_policies.rs"]
mod resource_policies;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
