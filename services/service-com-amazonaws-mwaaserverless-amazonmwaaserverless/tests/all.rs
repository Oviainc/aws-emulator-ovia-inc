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
#[path = "all/arn_validation_errors.rs"]
mod arn_validation_errors;
#[path = "all/child_listing_cursor_scope.rs"]
mod child_listing_cursor_scope;
#[path = "all/child_listing_error_set.rs"]
mod child_listing_error_set;
#[path = "all/missing_resource_lookups.rs"]
mod missing_resource_lookups;
#[path = "all/region_isolation.rs"]
mod region_isolation;
#[path = "all/resource_tag_listing.rs"]
mod resource_tag_listing;
#[path = "all/run_lifecycle_progression.rs"]
mod run_lifecycle_progression;
#[path = "all/run_stop.rs"]
mod run_stop;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/task_instance_reads.rs"]
mod task_instance_reads;
#[path = "all/validation_errors.rs"]
mod validation_errors;
#[path = "all/workflow_deletion_teardown.rs"]
mod workflow_deletion_teardown;
#[path = "all/workflow_lifecycle.rs"]
mod workflow_lifecycle;
#[path = "all/workflow_run_listing.rs"]
mod workflow_run_listing;
#[path = "all/workflow_run_listing_order.rs"]
mod workflow_run_listing_order;
#[path = "all/workflow_update_versions.rs"]
mod workflow_update_versions;
#[path = "all/workflow_version_cursor_continuity.rs"]
mod workflow_version_cursor_continuity;
#[path = "all/workflow_version_listing.rs"]
mod workflow_version_listing;
