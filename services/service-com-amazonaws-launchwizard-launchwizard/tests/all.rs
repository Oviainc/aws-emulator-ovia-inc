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
#[path = "all/deployment_lifecycle.rs"]
mod deployment_lifecycle;
#[path = "all/harness.rs"]
mod harness;
#[path = "all/not_found_contract.rs"]
mod not_found_contract;
#[path = "all/resource_tagging.rs"]
mod resource_tagging;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/validation_messages.rs"]
mod validation_messages;
#[path = "all/workload_catalogue.rs"]
mod workload_catalogue;
#[path = "all/workload_deployment_patterns.rs"]
mod workload_deployment_patterns;
