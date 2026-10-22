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
#[path = "all/account_customizations.rs"]
mod account_customizations;
#[path = "all/body_boundary_refusal.rs"]
mod body_boundary_refusal;
#[path = "all/body_validation_refusals.rs"]
mod body_validation_refusals;
#[path = "all/list_setting_order.rs"]
mod list_setting_order;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/support.rs"]
mod support;
#[path = "all/visible_services_round_trip.rs"]
mod visible_services_round_trip;
#[path = "all/wire.rs"]
mod wire;
