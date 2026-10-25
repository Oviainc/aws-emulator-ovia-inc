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
#[path = "all/bounds.rs"]
mod bounds;
#[path = "all/cancel_job.rs"]
mod cancel_job;
#[path = "all/compute_environment_update.rs"]
mod compute_environment_update;
#[path = "all/consumable_resource_listing.rs"]
mod consumable_resource_listing;
#[path = "all/consumable_resource_updates.rs"]
mod consumable_resource_updates;
#[path = "all/consumable_resources.rs"]
mod consumable_resources;
#[path = "all/job_definition_deregistration.rs"]
mod job_definition_deregistration;
#[path = "all/job_definition_registry.rs"]
mod job_definition_registry;
#[path = "all/job_lifecycle.rs"]
mod job_lifecycle;
#[path = "all/job_queue_updates.rs"]
mod job_queue_updates;
#[path = "all/job_state_change_events.rs"]
mod job_state_change_events;
#[path = "all/quota_share_listing.rs"]
mod quota_share_listing;
#[path = "all/refusal_contract.rs"]
mod refusal_contract;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/scheduling_policies.rs"]
mod scheduling_policies;
#[path = "all/scheduling_policy_listing.rs"]
mod scheduling_policy_listing;
#[path = "all/scheduling_policy_refusal_messages.rs"]
mod scheduling_policy_refusal_messages;
#[path = "all/scheduling_policy_teardown_and_conflicts.rs"]
mod scheduling_policy_teardown_and_conflicts;
#[path = "all/service_environment.rs"]
mod service_environment;
#[path = "all/service_job_capacity_placement.rs"]
mod service_job_capacity_placement;
#[path = "all/service_job_listing.rs"]
mod service_job_listing;
#[path = "all/service_job_priority_updates.rs"]
mod service_job_priority_updates;
#[path = "all/service_job_submission.rs"]
mod service_job_submission;
#[path = "all/service_job_termination.rs"]
mod service_job_termination;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/teardown_and_quota_shares.rs"]
mod teardown_and_quota_shares;
