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
#[path = "all/channel_lifecycle.rs"]
mod channel_lifecycle;
#[path = "all/cluster_lifecycle.rs"]
mod cluster_lifecycle;
#[path = "all/compatible_kafka_versions.rs"]
mod compatible_kafka_versions;
#[path = "all/connectivity_update.rs"]
mod connectivity_update;
#[path = "all/describe_cluster_v2.rs"]
mod describe_cluster_v2;
#[path = "all/kafka_harness.rs"]
mod kafka_harness;
#[path = "all/kafka_versions.rs"]
mod kafka_versions;
#[path = "all/list_clusters_v2.rs"]
mod list_clusters_v2;
#[path = "all/replicator_lifecycle.rs"]
mod replicator_lifecycle;
#[path = "all/replicator_listing.rs"]
mod replicator_listing;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/storage_updates.rs"]
mod storage_updates;
#[path = "all/topic_lifecycle.rs"]
mod topic_lifecycle;
#[path = "all/vpc_connection_lifecycle.rs"]
mod vpc_connection_lifecycle;
