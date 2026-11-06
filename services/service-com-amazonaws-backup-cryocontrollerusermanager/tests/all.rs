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
#[path = "all/backup_job_listing.rs"]
mod backup_job_listing;
#[path = "all/backup_plan.rs"]
mod backup_plan;
#[path = "all/backup_plan_deletion.rs"]
mod backup_plan_deletion;
#[path = "all/backup_plan_update.rs"]
mod backup_plan_update;
#[path = "all/backup_selection.rs"]
mod backup_selection;
#[path = "all/copy_job.rs"]
mod copy_job;
#[path = "all/error_bodies.rs"]
mod error_bodies;
#[path = "all/framework.rs"]
mod framework;
#[path = "all/framework_update.rs"]
mod framework_update;
#[path = "all/legal_hold.rs"]
mod legal_hold;
#[path = "all/legal_hold_cancel_state_refusal.rs"]
mod legal_hold_cancel_state_refusal;
#[path = "all/legal_hold_coverage.rs"]
mod legal_hold_coverage;
#[path = "all/legal_hold_date_range.rs"]
mod legal_hold_date_range;
#[path = "all/logically_air_gapped_vault.rs"]
mod logically_air_gapped_vault;
#[path = "all/recovery_point_deletion.rs"]
mod recovery_point_deletion;
#[path = "all/recovery_point_read.rs"]
mod recovery_point_read;
#[path = "all/restore_access_vault.rs"]
mod restore_access_vault;
#[path = "all/scheduled_runs_preview.rs"]
mod scheduled_runs_preview;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/supported_resource_types.rs"]
mod supported_resource_types;
#[path = "all/vault_and_backup_job.rs"]
mod vault_and_backup_job;
#[path = "all/vault_lock_refusal_order.rs"]
mod vault_lock_refusal_order;
#[path = "all/vault_read_projection.rs"]
mod vault_read_projection;
#[path = "all/vault_read_visibility.rs"]
mod vault_read_visibility;
#[path = "all/vault_type_and_parameter_refusals.rs"]
mod vault_type_and_parameter_refusals;
