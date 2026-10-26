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
#[path = "all/alias_lifecycle.rs"]
mod alias_lifecycle;
#[path = "all/aliases.rs"]
mod aliases;
#[path = "all/aws_managed_keys.rs"]
mod aws_managed_keys;
#[path = "all/client_error_status.rs"]
mod client_error_status;
#[path = "all/custom_key_store_connection.rs"]
mod custom_key_store_connection;
#[path = "all/custom_key_stores.rs"]
mod custom_key_stores;
#[path = "all/data_key.rs"]
mod data_key;
#[path = "all/data_key_pair_without_plaintext.rs"]
mod data_key_pair_without_plaintext;
#[path = "all/data_key_without_plaintext.rs"]
mod data_key_without_plaintext;
#[path = "all/decrypt_key_origin.rs"]
mod decrypt_key_origin;
#[path = "all/error_semantics.rs"]
mod error_semantics;
#[path = "all/get_key_policy_reads.rs"]
mod get_key_policy_reads;
#[path = "all/get_key_rotation_status_reads.rs"]
mod get_key_rotation_status_reads;
#[path = "all/grants.rs"]
mod grants;
#[path = "all/harness.rs"]
mod harness;
#[path = "all/imported_key_material.rs"]
mod imported_key_material;
#[path = "all/key_deletion_cancellation.rs"]
mod key_deletion_cancellation;
#[path = "all/key_last_usage.rs"]
mod key_last_usage;
#[path = "all/key_lifecycle.rs"]
mod key_lifecycle;
#[path = "all/key_policy.rs"]
mod key_policy;
#[path = "all/key_rotation_on_demand.rs"]
mod key_rotation_on_demand;
#[path = "all/key_rotation_schedule.rs"]
mod key_rotation_schedule;
#[path = "all/key_specs.rs"]
mod key_specs;
#[path = "all/list_key_rotations_reads.rs"]
mod list_key_rotations_reads;
#[path = "all/list_resource_tags_reads.rs"]
mod list_resource_tags_reads;
#[path = "all/multi_region_primary.rs"]
mod multi_region_primary;
#[path = "all/re_encrypt.rs"]
mod re_encrypt;
#[path = "all/replicate_key.rs"]
mod replicate_key;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/update_custom_key_store.rs"]
mod update_custom_key_store;
