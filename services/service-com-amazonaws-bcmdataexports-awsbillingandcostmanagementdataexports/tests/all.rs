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
#[path = "all/authorization_message_bound.rs"]
mod authorization_message_bound;
#[path = "all/catalog_dictionary.rs"]
mod catalog_dictionary;
#[path = "all/catalog_published_text.rs"]
mod catalog_published_text;
#[path = "all/catalog_schema.rs"]
mod catalog_schema;
#[path = "all/catalog_schema_versions.rs"]
mod catalog_schema_versions;
#[path = "all/data_query.rs"]
mod data_query;
#[path = "all/data_query_resolution.rs"]
mod data_query_resolution;
#[path = "all/delivery_executions.rs"]
mod delivery_executions;
#[path = "all/delivery_grant.rs"]
mod delivery_grant;
#[path = "all/destination_configuration.rs"]
mod destination_configuration;
#[path = "all/error_member_bounds.rs"]
mod error_member_bounds;
#[path = "all/execution_key_range.rs"]
mod execution_key_range;
#[path = "all/execution_listing.rs"]
mod execution_listing;
#[path = "all/execution_lookup.rs"]
mod execution_lookup;
#[path = "all/export_definition_validation.rs"]
mod export_definition_validation;
#[path = "all/export_lifecycle.rs"]
mod export_lifecycle;
#[path = "all/front_door_authorization.rs"]
mod front_door_authorization;
#[path = "all/minted_export_arn.rs"]
mod minted_export_arn;
#[path = "all/not_found_member_bounds.rs"]
mod not_found_member_bounds;
#[path = "all/not_found_message.rs"]
mod not_found_message;
#[path = "all/not_found_wire_status.rs"]
mod not_found_wire_status;
#[path = "all/owed_delivery_survives_update.rs"]
mod owed_delivery_survives_update;
#[path = "all/page_token_refusal.rs"]
mod page_token_refusal;
#[path = "all/page_token_spelling.rs"]
mod page_token_spelling;
#[path = "all/property_gated_columns.rs"]
mod property_gated_columns;
#[path = "all/query_alias_scope.rs"]
mod query_alias_scope;
#[path = "all/query_column_selection.rs"]
mod query_column_selection;
#[path = "all/query_statement_subset.rs"]
mod query_statement_subset;
#[path = "all/quota_service_code.rs"]
mod quota_service_code;
#[path = "all/refusal_message_bound.rs"]
mod refusal_message_bound;
#[path = "all/refusal_message_bounds.rs"]
mod refusal_message_bounds;
#[path = "all/reserved_tag_keys.rs"]
mod reserved_tag_keys;
#[path = "all/resource_tagging.rs"]
mod resource_tagging;
#[path = "all/shared_arn_parse.rs"]
mod shared_arn_parse;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/status_timestamp_wire_form.rs"]
mod status_timestamp_wire_form;
#[path = "all/table_catalog.rs"]
mod table_catalog;
#[path = "all/table_listing.rs"]
mod table_listing;
#[path = "all/table_property_refusal_determinism.rs"]
mod table_property_refusal_determinism;
#[path = "all/table_property_refusal_field.rs"]
mod table_property_refusal_field;
#[path = "all/tagging.rs"]
mod tagging;
#[path = "all/tagging_arn_validation.rs"]
mod tagging_arn_validation;
#[path = "all/unpublished_table_refusal_field.rs"]
mod unpublished_table_refusal_field;
#[path = "all/untagging_arn_validation.rs"]
mod untagging_arn_validation;
#[path = "all/untagging_authorization.rs"]
mod untagging_authorization;
#[path = "all/user_attached_policy_authorization.rs"]
mod user_attached_policy_authorization;
#[path = "all/validation_message_rendering.rs"]
mod validation_message_rendering;
#[path = "all/validation_messages.rs"]
mod validation_messages;
