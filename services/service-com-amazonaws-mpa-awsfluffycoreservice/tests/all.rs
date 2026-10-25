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
#[path = "all/absent_approval_team_version_delete.rs"]
mod absent_approval_team_version_delete;
#[path = "all/approval_sessions.rs"]
mod approval_sessions;
#[path = "all/approval_team_lifecycle.rs"]
mod approval_team_lifecycle;
#[path = "all/approver_mfa_synchronization.rs"]
mod approver_mfa_synchronization;
#[path = "all/authorization_gate.rs"]
mod authorization_gate;
#[path = "all/baseline_session_minting.rs"]
mod baseline_session_minting;
#[path = "all/identity_source_approval_portal.rs"]
mod identity_source_approval_portal;
#[path = "all/identity_source_deletion.rs"]
mod identity_source_deletion;
#[path = "all/identity_sources.rs"]
mod identity_sources;
#[path = "all/inactive_team_version_deletion.rs"]
mod inactive_team_version_deletion;
#[path = "all/management_account_gate_order.rs"]
mod management_account_gate_order;
#[path = "all/pending_team_update.rs"]
mod pending_team_update;
#[path = "all/policy_catalog.rs"]
mod policy_catalog;
#[path = "all/policy_version_detail.rs"]
mod policy_version_detail;
#[path = "all/policy_versions.rs"]
mod policy_versions;
#[path = "all/rejection_wording.rs"]
mod rejection_wording;
#[path = "all/reserved_tag_namespace.rs"]
mod reserved_tag_namespace;
#[path = "all/resource_arn_reads.rs"]
mod resource_arn_reads;
#[path = "all/resource_policies.rs"]
mod resource_policies;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/session_cancellation.rs"]
mod session_cancellation;
#[path = "all/session_listing.rs"]
mod session_listing;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/write_path_input_validation.rs"]
mod write_path_input_validation;
