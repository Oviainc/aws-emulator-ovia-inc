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
#[path = "all/allocation_fulfillment_type_recorded.rs"]
mod allocation_fulfillment_type_recorded;
#[path = "all/allocation_fulfillment_types.rs"]
mod allocation_fulfillment_types;
#[path = "all/amend_benefit_application.rs"]
mod amend_benefit_application;
#[path = "all/amend_caller_supplied_members.rs"]
mod amend_caller_supplied_members;
#[path = "all/amend_terminal_refusal.rs"]
mod amend_terminal_refusal;
#[path = "all/application_refusal_one_construction.rs"]
mod application_refusal_one_construction;
#[path = "all/application_status_gates.rs"]
mod application_status_gates;
#[path = "all/application_workflow_stage.rs"]
mod application_workflow_stage;
#[path = "all/arn_identifier_fields.rs"]
mod arn_identifier_fields;
#[path = "all/associate_benefit_application_resource.rs"]
mod associate_benefit_application_resource;
#[path = "all/benefit_allocations.rs"]
mod benefit_allocations;
#[path = "all/benefit_application_lifecycle.rs"]
mod benefit_application_lifecycle;
#[path = "all/benefit_catalog_record.rs"]
mod benefit_catalog_record;
#[path = "all/catalog_refusal_one_construction.rs"]
mod catalog_refusal_one_construction;
#[path = "all/catalog_resolution_one_accessor.rs"]
mod catalog_resolution_one_accessor;
#[path = "all/cancel_client_token_idempotency.rs"]
mod cancel_client_token_idempotency;
#[path = "all/client_token_idempotency.rs"]
mod client_token_idempotency;
#[path = "all/client_token_request_identity.rs"]
mod client_token_request_identity;
#[path = "all/create_identity_list_sets.rs"]
mod create_identity_list_sets;
#[path = "all/details_document_guard.rs"]
mod details_document_guard;
#[path = "all/disassociate_benefit_application_resource.rs"]
mod disassociate_benefit_application_resource;
#[path = "all/entitlement_denial.rs"]
mod entitlement_denial;
#[path = "all/field_path_length_bound.rs"]
mod field_path_length_bound;
#[path = "all/file_name_length_bound.rs"]
mod file_name_length_bound;
#[path = "all/file_upload_facts.rs"]
mod file_upload_facts;
#[path = "all/get_benefit.rs"]
mod get_benefit;
#[path = "all/get_benefit_allocation.rs"]
mod get_benefit_allocation;
#[path = "all/handler_enforced_length_wording.rs"]
mod handler_enforced_length_wording;
#[path = "all/idempotent_parameter_mismatch.rs"]
mod idempotent_parameter_mismatch;
#[path = "all/insert_application_write_is_atomic.rs"]
mod insert_application_write_is_atomic;
#[path = "all/list_applications_filter_members.rs"]
mod list_applications_filter_members;
#[path = "all/member_refusal_one_construction.rs"]
mod member_refusal_one_construction;
#[path = "all/pagination_cursor_issuance.rs"]
mod pagination_cursor_issuance;
#[path = "all/pagination_lookahead_one_construction.rs"]
mod pagination_lookahead_one_construction;
#[path = "all/persisted_string_bounds.rs"]
mod persisted_string_bounds;
#[path = "all/recall_benefit_application.rs"]
mod recall_benefit_application;
#[path = "all/refusal_member_path.rs"]
mod refusal_member_path;
#[path = "all/reserved_tag_key_refusal_form.rs"]
mod reserved_tag_key_refusal_form;
#[path = "all/reserved_tag_keys.rs"]
mod reserved_tag_keys;
#[path = "all/resource_tagging.rs"]
mod resource_tagging;
// `all/resource_tags.rs` is the `UntagResource` suite; it reuses the helpers of the
// sibling `resource_tagging` suite (`TagResource` / `ListTagsForResource`), and each
// suite's module doc says which operations it covers. (The build script requires the
// module name to match the file name, so the two names stay as they are.)
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/revision_conflict_message.rs"]
mod revision_conflict_message;
#[path = "all/service_error_rules.rs"]
mod service_error_rules;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/status_transition_revision.rs"]
mod status_transition_revision;
#[path = "all/tag_cap.rs"]
mod tag_cap;
#[path = "all/unconstrained_member_bounds.rs"]
mod unconstrained_member_bounds;
#[path = "all/unknown_catalog_lookup.rs"]
mod unknown_catalog_lookup;
#[path = "all/validation_error_body.rs"]
mod validation_error_body;
#[path = "all/validation_exception_message.rs"]
mod validation_exception_message;
#[path = "all/write_gate_order.rs"]
mod write_gate_order;
#[path = "all/write_that_changes_nothing.rs"]
mod write_that_changes_nothing;
