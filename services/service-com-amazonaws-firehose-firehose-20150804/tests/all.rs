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
#[path = "all/cloudwatch_logs_source_gate.rs"]
mod cloudwatch_logs_source_gate;
#[path = "all/delivery_stream_encryption.rs"]
mod delivery_stream_encryption;
#[path = "all/delivery_stream_lifecycle.rs"]
mod delivery_stream_lifecycle;
#[path = "all/delivery_stream_tag_updates.rs"]
mod delivery_stream_tag_updates;
#[path = "all/delivery_stream_tags.rs"]
mod delivery_stream_tags;
#[path = "all/description_defaults.rs"]
mod description_defaults;
#[path = "all/destination_update.rs"]
mod destination_update;
#[path = "all/refusal_messages.rs"]
mod refusal_messages;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/stream_encryption.rs"]
mod stream_encryption;
