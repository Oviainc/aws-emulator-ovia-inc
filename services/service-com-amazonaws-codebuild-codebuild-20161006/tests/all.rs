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
#[path = "all/batch_delete_builds.rs"]
mod batch_delete_builds;
#[path = "all/fleet_lifecycle.rs"]
mod fleet_lifecycle;
#[path = "all/list_builds.rs"]
mod list_builds;
#[path = "all/list_sandboxes.rs"]
mod list_sandboxes;
#[path = "all/list_sandboxes_for_project.rs"]
mod list_sandboxes_for_project;
#[path = "all/list_sandboxes_for_project_paging.rs"]
mod list_sandboxes_for_project_paging;
#[path = "all/project_build_batch_lifecycle.rs"]
mod project_build_batch_lifecycle;
#[path = "all/project_build_lifecycle.rs"]
mod project_build_lifecycle;
#[path = "all/report_group_lifecycle.rs"]
mod report_group_lifecycle;
#[path = "all/report_group_reads.rs"]
mod report_group_reads;
#[path = "all/retry_build.rs"]
mod retry_build;
#[path = "all/sandbox_lifecycle.rs"]
mod sandbox_lifecycle;
#[path = "all/sandbox_sessions.rs"]
mod sandbox_sessions;
#[path = "all/source_credentials.rs"]
mod source_credentials;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/webhook_lifecycle.rs"]
mod webhook_lifecycle;
