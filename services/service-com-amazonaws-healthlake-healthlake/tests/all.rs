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
#[path = "all/control_plane_error_semantics.rs"]
mod control_plane_error_semantics;
#[path = "all/data_transformation_profile.rs"]
mod data_transformation_profile;
#[path = "all/datastore_state_conflict.rs"]
mod datastore_state_conflict;
#[path = "all/error_envelopes.rs"]
mod error_envelopes;
#[path = "all/fhir_datastore_lifecycle.rs"]
mod fhir_datastore_lifecycle;
#[path = "all/fhir_rest_and_validate_source.rs"]
mod fhir_rest_and_validate_source;
#[path = "all/fhir_update_interaction.rs"]
mod fhir_update_interaction;
#[path = "all/list_tags_for_resource.rs"]
mod list_tags_for_resource;
#[path = "all/resource_tagging.rs"]
mod resource_tagging;
#[path = "all/restore_fhir_datastore.rs"]
mod restore_fhir_datastore;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/update_fhir_datastore.rs"]
mod update_fhir_datastore;
