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
#[path = "all/channel_lifecycle.rs"]
mod channel_lifecycle;
#[path = "all/channel_policy.rs"]
mod channel_policy;
#[path = "all/channel_policy_delete.rs"]
mod channel_policy_delete;
#[path = "all/channel_policy_lifecycle.rs"]
mod channel_policy_lifecycle;
#[path = "all/channel_reset.rs"]
mod channel_reset;
#[path = "all/control_plane_validation.rs"]
mod control_plane_validation;
#[path = "all/origin_endpoint_policy.rs"]
mod origin_endpoint_policy;
#[path = "all/server_materialised_members.rs"]
mod server_materialised_members;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/update_origin_endpoint.rs"]
mod update_origin_endpoint;
