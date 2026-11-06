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
#[path = "all/action_group_rule.rs"]
mod action_group_rule;
#[path = "all/agent_reported_state.rs"]
mod agent_reported_state;
#[path = "all/aggregated_profile_combination.rs"]
mod aggregated_profile_combination;
#[path = "all/analysis_list_presence.rs"]
mod analysis_list_presence;
#[path = "all/anomaly_feedback.rs"]
mod anomaly_feedback;
#[path = "all/create_idempotency_records.rs"]
mod create_idempotency_records;
#[path = "all/document_number_magnitudes.rs"]
mod document_number_magnitudes;
#[path = "all/document_read_concurrency.rs"]
mod document_read_concurrency;
#[path = "all/findings_report_account_summary.rs"]
mod findings_report_account_summary;
#[path = "all/findings_reports_listing.rs"]
mod findings_reports_listing;
#[path = "all/frame_metric_data.rs"]
mod frame_metric_data;
#[path = "all/get_profile_interval.rs"]
mod get_profile_interval;
#[path = "all/get_profile_latest_profile.rs"]
mod get_profile_latest_profile;
#[path = "all/get_profile_window_companion.rs"]
mod get_profile_window_companion;
#[path = "all/get_profile_window_length.rs"]
mod get_profile_window_length;
#[path = "all/group_not_found_sentence_source.rs"]
mod group_not_found_sentence_source;
#[path = "all/input_refusal_is_modelled.rs"]
mod input_refusal_is_modelled;
#[path = "all/instant_comparison_resolution.rs"]
mod instant_comparison_resolution;
#[path = "all/interval_resolution.rs"]
mod interval_resolution;
#[path = "all/interval_validation.rs"]
mod interval_validation;
#[path = "all/last_statement_removal_deletes_the_policy.rs"]
mod last_statement_removal_deletes_the_policy;
#[path = "all/latest_aggregated_profile.rs"]
mod latest_aggregated_profile;
#[path = "all/list_pagination.rs"]
mod list_pagination;
#[path = "all/message_timestamp_rendering.rs"]
mod message_timestamp_rendering;
#[path = "all/minted_ids_follow_applied_writes.rs"]
mod minted_ids_follow_applied_writes;
#[path = "all/not_found_surfaces.rs"]
mod not_found_surfaces;
#[path = "all/notification_channels.rs"]
mod notification_channels;
#[path = "all/notification_configuration.rs"]
mod notification_configuration;
#[path = "all/pending_aggregation.rs"]
mod pending_aggregation;
#[path = "all/permission_writes.rs"]
mod permission_writes;
#[path = "all/policy_surface.rs"]
mod policy_surface;
#[path = "all/posted_profile_validation.rs"]
mod posted_profile_validation;
#[path = "all/principal_arn_validation.rs"]
mod principal_arn_validation;
#[path = "all/profile_representations.rs"]
mod profile_representations;
#[path = "all/profile_times.rs"]
mod profile_times;
#[path = "all/profiling_group_lifecycle.rs"]
mod profiling_group_lifecycle;
#[path = "all/recommendations_analysis.rs"]
mod recommendations_analysis;
#[path = "all/recommendations_pending_aggregation.rs"]
mod recommendations_pending_aggregation;
#[path = "all/refusal_message_formats.rs"]
mod refusal_message_formats;
#[path = "all/refusal_message_rules.rs"]
mod refusal_message_rules;
#[path = "all/removal_of_the_last_statement_reports_no_policy.rs"]
mod removal_of_the_last_statement_reports_no_policy;
#[path = "all/removal_reports_the_policy.rs"]
mod removal_reports_the_policy;
#[path = "all/resource_policy.rs"]
mod resource_policy;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/served_media_type.rs"]
mod served_media_type;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/stored_profile_representation.rs"]
mod stored_profile_representation;
#[path = "all/tag_resource.rs"]
mod tag_resource;
#[path = "all/unissued_cursor.rs"]
mod unissued_cursor;
#[path = "all/unknown_channel_refusal.rs"]
mod unknown_channel_refusal;
#[path = "all/untag_resource.rs"]
mod untag_resource;
#[path = "all/update_profiling_group.rs"]
mod update_profiling_group;
#[path = "all/wire_response_shape.rs"]
mod wire_response_shape;
