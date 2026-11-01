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
#[path = "all/append_output_name_conflict.rs"]
mod append_output_name_conflict;
#[path = "all/associate_feed_reference_integrity.rs"]
mod associate_feed_reference_integrity;
#[path = "all/claim_without_supplied_outputs.rs"]
mod claim_without_supplied_outputs;
#[path = "all/completed_deletion.rs"]
mod completed_deletion;
#[path = "all/create_replay_is_idempotent.rs"]
mod create_replay_is_idempotent;
#[path = "all/delete_feed_policy.rs"]
mod delete_feed_policy;
#[path = "all/dictionary_lifecycle.rs"]
mod dictionary_lifecycle;
#[path = "all/dictionary_listing.rs"]
mod dictionary_listing;
#[path = "all/dictionary_mutation.rs"]
mod dictionary_mutation;
#[path = "all/dictionary_service_forms.rs"]
mod dictionary_service_forms;
#[path = "all/error_contract.rs"]
mod error_contract;
#[path = "all/error_message_rule.rs"]
mod error_message_rule;
#[path = "all/feed_association.rs"]
mod feed_association;
#[path = "all/feed_association_retry.rs"]
mod feed_association_retry;
#[path = "all/feed_audio_binding.rs"]
mod feed_audio_binding;
#[path = "all/feed_lifecycle.rs"]
mod feed_lifecycle;
#[path = "all/feed_output_validation.rs"]
mod feed_output_validation;
#[path = "all/feed_policy.rs"]
mod feed_policy;
#[path = "all/feed_reconfiguration_status.rs"]
mod feed_reconfiguration_status;
#[path = "all/feed_release_retry.rs"]
mod feed_release_retry;
#[path = "all/feed_usable_on_create.rs"]
mod feed_usable_on_create;
#[path = "all/feed_write_status_rule.rs"]
mod feed_write_status_rule;
#[path = "all/fixture_catalog.rs"]
mod fixture_catalog;
#[path = "all/fixture_data_source.rs"]
mod fixture_data_source;
#[path = "all/fixture_search.rs"]
mod fixture_search;
#[path = "all/harness.rs"]
mod harness;
#[path = "all/minted_arn_region.rs"]
mod minted_arn_region;
#[path = "all/output_config_defaults.rs"]
mod output_config_defaults;
#[path = "all/output_list_rule.rs"]
mod output_list_rule;
#[path = "all/output_provenance.rs"]
mod output_provenance;
#[path = "all/output_quota_rule.rs"]
mod output_quota_rule;
#[path = "all/page_token_spelling.rs"]
mod page_token_spelling;
#[path = "all/pagination_token_binding.rs"]
mod pagination_token_binding;
#[path = "all/put_feed_policy_scope.rs"]
mod put_feed_policy_scope;
#[path = "all/refusals_from_state.rs"]
mod refusals_from_state;
#[path = "all/release_after_reconfiguration.rs"]
mod release_after_reconfiguration;
#[path = "all/region_isolation.rs"]
mod region_isolation;
#[path = "all/reserved_tag_namespace.rs"]
mod reserved_tag_namespace;
#[path = "all/resource_tagging.rs"]
mod resource_tagging;
#[path = "all/resource_untagging.rs"]
mod resource_untagging;
#[path = "all/response_members.rs"]
mod response_members;
#[path = "all/shared_create_quota_rule.rs"]
mod shared_create_quota_rule;
#[path = "all/shared_pagination_rules.rs"]
mod shared_pagination_rules;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/supplied_output_cardinality.rs"]
mod supplied_output_cardinality;
#[path = "all/supplied_tag_validation.rs"]
mod supplied_tag_validation;
#[path = "all/tag_listing.rs"]
mod tag_listing;
#[path = "all/tagging_region_scope.rs"]
mod tagging_region_scope;
#[path = "all/update_replay_is_idempotent.rs"]
mod update_replay_is_idempotent;
