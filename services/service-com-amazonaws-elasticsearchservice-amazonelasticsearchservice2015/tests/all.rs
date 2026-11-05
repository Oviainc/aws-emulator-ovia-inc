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
#[path = "all/cluster_instance_types.rs"]
mod cluster_instance_types;
#[path = "all/compatible_versions.rs"]
mod compatible_versions;
#[path = "all/describe_domains_batch.rs"]
mod describe_domains_batch;
#[path = "all/domain_lifecycle.rs"]
mod domain_lifecycle;
#[path = "all/domain_tags.rs"]
mod domain_tags;
#[path = "all/harness.rs"]
mod harness;
#[path = "all/outbound_cross_cluster_search_connections.rs"]
mod outbound_cross_cluster_search_connections;
#[path = "all/package_associations.rs"]
mod package_associations;
#[path = "all/package_versions.rs"]
mod package_versions;
#[path = "all/packages.rs"]
mod packages;
#[path = "all/shared_es_signing_name_claim.rs"]
mod shared_es_signing_name_claim;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
