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
#[path = "all/campaign_communication_limits.rs"]
mod campaign_communication_limits;
#[path = "all/campaign_communication_limits_deletion.rs"]
mod campaign_communication_limits_deletion;
#[path = "all/campaign_communication_time_deletion.rs"]
mod campaign_communication_time_deletion;
#[path = "all/campaign_communication_time_update.rs"]
mod campaign_communication_time_update;
#[path = "all/campaign_flow_association.rs"]
mod campaign_flow_association;
#[path = "all/campaign_lifecycle.rs"]
mod campaign_lifecycle;
#[path = "all/campaign_name_uniqueness.rs"]
mod campaign_name_uniqueness;
#[path = "all/campaign_not_found_message.rs"]
mod campaign_not_found_message;
#[path = "all/campaign_resume.rs"]
mod campaign_resume;
#[path = "all/campaign_schedule_completion.rs"]
mod campaign_schedule_completion;
#[path = "all/common.rs"]
mod common;
#[path = "all/connect_instance_integration_deletion.rs"]
mod connect_instance_integration_deletion;
#[path = "all/connect_instance_integrations.rs"]
mod connect_instance_integrations;
#[path = "all/instance_communication_limits.rs"]
mod instance_communication_limits;
#[path = "all/instance_onboarding.rs"]
mod instance_onboarding;
#[path = "all/instance_onboarding_tags_and_outbound_requests.rs"]
mod instance_onboarding_tags_and_outbound_requests;
#[path = "all/onboarding_job_deletion.rs"]
mod onboarding_job_deletion;
#[path = "all/outbound_request_expiration.rs"]
mod outbound_request_expiration;
#[path = "all/profile_outbound_requests.rs"]
mod profile_outbound_requests;
#[path = "all/signing_name_path_claim.rs"]
mod signing_name_path_claim;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/tag_key_rules.rs"]
mod tag_key_rules;
