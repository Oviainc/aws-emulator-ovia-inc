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
#[path = "all/front_end_refusals.rs"]
mod front_end_refusals;
#[path = "all/list_links.rs"]
mod list_links;
#[path = "all/list_sinks.rs"]
mod list_sinks;
#[path = "all/list_tags_for_resource.rs"]
mod list_tags_for_resource;
#[path = "all/missing_required_properties.rs"]
mod missing_required_properties;
#[path = "all/refusal_classes.rs"]
mod refusal_classes;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/same_account_link_rejection.rs"]
mod same_account_link_rejection;
#[path = "all/sink_policy.rs"]
mod sink_policy;
#[path = "all/sinks_and_links.rs"]
mod sinks_and_links;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/untag_resource.rs"]
mod untag_resource;
#[path = "all/update_link.rs"]
mod update_link;
