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
#[path = "all/api_cache.rs"]
mod api_cache;
#[path = "all/data_source_service_role.rs"]
mod data_source_service_role;
#[path = "all/data_sources.rs"]
mod data_sources;
#[path = "all/environment_variables.rs"]
mod environment_variables;
#[path = "all/error_contract.rs"]
mod error_contract;
#[path = "all/graphql_api_create_rules.rs"]
mod graphql_api_create_rules;
#[path = "all/graphql_api_lifecycle.rs"]
mod graphql_api_lifecycle;
#[path = "all/graphql_api_update.rs"]
mod graphql_api_update;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/source_api_association.rs"]
mod source_api_association;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
