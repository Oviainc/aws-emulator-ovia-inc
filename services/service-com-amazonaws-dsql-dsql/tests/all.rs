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
#[path = "all/cluster_lifecycle.rs"]
mod cluster_lifecycle;
#[path = "all/cluster_policy.rs"]
mod cluster_policy;
#[path = "all/error_contract.rs"]
mod error_contract;
#[path = "all/harness.rs"]
mod harness;
#[path = "all/policy_document_class.rs"]
mod policy_document_class;
#[path = "all/policy_version_conflict_and_lockout.rs"]
mod policy_version_conflict_and_lockout;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/stream_lifecycle.rs"]
mod stream_lifecycle;
#[path = "all/stream_target_authorization.rs"]
mod stream_target_authorization;
#[path = "all/tag_resource.rs"]
mod tag_resource;
#[path = "all/vpc_endpoint_service_name.rs"]
mod vpc_endpoint_service_name;
