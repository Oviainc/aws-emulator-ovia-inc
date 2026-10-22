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
#[path = "all/catalog_fidelity.rs"]
mod catalog_fidelity;
#[path = "all/instance_type_catalog.rs"]
mod instance_type_catalog;
#[path = "all/region_catalog.rs"]
mod region_catalog;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/validation_messages.rs"]
mod validation_messages;
#[path = "all/volume_lifecycle.rs"]
mod volume_lifecycle;
#[path = "all/wire_error_responses.rs"]
mod wire_error_responses;
#[path = "all/workspace_instance_lifecycle.rs"]
mod workspace_instance_lifecycle;
