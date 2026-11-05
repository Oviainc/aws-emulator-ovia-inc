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
#[path = "all/application_associations.rs"]
mod application_associations;
#[path = "all/application_deletion.rs"]
mod application_deletion;
#[path = "all/application_updates.rs"]
mod application_updates;
#[path = "all/applications.rs"]
mod applications;
#[path = "all/data_integration_associations.rs"]
mod data_integration_associations;
#[path = "all/data_integrations.rs"]
mod data_integrations;
#[path = "all/event_integrations.rs"]
mod event_integrations;
#[path = "all/harness.rs"]
mod harness;
#[path = "all/not_found_messages.rs"]
mod not_found_messages;
#[path = "all/request_body_rejection.rs"]
mod request_body_rejection;
#[path = "all/server_defaulted_members.rs"]
mod server_defaulted_members;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
