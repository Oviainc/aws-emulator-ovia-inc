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
#[path = "all/client_rejections.rs"]
mod client_rejections;
#[path = "all/client_token_store.rs"]
mod client_token_store;
#[path = "all/detector_data_sources_projection.rs"]
mod detector_data_sources_projection;
#[path = "all/detector_lifecycle.rs"]
mod detector_lifecycle;
#[path = "all/findings_archive_state.rs"]
mod findings_archive_state;
#[path = "all/findings_feedback.rs"]
mod findings_feedback;
#[path = "all/investigation_lifecycle.rs"]
mod investigation_lifecycle;
#[path = "all/ip_set_lifecycle.rs"]
mod ip_set_lifecycle;
#[path = "all/malware_scan_settings.rs"]
mod malware_scan_settings;
#[path = "all/member_accounts.rs"]
mod member_accounts;
#[path = "all/member_association.rs"]
mod member_association;
#[path = "all/modeled_constraint_refusals.rs"]
mod modeled_constraint_refusals;
#[path = "all/sample_finding_catalog.rs"]
mod sample_finding_catalog;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
