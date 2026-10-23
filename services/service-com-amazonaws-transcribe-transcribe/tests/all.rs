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
#[path = "all/client_error_wire_status.rs"]
mod client_error_wire_status;
#[path = "all/rpc_success_status.rs"]
mod rpc_success_status;
#[path = "all/service_claim.rs"]
mod service_claim;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/transcription_jobs.rs"]
mod transcription_jobs;
#[path = "all/vocabularies.rs"]
mod vocabularies;
#[path = "all/vocabulary_delete_and_tag_sets.rs"]
mod vocabulary_delete_and_tag_sets;
