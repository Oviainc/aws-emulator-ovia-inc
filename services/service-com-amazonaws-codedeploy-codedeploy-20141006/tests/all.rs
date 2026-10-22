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
#[path = "all/codedeploy_lifecycle.rs"]
mod codedeploy_lifecycle;
#[path = "all/git_hub_account_token_names.rs"]
mod git_hub_account_token_names;
#[path = "all/on_premises_instances.rs"]
mod on_premises_instances;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/stop_deployment.rs"]
mod stop_deployment;
