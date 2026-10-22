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
#[path = "all/codegen_job_lifecycle.rs"]
mod codegen_job_lifecycle;

#[path = "all/component_lifecycle.rs"]
mod component_lifecycle;

#[path = "all/component_refusals.rs"]
mod component_refusals;

#[path = "all/component_wire_fidelity.rs"]
mod component_wire_fidelity;

#[path = "all/export_components.rs"]
mod export_components;

#[path = "all/form_lifecycle.rs"]
mod form_lifecycle;

#[path = "all/form_update.rs"]
mod form_update;

#[path = "all/metadata_flags.rs"]
mod metadata_flags;

#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;

#[path = "all/theme_lifecycle.rs"]
mod theme_lifecycle;
