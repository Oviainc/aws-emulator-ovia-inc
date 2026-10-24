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
#[path = "all/gateway_lifecycle.rs"]
mod gateway_lifecycle;
#[path = "all/hypervisor_import_concurrency.rs"]
mod hypervisor_import_concurrency;
#[path = "all/hypervisor_listing.rs"]
mod hypervisor_listing;
#[path = "all/hypervisor_property_mappings.rs"]
mod hypervisor_property_mappings;
#[path = "all/hypervisor_reads.rs"]
mod hypervisor_reads;
#[path = "all/metadata_sync.rs"]
mod metadata_sync;
#[path = "all/negative_paths.rs"]
mod negative_paths;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/virtual_machine_listing.rs"]
mod virtual_machine_listing;
