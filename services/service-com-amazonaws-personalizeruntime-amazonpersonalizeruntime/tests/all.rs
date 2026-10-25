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
#[path = "all/action_filter_value_forms.rs"]
mod action_filter_value_forms;
#[path = "all/filter_value_quoting.rs"]
mod filter_value_quoting;
#[path = "all/inference.rs"]
mod inference;
#[path = "all/refusal_wording.rs"]
mod refusal_wording;
#[path = "all/score_denominator.rs"]
mod score_denominator;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/support.rs"]
mod support;
#[path = "all/unresolved_resource_arns.rs"]
mod unresolved_resource_arns;
#[path = "all/unused_personalization_input.rs"]
mod unused_personalization_input;
