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
#[path = "all/constraint_refusal_messages.rs"]
mod constraint_refusal_messages;
#[path = "all/ingestion.rs"]
mod ingestion;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/tracking_id_refusals.rs"]
mod tracking_id_refusals;
