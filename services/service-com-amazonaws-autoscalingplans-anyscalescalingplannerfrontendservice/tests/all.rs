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
#[path = "all/absent_plan_reads.rs"]
mod absent_plan_reads;
#[path = "all/application_source_exclusivity.rs"]
mod application_source_exclusivity;
#[path = "all/applied_policies_and_status_narratives.rs"]
mod applied_policies_and_status_narratives;
#[path = "all/describe_scaling_plans_selector_bounds.rs"]
mod describe_scaling_plans_selector_bounds;
#[path = "all/describe_scaling_plans_version_member.rs"]
mod describe_scaling_plans_version_member;
#[path = "all/error_message_wording.rs"]
mod error_message_wording;
#[path = "all/forecast_data.rs"]
mod forecast_data;
#[path = "all/forecast_datapoints.rs"]
mod forecast_datapoints;
#[path = "all/forecast_window_precision.rs"]
mod forecast_window_precision;
#[path = "all/identity_key_separators.rs"]
mod identity_key_separators;
#[path = "all/plan_version_member_validation.rs"]
mod plan_version_member_validation;
#[path = "all/predictive_scaling_predicate.rs"]
mod predictive_scaling_predicate;
#[path = "all/scaling_instruction_validation.rs"]
mod scaling_instruction_validation;
#[path = "all/scaling_mode_conditional_members.rs"]
mod scaling_mode_conditional_members;
#[path = "all/scaling_plan_lifecycle.rs"]
mod scaling_plan_lifecycle;
#[path = "all/scaling_plan_resource_policies.rs"]
mod scaling_plan_resource_policies;
#[path = "all/scheduled_action_buffer_time.rs"]
mod scheduled_action_buffer_time;
#[path = "all/stack_arn_predicate_consistency.rs"]
mod stack_arn_predicate_consistency;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/tag_filter_entry_bounds.rs"]
mod tag_filter_entry_bounds;
