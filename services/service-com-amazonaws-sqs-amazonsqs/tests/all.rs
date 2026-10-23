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
#[path = "all/delete_message_batch.rs"]
mod delete_message_batch;
#[path = "all/message_lifecycle.rs"]
mod message_lifecycle;
#[path = "all/message_plane.rs"]
mod message_plane;
#[path = "all/queue_configuration.rs"]
mod queue_configuration;
#[path = "all/queue_does_not_exist.rs"]
mod queue_does_not_exist;
#[path = "all/queue_tags.rs"]
mod queue_tags;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/support.rs"]
mod support;
