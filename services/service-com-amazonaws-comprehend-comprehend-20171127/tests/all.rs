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
#[path = "all/comprehend_client.rs"]
mod comprehend_client;
#[path = "all/document_classifiers.rs"]
mod document_classifiers;
#[path = "all/key_phrases_jobs.rs"]
mod key_phrases_jobs;
#[path = "all/request_validation.rs"]
mod request_validation;
#[path = "all/sentiment_jobs.rs"]
mod sentiment_jobs;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/sync_inference.rs"]
mod sync_inference;
