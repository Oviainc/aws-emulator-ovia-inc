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
#[path = "all/account_settings.rs"]
mod account_settings;
#[path = "all/compliance_inquiry_export.rs"]
mod compliance_inquiry_export;
#[path = "all/compliance_inquiry_feedback.rs"]
mod compliance_inquiry_feedback;
#[path = "all/compliance_inquiry_lifecycle.rs"]
mod compliance_inquiry_lifecycle;
#[path = "all/compliance_inquiry_listing.rs"]
mod compliance_inquiry_listing;
#[path = "all/compliance_inquiry_queries.rs"]
mod compliance_inquiry_queries;
#[path = "all/customer_agreements.rs"]
mod customer_agreements;
#[path = "all/front_door_errors.rs"]
mod front_door_errors;
#[path = "all/harness.rs"]
mod harness;
#[path = "all/pagination_token_opacity.rs"]
mod pagination_token_opacity;
#[path = "all/report_projection.rs"]
mod report_projection;
#[path = "all/report_workflow.rs"]
mod report_workflow;
#[path = "all/resource_tag_writes.rs"]
mod resource_tag_writes;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/timestamp_wire_form.rs"]
mod timestamp_wire_form;
#[path = "all/validation_message_rendering.rs"]
mod validation_message_rendering;
