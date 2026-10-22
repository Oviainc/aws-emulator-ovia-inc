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
#[path = "all/amend_benefit_application.rs"]
mod amend_benefit_application;
#[path = "all/benefit_application_lifecycle.rs"]
mod benefit_application_lifecycle;
#[path = "all/entitlement_denial.rs"]
mod entitlement_denial;
#[path = "all/recall_benefit_application.rs"]
mod recall_benefit_application;
#[path = "all/resource_tagging.rs"]
mod resource_tagging;
// `all/resource_tags.rs` is the `UntagResource` suite; it reuses the helpers of the
// sibling `resource_tagging` suite (`TagResource` / `ListTagsForResource`), and each
// suite's module doc says which operations it covers. (The build script requires the
// module name to match the file name, so the two names stay as they are.)
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/service_error_rules.rs"]
mod service_error_rules;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/validation_error_body.rs"]
mod validation_error_body;
#[path = "all/validation_exception_message.rs"]
mod validation_exception_message;
