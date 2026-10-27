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
#[path = "all/amplify_registry.rs"]
mod amplify_registry;
#[path = "all/app_registration_gate.rs"]
mod app_registration_gate;
#[path = "all/backend_job_listing.rs"]
mod backend_job_listing;
#[path = "all/backend_job_not_found.rs"]
mod backend_job_not_found;
#[path = "all/backend_lifecycle.rs"]
mod backend_lifecycle;
#[path = "all/backend_storage.rs"]
mod backend_storage;
#[path = "all/challenge_tokens.rs"]
mod challenge_tokens;
#[path = "all/control_plane_fidelity.rs"]
mod control_plane_fidelity;
#[path = "all/error_message_form.rs"]
mod error_message_form;
#[path = "all/remove_all_backends.rs"]
mod remove_all_backends;
#[path = "all/s3_buckets.rs"]
mod s3_buckets;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
