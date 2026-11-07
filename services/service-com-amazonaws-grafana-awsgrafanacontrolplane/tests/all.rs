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
#[path = "all/delete_service_account.rs"]
mod delete_service_account;

#[path = "all/delete_service_account_token.rs"]
mod delete_service_account_token;

#[path = "all/list_versions.rs"]
mod list_versions;

#[path = "all/list_workspace_service_accounts.rs"]
mod list_workspace_service_accounts;

#[path = "all/not_found_messages.rs"]
mod not_found_messages;

#[path = "all/request_body_refusals.rs"]
mod request_body_refusals;

#[path = "all/resource_tags.rs"]
mod resource_tags;

#[path = "all/service_account_tokens.rs"]
mod service_account_tokens;

#[path = "all/service_accounts.rs"]
mod service_accounts;

#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;

#[path = "all/untag_resource.rs"]
mod untag_resource;

#[path = "all/workspace_api_keys.rs"]
mod workspace_api_keys;

#[path = "all/workspace_authentication.rs"]
mod workspace_authentication;

#[path = "all/workspace_authorization.rs"]
mod workspace_authorization;

#[path = "all/workspace_configuration.rs"]
mod workspace_configuration;

#[path = "all/workspace_grafana_api.rs"]
mod workspace_grafana_api;

#[path = "all/workspace_lifecycle.rs"]
mod workspace_lifecycle;

#[path = "all/workspace_mutation_gating.rs"]
mod workspace_mutation_gating;

#[path = "all/workspace_native_api.rs"]
mod workspace_native_api;

#[path = "all/workspace_refusal_messages.rs"]
mod workspace_refusal_messages;
