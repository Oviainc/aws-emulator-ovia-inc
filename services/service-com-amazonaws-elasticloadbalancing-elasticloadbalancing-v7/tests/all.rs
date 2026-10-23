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
#[path = "all/elbv2_listener_chain_on_the_shared_endpoint.rs"]
mod elbv2_listener_chain_on_the_shared_endpoint;
#[path = "all/listeners_health_check_and_attributes.rs"]
mod listeners_health_check_and_attributes;
#[path = "all/load_balancer_lifecycle.rs"]
mod load_balancer_lifecycle;
#[path = "all/shared_endpoint_api_version_dispatch.rs"]
mod shared_endpoint_api_version_dispatch;
#[path = "all/shared_endpoint_api_versions.rs"]
mod shared_endpoint_api_versions;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/wire.rs"]
mod wire;
