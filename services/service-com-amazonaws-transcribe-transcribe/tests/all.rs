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
#[path = "all/call_analytics_categories.rs"]
mod call_analytics_categories;
#[path = "all/call_analytics_category_listing.rs"]
mod call_analytics_category_listing;
#[path = "all/call_analytics_category_updates.rs"]
mod call_analytics_category_updates;
#[path = "all/call_analytics_job_delete.rs"]
mod call_analytics_job_delete;
#[path = "all/call_analytics_job_results.rs"]
mod call_analytics_job_results;
#[path = "all/call_analytics_job_start_limits.rs"]
mod call_analytics_job_start_limits;
#[path = "all/call_analytics_jobs.rs"]
mod call_analytics_jobs;
#[path = "all/call_analytics_rule_time_ranges.rs"]
mod call_analytics_rule_time_ranges;
#[path = "all/client_error_wire_status.rs"]
mod client_error_wire_status;
#[path = "all/delete_requires_the_entity.rs"]
mod delete_requires_the_entity;
#[path = "all/describe_language_model.rs"]
mod describe_language_model;
#[path = "all/job_concurrency_quota.rs"]
mod job_concurrency_quota;
#[path = "all/language_model_listing.rs"]
mod language_model_listing;
#[path = "all/language_models.rs"]
mod language_models;
#[path = "all/medical_scribe_job_listing.rs"]
mod medical_scribe_job_listing;
#[path = "all/medical_scribe_jobs.rs"]
mod medical_scribe_jobs;
#[path = "all/medical_scribe_settings_references.rs"]
mod medical_scribe_settings_references;
#[path = "all/medical_transcription_job_listing.rs"]
mod medical_transcription_job_listing;
#[path = "all/medical_transcription_jobs.rs"]
mod medical_transcription_jobs;
#[path = "all/medical_vocabularies.rs"]
mod medical_vocabularies;
#[path = "all/medical_vocabulary_constraints.rs"]
mod medical_vocabulary_constraints;
#[path = "all/rpc_success_status.rs"]
mod rpc_success_status;
#[path = "all/service_claim.rs"]
mod service_claim;
#[path = "all/settings_co_requirements.rs"]
mod settings_co_requirements;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/tagging_resource_families.rs"]
mod tagging_resource_families;
#[path = "all/transcription_job_redacted_output.rs"]
mod transcription_job_redacted_output;
#[path = "all/transcription_job_request_rules.rs"]
mod transcription_job_request_rules;
#[path = "all/transcription_jobs.rs"]
mod transcription_jobs;
#[path = "all/vocabularies.rs"]
mod vocabularies;
#[path = "all/vocabulary_delete_and_tag_sets.rs"]
mod vocabulary_delete_and_tag_sets;
#[path = "all/vocabulary_file_uri.rs"]
mod vocabulary_file_uri;
#[path = "all/vocabulary_filter_file_uri.rs"]
mod vocabulary_filter_file_uri;
