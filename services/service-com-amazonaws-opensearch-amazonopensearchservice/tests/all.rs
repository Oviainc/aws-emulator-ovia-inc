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
#[path = "all/change_progress.rs"]
mod change_progress;
#[path = "all/data_source_registry.rs"]
mod data_source_registry;
#[path = "all/default_application_setting.rs"]
mod default_application_setting;
#[path = "all/domain_health.rs"]
mod domain_health;
#[path = "all/domain_lifecycle.rs"]
mod domain_lifecycle;
#[path = "all/service_software_rollback.rs"]
mod service_software_rollback;
#[path = "all/shared_es_signing_name_claim.rs"]
mod shared_es_signing_name_claim;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
