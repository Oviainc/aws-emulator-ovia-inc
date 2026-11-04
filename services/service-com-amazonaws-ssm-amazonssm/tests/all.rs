// SPDX-License-Identifier: CC0-1.0

//! Single integration-test target for this crate.
//!
//! Put a new suite at `tests/all/<name>.rs` and declare it below, keeping the list
//! sorted.
//!
//! A suite that is not declared here compiles nowhere and its tests never run. A
//! suite left at the top level of `tests/` fails the build.

// `tests/all.rs` is a test-target CRATE ROOT, so a bare `mod` resolves to a SIBLING
// (`tests/parameter_store.rs`) — which the layout check refuses. The `#[path]` is what
// points it into `tests/all/`.
#[path = "all/documents.rs"]
mod documents;
#[path = "all/hybrid_activations.rs"]
mod hybrid_activations;
#[path = "all/instance_properties.rs"]
mod instance_properties;
#[path = "all/managed_node_filter_validation.rs"]
mod managed_node_filter_validation;
#[path = "all/managed_node_information.rs"]
mod managed_node_information;
#[path = "all/parameter_deletion.rs"]
mod parameter_deletion;
#[path = "all/parameter_deletion_visibility.rs"]
mod parameter_deletion_visibility;
#[path = "all/parameter_history.rs"]
mod parameter_history;
#[path = "all/parameter_labels.rs"]
mod parameter_labels;
#[path = "all/parameter_metadata.rs"]
mod parameter_metadata;
#[path = "all/parameter_not_found_payload.rs"]
mod parameter_not_found_payload;
#[path = "all/parameter_store.rs"]
mod parameter_store;
#[path = "all/parameters_batch.rs"]
mod parameters_batch;
#[path = "all/parameters_by_path.rs"]
mod parameters_by_path;
#[path = "all/published_documents.rs"]
mod published_documents;
#[path = "all/run_command.rs"]
mod run_command;
#[path = "all/secure_string_decryption.rs"]
mod secure_string_decryption;
#[path = "all/secure_string_encryption_at_rest.rs"]
mod secure_string_encryption_at_rest;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
