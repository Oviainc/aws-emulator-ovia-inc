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
#[path = "all/allowed_node_type_modifications.rs"]
mod allowed_node_type_modifications;
#[path = "all/cache_cluster_lifecycle.rs"]
mod cache_cluster_lifecycle;
#[path = "all/cache_engine_versions.rs"]
mod cache_engine_versions;
#[path = "all/cache_parameter_groups.rs"]
mod cache_parameter_groups;
#[path = "all/engine_default_parameters.rs"]
mod engine_default_parameters;
#[path = "all/events.rs"]
mod events;
#[path = "all/rbac_users.rs"]
mod rbac_users;
#[path = "all/replication_group_lifecycle.rs"]
mod replication_group_lifecycle;
#[path = "all/reserved_cache_nodes.rs"]
mod reserved_cache_nodes;
#[path = "all/reserved_cache_nodes_offerings.rs"]
mod reserved_cache_nodes_offerings;
#[path = "all/serverless_cache_describe.rs"]
mod serverless_cache_describe;
#[path = "all/service_updates.rs"]
mod service_updates;
#[path = "all/serverless_cache_lifecycle.rs"]
mod serverless_cache_lifecycle;
#[path = "all/snapshot_copies.rs"]
mod snapshot_copies;
#[path = "all/snapshots.rs"]
mod snapshots;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/update_actions.rs"]
mod update_actions;

// Declared after the list above rather than inside it: suites land in this file
// concurrently, and every insertion into the sorted run collides with another
// branch's insertion at the same place. These two are the snapshot-rule suite
// and the two drivers it calls through.
#[path = "all/snapshot_lifecycle.rs"]
mod snapshot_lifecycle;
#[path = "all/harness.rs"]
mod harness;

// Appended for the same reason: these two are the `DescribeUsers` describe-axis
// suite and the page walk that calls through its helpers.
#[path = "all/users.rs"]
mod users;
#[path = "all/describe_users.rs"]
mod describe_users;

// Appended for the same reason: the `DescribeUpdateActions` suite.
#[path = "all/describe_update_actions.rs"]
mod describe_update_actions;

// Appended for the same reason: the awsQuery error-envelope, subnet-resolution
// and identifier-validation suite.
#[path = "all/subnet_resolution_and_error_envelope.rs"]
mod subnet_resolution_and_error_envelope;

// Appended for the same reason: the awsQuery error-envelope suite and the
// `DeleteCacheParameterGroup` suite it shares a fix with.
#[path = "all/query_error_envelope.rs"]
mod query_error_envelope;
#[path = "all/cache_parameter_group_delete.rs"]
mod cache_parameter_group_delete;
