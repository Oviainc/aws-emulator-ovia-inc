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
#[path = "all/ad_configuration.rs"]
mod ad_configuration;
#[path = "all/batch_get_channel.rs"]
mod batch_get_channel;
#[path = "all/batch_get_stream_key.rs"]
mod batch_get_stream_key;
#[path = "all/channel_lifecycle.rs"]
mod channel_lifecycle;
#[path = "all/live_stream_reads.rs"]
mod live_stream_reads;
#[path = "all/recording_configuration_lifecycle.rs"]
mod recording_configuration_lifecycle;
#[path = "all/recording_configuration_list.rs"]
mod recording_configuration_list;
#[path = "all/signing_name_routing.rs"]
mod signing_name_routing;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/stream_key_lifecycle.rs"]
mod stream_key_lifecycle;
#[path = "all/stream_key_read.rs"]
mod stream_key_read;
#[path = "all/viewer_session_revocation.rs"]
mod viewer_session_revocation;
