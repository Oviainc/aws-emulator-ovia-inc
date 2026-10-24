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
#[path = "all/findings_reports_listing.rs"]
mod findings_reports_listing;
#[path = "all/notification_channels.rs"]
mod notification_channels;
#[path = "all/notification_configuration.rs"]
mod notification_configuration;
#[path = "all/permission_writes.rs"]
mod permission_writes;
#[path = "all/policy_surface.rs"]
mod policy_surface;
#[path = "all/profile_times.rs"]
mod profile_times;
#[path = "all/profiling_group_lifecycle.rs"]
mod profiling_group_lifecycle;
#[path = "all/recommendations_analysis.rs"]
mod recommendations_analysis;
#[path = "all/refusal_message_formats.rs"]
mod refusal_message_formats;
#[path = "all/refusal_message_rules.rs"]
mod refusal_message_rules;
#[path = "all/resource_policy.rs"]
mod resource_policy;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/tag_resource.rs"]
mod tag_resource;
#[path = "all/untag_resource.rs"]
mod untag_resource;
#[path = "all/update_profiling_group.rs"]
mod update_profiling_group;
#[path = "all/wire_response_shape.rs"]
mod wire_response_shape;
