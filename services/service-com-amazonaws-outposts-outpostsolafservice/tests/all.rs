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
#[path = "all/capacity_task_lifecycle.rs"]
mod capacity_task_lifecycle;
#[path = "all/error_classification.rs"]
mod error_classification;
#[path = "all/error_message_rules.rs"]
mod error_message_rules;
#[path = "all/private_connectivity.rs"]
mod private_connectivity;
#[path = "all/quote_lifecycle.rs"]
mod quote_lifecycle;
#[path = "all/site_and_outpost_lifecycle.rs"]
mod site_and_outpost_lifecycle;
#[path = "all/site_and_outpost_projection.rs"]
mod site_and_outpost_projection;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/update_persistence.rs"]
mod update_persistence;
