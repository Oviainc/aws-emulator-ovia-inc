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
#[path = "all/channel_listing.rs"]
mod channel_listing;
#[path = "all/channel_policy.rs"]
mod channel_policy;
#[path = "all/channel_policy_delete.rs"]
mod channel_policy_delete;
#[path = "all/channel_policy_lifecycle.rs"]
mod channel_policy_lifecycle;
#[path = "all/channel_reset.rs"]
mod channel_reset;
#[path = "all/channel_update.rs"]
mod channel_update;
#[path = "all/control_plane_validation.rs"]
mod control_plane_validation;
#[path = "all/data_plane_credentials.rs"]
mod data_plane_credentials;
#[path = "all/error_bodies.rs"]
mod error_bodies;
#[path = "all/origin_endpoint_policy.rs"]
mod origin_endpoint_policy;
#[path = "all/origin_endpoint_reset.rs"]
mod origin_endpoint_reset;
#[path = "all/refusal_text_and_entity_tags.rs"]
mod refusal_text_and_entity_tags;
#[path = "all/rejection_rule_and_negative_paths.rs"]
mod rejection_rule_and_negative_paths;
#[path = "all/resource_tagging.rs"]
mod resource_tagging;
#[path = "all/server_materialised_members.rs"]
mod server_materialised_members;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/update_channel.rs"]
mod update_channel;
#[path = "all/update_origin_endpoint.rs"]
mod update_origin_endpoint;
#[path = "all/update_roundtrip.rs"]
mod update_roundtrip;

// Declared last, out of the sorted order above, on purpose: every new suite adds a line
// to this file, and a line inserted in the middle of the list conflicts with every other
// suite landing in the same window. Appending keeps this change out of that contended
// region. The suite itself is an ordinary `tests/all/<name>.rs` file.
#[path = "all/resource_tags.rs"]
mod resource_tags;
