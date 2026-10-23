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
#[path = "all/account_instance_restriction.rs"]
mod account_instance_restriction;
#[path = "all/application_provider_describe.rs"]
mod application_provider_describe;
#[path = "all/application_providers.rs"]
mod application_providers;
#[path = "all/instance_create.rs"]
mod instance_create;
#[path = "all/instance_describe.rs"]
mod instance_describe;
#[path = "all/instance_update.rs"]
mod instance_update;
#[path = "all/ssoadmin_permission_sets.rs"]
mod ssoadmin_permission_sets;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/trusted_token_issuers.rs"]
mod trusted_token_issuers;
