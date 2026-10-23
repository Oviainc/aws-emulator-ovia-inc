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
#[path = "all/arn_addressed_tag_refusals.rs"]
mod arn_addressed_tag_refusals;
#[path = "all/emulator.rs"]
mod emulator;
#[path = "all/global_secondary_indexes.rs"]
mod global_secondary_indexes;
#[path = "all/index_capacity_and_item_collections.rs"]
mod index_capacity_and_item_collections;
#[path = "all/key_schema_element_refusals.rs"]
mod key_schema_element_refusals;
#[path = "all/number_canonicalization.rs"]
mod number_canonicalization;
#[path = "all/regions_indexes_and_capacity.rs"]
mod regions_indexes_and_capacity;

#[path = "all/resource_tags.rs"]
mod resource_tags;

#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;

#[path = "all/table_control_plane.rs"]
mod table_control_plane;

#[path = "all/table_description_members.rs"]
mod table_description_members;

#[path = "all/table_description_shape.rs"]
mod table_description_shape;

#[path = "all/table_items.rs"]
mod table_items;

#[path = "all/validation_message_wording.rs"]
mod validation_message_wording;

#[path = "all/value_rules_and_resource_policy.rs"]
mod value_rules_and_resource_policy;
