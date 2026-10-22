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
#[path = "all/custom_lens_import.rs"]
mod custom_lens_import;

#[path = "all/lens_review_answers.rs"]
mod lens_review_answers;

#[path = "all/lookup_error_envelopes.rs"]
mod lookup_error_envelopes;

#[path = "all/milestone_listing.rs"]
mod milestone_listing;

#[path = "all/review_template_answers.rs"]
mod review_template_answers;

#[path = "all/review_template_lifecycle.rs"]
mod review_template_lifecycle;

#[path = "all/share_invitations.rs"]
mod share_invitations;

#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;

#[path = "all/validation_envelopes.rs"]
mod validation_envelopes;

#[path = "all/workload_computed_members.rs"]
mod workload_computed_members;

#[path = "all/workload_harness.rs"]
mod workload_harness;

#[path = "all/workload_lifecycle.rs"]
mod workload_lifecycle;

#[path = "all/workload_shares.rs"]
mod workload_shares;
