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
#[path = "all/env_endpoint_not_found_wording.rs"]
mod env_endpoint_not_found_wording;
#[path = "all/env_endpoint_payload.rs"]
mod env_endpoint_payload;
#[path = "all/environment_lifecycle.rs"]
mod environment_lifecycle;
#[path = "all/environment_service_defaults.rs"]
mod environment_service_defaults;
#[path = "all/rejection_contract.rs"]
mod rejection_contract;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
