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
#[path = "all/audit_records.rs"]
mod audit_records;
#[path = "all/dashboard_lifecycle.rs"]
mod dashboard_lifecycle;
#[path = "all/resource_not_found.rs"]
mod resource_not_found;
#[path = "all/resource_policy.rs"]
mod resource_policy;
#[path = "all/response_media_type.rs"]
mod response_media_type;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/scheduled_report_lifecycle.rs"]
mod scheduled_report_lifecycle;
#[path = "all/scheduled_report_listing.rs"]
mod scheduled_report_listing;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/tag_resource.rs"]
mod tag_resource;
#[path = "all/untag_resource.rs"]
mod untag_resource;
