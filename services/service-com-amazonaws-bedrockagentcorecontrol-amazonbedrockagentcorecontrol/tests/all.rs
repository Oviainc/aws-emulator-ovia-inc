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
#[path = "all/agent_runtime_lifecycle.rs"]
mod agent_runtime_lifecycle;
#[path = "all/gateway_lifecycle.rs"]
mod gateway_lifecycle;
#[path = "all/gateway_mutation.rs"]
mod gateway_mutation;
#[path = "all/gateway_target_listing.rs"]
mod gateway_target_listing;
#[path = "all/gateway_wire_shape.rs"]
mod gateway_wire_shape;
#[path = "all/harness_lifecycle.rs"]
mod harness_lifecycle;
#[path = "all/memory_lifecycle.rs"]
mod memory_lifecycle;
#[path = "all/service_claim.rs"]
mod service_claim;
#[path = "all/signing_name_routing.rs"]
mod signing_name_routing;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/validation_message_rendering.rs"]
mod validation_message_rendering;
