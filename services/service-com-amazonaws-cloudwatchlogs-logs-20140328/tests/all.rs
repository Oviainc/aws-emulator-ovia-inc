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
#[path = "all/account_policies.rs"]
mod account_policies;
#[path = "all/account_policy_release.rs"]
mod account_policy_release;
#[path = "all/account_policy_removal.rs"]
mod account_policy_removal;
#[path = "all/account_policy_selection_criteria.rs"]
mod account_policy_selection_criteria;
#[path = "all/data_protection_policies.rs"]
mod data_protection_policies;
#[path = "all/data_protection_policy.rs"]
mod data_protection_policy;
#[path = "all/deletion_protection.rs"]
mod deletion_protection;
#[path = "all/deliveries.rs"]
mod deliveries;
#[path = "all/delivery_destinations.rs"]
mod delivery_destinations;
#[path = "all/destinations.rs"]
mod destinations;
#[path = "all/harness.rs"]
mod harness;
#[path = "all/import_tasks.rs"]
mod import_tasks;
#[path = "all/ingestion_window.rs"]
mod ingestion_window;
#[path = "all/integrations.rs"]
mod integrations;
#[path = "all/log_anomaly_detectors.rs"]
mod log_anomaly_detectors;
#[path = "all/log_core_loop.rs"]
mod log_core_loop;
#[path = "all/log_group_auth_and_encryption.rs"]
mod log_group_auth_and_encryption;
#[path = "all/log_group_name_pattern_lists.rs"]
mod log_group_name_pattern_lists;
#[path = "all/log_stream_removal.rs"]
mod log_stream_removal;
#[path = "all/metric_filters.rs"]
mod metric_filters;
#[path = "all/query_definitions.rs"]
mod query_definitions;
#[path = "all/refusal_error_codes.rs"]
mod refusal_error_codes;
#[path = "all/refusal_messages.rs"]
mod refusal_messages;
#[path = "all/resource_policies.rs"]
mod resource_policies;
#[path = "all/resource_tag_removal.rs"]
mod resource_tag_removal;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/retention_policy.rs"]
mod retention_policy;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/storage_tier_policy.rs"]
mod storage_tier_policy;
#[path = "all/subscription_filters.rs"]
mod subscription_filters;
#[path = "all/syslog_configurations.rs"]
mod syslog_configurations;
#[path = "all/transformers.rs"]
mod transformers;
#[path = "all/validation_message_rendering.rs"]
mod validation_message_rendering;
