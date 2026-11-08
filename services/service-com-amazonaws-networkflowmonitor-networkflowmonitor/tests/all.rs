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
#[path = "all/control_plane_updates.rs"]
mod control_plane_updates;
#[path = "all/error_message_rules.rs"]
mod error_message_rules;
#[path = "all/harness.rs"]
mod harness;
#[path = "all/monitor_update.rs"]
mod monitor_update;
#[path = "all/query_lifecycle.rs"]
mod query_lifecycle;
#[path = "all/query_terminal_and_result_reporting.rs"]
mod query_terminal_and_result_reporting;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/scope_listing.rs"]
mod scope_listing;
#[path = "all/scope_monitor_lifecycle.rs"]
mod scope_monitor_lifecycle;
#[path = "all/scope_quota.rs"]
mod scope_quota;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/wire_status_and_error_classes.rs"]
mod wire_status_and_error_classes;
