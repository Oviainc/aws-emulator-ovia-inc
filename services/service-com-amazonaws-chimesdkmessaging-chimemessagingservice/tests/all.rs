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
#[path = "all/channel_flows.rs"]
mod channel_flows;
#[path = "all/channel_membership.rs"]
mod channel_membership;
#[path = "all/channel_messaging.rs"]
mod channel_messaging;
#[path = "all/channel_resource_refusals.rs"]
mod channel_resource_refusals;
#[path = "all/channel_updates.rs"]
mod channel_updates;
#[path = "all/generated_id_shapes.rs"]
mod generated_id_shapes;
#[path = "all/identity_names.rs"]
mod identity_names;
#[path = "all/resource_tagging.rs"]
mod resource_tagging;
#[path = "all/service_claim.rs"]
mod service_claim;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
