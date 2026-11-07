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
#[path = "all/error_code_member.rs"]
mod error_code_member;
#[path = "all/feed_and_query_results.rs"]
mod feed_and_query_results;
#[path = "all/health_events.rs"]
mod health_events;
#[path = "all/internet_events.rs"]
mod internet_events;
#[path = "all/monitor_lifecycle.rs"]
mod monitor_lifecycle;
#[path = "all/query_interface.rs"]
mod query_interface;
#[path = "all/refusal_shapes.rs"]
mod refusal_shapes;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/untag_resource.rs"]
mod untag_resource;
