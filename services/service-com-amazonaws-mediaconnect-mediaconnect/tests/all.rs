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
#[path = "all/bridge_output_admission.rs"]
mod bridge_output_admission;
#[path = "all/create_flow_stored_formats.rs"]
mod create_flow_stored_formats;
#[path = "all/create_router_input.rs"]
mod create_router_input;
#[path = "all/create_router_network_interface_rules.rs"]
mod create_router_network_interface_rules;
#[path = "all/entitlement_listing.rs"]
mod entitlement_listing;
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
#[path = "all/flow_ndi_sizing.rs"]
mod flow_ndi_sizing;
#[path = "all/flow_outputs.rs"]
mod flow_outputs;
#[path = "all/flow_projection.rs"]
mod flow_projection;
#[path = "all/flow_source_failover_bound.rs"]
mod flow_source_failover_bound;
#[path = "all/flow_source_metadata.rs"]
mod flow_source_metadata;
#[path = "all/flow_source_thumbnail.rs"]
mod flow_source_thumbnail;
#[path = "all/flow_sources.rs"]
mod flow_sources;
#[path = "all/flow_sub_resource_tags.rs"]
mod flow_sub_resource_tags;
#[path = "all/gateway_instances.rs"]
mod gateway_instances;
#[path = "all/gateway_lifecycle.rs"]
mod gateway_lifecycle;
#[path = "all/get_router_input.rs"]
mod get_router_input;
#[path = "all/lifecycle_preconditions.rs"]
mod lifecycle_preconditions;
#[path = "all/ops_name_bindings.rs"]
mod ops_name_bindings;
#[path = "all/remove_flow_media_stream.rs"]
mod remove_flow_media_stream;
#[path = "all/remove_flow_output.rs"]
mod remove_flow_output;
#[path = "all/remove_flow_vpc_interface.rs"]
mod remove_flow_vpc_interface;
#[path = "all/reservation_description.rs"]
mod reservation_description;
#[path = "all/reservation_listing.rs"]
mod reservation_listing;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/revoke_flow_entitlement.rs"]
mod revoke_flow_entitlement;
#[path = "all/router_input_batch_get.rs"]
mod router_input_batch_get;
#[path = "all/router_input_deletion.rs"]
mod router_input_deletion;
#[path = "all/router_input_restart.rs"]
mod router_input_restart;
#[path = "all/router_input_start.rs"]
mod router_input_start;
#[path = "all/router_network_interface_deletion.rs"]
mod router_network_interface_deletion;
#[path = "all/router_network_interfaces.rs"]
mod router_network_interfaces;
#[path = "all/router_output_lifecycle.rs"]
mod router_output_lifecycle;
#[path = "all/router_output_restart.rs"]
mod router_output_restart;
#[path = "all/router_output_stop.rs"]
mod router_output_stop;
#[path = "all/router_outputs.rs"]
mod router_outputs;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/update_bridge.rs"]
mod update_bridge;
#[path = "all/update_bridge_output.rs"]
mod update_bridge_output;
#[path = "all/update_bridge_source.rs"]
mod update_bridge_source;
#[path = "all/update_bridge_state.rs"]
mod update_bridge_state;
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
#[path = "all/update_router_network_interface.rs"]
mod update_router_network_interface;
#[path = "all/update_router_output.rs"]
mod update_router_output;
// Declared here rather than in alphabetical order on purpose: this list is appended to by
// every mediaconnect change at once, and the middle of it is where those insertions land.
#[path = "all/describe_gateway_instance.rs"]
mod describe_gateway_instance;
#[path = "all/gateway_instance_listing.rs"]
mod gateway_instance_listing;
