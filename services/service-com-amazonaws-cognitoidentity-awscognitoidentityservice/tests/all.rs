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
#[path = "all/anonymous_credential_exchange.rs"]
mod anonymous_credential_exchange;
#[path = "all/anonymous_credential_vend.rs"]
mod anonymous_credential_vend;
#[path = "all/developer_identity_tokens.rs"]
mod developer_identity_tokens;
#[path = "all/identity_pool_flow.rs"]
mod identity_pool_flow;
#[path = "all/identity_pool_roles.rs"]
mod identity_pool_roles;
#[path = "all/list_identity_pools.rs"]
mod list_identity_pools;
#[path = "all/lookup_developer_identity.rs"]
mod lookup_developer_identity;
#[path = "all/principal_tag_attribute_map.rs"]
mod principal_tag_attribute_map;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/unlink_identity.rs"]
mod unlink_identity;
#[path = "all/update_identity_pool.rs"]
mod update_identity_pool;
