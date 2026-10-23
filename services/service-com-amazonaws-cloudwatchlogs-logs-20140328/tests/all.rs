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
#[path = "all/deletion_protection.rs"]
mod deletion_protection;
#[path = "all/harness.rs"]
mod harness;
#[path = "all/ingestion_window.rs"]
mod ingestion_window;
#[path = "all/log_core_loop.rs"]
mod log_core_loop;
#[path = "all/log_group_auth_and_encryption.rs"]
mod log_group_auth_and_encryption;
#[path = "all/log_group_name_pattern_lists.rs"]
mod log_group_name_pattern_lists;
#[path = "all/log_stream_removal.rs"]
mod log_stream_removal;
#[path = "all/resource_policies.rs"]
mod resource_policies;
#[path = "all/retention_policy.rs"]
mod retention_policy;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/subscription_filters.rs"]
mod subscription_filters;
