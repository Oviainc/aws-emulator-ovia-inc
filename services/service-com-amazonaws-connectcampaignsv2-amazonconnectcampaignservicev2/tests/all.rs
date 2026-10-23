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
#[path = "all/campaign_lifecycle.rs"]
mod campaign_lifecycle;
#[path = "all/campaign_not_found_message.rs"]
mod campaign_not_found_message;
#[path = "all/common.rs"]
mod common;
#[path = "all/instance_onboarding.rs"]
mod instance_onboarding;
#[path = "all/instance_onboarding_tags_and_outbound_requests.rs"]
mod instance_onboarding_tags_and_outbound_requests;
#[path = "all/onboarding_job_deletion.rs"]
mod onboarding_job_deletion;
#[path = "all/signing_name_path_claim.rs"]
mod signing_name_path_claim;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
