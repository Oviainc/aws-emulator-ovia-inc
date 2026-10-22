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
#[path = "all/auto_management_configuration.rs"]
mod auto_management_configuration;
#[path = "all/catalog_pagination.rs"]
mod catalog_pagination;
#[path = "all/default_quota_catalog.rs"]
mod default_quota_catalog;
#[path = "all/error_envelope.rs"]
mod error_envelope;
#[path = "all/quota_workflow.rs"]
mod quota_workflow;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/support_case_and_templates.rs"]
mod support_case_and_templates;
#[path = "all/tags.rs"]
mod tags;
