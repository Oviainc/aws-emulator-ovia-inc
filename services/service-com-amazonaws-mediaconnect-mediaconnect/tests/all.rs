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
#[path = "all/bridge_lifecycle.rs"]
mod bridge_lifecycle;
#[path = "all/bridge_members_and_deletion.rs"]
mod bridge_members_and_deletion;
#[path = "all/error_envelopes.rs"]
mod error_envelopes;
#[path = "all/flow_entitlements.rs"]
mod flow_entitlements;
#[path = "all/flow_identity_and_description.rs"]
mod flow_identity_and_description;
#[path = "all/flow_lifecycle.rs"]
mod flow_lifecycle;
#[path = "all/flow_media_streams.rs"]
mod flow_media_streams;
#[path = "all/flow_outputs.rs"]
mod flow_outputs;
#[path = "all/flow_projection.rs"]
mod flow_projection;
#[path = "all/flow_source_failover_bound.rs"]
mod flow_source_failover_bound;
#[path = "all/flow_sources.rs"]
mod flow_sources;
#[path = "all/gateway_lifecycle.rs"]
mod gateway_lifecycle;
#[path = "all/lifecycle_preconditions.rs"]
mod lifecycle_preconditions;
#[path = "all/ops_name_bindings.rs"]
mod ops_name_bindings;
#[path = "all/remove_flow_media_stream.rs"]
mod remove_flow_media_stream;
#[path = "all/remove_flow_output.rs"]
mod remove_flow_output;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/revoke_flow_entitlement.rs"]
mod revoke_flow_entitlement;
#[path = "all/router_input_deletion.rs"]
mod router_input_deletion;
#[path = "all/router_outputs.rs"]
mod router_outputs;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/update_bridge.rs"]
mod update_bridge;
#[path = "all/update_bridge_source.rs"]
mod update_bridge_source;
#[path = "all/update_flow.rs"]
mod update_flow;
#[path = "all/update_flow_entitlement.rs"]
mod update_flow_entitlement;
#[path = "all/update_flow_media_stream.rs"]
mod update_flow_media_stream;
#[path = "all/update_flow_output.rs"]
mod update_flow_output;
#[path = "all/update_flow_source.rs"]
mod update_flow_source;
