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
#[path = "all/app_authorization_and_ingestion_dependency.rs"]
mod app_authorization_and_ingestion_dependency;
#[path = "all/app_bundle_ingestion_lifecycle.rs"]
mod app_bundle_ingestion_lifecycle;
#[path = "all/create_app_authorization_contract.rs"]
mod create_app_authorization_contract;
#[path = "all/ingestion_deletion.rs"]
mod ingestion_deletion;
#[path = "all/ingestion_destination_lifecycle.rs"]
mod ingestion_destination_lifecycle;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/user_access_tasks.rs"]
mod user_access_tasks;
