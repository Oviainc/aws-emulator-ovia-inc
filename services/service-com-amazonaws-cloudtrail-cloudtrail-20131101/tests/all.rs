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
#[path = "all/dashboard_lifecycle.rs"]
mod dashboard_lifecycle;
#[path = "all/disable_federation.rs"]
mod disable_federation;
#[path = "all/emulator.rs"]
mod emulator;
#[path = "all/event_data_store_reads.rs"]
mod event_data_store_reads;
#[path = "all/import_stop.rs"]
mod import_stop;
#[path = "all/lake_queries.rs"]
mod lake_queries;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/target_prefix_dispatch.rs"]
mod target_prefix_dispatch;
#[path = "all/target_prefix_routing.rs"]
mod target_prefix_routing;
#[path = "all/target_routing.rs"]
mod target_routing;
#[path = "all/trail_lifecycle.rs"]
mod trail_lifecycle;
#[path = "all/trail_update.rs"]
mod trail_update;
