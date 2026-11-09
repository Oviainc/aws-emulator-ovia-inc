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
#[path = "all/boundary_validation_exception.rs"]
mod boundary_validation_exception;
#[path = "all/client_token_endpoint_scope.rs"]
mod client_token_endpoint_scope;
#[path = "all/client_token_parameter_distinctness.rs"]
mod client_token_parameter_distinctness;
#[path = "all/cluster_encryption_status.rs"]
mod cluster_encryption_status;
#[path = "all/cluster_lifecycle.rs"]
mod cluster_lifecycle;
#[path = "all/cluster_mutation_status_gate.rs"]
mod cluster_mutation_status_gate;
#[path = "all/cluster_policy.rs"]
mod cluster_policy;
#[path = "all/cluster_policy_mutation_status_gate.rs"]
mod cluster_policy_mutation_status_gate;
#[path = "all/cluster_region_derived_values.rs"]
mod cluster_region_derived_values;
#[path = "all/create_cluster_replay_configuration.rs"]
mod create_cluster_replay_configuration;
#[path = "all/delete_cluster_idempotency.rs"]
mod delete_cluster_idempotency;
#[path = "all/delete_cluster_in_flight.rs"]
mod delete_cluster_in_flight;
#[path = "all/encryption_key_selection.rs"]
mod encryption_key_selection;
#[path = "all/error_contract.rs"]
mod error_contract;
#[path = "all/fixed_refusal_sentences.rs"]
mod fixed_refusal_sentences;
#[path = "all/harness.rs"]
mod harness;
#[path = "all/identifier_format_refusal.rs"]
mod identifier_format_refusal;
#[path = "all/idempotent_replay_result.rs"]
mod idempotent_replay_result;
#[path = "all/incomplete_replay_record.rs"]
mod incomplete_replay_record;
#[path = "all/inline_policy_validation.rs"]
mod inline_policy_validation;
#[path = "all/input_verdict_precedes_the_token_ledger.rs"]
mod input_verdict_precedes_the_token_ledger;
#[path = "all/malformed_resource_identifier.rs"]
mod malformed_resource_identifier;
#[path = "all/modeled_validation_exception.rs"]
mod modeled_validation_exception;
#[path = "all/multi_region_cluster_status.rs"]
mod multi_region_cluster_status;
#[path = "all/multi_region_witness.rs"]
mod multi_region_witness;
#[path = "all/nested_arn_not_found.rs"]
mod nested_arn_not_found;
#[path = "all/pagination_cursor_binding.rs"]
mod pagination_cursor_binding;
#[path = "all/pagination_token_refusal.rs"]
mod pagination_token_refusal;
#[path = "all/peering_unwound_by_deletion.rs"]
mod peering_unwound_by_deletion;
#[path = "all/peering_unwound_by_update.rs"]
mod peering_unwound_by_update;
#[path = "all/policy_document_class.rs"]
mod policy_document_class;
#[path = "all/policy_principal_resolution.rs"]
mod policy_principal_resolution;
#[path = "all/policy_principal_set.rs"]
mod policy_principal_set;
#[path = "all/policy_token_optional_claim_distinctness.rs"]
mod policy_token_optional_claim_distinctness;
#[path = "all/policy_version_conflict_and_lockout.rs"]
mod policy_version_conflict_and_lockout;
#[path = "all/quota_refusal_identity.rs"]
mod quota_refusal_identity;
#[path = "all/refusal_resource_identity.rs"]
mod refusal_resource_identity;
#[path = "all/reserved_tag_keys.rs"]
mod reserved_tag_keys;
#[path = "all/resource_identifier_absence.rs"]
mod resource_identifier_absence;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/retired_cluster_token_cleanup.rs"]
mod retired_cluster_token_cleanup;
#[path = "all/shared_page_walk.rs"]
mod shared_page_walk;
#[path = "all/shared_recorded_delete.rs"]
mod shared_recorded_delete;
#[path = "all/shared_recorded_policy_write.rs"]
mod shared_recorded_policy_write;
#[path = "all/shared_recorded_status.rs"]
mod shared_recorded_status;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/stream_deletion.rs"]
mod stream_deletion;
#[path = "all/stream_lifecycle.rs"]
mod stream_lifecycle;
#[path = "all/stream_listing.rs"]
mod stream_listing;
#[path = "all/stream_target_authorization.rs"]
mod stream_target_authorization;
#[path = "all/stream_target_health.rs"]
mod stream_target_health;
#[path = "all/tag_resource.rs"]
mod tag_resource;
#[path = "all/tagging_arn_refusal_body.rs"]
mod tagging_arn_refusal_body;
#[path = "all/tagging_arn_scope.rs"]
mod tagging_arn_scope;
#[path = "all/token_conflict_resource_identity.rs"]
mod token_conflict_resource_identity;
#[path = "all/update_cluster_pending_setup_exemption.rs"]
mod update_cluster_pending_setup_exemption;
#[path = "all/vpc_endpoint_service_name.rs"]
mod vpc_endpoint_service_name;
