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
#[path = "all/dax_client_fault_wire_status.rs"]
mod dax_client_fault_wire_status;
#[path = "all/dax_cluster_lifecycle.rs"]
mod dax_cluster_lifecycle;
#[path = "all/dax_default_parameters.rs"]
mod dax_default_parameters;
#[path = "all/dax_describe_parameters.rs"]
mod dax_describe_parameters;
#[path = "all/dax_parameter_groups.rs"]
mod dax_parameter_groups;
#[path = "all/dax_rejection_paths.rs"]
mod dax_rejection_paths;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
