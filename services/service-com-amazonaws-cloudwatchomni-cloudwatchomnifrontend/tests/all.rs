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
#[path = "all/access_grant_lifecycle.rs"]
mod access_grant_lifecycle;
#[path = "all/access_profile_lifecycle.rs"]
mod access_profile_lifecycle;
#[path = "all/alert_lifecycle.rs"]
mod alert_lifecycle;
#[path = "all/domain_space_lifecycle.rs"]
mod domain_space_lifecycle;
#[path = "all/omni_dashboard_lifecycle.rs"]
mod omni_dashboard_lifecycle;
#[path = "all/probe.rs"]
mod probe;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
