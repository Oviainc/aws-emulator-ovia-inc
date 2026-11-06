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
#[path = "all/aws_refusal_wording.rs"]
mod aws_refusal_wording;
#[path = "all/ca_certificate_material.rs"]
mod ca_certificate_material;
#[path = "all/cluster_envelope_flags.rs"]
mod cluster_envelope_flags;
#[path = "all/cluster_lifecycle.rs"]
mod cluster_lifecycle;
#[path = "all/cluster_networking_and_restore_window.rs"]
mod cluster_networking_and_restore_window;
#[path = "all/cluster_parameter_groups.rs"]
mod cluster_parameter_groups;
#[path = "all/cluster_snapshot_lifecycle.rs"]
mod cluster_snapshot_lifecycle;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/static_reference_catalogs.rs"]
mod static_reference_catalogs;
#[path = "all/subnet_and_parameter_groups.rs"]
mod subnet_and_parameter_groups;
