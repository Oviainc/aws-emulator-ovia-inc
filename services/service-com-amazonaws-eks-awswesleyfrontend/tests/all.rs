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
#[path = "all/access_entries.rs"]
mod access_entries;
#[path = "all/access_entry_listing.rs"]
mod access_entry_listing;
#[path = "all/access_policy_catalog.rs"]
mod access_policy_catalog;
#[path = "all/addon_lifecycle.rs"]
mod addon_lifecycle;
#[path = "all/cancel_update.rs"]
mod cancel_update;
#[path = "all/capability_lifecycle.rs"]
mod capability_lifecycle;
#[path = "all/certificate_authority.rs"]
mod certificate_authority;
#[path = "all/cluster_lifecycle.rs"]
mod cluster_lifecycle;
#[path = "all/cluster_versions.rs"]
mod cluster_versions;
#[path = "all/connected_clusters.rs"]
mod connected_clusters;
#[path = "all/encryption_config_association.rs"]
mod encryption_config_association;
#[path = "all/identity_provider_config.rs"]
mod identity_provider_config;
#[path = "all/identity_provider_config_not_found.rs"]
mod identity_provider_config_not_found;
#[path = "all/identity_provider_configs.rs"]
mod identity_provider_configs;
#[path = "all/list_pod_identity_associations.rs"]
mod list_pod_identity_associations;
#[path = "all/nodegroup_lifecycle.rs"]
mod nodegroup_lifecycle;
#[path = "all/pod_identity_associations.rs"]
mod pod_identity_associations;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/wire.rs"]
mod wire;
