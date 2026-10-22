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
#[path = "all/access_preview.rs"]
mod access_preview;
#[path = "all/analyzer_lifecycle.rs"]
mod analyzer_lifecycle;
#[path = "all/archive_rule_update.rs"]
mod archive_rule_update;
#[path = "all/check_no_public_access.rs"]
mod check_no_public_access;
#[path = "all/error_payloads.rs"]
mod error_payloads;
#[path = "all/harness.rs"]
mod harness;
#[path = "all/modeled_rejections.rs"]
mod modeled_rejections;
#[path = "all/modelled_wire_shape.rs"]
mod modelled_wire_shape;
#[path = "all/no_content_responses.rs"]
mod no_content_responses;
#[path = "all/policy_access_check.rs"]
mod policy_access_check;
#[path = "all/policy_checks.rs"]
mod policy_checks;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/response_vocabulary.rs"]
mod response_vocabulary;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/update_analyzer.rs"]
mod update_analyzer;
