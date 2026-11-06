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
#[path = "all/boundary_refusal_code.rs"]
mod boundary_refusal_code;
#[path = "all/connection_sessions.rs"]
mod connection_sessions;
#[path = "all/list_retained_messages.rs"]
mod list_retained_messages;
#[path = "all/publish_topic_and_payload_limits.rs"]
mod publish_topic_and_payload_limits;
#[path = "all/refusal_wording.rs"]
mod refusal_wording;
#[path = "all/reserved_topic_namespace.rs"]
mod reserved_topic_namespace;
#[path = "all/response_envelope.rs"]
mod response_envelope;
#[path = "all/shadow_and_retained_message.rs"]
mod shadow_and_retained_message;
#[path = "all/shadow_client_token.rs"]
mod shadow_client_token;
#[path = "all/shadow_document_media_type.rs"]
mod shadow_document_media_type;
#[path = "all/shadow_version_counter.rs"]
mod shadow_version_counter;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
