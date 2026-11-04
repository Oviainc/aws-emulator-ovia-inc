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
#[path = "all/attendee_capabilities.rs"]
mod attendee_capabilities;
#[path = "all/batch_update_attendee_capabilities_except.rs"]
mod batch_update_attendee_capabilities_except;
#[path = "all/capability_refusal_message.rs"]
mod capability_refusal_message;
#[path = "all/create_meeting_with_attendees.rs"]
mod create_meeting_with_attendees;
#[path = "all/delete_meeting_idempotence.rs"]
mod delete_meeting_idempotence;
#[path = "all/meeting_attendee_lifecycle.rs"]
mod meeting_attendee_lifecycle;
#[path = "all/meeting_projection.rs"]
mod meeting_projection;
#[path = "all/meeting_transcription.rs"]
mod meeting_transcription;
#[path = "all/not_found_message.rs"]
mod not_found_message;
#[path = "all/resource_tagging.rs"]
mod resource_tagging;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/stop_meeting_transcription.rs"]
mod stop_meeting_transcription;
