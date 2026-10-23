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
#[path = "all/firewall_lifecycle.rs"]
mod firewall_lifecycle;
#[path = "all/harness.rs"]
mod harness;
#[path = "all/resource_policies.rs"]
mod resource_policies;
#[path = "all/rule_group_capacity_and_resource_policy.rs"]
mod rule_group_capacity_and_resource_policy;
#[path = "all/rule_group_lifecycle.rs"]
mod rule_group_lifecycle;
#[path = "all/rule_group_mutations.rs"]
mod rule_group_mutations;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
