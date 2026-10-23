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
#[path = "all/catalog_dictionary.rs"]
mod catalog_dictionary;
#[path = "all/catalog_published_text.rs"]
mod catalog_published_text;
#[path = "all/catalog_schema.rs"]
mod catalog_schema;
#[path = "all/data_query.rs"]
mod data_query;
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
#[path = "all/not_found_message.rs"]
mod not_found_message;
#[path = "all/query_statement_subset.rs"]
mod query_statement_subset;
#[path = "all/resource_tagging.rs"]
mod resource_tagging;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/status_timestamp_wire_form.rs"]
mod status_timestamp_wire_form;
#[path = "all/table_catalog.rs"]
mod table_catalog;
#[path = "all/table_listing.rs"]
mod table_listing;
#[path = "all/tagging.rs"]
mod tagging;
#[path = "all/tagging_arn_validation.rs"]
mod tagging_arn_validation;
#[path = "all/validation_message_rendering.rs"]
mod validation_message_rendering;
#[path = "all/validation_messages.rs"]
mod validation_messages;
