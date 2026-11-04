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
#[path = "all/route_claims.rs"]
mod route_claims;
#[path = "all/signaling_channel_lifecycle.rs"]
mod signaling_channel_lifecycle;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/stream_configuration.rs"]
mod stream_configuration;
#[path = "all/stream_data_endpoint.rs"]
mod stream_data_endpoint;
#[path = "all/stream_data_retention.rs"]
mod stream_data_retention;
#[path = "all/stream_data_retention_projection.rs"]
mod stream_data_retention_projection;
#[path = "all/stream_deletion_state.rs"]
mod stream_deletion_state;
#[path = "all/stream_lifecycle.rs"]
mod stream_lifecycle;
#[path = "all/stream_record_state_model.rs"]
mod stream_record_state_model;
#[path = "all/stream_tags.rs"]
mod stream_tags;
#[path = "all/stream_version_mismatch_message.rs"]
mod stream_version_mismatch_message;
