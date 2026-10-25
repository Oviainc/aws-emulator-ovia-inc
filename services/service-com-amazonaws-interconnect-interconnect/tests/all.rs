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
#[path = "all/arn_scope_refusals.rs"]
mod arn_scope_refusals;
#[path = "all/attach_points.rs"]
mod attach_points;
#[path = "all/billing_tier.rs"]
mod billing_tier;
#[path = "all/caller_created_gateways_are_attach_points.rs"]
mod caller_created_gateways_are_attach_points;
#[path = "all/connection_lifecycle.rs"]
mod connection_lifecycle;
#[path = "all/connection_proposal.rs"]
mod connection_proposal;
#[path = "all/connection_state_progression.rs"]
mod connection_state_progression;
#[path = "all/create_connection_attach_point_validation.rs"]
mod create_connection_attach_point_validation;
#[path = "all/emulator.rs"]
mod emulator;
#[path = "all/list_tags_for_resource.rs"]
mod list_tags_for_resource;
#[path = "all/refusals_and_gateway_attach_points.rs"]
mod refusals_and_gateway_attach_points;
#[path = "all/region_scoped_connections.rs"]
mod region_scoped_connections;
#[path = "all/reserved_tag_keys.rs"]
mod reserved_tag_keys;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/tag_resource.rs"]
mod tag_resource;
