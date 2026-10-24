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
#[path = "all/access_policy.rs"]
mod access_policy;
#[path = "all/account_settings.rs"]
mod account_settings;
#[path = "all/client_error_status.rs"]
mod client_error_status;
#[path = "all/collection_group_capacity.rs"]
mod collection_group_capacity;
#[path = "all/collection_group_delete.rs"]
mod collection_group_delete;
#[path = "all/collection_group_lifecycle.rs"]
mod collection_group_lifecycle;
#[path = "all/collection_lifecycle.rs"]
mod collection_lifecycle;
#[path = "all/lifecycle_policy.rs"]
mod lifecycle_policy;
#[path = "all/policies_stats.rs"]
mod policies_stats;
#[path = "all/policy_and_group_writes.rs"]
mod policy_and_group_writes;
#[path = "all/security_config_update.rs"]
mod security_config_update;
#[path = "all/security_configs.rs"]
mod security_configs;
#[path = "all/security_policy_listing.rs"]
mod security_policy_listing;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
