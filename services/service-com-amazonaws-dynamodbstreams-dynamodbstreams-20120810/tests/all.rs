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
#[path = "all/list_streams_discovery.rs"]
mod list_streams_discovery;
#[path = "all/not_found_messages.rs"]
mod not_found_messages;
#[path = "all/record_creation_time_whole_seconds.rs"]
mod record_creation_time_whole_seconds;
#[path = "all/record_retention_budget.rs"]
mod record_retention_budget;
#[path = "all/sequence_number_positions.rs"]
mod sequence_number_positions;
#[path = "all/service_enforced_parameter_bounds.rs"]
mod service_enforced_parameter_bounds;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/stream_consumer.rs"]
mod stream_consumer;
#[path = "all/stream_enable_readiness.rs"]
mod stream_enable_readiness;
