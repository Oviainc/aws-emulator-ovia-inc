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
#[path = "all/app_version_app_components.rs"]
mod app_version_app_components;
#[path = "all/application_lifecycle.rs"]
mod application_lifecycle;
#[path = "all/list_app_versions.rs"]
mod list_app_versions;
#[path = "all/publish_app_version.rs"]
mod publish_app_version;
#[path = "all/resiliency_policy_deletion.rs"]
mod resiliency_policy_deletion;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
